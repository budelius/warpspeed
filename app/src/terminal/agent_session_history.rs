use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::terminal::CLIAgent;

const MAX_CANDIDATES: usize = 200;
const MAX_HEAD_LINES: usize = 200;
const MAX_HEAD_BYTES: usize = 256 * 1024;
const TITLE_MAX_CHARS: usize = 80;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PastAgentSession {
    pub agent: CLIAgent,
    pub session_id: String,
    pub cwd: PathBuf,
    pub title: Option<String>,
    pub modified: SystemTime,
}

pub(super) struct Candidate {
    agent: CLIAgent,
    path: PathBuf,
    session_id: String,
    modified: SystemTime,
}

fn read_head(path: &Path) -> Vec<Value> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let mut reader = BufReader::new(file);
    let mut entries = Vec::new();
    let mut consumed = 0;
    let mut line = String::new();
    while entries.len() < MAX_HEAD_LINES && consumed < MAX_HEAD_BYTES {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(read) => consumed += read,
        }
        if let Ok(value) = serde_json::from_str(line.trim()) {
            entries.push(value);
        }
    }
    entries
}

fn title(text: &str) -> Option<String> {
    let first = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let mut title = first.chars().take(TITLE_MAX_CHARS).collect::<String>();
    if first.chars().count() > TITLE_MAX_CHARS {
        title.push('…');
    }
    Some(title)
}

fn files(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let path = entry.path();
            if path.extension()? != "jsonl" {
                return None;
            }
            Some((path, entry.metadata().ok()?.modified().ok()?))
        })
        .collect()
}

fn directories(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_type().ok()?.is_dir().then(|| entry.path()))
        .collect()
}

fn is_safe_session_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn claude_candidates(root: &Path) -> Vec<Candidate> {
    directories(root)
        .into_iter()
        .flat_map(|project| files(&project))
        .filter_map(|(path, modified)| {
            let session_id = path.file_stem()?.to_str()?.to_owned();
            is_safe_session_id(&session_id).then_some(Candidate {
                agent: CLIAgent::Claude,
                path,
                session_id,
                modified,
            })
        })
        .collect()
}

pub(super) fn pi_candidates(root: &Path) -> Vec<Candidate> {
    directories(root)
        .into_iter()
        .flat_map(|project| files(&project))
        .filter_map(|(path, modified)| {
            let (_, session_id) = path.file_stem()?.to_str()?.rsplit_once('_')?;
            let session_id = session_id.to_owned();
            is_safe_session_id(&session_id).then_some(Candidate {
                agent: CLIAgent::Pi,
                path,
                session_id,
                modified,
            })
        })
        .collect()
}

fn vibe_sessions(root: &Path) -> Vec<PastAgentSession> {
    let Ok(file) = File::open(root.join(".session_index.json")) else {
        return Vec::new();
    };
    let Ok(Value::Object(index)) = serde_json::from_reader(BufReader::new(file)) else {
        return Vec::new();
    };
    index
        .into_values()
        .filter_map(|entry| {
            let session_id = entry["session_id"].as_str()?;
            if !is_safe_session_id(session_id) {
                return None;
            }
            let cwd = PathBuf::from(entry["cwd"].as_str()?);
            let modified = SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_nanos(entry["mtime_ns"].as_u64()?))?;
            Some(PastAgentSession {
                agent: CLIAgent::Vibe,
                session_id: session_id.to_owned(),
                cwd,
                title: entry["title"].as_str().and_then(title),
                modified,
            })
        })
        .collect()
}

fn codex_session_id(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?.strip_prefix("rollout-")?;
    let groups = stem.split('-').collect::<Vec<_>>();
    if groups.len() < 5 {
        return None;
    }
    let id = groups[groups.len() - 5..].join("-");
    uuid::Uuid::parse_str(&id).ok().map(|_| id)
}

fn codex_candidates(root: &Path) -> Vec<Candidate> {
    directories(root)
        .into_iter()
        .flat_map(|year| directories(&year))
        .flat_map(|month| directories(&month))
        .flat_map(|day| files(&day))
        .filter_map(|(path, modified)| {
            Some(Candidate {
                agent: CLIAgent::Codex,
                session_id: codex_session_id(&path)?,
                path,
                modified,
            })
        })
        .collect()
}

fn first_claude_text(content: &Value) -> Option<&str> {
    if let Some(text) = content.as_str() {
        return Some(text);
    }
    content
        .as_array()?
        .iter()
        .find(|part| part["type"] == "text")
        .and_then(|part| part["text"].as_str())
}

