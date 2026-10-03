use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

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
    /// The shell's working directory: the repo that is handed off, and where the agent was
    /// launched from.
    pub cwd: PathBuf,
    /// The session file the agent last reported through its hooks.
    pub transcript_path: Option<PathBuf>,
}

/// Everything the import request needs once the handoff branch has been pushed.
///
/// Deliberately not `Debug`: it holds the whole session file and a remote URL that may carry
/// credentials.
pub(crate) struct PreparedHandoff {
    pub agent_name: &'static str,
    pub session_file: Vec<u8>,
    pub session_file_path: PathBuf,
    pub repo_url: String,
    pub remote: String,
    pub branch: String,
    pub commit: String,
}

/// What a move uploaded, so cut-over can tell whether the local session moved on since.
#[derive(Clone, Debug)]
pub(crate) struct UploadedSession {
    pub session_id: String,
    pub file_path: PathBuf,
    pub file_len: u64,
    /// The handoff branch as `<remote>/<branch>`.
    pub pushed_branch: String,
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

/// The directory under which `agent` keeps its session files.
fn agent_session_root(agent: CLIAgent, roots: &AgentSessionRoots) -> Option<PathBuf> {
    match agent {
        CLIAgent::Claude => Some(roots.claude_config.join("projects")),
        CLIAgent::Pi => Some(
            roots
                .pi_session_dir
                .clone()
                .unwrap_or_else(|| roots.pi_agent_dir.join("sessions")),
        ),
        CLIAgent::Codex => Some(roots.codex_home.join("sessions")),
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

/// Whether `path`, as reported by the agent's hooks, is this session's file. The path arrives
/// through terminal output, so it is trusted only for a file named after the session under the
/// agent's own session directory; anything else could make the move upload an unrelated file.
fn is_reported_session_file(
    path: &Path,
    source: &HandoffSource,
    roots: &AgentSessionRoots,
) -> bool {
    is_valid_session_id(&source.session_id)
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        && agent_session_root(source.agent, roots).is_some_and(|root| path.starts_with(root))
        && path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.ends_with(&format!("{}.jsonl", source.session_id)))
        && path.is_file()
}

/// The session file to upload for `source`. The reported transcript wins because the agent may
/// have changed directory since launch, while its session file stays keyed by the launch
/// directory.
pub(crate) fn resolve_session_file(
    source: &HandoffSource,
    roots: &AgentSessionRoots,
) -> Result<PathBuf> {
    match source
        .transcript_path
        .as_deref()
        .filter(|path| is_reported_session_file(path, source, roots))
    {
        Some(path) => Ok(path.to_path_buf()),
        None => session_file_path(source.agent, &source.session_id, &source.cwd, roots),
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
            "--no-gpg-sign",
            &tree,
            "-p",
            "HEAD",
            "-m",
            &format!("WarpSpeed handoff of {session_id}"),
        ],
        &envs,
    )
}

/// The remote a handoff branch is pushed to: `gitea` when present, else `origin`.
pub(crate) fn select_remote(repo: &Path) -> Result<String> {
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
    let path = resolve_session_file(source, roots)?;
    let session_file = read_session_file(&path)?;

    let repo = source.cwd.as_path();
    let remote = select_remote(repo)?;
    let repo_url = strip_url_userinfo(&git(repo, ["remote", "get-url", remote.as_str()], &[])?);
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
        session_file_path: path,
        repo_url,
        remote,
        branch,
        commit,
    })
}

/// `url` without `user[:password]@`, so a token embedded in a remote URL never leaves the machine.
/// Remotes that are not URLs with an authority (scp-style `git@host:path`, local paths) are
/// returned unchanged.
pub(crate) fn strip_url_userinfo(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_owned();
    };
    if !parsed.has_host() || (parsed.username().is_empty() && parsed.password().is_none()) {
        return url.to_owned();
    }
    if parsed.set_username("").is_err() || parsed.set_password(None).is_err() {
        return url.to_owned();
    }
    parsed.into()
}

/// How far a move to AgentCtrl has got. The local agent is only touched from `CuttingOver` on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MoveStep {
    Preparing,
    Importing,
    WaitingForResume {
        job_id: String,
    },
    CuttingOver {
        job_id: String,
    },
    Done {
        job_id: String,
    },
    Failed {
        reason: String,
        pushed_branch: Option<String>,
    },
}

