#![expect(
    dead_code,
    reason = "the move-to-AgentCtrl action that calls this is not wired yet"
)]

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail, ensure};
use chrono::{DateTime, Local};
use command::blocking::Command;
use managed_agent_sessions::MAX_SESSION_FILE_BYTES;
use tempfile::TempDir;

use crate::terminal::CLIAgent;

const MAX_SESSION_ID_CHARS: usize = 128;
const BRANCH_ID_CHARS: usize = 8;
const PREFERRED_REMOTES: [&str; 2] = ["gitea", "origin"];

/// A local agent session the user asked to move into a managed run.
#[derive(Clone, Debug)]
pub(crate) struct HandoffSource {
    pub agent: CLIAgent,
    pub session_id: String,
    pub cwd: PathBuf,
}

/// Everything the import request needs once the handoff branch has been pushed.
#[derive(Clone, Debug)]
pub(crate) struct PreparedHandoff {
    pub agent_name: &'static str,
    pub session_file: Vec<u8>,
    pub repo_url: String,
    pub remote: String,
    pub branch: String,
    pub commit: String,
}

/// Where each supported agent keeps its session files.
#[derive(Clone, Debug)]
pub(crate) struct AgentSessionRoots {
    pub claude_config: PathBuf,
    pub pi_agent_dir: PathBuf,
    /// Pi's `PI_CODING_AGENT_SESSION_DIR`, which holds session files directly rather than in
    /// per-project directories.
    pub pi_session_dir: Option<PathBuf>,
    pub codex_home: PathBuf,
}

impl AgentSessionRoots {
    /// The default locations under `home`, ignoring environment overrides.
    pub(crate) fn under_home(home: &Path) -> Self {
        Self {
            claude_config: home.join(".claude"),
            pi_agent_dir: home.join(".pi").join("agent"),
            pi_session_dir: None,
            codex_home: home.join(".codex"),
        }
    }

    /// The locations the agents themselves would use, honoring their environment overrides.
    pub(crate) fn from_env(home: &Path) -> Self {
        let env_dir = |key| {
            std::env::var_os(key)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let defaults = Self::under_home(home);
        Self {
            claude_config: env_dir("CLAUDE_CONFIG_DIR").unwrap_or(defaults.claude_config),
            pi_agent_dir: env_dir("PI_CODING_AGENT_DIR").unwrap_or(defaults.pi_agent_dir),
            pi_session_dir: env_dir("PI_CODING_AGENT_SESSION_DIR"),
            codex_home: env_dir("CODEX_HOME").unwrap_or(defaults.codex_home),
        }
    }
}

pub(crate) fn moved_agent_name(agent: CLIAgent) -> Option<&'static str> {
    match agent {
        CLIAgent::Claude => Some("staging-claude-moved"),
        CLIAgent::Pi => Some("staging-pi-moved"),
        CLIAgent::Codex => Some("staging-codex-moved"),
        CLIAgent::Gemini
        | CLIAgent::Amp
        | CLIAgent::Droid
        | CLIAgent::OpenCode
        | CLIAgent::Copilot
        | CLIAgent::OhMyPi
        | CLIAgent::Auggie
        | CLIAgent::CursorCli
        | CLIAgent::Goose
        | CLIAgent::Hermes
        | CLIAgent::Vibe
        | CLIAgent::Antigravity
        | CLIAgent::WarpTui
        | CLIAgent::Unknown => None,
    }
}

/// Claude's `projects/` directory name for `cwd`.
pub(crate) fn claude_project_slug(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Pi's per-project session directory name for `cwd`.
pub(crate) fn pi_session_dir_name(cwd: &Path) -> String {
    let cwd = cwd.to_string_lossy();
    let cwd = cwd.strip_prefix(['/', '\\']).unwrap_or(&cwd);
    format!("--{}--", cwd.replace(['/', '\\', ':'], "-"))
}

fn is_valid_session_id(session_id: &str) -> bool {
    (1..=MAX_SESSION_ID_CHARS).contains(&session_id.len())
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn file_in_dir_ending_with(dir: &Path, suffix: &str) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.ends_with(suffix))
        })
}

fn file_under_ending_with(root: &Path, suffix: &str) -> Option<PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .flatten()
        .filter(|entry| entry.file_type().is_file())
        .find(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(suffix))
        })
        .map(walkdir::DirEntry::into_path)
}

