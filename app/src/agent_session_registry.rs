//! A standalone record of the CLI agent sessions in the app state as it was last saved.
//!
//! Everything here is also in the pane snapshots, and restoring a window puts each pane's resume
//! command back in its input box, which is the fast path. This exists for the times that path is
//! not available: the window was closed rather than restored, the layout was rearranged, the
//! database was reset, a pane failed to come back, or the agent has no resume command and the
//! session ID is the only thread left to pull. Those are exactly the moments when the terminal
//! cannot be asked, so the answer is left in files that can be read without it.
//!
//! Written on every app state save, replacing the previous contents, so it describes the last
//! saved state and not an accumulating history.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::Serialize;

use crate::app_state::{
    AppState, LeafContents, PaneNodeSnapshot, TabSnapshot, TerminalPaneSnapshot,
};

const JSON_FILE_NAME: &str = "agent-sessions.json";
const MARKDOWN_FILE_NAME: &str = "agent-sessions.md";

/// One pane that was running a CLI agent, and where to find it again.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct AgentSessionRecord {
    pub window_index: usize,
    pub tab_index: usize,
    pub tab_title: Option<String>,
    /// The pane's UUID, hex-encoded, matching the `uuid` column of `terminal_panes`.
    pub pane_uuid: String,
    pub agent: String,
    pub session_id: Option<String>,
    /// The command that reopens the session, for the agents whose CLI can be told to.
    pub resume_command: Option<String>,
    pub cwd: String,
    pub shell: Option<String>,
    /// The session's last prompt or summary, for telling sessions apart.
    pub title: Option<String>,
    pub captured_unix_seconds: Option<i64>,
}

#[derive(Debug, Serialize)]
struct Registry<'a> {
    /// Bumped when a consumer would have to be changed, never for an added field.
    format_version: u32,
    written_unix_seconds: i64,
    sessions: &'a [AgentSessionRecord],
}

const FORMAT_VERSION: u32 = 1;

fn registry_dir() -> PathBuf {
    warp_core::paths::state_dir()
}

/// Every CLI agent session in this app state, in window, tab, then pane order.
pub(crate) fn collect(app_state: &AppState) -> Vec<AgentSessionRecord> {
    let mut records = Vec::new();
    for (window_index, window) in app_state.windows.iter().enumerate() {
        for (tab_index, tab) in window.tabs.iter().enumerate() {
            collect_node(window_index, tab_index, tab, &tab.root, &mut records);
        }
    }
    records
}

fn collect_node(
    window_index: usize,
    tab_index: usize,
    tab: &TabSnapshot,
    node: &PaneNodeSnapshot,
    records: &mut Vec<AgentSessionRecord>,
) {
    match node {
        PaneNodeSnapshot::Branch(branch) => {
            for (_, child) in &branch.children {
                collect_node(window_index, tab_index, tab, child, records);
            }
        }
        PaneNodeSnapshot::Leaf(leaf) => {
            let LeafContents::Terminal(terminal) = &leaf.contents else {
                return;
            };
            if let Some(record) = record_for(window_index, tab_index, tab, terminal) {
                records.push(record);
            }
        }
    }
}

fn record_for(
    window_index: usize,
    tab_index: usize,
    tab: &TabSnapshot,
    terminal: &TerminalPaneSnapshot,
) -> Option<AgentSessionRecord> {
    let descriptor = terminal.cli_agent_resume_descriptor.as_ref()?;
    Some(AgentSessionRecord {
        window_index,
        tab_index,
        tab_title: tab.custom_title.clone(),
        pane_uuid: hex::encode(&terminal.uuid),
        agent: descriptor.agent.display_name().to_owned(),
        session_id: descriptor.session_id.clone(),
        resume_command: descriptor.resume_command(),
        cwd: descriptor.cwd.to_string_lossy().into_owned(),
        shell: terminal
            .shell_launch_data
            .as_ref()
            .map(|shell| shell.shell_detail()),
        title: descriptor.title.clone(),
        captured_unix_seconds: descriptor.captured_unix_seconds,
    })
}

/// Replaces the registry files with a record of `app_state`.
///
/// Errors are the caller's to log and swallow: this is a convenience alongside the real
/// persistence, and failing to write it must never fail a save.
pub(crate) fn write(app_state: &AppState) -> Result<()> {
    write_to_dir(&registry_dir(), &collect(app_state))
}

fn write_to_dir(dir: &Path, records: &[AgentSessionRecord]) -> Result<()> {
    fs::create_dir_all(dir).context("Failed to create the agent session registry directory")?;

    let registry = Registry {
        format_version: FORMAT_VERSION,
        written_unix_seconds: chrono::Utc::now().timestamp(),
        sessions: records,
    };
    let json = serde_json::to_string_pretty(&registry)
        .context("Failed to encode the agent session registry")?;

    write_atomically(&dir.join(JSON_FILE_NAME), json.as_bytes())?;
    write_atomically(
        &dir.join(MARKDOWN_FILE_NAME),
        render_markdown(records).as_bytes(),
    )
}

/// Writes through a sibling temporary file so a reader never sees a half-written registry, and a
/// crash mid-write leaves the previous one intact rather than an empty file where the session IDs
/// used to be.
fn write_atomically(path: &Path, contents: &[u8]) -> Result<()> {
    let temp_path = path.with_extension("tmp");
    {
        let mut file = fs::File::create(&temp_path)
            .with_context(|| format!("Failed to create {}", temp_path.display()))?;
        file.write_all(contents)
            .with_context(|| format!("Failed to write {}", temp_path.display()))?;
        file.flush()
            .with_context(|| format!("Failed to flush {}", temp_path.display()))?;
    }
    fs::rename(&temp_path, path).with_context(|| format!("Failed to replace {}", path.display()))
}

fn render_markdown(records: &[AgentSessionRecord]) -> String {
    let mut out = String::from("# CLI agent sessions\n\n");
    out.push_str(
        "Written by WarpSpeed every time it saves its window state, describing that state and \
         nothing earlier. Each entry is a pane that was running a CLI agent when the state was \
         saved.\n\n",
    );

    if records.is_empty() {
        out.push_str("No pane was running a CLI agent.\n");
        return out;
    }

    for record in records {
        out.push_str(&format!(
            "## {} in window {}, tab {}{}\n\n",
            record.agent,
            record.window_index + 1,
            record.tab_index + 1,
            record
                .tab_title
                .as_deref()
                .map(|title| format!(" ({title})"))
                .unwrap_or_default(),
        ));
        if let Some(title) = &record.title {
            out.push_str(&format!("- Last prompt: {title}\n"));
        }
        out.push_str(&format!("- Directory: `{}`\n", record.cwd));
        if let Some(shell) = &record.shell {
            out.push_str(&format!("- Shell: `{shell}`\n"));
        }
        match &record.session_id {
            Some(session_id) => out.push_str(&format!("- Session ID: `{session_id}`\n")),
            None => out.push_str(
                "- Session ID: none was reported. Warp learns a session ID from the agent's Warp \
                 plugin, so install it to make this session resumable.\n",
            ),
        }
        out.push_str(&format!("- Pane UUID: `{}`\n", record.pane_uuid));
        match &record.resume_command {
            Some(command) => out.push_str(&format!("\n```\ncd {}\n{command}\n```\n\n", record.cwd)),
            None => out.push_str(
                "\nWarpSpeed knows no command that resumes this agent by session ID; use the \
                 agent's own session picker.\n\n",
            ),
        }
    }

    out
}

#[cfg(test)]
#[path = "agent_session_registry_tests.rs"]
mod tests;