pub(super) fn describe(candidate: Candidate) -> PastAgentSession {
    let head = read_head(&candidate.path);
    let (session_id, cwd, title) = match candidate.agent {
        CLIAgent::Claude => {
            let cwd = head
                .iter()
                .find_map(|entry| entry["cwd"].as_str())
                .map(PathBuf::from)
                .unwrap_or_default();
            let title = head
                .iter()
                .filter(|entry| entry["message"]["role"] == "user")
                .find_map(|entry| first_claude_text(&entry["message"]["content"]))
                .and_then(title);
            (candidate.session_id, cwd, title)
        }
        CLIAgent::Codex => {
            let cwd = crate::ai::agent_sdk::driver::harness::codex_transcript::parse_session_meta(
                head.first(),
            )
            .map(|metadata| metadata.cwd)
            .unwrap_or_default();
            let title = head
                .iter()
                .find(|entry| {
                    entry["type"] == "event_msg" && entry["payload"]["type"] == "user_message"
                })
                .and_then(|entry| entry["payload"]["message"].as_str())
                .and_then(title);
            (candidate.session_id, cwd, title)
        }
        CLIAgent::Pi => {
            let session = head.iter().find(|entry| entry["type"] == "session");
            let session_id = session
                .and_then(|entry| entry["id"].as_str())
                .filter(|session_id| is_safe_session_id(session_id))
                .map(str::to_owned)
                .unwrap_or(candidate.session_id);
            let cwd = session
                .and_then(|entry| entry["cwd"].as_str())
                .map(PathBuf::from)
                .unwrap_or_default();
            let title = head
                .iter()
                .find(|entry| entry["type"] == "message" && entry["message"]["role"] == "user")
                .and_then(|entry| entry["message"]["content"][0]["text"].as_str())
                .and_then(title);
            (session_id, cwd, title)
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
        | CLIAgent::Unknown => (candidate.session_id, PathBuf::new(), None),
    };
    PastAgentSession {
        agent: candidate.agent,
        session_id,
        cwd,
        title,
        modified: candidate.modified,
    }
}

#[cfg(not(target_family = "wasm"))]
fn candidates(agent: CLIAgent) -> Vec<Candidate> {
    match agent {
        CLIAgent::Claude => {
            crate::ai::agent_sdk::driver::harness::claude_transcript::claude_config_dir()
                .ok()
                .map(|root| claude_candidates(&root.join("projects")))
                .unwrap_or_default()
        }
        CLIAgent::Codex => {
            crate::ai::agent_sdk::driver::harness::codex_transcript::codex_sessions_root()
                .ok()
                .map(|root| codex_candidates(&root))
                .unwrap_or_default()
        }
        CLIAgent::Pi => dirs::home_dir()
            .map(|home| pi_candidates(&home.join(".pi").join("agent").join("sessions")))
            .unwrap_or_default(),
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
        | CLIAgent::Unknown => Vec::new(),
    }
}

#[cfg(target_family = "wasm")]
fn candidates(_agent: CLIAgent) -> Vec<Candidate> {
    Vec::new()
}

fn same_directory(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    if let (Ok(left), Ok(right)) = (left.canonicalize(), right.canonicalize()) {
        return left == right;
    }
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .replace('\\', "/")
            .eq_ignore_ascii_case(&right.to_string_lossy().replace('\\', "/"))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Whether `path` is `folder` or lies beneath it, normalized as `same_directory` does.
pub(crate) fn is_within(path: &Path, folder: &Path) -> bool {
    let (path, folder) = match (path.canonicalize(), folder.canonicalize()) {
        (Ok(path), Ok(folder)) => (path, folder),
        _ => (path.to_path_buf(), folder.to_path_buf()),
    };
    #[cfg(windows)]
    {
        let normalize = |path: &Path| path.to_string_lossy().replace('\\', "/").to_lowercase();
        let (path, folder) = (normalize(&path), normalize(&folder));
        let folder = folder.trim_end_matches('/');
        path == folder
            || path
                .strip_prefix(folder)
                .is_some_and(|rest| rest.starts_with('/'))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(folder)
    }
}

fn scan(agent: CLIAgent, not_before: Option<SystemTime>) -> Vec<PastAgentSession> {
    let mut sessions = if agent == CLIAgent::Vibe {
        #[cfg(not(target_family = "wasm"))]
        {
            dirs::home_dir()
                .map(|home| vibe_sessions(&home.join(".vibe").join("logs").join("session")))
                .unwrap_or_default()
        }
        #[cfg(target_family = "wasm")]
        {
            Vec::new()
        }
    } else {
        let mut candidates = candidates(agent)
            .into_iter()
            .filter(|candidate| not_before.is_none_or(|time| candidate.modified >= time))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| right.modified.cmp(&left.modified));
        candidates.truncate(MAX_CANDIDATES);
        candidates.into_iter().map(describe).collect()
    };
    sessions.retain(|session| not_before.is_none_or(|time| session.modified >= time));
    sessions.sort_by(|left, right| right.modified.cmp(&left.modified));
    sessions.truncate(MAX_CANDIDATES);
    sessions
}

pub(crate) fn find_all(agent: CLIAgent) -> Vec<PastAgentSession> {
    scan(agent, None)
}

pub(crate) const RESUMABLE_AGENTS: [CLIAgent; 3] =
    [CLIAgent::Claude, CLIAgent::Codex, CLIAgent::Pi];

/// Every resumable agent's past sessions, newest first.
pub(crate) fn find_all_resumable() -> Vec<PastAgentSession> {
    let mut sessions = RESUMABLE_AGENTS
        .into_iter()
        .flat_map(find_all)
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| right.modified.cmp(&left.modified));
    sessions
}

