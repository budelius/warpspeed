use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail, ensure};
use chrono::{DateTime, Local};
use command::blocking::Command;
use managed_agent_sessions::{
    MAX_SESSION_FILE_BYTES, RepositoryInfo, is_node_at_capacity, normalize_repo_url,
    redact_url_credentials,
};
use tempfile::TempDir;

use super::agent_session_history::current_dsh_log;
use super::agent_session_paths::{
    dsh_home_override, kimi_home_override, kimi_indexed_session_dirs,
};
use crate::terminal::CLIAgent;

const MAX_SESSION_ID_CHARS: usize = 128;
const BRANCH_ID_CHARS: usize = 8;
const PREFERRED_REMOTES: [&str; 3] = ["gitea", "origin", "gitlab"];

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
    /// [`session_content_len`] of `session_file_path` as uploaded.
    pub session_len: u64,
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

/// A repository configured on AgentCtrl that a pane offered for a new run, waiting for the user to
/// pick the agent.
#[derive(Clone, Debug)]
pub(crate) struct AgentCtrlStartOffer {
    /// The pane's remote URL, without userinfo.
    pub repo_url: String,
    pub repo_name: String,
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
    /// `KIMI_CODE_HOME`, by default `~/.kimi-code`.
    pub kimi_home: PathBuf,
    /// `DSH_HOME`, by default `~/.dsh`.
    pub dsh_home: PathBuf,
}

impl AgentSessionRoots {
    /// The default locations under `home`, ignoring environment overrides.
    pub(crate) fn under_home(home: &Path) -> Self {
        Self {
            claude_config: home.join(".claude"),
            pi_agent_dir: home.join(".pi").join("agent"),
            pi_session_dir: None,
            codex_home: home.join(".codex"),
            kimi_home: home.join(".kimi-code"),
            dsh_home: home.join(".dsh"),
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
            kimi_home: kimi_home_override(std::env::var_os("KIMI_CODE_HOME"))
                .unwrap_or(defaults.kimi_home),
            dsh_home: dsh_home_override(std::env::var_os("DSH_HOME")).unwrap_or(defaults.dsh_home),
        }
    }
}

pub(crate) fn agent_ctrl_profile(agent: CLIAgent) -> Option<&'static str> {
    match agent {
        CLIAgent::Claude => managed_agent_sessions::agent_ctrl_profile("claude"),
        CLIAgent::Pi => managed_agent_sessions::agent_ctrl_profile("pi"),
        CLIAgent::Codex => managed_agent_sessions::agent_ctrl_profile("codex"),
        CLIAgent::Kimi => managed_agent_sessions::agent_ctrl_profile("kimi"),
        CLIAgent::DeepSeekHarness => managed_agent_sessions::agent_ctrl_profile("dsh"),
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
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
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

fn has_parent_dir(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir))
}

fn is_named(path: &Path, name: &str) -> bool {
    path.file_name().and_then(OsStr::to_str) == Some(name)
}

/// Kimi's directory for session `session_id`: the one Kimi's `session_index.jsonl` records for it
/// when that is under `sessions/`, else the workspace directory under `sessions/` that holds it.
fn kimi_session_dir(kimi_home: &Path, session_id: &str) -> Option<PathBuf> {
    let sessions = kimi_home.join("sessions");
    let is_session_dir = |dir: &Path| {
        !has_parent_dir(dir)
            && dir.starts_with(&sessions)
            && is_named(dir, session_id)
            && dir.is_dir()
    };
    kimi_indexed_session_dirs(kimi_home)
        .into_iter()
        .flatten()
        .find_map(|(id, dir)| (id == session_id && is_session_dir(&dir)).then_some(dir))
        .or_else(|| {
            fs::read_dir(&sessions)
                .ok()?
                .flatten()
                .map(|workspace| workspace.path().join(session_id))
                .find(|dir| dir.is_dir())
        })
}