/// Locates the session file `agent` wrote for `session_id` while running in `cwd`.
pub(crate) fn session_file_path(
    agent: CLIAgent,
    session_id: &str,
    cwd: &Path,
    roots: &AgentSessionRoots,
) -> Result<PathBuf> {
    ensure!(
        is_valid_session_id(session_id),
        "invalid session id {session_id:?}"
    );
    match agent {
        CLIAgent::Claude => {
            let path = roots
                .claude_config
                .join("projects")
                .join(claude_project_slug(cwd))
                .join(format!("{session_id}.jsonl"));
            ensure!(
                path.is_file(),
                "no claude session file at {}",
                path.display()
            );
            Ok(path)
        }
        CLIAgent::Pi => {
            let dir = roots.pi_session_dir.clone().unwrap_or_else(|| {
                roots
                    .pi_agent_dir
                    .join("sessions")
                    .join(pi_session_dir_name(cwd))
            });
            file_in_dir_ending_with(&dir, &format!("_{session_id}.jsonl"))
                .ok_or_else(|| anyhow!("no pi session file for {session_id} in {}", dir.display()))
        }
        CLIAgent::Codex => {
            let sessions = roots.codex_home.join("sessions");
            file_under_ending_with(&sessions, &format!("-{session_id}.jsonl")).ok_or_else(|| {
                anyhow!(
                    "no codex session file for {session_id} under {}",
                    sessions.display()
                )
            })
        }
        CLIAgent::Gemini
        | CLIAgent::Amp
        | CLIAgent::Droid
        | CLIAgent::OpenCode
        | CLIAgent::Copilot
        | CLIAgent::OhMyPi
        | CLIAgent::Auggie
        | CLIAgent::CursorCli
        | CLIAgent::Goose
        | CLIAgent::Hermes
        | CLIAgent::Vibe
        | CLIAgent::Antigravity
        | CLIAgent::WarpTui
        | CLIAgent::Unknown => bail!("{agent:?} sessions cannot be moved"),
    }
}

pub(crate) fn handoff_branch_name(now: DateTime<Local>, session_id: &str) -> String {
    let short_id = session_id
        .char_indices()
        .nth(BRANCH_ID_CHARS)
        .map_or(session_id, |(end, _)| &session_id[..end]);
    format!("handoff/{}-{short_id}", now.format("%Y%m%d-%H%M"))
}

fn git<I, S>(repo: &Path, args: I, envs: &[(&str, &OsStr)]) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_owned())
        .collect::<Vec<_>>();
    let mut command = Command::new("git");
    command
        .args(&args)
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0");
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command
        .output()
        .with_context(|| format!("failed to run git {args:?}"))?;
    ensure!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Commits the working tree of `repo` (tracked changes plus untracked, non-ignored files) on top
/// of `HEAD` without touching the branch, the index, or the working tree. Returns the commit sha.
pub(crate) fn create_handoff_commit(repo: &Path, session_id: &str) -> Result<String> {
    // A private index lets `git add -A` snapshot everything while the user's staging area stays
    // exactly as they left it.
    let index_dir = TempDir::new().context("failed to create a temporary git index")?;
    let index = index_dir.path().join("index");
    let envs = [("GIT_INDEX_FILE", index.as_os_str())];
    git(repo, ["read-tree", "HEAD"], &envs)?;
    git(repo, ["add", "-A"], &envs)?;
    let tree = git(repo, ["write-tree"], &envs)?;
    git(
        repo,
        [
            "commit-tree",
            &tree,
            "-p",
            "HEAD",
            "-m",
            &format!("WarpSpeed handoff of {session_id}"),
        ],
        &envs,
    )
}

fn select_remote(repo: &Path) -> Result<String> {
    let remotes = git(repo, ["remote"], &[])?;
    PREFERRED_REMOTES
        .into_iter()
        .find(|preferred| remotes.lines().any(|remote| remote.trim() == *preferred))
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("{} has no `gitea` or `origin` remote", repo.display()))
}

fn read_session_file(path: &Path) -> Result<Vec<u8>> {
    let size = fs::metadata(path)
        .with_context(|| format!("failed to stat {}", path.display()))?
        .len();
    ensure!(
        size <= MAX_SESSION_FILE_BYTES as u64,
        "session file {} is too large ({size} bytes, limit {MAX_SESSION_FILE_BYTES})",
        path.display()
    );
    let content = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    ensure!(
        content.len() <= MAX_SESSION_FILE_BYTES,
        "session file {} is too large ({} bytes, limit {MAX_SESSION_FILE_BYTES})",
        path.display(),
        content.len()
    );
    Ok(content)
}

/// Reads the session file, commits the working tree to a handoff branch, and pushes it. Blocks on
/// filesystem and git work, so it must run off the main thread.
pub(crate) fn prepare_handoff(
    source: &HandoffSource,
    home: &Path,
    now: DateTime<Local>,
) -> Result<PreparedHandoff> {
    prepare_handoff_with_roots(source, &AgentSessionRoots::from_env(home), now)
}

fn prepare_handoff_with_roots(
    source: &HandoffSource,
    roots: &AgentSessionRoots,
    now: DateTime<Local>,
) -> Result<PreparedHandoff> {
    let agent_name = moved_agent_name(source.agent)
        .ok_or_else(|| anyhow!("{:?} sessions cannot be moved", source.agent))?;
    let path = session_file_path(source.agent, &source.session_id, &source.cwd, roots)?;
    let session_file = read_session_file(&path)?;

    let repo = source.cwd.as_path();
    let remote = select_remote(repo)?;
    let repo_url = git(repo, ["remote", "get-url", remote.as_str()], &[])?;
    let commit = create_handoff_commit(repo, &source.session_id)?;
    let branch = handoff_branch_name(now, &source.session_id);
    git(
        repo,
        [
            "push".to_owned(),
            remote.clone(),
            format!("{commit}:refs/heads/{branch}"),
        ],
        &[],
    )
    .with_context(|| format!("failed to push {branch} to {remote}"))?;

    Ok(PreparedHandoff {
        agent_name,
        session_file,
        repo_url,
        remote,
        branch,
        commit,
    })
}

#[cfg(test)]
#[path = "agent_session_handoff_tests.rs"]
mod tests;