/// The message shown when a move fails after reaching `step_reached`. `pushed_branch` is the
/// handoff branch as `<remote>/<branch>`, when it was pushed.
pub(crate) fn move_failure_message(
    step_reached: &MoveStep,
    error: &str,
    pushed_branch: Option<&str>,
) -> String {
    let error = redact_url_credentials(error);
    let leftover_branch = pushed_branch
        .map(|branch| format!(" The handoff branch {branch} was pushed and can be deleted."))
        .unwrap_or_default();
    let before_cut_over = |stage: &str| {
        format!(
            "Could not move the session to AgentCtrl while {stage}: {error}. The local session \
             is still running.{leftover_branch}"
        )
    };
    match step_reached {
        MoveStep::Preparing => before_cut_over("preparing the handoff"),
        MoveStep::Importing => before_cut_over("importing it"),
        MoveStep::WaitingForResume { job_id } => {
            before_cut_over(&format!("waiting for {job_id} to resume"))
        }
        MoveStep::CuttingOver { job_id } | MoveStep::Done { job_id } => format!(
            "Moved to AgentCtrl as {job_id}, but the local agent did not exit; close it manually. \
             Meanwhile the cloud run {job_id} is live; open it with: warpspeed agent-ctrl open {job_id}"
        ),
        MoveStep::Failed { reason, .. } => reason.clone(),
    }
}

/// The message shown when the local session file grew after it was uploaded, so the cloud run
/// would be missing that work.
pub(crate) fn session_changed_message(pushed_branch: Option<&str>) -> String {
    let leftover_branch = pushed_branch
        .map(|branch| format!(" The handoff branch {branch} was pushed and can be deleted."))
        .unwrap_or_default();
    format!(
        "The local session changed during the move to AgentCtrl, so it keeps running \
         here.{leftover_branch}"
    )
}

/// Whether the managed run of a failed move was asked to stop; `stop_error` is why the stop
/// request itself failed.
pub(crate) fn stop_request_outcome(job_id: &str, stop_error: Option<&str>) -> String {
    match stop_error {
        None => format!("The cloud run {job_id} was asked to stop."),
        Some(error) => format!(
            "WarpSpeed could not ask run {job_id} to stop ({}); stop it with: warpspeed \
             agent-ctrl stop {job_id}",
            redact_url_credentials(error)
        ),
    }
}

/// What cut-over does with the local agent once the managed run has resumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CutOver {
    /// The moved session is still the pane's agent and has not changed since the upload.
    ExitLocalAgent,
    /// The moved session no longer runs in the pane, so there is nothing to exit.
    LeaveLocalAgent,
    /// The session file grew after the upload (or cannot be read), so the managed run would be
    /// missing local work.
    SessionChanged,
}

/// Decides [`CutOver`] from the pane's current agent session id and the moved session file's
/// current length (`None` when it cannot be read), against what was uploaded.
pub(crate) fn cut_over_decision(
    current_session_id: Option<&str>,
    moved_session_id: &str,
    current_file_len: Option<u64>,
    uploaded_len: u64,
) -> CutOver {
    if current_file_len.is_none_or(|len| len > uploaded_len) {
        CutOver::SessionChanged
    } else if current_session_id == Some(moved_session_id) {
        CutOver::ExitLocalAgent
    } else {
        CutOver::LeaveLocalAgent
    }
}

/// Removes `user[:password]@` from every URL in `text`, since git and HTTP errors can echo a
/// remote URL that embeds a token.
fn redact_url_credentials(text: &str) -> String {
    let mut redacted = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(scheme_end) = rest.find("://") {
        let authority_start = scheme_end + 3;
        redacted.push_str(&rest[..authority_start]);
        rest = &rest[authority_start..];
        let authority_end = rest
            .find(|c: char| c == '/' || c == '\'' || c == '"' || c.is_whitespace())
            .unwrap_or(rest.len());
        if let Some(at) = rest[..authority_end].rfind('@') {
            rest = &rest[at + 1..];
        }
    }
    redacted.push_str(rest);
    redacted
}

/// The `Idempotency-Key` for importing `session_id` at handoff `commit`, kept within the server's
/// `[A-Za-z0-9._:-]{16,128}`.
pub(crate) fn move_idempotency_key(session_id: &str, commit: &str) -> String {
    let session_id = session_id
        .char_indices()
        .nth(100)
        .map_or(session_id, |(end, _)| &session_id[..end]);
    let commit = commit.get(..12).unwrap_or(commit);
    format!("move-{session_id}-{commit}")
}

/// What to type into a moved agent to make it exit.
pub(crate) fn agent_exit_command(agent: CLIAgent) -> Option<&'static str> {
    match agent {
        CLIAgent::Claude | CLIAgent::Codex => Some("/exit"),
        CLIAgent::Pi => Some("/quit"),
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

#[cfg(test)]
#[path = "agent_session_handoff_tests.rs"]
mod tests;
