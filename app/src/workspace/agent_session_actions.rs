//! What clicking an agent-session row does.

use std::path::{Path, PathBuf};

use warp_util::path::ShellFamily;

use crate::app_state::CliAgentResumeDescriptor;
use crate::terminal::agent_sessions_index::{AgentSessionRow, AgentSessionState};
use crate::terminal::cli_agent::CLIAgent;
use crate::terminal::shell::ShellType;
use crate::workspace::PaneViewLocator;

/// What a row's click does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentSessionAction {
    Focus(PaneViewLocator),
    Start(PaneViewLocator),
    Attach(warp_core::SessionId),
    /// `target` is the pane to resume in, or to split beside when it is busy.
    Resume {
        agent: CLIAgent,
        session_id: String,
        cwd: PathBuf,
        target: Option<PaneViewLocator>,
    },
}

/// The action for `row`, or `None` when nothing can reopen it.
pub(crate) fn action_for(
    row: &AgentSessionRow,
    target: Option<PaneViewLocator>,
) -> Option<AgentSessionAction> {
    match &row.state {
        AgentSessionState::Running { locator } => Some(AgentSessionAction::Focus(*locator)),
        AgentSessionState::Pending { locator } => Some(AgentSessionAction::Start(*locator)),
        AgentSessionState::Detached { session_id } => Some(AgentSessionAction::Attach(*session_id)),
        AgentSessionState::Past => {
            let agent = row.agent?;
            let session_id = row.session_id.clone()?;
            resume_descriptor(agent, &session_id, &row.cwd).resume_command()?;
            Some(AgentSessionAction::Resume {
                agent,
                session_id,
                cwd: row.cwd.clone(),
                target,
            })
        }
    }
}

pub(crate) fn resume_descriptor(
    agent: CLIAgent,
    session_id: &str,
    cwd: &Path,
) -> CliAgentResumeDescriptor {
    CliAgentResumeDescriptor {
        agent,
        session_id: Some(session_id.to_owned()),
        cwd: cwd.to_path_buf(),
        title: None,
        captured_unix_seconds: None,
    }
}

/// `cd <cwd>` joined to `resume` with the shell's separator.
pub(crate) fn resume_command_line(cwd: &Path, resume: &str, shell: ShellKind) -> String {
    let cwd = cwd.to_string_lossy();
    let cwd = shell.family().shell_escape(&cwd);
    let separator = match shell {
        ShellKind::Posix => " && ",
        ShellKind::Fish => "; and ",
        ShellKind::PowerShell => "; ",
    };
    format!("cd {cwd}{separator}{resume}")
}

/// The shells as far as joining two commands goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShellKind {
    Posix,
    Fish,
    PowerShell,
}

impl ShellKind {
    fn family(self) -> ShellFamily {
        match self {
            ShellKind::Posix | ShellKind::Fish => ShellFamily::Posix,
            ShellKind::PowerShell => ShellFamily::PowerShell,
        }
    }
}

impl From<ShellType> for ShellKind {
    fn from(shell: ShellType) -> Self {
        match shell {
            ShellType::Zsh | ShellType::Bash => ShellKind::Posix,
            ShellType::Fish => ShellKind::Fish,
            ShellType::PowerShell => ShellKind::PowerShell,
        }
    }
}

impl From<ShellFamily> for ShellKind {
    /// Fish is indistinguishable from the other POSIX shells here; fish 3 accepts `&&` too.
    fn from(family: ShellFamily) -> Self {
        match family {
            ShellFamily::Posix => ShellKind::Posix,
            ShellFamily::PowerShell => ShellKind::PowerShell,
        }
    }
}

#[cfg(test)]
#[path = "agent_session_actions_tests.rs"]
mod tests;