/// dsh keeps a session at `sessions/<project key>/<id>/`, and the project key is lossy, so the
/// directory is found by id under any project. dsh reads only the highest generation there.
fn dsh_session_file(dsh_home: &Path, session_id: &str) -> Option<PathBuf> {
    fs::read_dir(dsh_home.join("sessions"))
        .ok()?
        .flatten()
        .map(|project| project.path().join(session_id))
        .filter(|dir| dir.is_dir())
        .find_map(|dir| current_dsh_log(&dir).map(|(path, _)| path))
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
        CLIAgent::Kimi => kimi_session_dir(&roots.kimi_home, session_id).ok_or_else(|| {
            anyhow!(
                "no kimi session {session_id} under {}",
                roots.kimi_home.join("sessions").display()
            )
        }),
        CLIAgent::DeepSeekHarness => {
            dsh_session_file(&roots.dsh_home, session_id).ok_or_else(|| {
                anyhow!(
                    "no dsh session file for {session_id} under {}",
                    roots.dsh_home.join("sessions").display()
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
        CLIAgent::Kimi => Some(roots.kimi_home.join("sessions")),
        CLIAgent::DeepSeekHarness => Some(roots.dsh_home.join("sessions")),
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

/// Whether `path`, as reported by the agent's hooks, is this session's file (Kimi: its
/// directory). The path arrives through terminal output or the agent-event channel, so it is
/// trusted only for an entry named after the session under the agent's own session directory;
/// anything else could make the move upload unrelated data.
fn is_reported_session_file(
    path: &Path,
    source: &HandoffSource,
    roots: &AgentSessionRoots,
) -> bool {
    let id = source.session_id.as_str();
    let named_for_session = match source.agent {
        CLIAgent::Kimi => is_named(path, id) && path.is_dir(),
        CLIAgent::DeepSeekHarness => {
            path.parent().is_some_and(|dir| is_named(dir, id))
                && path
                    .parent()
                    .and_then(current_dsh_log)
                    .is_some_and(|(current, _)| current == path)
                && path.is_file()
        }
        CLIAgent::Claude
        | CLIAgent::Pi
        | CLIAgent::Codex
        | CLIAgent::Gemini
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
        | CLIAgent::Unknown => {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.ends_with(&format!("{id}.jsonl")))
                && path.is_file()
        }
    };
    is_valid_session_id(id)
        && !has_parent_dir(path)
        && agent_session_root(source.agent, roots).is_some_and(|root| path.starts_with(root))
        && named_for_session
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
    // Kimi and dsh ids start with a fixed `session_`/`session-`, which would name every branch alike.
    let distinct = ["session_", "session-"]
        .into_iter()
        .find_map(|prefix| session_id.strip_prefix(prefix))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(session_id);
    let short_id = distinct
        .char_indices()
        .nth(BRANCH_ID_CHARS)
        .map_or(distinct, |(end, _)| &distinct[..end]);
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

/// The remote a handoff branch is pushed to: `gitea`, `origin`, or `gitlab`, in that order.
pub(crate) fn select_remote(repo: &Path) -> Result<String> {
    let remotes = git(repo, ["remote"], &[])?;
    PREFERRED_REMOTES
        .into_iter()
        .find(|preferred| remotes.lines().any(|remote| remote.trim() == *preferred))
        .map(str::to_owned)
        .ok_or_else(|| {
            anyhow!(
                "{} has no `gitea`, `origin`, or `gitlab` remote",
                repo.display()
            )
        })
}

/// The remote [`select_remote`] picks for the repository containing `cwd`, and its URL without
/// userinfo.
pub(crate) fn repo_url_for(cwd: &Path) -> Result<(String, String)> {
    let remote = select_remote(cwd)?;
    let url = git(cwd, ["remote", "get-url", remote.as_str()], &[])?;
    Ok((remote, strip_url_userinfo(&url)))
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

/// The bytes a move uploads for a session, and the [`session_content_len`] they were read at.
struct SessionPayload {
    bytes: Vec<u8>,
    content_len: u64,
}

/// Lengths only: the bytes are the user's whole session.
impl std::fmt::Debug for SessionPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionPayload")
            .field("bytes", &self.bytes.len())
            .field("content_len", &self.content_len)
            .finish()
    }
}

/// The combined size of the regular files at `path`: the file itself, or every file under a
/// session directory. Agents only ever append, so growth means the session moved on. `None` when
/// it cannot be read.
pub(crate) fn session_content_len(path: &Path) -> Option<u64> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_dir() {
        return Some(metadata.len());
    }
    let mut total = 0;
    for entry in walkdir::WalkDir::new(path) {
        let entry = entry.ok()?;
        if entry.file_type().is_file() {
            total += entry.metadata().ok()?.len();
        }
    }
    Some(total)
}

/// Packs Kimi session directory `dir` as a tar of `<session_id>/<relative path>` regular files,
/// the layout the AgentCtrl node unpacks. Symlinks are skipped.
fn pack_kimi_session(dir: &Path, session_id: &str) -> Result<SessionPayload> {
    let too_large = |len: u64| {
        anyhow!(
            "session {} is too large ({len} bytes, limit {MAX_SESSION_FILE_BYTES})",
            dir.display()
        )
    };
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry.with_context(|| format!("failed to read {}", dir.display()))?;
        if entry.file_type().is_file() {
            let len = entry
                .metadata()
                .with_context(|| format!("failed to stat {}", entry.path().display()))?
                .len();
            files.push((entry.into_path(), len));
        }
    }
    ensure!(
        files
            .iter()
            .any(|(path, _)| *path == dir.join("state.json")),
        "{} has no state.json",
        dir.display()
    );
    let listed: u64 = files.iter().map(|(_, len)| len).sum();
    if listed > MAX_SESSION_FILE_BYTES as u64 {
        return Err(too_large(listed));
    }
    let mut archive = tar::Builder::new(Vec::new());
    let mut content_len = 0;
    for (path, _) in &files {
        let content =
            fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
        content_len += content.len() as u64;
        if content_len > MAX_SESSION_FILE_BYTES as u64 {
            return Err(too_large(content_len));
        }
        let name = std::iter::once(session_id.to_owned())
            .chain(
                path.strip_prefix(dir)?
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy().into_owned()),
            )
            .collect::<Vec<_>>()
            .join("/");
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        archive
            .append_data(&mut header, &name, content.as_slice())
            .with_context(|| format!("failed to pack {}", path.display()))?;
    }
    let bytes = archive
        .into_inner()
        .context("failed to finish the session archive")?;
    if bytes.len() > MAX_SESSION_FILE_BYTES {
        return Err(too_large(bytes.len() as u64));
    }
    Ok(SessionPayload { bytes, content_len })
}

/// What a move uploads for the session at `path`: a Kimi session directory as a tar, any other
/// agent's session file as is.
fn read_session_payload(source: &HandoffSource, path: &Path) -> Result<SessionPayload> {
    if path.is_dir() {
        return pack_kimi_session(path, &source.session_id);
    }
    let bytes = read_session_file(path)?;
    Ok(SessionPayload {
        content_len: bytes.len() as u64,
        bytes,
    })
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
    let agent_name = agent_ctrl_profile(source.agent)
        .ok_or_else(|| anyhow!("{:?} sessions cannot be moved", source.agent))?;
    let path = resolve_session_file(source, roots)?;
    let payload = read_session_payload(source, &path)?;

    let repo = source.cwd.as_path();
    let (remote, repo_url) = repo_url_for(repo)?;
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
        session_file: payload.bytes,
        session_file_path: path,
        session_len: payload.content_len,
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
    /// `pushed_branch` is the handoff branch as `<remote>/<branch>`, here and in `Done`.
    CuttingOver {
        job_id: String,
        pushed_branch: String,
    },
    Done {
        job_id: String,
        pushed_branch: String,
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
        MoveStep::CuttingOver { job_id, .. } | MoveStep::Done { job_id, .. } => format!(
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

/// The warning shown for the whole of a move of `agent`'s session, when the agent is known.
pub(crate) fn move_progress_message(agent: Option<CLIAgent>) -> String {
    let session = agent.map_or_else(
        || "the agent session".to_owned(),
        |agent| format!("{} session", agent.display_name()),
    );
    format!("Moving {session} to AgentCtrl… don't use the agent until this finishes.")
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

/// The repository among those configured on AgentCtrl that `repo_url` names, compared the way the
/// node compares them.
pub(crate) fn configured_repository<'a>(
    repo_url: &str,
    repositories: &'a [RepositoryInfo],
) -> Option<&'a RepositoryInfo> {
    let repo_url = normalize_repo_url(repo_url);
    repositories
        .iter()
        .find(|repository| normalize_repo_url(&repository.url) == repo_url)
}

/// The question asked before starting an agent on AgentCtrl in `repository`, reached through the
/// pane's `remote`.
pub(crate) fn start_agent_prompt(remote: &str, repository: &RepositoryInfo) -> String {
    format!(
        "Start which agent on {remote} {} ({})?",
        repository.name, repository.branch
    )
}

/// The notice shown while a run of `agent` is being started on AgentCtrl.
pub(crate) fn start_agent_progress_message(agent: CLIAgent) -> String {
    format!("Starting {} on AgentCtrl…", agent.display_name())
}

/// Why the pane's repository cannot be offered for a start on AgentCtrl.
pub(crate) fn start_agent_unavailable_message(error: &anyhow::Error) -> String {
    format!(
        "Cannot start an agent on AgentCtrl from this pane: {}",
        redact_url_credentials(&format!("{error:#}"))
    )
}

/// The message shown when starting an agent on AgentCtrl failed with `error`.
pub(crate) fn start_agent_failure_message(error: &anyhow::Error) -> String {
    if is_node_at_capacity(error) {
        "AgentCtrl node is at capacity; stop a run or try later".to_owned()
    } else {
        format!(
            "Could not start the agent on AgentCtrl: {}",
            redact_url_credentials(&format!("{error:#}"))
        )
    }
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
        CLIAgent::Claude | CLIAgent::Codex | CLIAgent::Kimi => Some("/exit"),
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
        | CLIAgent::DeepSeekHarness
        | CLIAgent::WarpTui
        | CLIAgent::Unknown => None,
    }
}

#[cfg(test)]
#[path = "agent_session_handoff_tests.rs"]
mod tests;