/// The sessions `agent` recorded in `cwd` since `not_before`.
pub(crate) fn find_recent(
    agent: CLIAgent,
    cwd: &Path,
    not_before: SystemTime,
) -> Vec<PastAgentSession> {
    scan(agent, Some(not_before))
        .into_iter()
        .filter(|session| same_directory(&session.cwd, cwd))
        .collect()
}

/// The session a command explicitly resumes, e.g. `claude --resume <id>`.
pub(crate) fn session_id_from_command(agent: CLIAgent, command: &str) -> Option<String> {
    let words = command
        .split_whitespace()
        .map(|word| word.trim_matches(['\'', '"']))
        .collect::<Vec<_>>();
    let argument_after = |flag: &str| {
        words
            .iter()
            .position(|word| *word == flag)
            .and_then(|index| words.get(index + 1))
            .filter(|value| !value.is_empty() && !value.starts_with('-'))
            .map(|value| (*value).to_owned())
    };
    match agent {
        CLIAgent::Claude | CLIAgent::Vibe => argument_after("--resume"),
        CLIAgent::Pi => argument_after("--session"),
        CLIAgent::Codex => words
            .windows(2)
            .find(|pair| pair[0] == "resume" && !pair[1].starts_with('-'))
            .map(|pair| pair[1].to_owned()),
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
        | CLIAgent::Antigravity
        | CLIAgent::WarpTui
        | CLIAgent::Unknown => None,
    }
}

/// How long before a snapshot an agent's conversation may have last been written to and still
/// be the one that pane was running. Agents sit idle for days between turns.
const RESTORE_LOOKBACK: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How long after a snapshot its agents may have kept writing: the snapshot is taken while they
/// run, and they run on until the window closes.
const RESTORE_GRACE: Duration = Duration::from_secs(60 * 60);

/// A pane whose snapshot records which agent it ran, but not which session.
pub(crate) struct UnidentifiedAgent {
    pub agent: CLIAgent,
    pub cwd: PathBuf,
    pub title: Option<String>,
}

/// Picks, for each unidentified agent, the past session it most plausibly was: same agent, same
/// directory, written to around when the snapshot was saved, and not `excluded` -- sessions
/// other panes already hold, or that an agent is still running. A session whose title matches
/// the pane's is preferred, then the most recently written one. No session is given to two
/// panes.
pub(crate) fn match_for_restore(
    agents: &[UnidentifiedAgent],
    saved_at: SystemTime,
    candidates: &[PastAgentSession],
    excluded: &HashSet<String>,
) -> Vec<Option<String>> {
    let earliest = saved_at
        .checked_sub(RESTORE_LOOKBACK)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let latest = saved_at + RESTORE_GRACE;
    let mut eligible = candidates
        .iter()
        .filter(|session| {
            session.modified >= earliest
                && session.modified <= latest
                && !excluded.contains(&session.session_id)
        })
        .collect::<Vec<_>>();
    eligible.sort_by(|left, right| right.modified.cmp(&left.modified));

    let mut used = HashSet::new();
    let mut matches = vec![None; agents.len()];
    for (index, wanted) in agents.iter().enumerate() {
        if let Some(title) = wanted.title.as_deref() {
            matches[index] = take_match(wanted, &eligible, &mut used, |session| {
                session.title.as_deref() == Some(title)
            });
        }
    }
    for (index, wanted) in agents.iter().enumerate() {
        if matches[index].is_none() {
            matches[index] = take_match(wanted, &eligible, &mut used, |_| true);
        }
    }
    matches
}

fn take_match(
    wanted: &UnidentifiedAgent,
    eligible: &[&PastAgentSession],
    used: &mut HashSet<String>,
    accept: impl Fn(&PastAgentSession) -> bool,
) -> Option<String> {
    let session = eligible.iter().find(|session| {
        session.agent == wanted.agent
            && same_directory(&session.cwd, &wanted.cwd)
            && !used.contains(&session.session_id)
            && accept(session)
    })?;
    used.insert(session.session_id.clone());
    Some(session.session_id.clone())
}

#[cfg(test)]
#[path = "agent_session_history_tests.rs"]
mod tests;
