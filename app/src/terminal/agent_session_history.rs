use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

#[cfg(not(target_family = "wasm"))]
use super::agent_session_paths::{dsh_home, kimi_home, kimi_indexed_session_dirs};
use crate::terminal::CLIAgent;

const MAX_CANDIDATES: usize = 200;
const MAX_HEAD_LINES: usize = 200;
const MAX_HEAD_BYTES: usize = 256 * 1024;
const MAX_PREVIEW_BYTES: u64 = 512 * 1024;
const MAX_LAST_PROMPT_BYTES: u64 = 64 * 1024;
const MAX_FOLDER_SCAN_BYTES: u64 = 64 * 1024;
const MAX_FOLDER_SCAN_LINES: usize = 32;
const MAX_PREVIEW_MESSAGES: usize = 8;
const MAX_PREVIEW_MESSAGE_CHARS: usize = 1_200;
const TITLE_MAX_CHARS: usize = 80;
#[cfg(not(target_family = "wasm"))]
const DSH_HEAD_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentSessionPreviewRole {
    User,
    Agent,
}

impl AgentSessionPreviewRole {
    pub(crate) fn label(self) -> &'static str {
        match self {
            AgentSessionPreviewRole::User => "You",
            AgentSessionPreviewRole::Agent => "Agent",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentSessionPreviewMessage {
    pub role: AgentSessionPreviewRole,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PastAgentSession {
    pub agent: CLIAgent,
    pub session_id: String,
    pub cwd: PathBuf,
    /// `cwd` resolved on disk by the scan, off the UI thread, so folder checks need not touch the
    /// filesystem. `None` when it could not be resolved.
    pub canonical_cwd: Option<PathBuf>,
    pub title: Option<String>,
    /// The first line of the most recent user prompt.
    pub last_prompt: Option<String>,
    pub modified: SystemTime,
    pub preview: Vec<AgentSessionPreviewMessage>,
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

fn read_tail(path: &Path) -> Vec<Value> {
    read_tail_with_limit(path, MAX_PREVIEW_BYTES)
}

fn read_tail_with_limit(path: &Path, limit: u64) -> Vec<Value> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let Ok(length) = file.metadata().map(|metadata| metadata.len()) else {
        return Vec::new();
    };
    let start = length.saturating_sub(limit);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut reader = BufReader::new(file.take(limit));
    let mut line = Vec::new();
    if start > 0 {
        let _ = reader.read_until(b'\n', &mut line);
    }
    let mut entries = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if let Ok(value) = serde_json::from_slice(&line) {
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

pub(crate) fn is_safe_session_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn claude_candidates_in(project: &Path) -> Vec<Candidate> {
    files(project)
        .into_iter()
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

fn claude_candidates(root: &Path) -> Vec<Candidate> {
    directories(root)
        .into_iter()
        .flat_map(|project| claude_candidates_in(&project))
        .collect()
}

fn pi_candidates_in(project: &Path) -> Vec<Candidate> {
    files(project)
        .into_iter()
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

pub(super) fn pi_candidates(root: &Path) -> Vec<Candidate> {
    directories(root)
        .into_iter()
        .flat_map(|project| pi_candidates_in(&project))
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
                canonical_cwd: None,
                title: entry["title"].as_str().and_then(title),
                last_prompt: None,
                modified,
                preview: Vec::new(),
            })
        })
        .collect()
}

/// Kimi Code's sessions: directories from its index, or every `sessions/*/*` directory when the
/// index is gone, each described by its `state.json`.
#[cfg(not(target_family = "wasm"))]
fn kimi_sessions(home: &Path) -> Vec<PastAgentSession> {
    let dirs = kimi_indexed_session_dirs(home).unwrap_or_else(|| {
        directories(&home.join("sessions"))
            .into_iter()
            .flat_map(|workspace| directories(&workspace))
            .filter_map(|dir| {
                let session_id = dir.file_name()?.to_str()?.to_owned();
                is_safe_session_id(&session_id).then_some((session_id, dir))
            })
            .collect()
    });
    dirs.into_iter()
        .filter_map(|(session_id, dir)| kimi_session(session_id, &dir))
        .collect()
}

#[cfg(not(target_family = "wasm"))]
fn kimi_session(session_id: String, dir: &Path) -> Option<PastAgentSession> {
    let state_path = dir.join("state.json");
    let state: Value =
        serde_json::from_reader(BufReader::new(File::open(&state_path).ok()?)).ok()?;
    if state["archived"] == true {
        return None;
    }
    let cwd = state["cwd"]
        .as_str()
        .or_else(|| state["workDir"].as_str())
        .map(PathBuf::from)
        .unwrap_or_default();
    let modified = epoch_millis_or_rfc3339(&state["updatedAt"])
        .or_else(|| std::fs::metadata(&state_path).ok()?.modified().ok())?;
    let title = state["title"]
        .as_str()
        .or_else(|| state["lastPrompt"].as_str())
        .and_then(title);
    Some(PastAgentSession {
        agent: CLIAgent::Kimi,
        session_id,
        cwd,
        canonical_cwd: None,
        title,
        last_prompt: None,
        modified,
        preview: Vec::new(),
    })
}

/// DeepSeek Harness's sessions under `root` (`<DSH_HOME>/sessions`), each described by the head
/// of its current log.
#[cfg(not(target_family = "wasm"))]
fn dsh_sessions(root: &Path) -> Vec<PastAgentSession> {
    directories(root)
        .into_iter()
        .filter(|project| {
            project
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.len() > 4 && name.starts_with("--") && name.ends_with("--")
                })
        })
        .flat_map(|project| directories(&project))
        .filter_map(|dir| dsh_session(&dir))
        .collect()
}

#[cfg(not(target_family = "wasm"))]
fn dsh_session(dir: &Path) -> Option<PastAgentSession> {
    let (path, compressed) = current_dsh_log(dir)?;
    let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
    let head = read_dsh_head(&path, compressed);
    let mut rows = head
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok());
    let header = rows.next().filter(|header| header["type"] == "session")?;
    let session_id = header["id"]
        .as_str()
        .filter(|id| is_safe_session_id(id))?
        .to_owned();
    let cwd = header["cwd"]
        .as_str()
        .map(PathBuf::from)
        .unwrap_or_default();
    let title = rows
        .find(|row| row["type"] == "user/message" && row["data"]["source"]["kind"] == "user")
        .and_then(|row| {
            row["data"]["content"]
                .as_array()?
                .iter()
                .find(|part| part["type"] == "text")?["text"]
                .as_str()
                .and_then(title)
        });
    Some(PastAgentSession {
        agent: CLIAgent::DeepSeekHarness,
        session_id,
        cwd,
        canonical_cwd: None,
        title,
        last_prompt: None,
        modified,
        preview: Vec::new(),
    })
}

/// The session's current log, the highest generation of `session[.vN].jsonl[.zstd]` in `dir`
/// (N has no leading zero, as dsh writes it), and whether it is zstd-compressed.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn current_dsh_log(dir: &Path) -> Option<(PathBuf, bool)> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let (stem, compressed) = match name.strip_suffix(".zstd") {
                Some(stem) => (stem, true),
                None => (name.as_str(), false),
            };
            let generation = match stem.strip_prefix("session")?.strip_suffix(".jsonl")? {
                "" => 0,
                version => {
                    let digits = version.strip_prefix(".v")?;
                    let canonical = !digits.starts_with('0')
                        && digits.bytes().all(|byte| byte.is_ascii_digit());
                    canonical.then(|| digits.parse::<u32>().ok()).flatten()?
                }
            };
            Some((generation, entry.path(), compressed))
        })
        .max_by_key(|(generation, _, _)| *generation)
        .map(|(_, path, compressed)| (path, compressed))
}

/// Up to [`DSH_HEAD_BYTES`] of the log's text. A log dsh is still appending to can end in a
/// partial frame; what decoded before it is kept.
#[cfg(not(target_family = "wasm"))]
fn read_dsh_head(path: &Path, compressed: bool) -> String {
    let mut head = Vec::new();
    if let Ok(file) = File::open(path) {
        let _ = if compressed {
            zstd::stream::read::Decoder::new(file)
                .and_then(|decoder| decoder.take(DSH_HEAD_BYTES).read_to_end(&mut head))
        } else {
            file.take(DSH_HEAD_BYTES).read_to_end(&mut head)
        };
    }
    String::from_utf8_lossy(&head).into_owned()
}

/// A timestamp Kimi wrote as epoch milliseconds, or as an RFC 3339 string in older versions.
#[cfg(not(target_family = "wasm"))]
fn epoch_millis_or_rfc3339(value: &Value) -> Option<SystemTime> {
    let millis = match value.as_u64() {
        Some(millis) => millis,
        None => u64::try_from(
            chrono::DateTime::parse_from_rfc3339(value.as_str()?)
                .ok()?
                .timestamp_millis(),
        )
        .ok()?,
    };
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(millis))
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

fn message_text(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => String::new(),
    };
    preview_text(&text)
}

fn preview_text(text: &str) -> Option<String> {
    let sanitized = text
        .chars()
        .map(|character| {
            if character == '\n' || character == '\t' || !character.is_control() {
                character
            } else {
                '�'
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim();
    if sanitized.is_empty() {
        return None;
    }
    let mut preview = sanitized
        .chars()
        .take(MAX_PREVIEW_MESSAGE_CHARS)
        .collect::<String>();
    if sanitized.chars().count() > MAX_PREVIEW_MESSAGE_CHARS {
        preview.push('…');
    }
    Some(preview)
}

fn preview_message(agent: CLIAgent, entry: &Value) -> Option<AgentSessionPreviewMessage> {
    let (role, content) = match agent {
        CLIAgent::Claude => {
            let role = match entry["message"]["role"].as_str()? {
                "user" => AgentSessionPreviewRole::User,
                "assistant" => AgentSessionPreviewRole::Agent,
                _ => return None,
            };
            (role, &entry["message"]["content"])
        }
        CLIAgent::Codex => {
            if entry["type"] != "event_msg" {
                return None;
            }
            let role = match entry["payload"]["type"].as_str()? {
                "user_message" => AgentSessionPreviewRole::User,
                "agent_message" => AgentSessionPreviewRole::Agent,
                _ => return None,
            };
            (role, &entry["payload"]["message"])
        }
        CLIAgent::Pi => {
            if entry["type"] != "message" {
                return None;
            }
            let role = match entry["message"]["role"].as_str()? {
                "user" => AgentSessionPreviewRole::User,
                "assistant" => AgentSessionPreviewRole::Agent,
                _ => return None,
            };
            (role, &entry["message"]["content"])
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
        | CLIAgent::Kimi
        | CLIAgent::DeepSeekHarness
        | CLIAgent::WarpTui
        | CLIAgent::Unknown => return None,
    };
    Some(AgentSessionPreviewMessage {
        role,
        text: message_text(content)?,
    })
}

fn last_prompt_from_entries(agent: CLIAgent, entries: &[Value]) -> Option<String> {
    entries.iter().rev().find_map(|entry| {
        let message = preview_message(agent, entry)?;
        if message.role != AgentSessionPreviewRole::User {
            return None;
        }
        let text = message.text.trim_start();
        if matches!(text.trim(), "/clear" | "/exit" | "/quit") {
            return None;
        }
        if [
            "<task-notification>",
            "<system-reminder>",
            "<tool-result>",
            "<local-command-",
            "<command-name>",
        ]
        .into_iter()
        .any(|prefix| text.starts_with(prefix))
        {
            return None;
        }
        title(text)
    })
}

fn last_prompt_for_candidate(candidate: &Candidate) -> Option<String> {
    last_prompt_from_entries(
        candidate.agent,
        &read_tail_with_limit(&candidate.path, MAX_LAST_PROMPT_BYTES),
    )
}

fn preview(candidate: &Candidate) -> Vec<AgentSessionPreviewMessage> {
    let messages = read_tail(&candidate.path)
        .iter()
        .filter_map(|entry| preview_message(candidate.agent, entry))
        .collect::<Vec<_>>();
    messages[messages.len().saturating_sub(MAX_PREVIEW_MESSAGES)..].to_vec()
}

pub(super) fn describe(candidate: Candidate) -> PastAgentSession {
    let head = read_head(&candidate.path);
    let last_prompt = last_prompt_for_candidate(&candidate);
    let preview = preview(&candidate);
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
        | CLIAgent::Kimi
        | CLIAgent::DeepSeekHarness
        | CLIAgent::WarpTui
        | CLIAgent::Unknown => (candidate.session_id, PathBuf::new(), None),
    };
    PastAgentSession {
        agent: candidate.agent,
        session_id,
        cwd,
        canonical_cwd: None,
        title,
        last_prompt,
        modified: candidate.modified,
        preview,
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
        | CLIAgent::Kimi
        | CLIAgent::DeepSeekHarness
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

fn canonical_directory(path: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    path.canonicalize().ok()
}

/// A folder together with its canonical form, resolved once so that many `is_within_known`
/// checks against it touch the filesystem only once.
#[derive(Clone, Debug)]
pub(crate) struct ResolvedPath {
    path: PathBuf,
    canonical: Option<PathBuf>,
}

impl ResolvedPath {
    pub(crate) fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            canonical: canonical_directory(path),
        }
    }
}

/// Whether `path`, whose canonical form is `canonical` when known, is `folder` or lies beneath
/// it, normalized as `same_directory` does. It does no filesystem calls.
pub(crate) fn is_within_known(
    path: &Path,
    canonical: Option<&Path>,
    folder: &ResolvedPath,
) -> bool {
    if path.as_os_str().is_empty() || folder.path.as_os_str().is_empty() {
        return false;
    }
    let (path, folder) = match (canonical, &folder.canonical) {
        (Some(path), Some(folder)) => (path, folder.as_path()),
        _ => (path, folder.path.as_path()),
    };
    #[cfg(windows)]
    {
        let normalize = |path: &Path| {
            let path = path.to_string_lossy().replace('\\', "/");
            let bytes = path.as_bytes();
            let path = if bytes.len() >= 2
                && bytes[0] == b'/'
                && bytes[1].is_ascii_alphabetic()
                && (bytes.len() == 2 || bytes[2] == b'/')
            {
                format!("{}:{}", char::from(bytes[1]), &path[2..])
            } else {
                path
            };
            path.to_lowercase()
        };
        let (path, folder) = (normalize(path), normalize(folder));
        let path = path.trim_end_matches('/');
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

/// The sessions of agents that keep an index or a directory per session, read whole instead of
/// through [`candidates`] and [`describe`]. `None` for every other agent.
#[cfg(not(target_family = "wasm"))]
fn directory_sessions(agent: CLIAgent) -> Option<Vec<PastAgentSession>> {
    match agent {
        CLIAgent::Vibe => Some(
            dirs::home_dir()
                .map(|home| vibe_sessions(&home.join(".vibe").join("logs").join("session")))
                .unwrap_or_default(),
        ),
        CLIAgent::Kimi => Some(
            kimi_home()
                .map(|home| kimi_sessions(&home))
                .unwrap_or_default(),
        ),
        CLIAgent::DeepSeekHarness => Some(
            dsh_home()
                .map(|home| dsh_sessions(&home.join("sessions")))
                .unwrap_or_default(),
        ),
        CLIAgent::Claude
        | CLIAgent::Gemini
        | CLIAgent::Codex
        | CLIAgent::Amp
        | CLIAgent::Droid
        | CLIAgent::OpenCode
        | CLIAgent::Copilot
        | CLIAgent::Pi
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

fn scan(agent: CLIAgent, not_before: Option<SystemTime>) -> Vec<PastAgentSession> {
    #[cfg(not(target_family = "wasm"))]
    let listed = directory_sessions(agent);
    #[cfg(target_family = "wasm")]
    let listed = None;
    let mut sessions = listed.unwrap_or_else(|| {
        let mut candidates = candidates(agent)
            .into_iter()
            .filter(|candidate| not_before.is_none_or(|time| candidate.modified >= time))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| right.modified.cmp(&left.modified));
        candidates.truncate(MAX_CANDIDATES);
        candidates.into_iter().map(describe).collect()
    });
    sessions.retain(|session| not_before.is_none_or(|time| session.modified >= time));
    sessions.sort_by(|left, right| right.modified.cmp(&left.modified));
    sessions.truncate(MAX_CANDIDATES);
    resolve_folders(&mut sessions);
    sessions
}

fn candidate_cwd(candidate: &Candidate) -> Option<PathBuf> {
    let file = File::open(&candidate.path).ok()?;
    let mut reader = BufReader::new(file.take(MAX_FOLDER_SCAN_BYTES));
    let mut line = String::new();
    for _ in 0..MAX_FOLDER_SCAN_LINES {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let cwd = match candidate.agent {
            CLIAgent::Claude | CLIAgent::Pi => entry["cwd"].as_str().map(PathBuf::from),
            CLIAgent::Codex => {
                crate::ai::agent_sdk::driver::harness::codex_transcript::parse_session_meta(Some(
                    &entry,
                ))
                .map(|metadata| metadata.cwd)
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
            | CLIAgent::Kimi
            | CLIAgent::DeepSeekHarness
            | CLIAgent::WarpTui
            | CLIAgent::Unknown => None,
        };
        if cwd.as_ref().is_some_and(|cwd| !cwd.as_os_str().is_empty()) {
            return cwd;
        }
    }
    None
}

fn sessions_in_folder(candidates: Vec<Candidate>, folder: &Path) -> Vec<PastAgentSession> {
    let folder = ResolvedPath::new(folder);
    let mut sessions = candidates
        .into_iter()
        .filter(|candidate| {
            candidate_cwd(candidate).is_some_and(|cwd| is_within_known(&cwd, None, &folder))
        })
        .map(describe)
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| right.modified.cmp(&left.modified));
    resolve_folders(&mut sessions);
    sessions
}

#[cfg(not(target_family = "wasm"))]
fn sessions_for_agent_in_folder(agent: CLIAgent, folder: &Path) -> Vec<PastAgentSession> {
    match agent {
        CLIAgent::Claude => {
            crate::ai::agent_sdk::driver::harness::claude_transcript::claude_config_dir()
                .ok()
                .map(|root| sessions_in_folder(claude_candidates(&root.join("projects")), folder))
                .unwrap_or_default()
        }
        CLIAgent::Pi => {
            let candidates = std::env::var_os("PI_CODING_AGENT_SESSION_DIR")
                .map(PathBuf::from)
                .map(|root| pi_candidates_in(&root))
                .or_else(|| {
                    dirs::home_dir()
                        .map(|home| pi_candidates(&home.join(".pi").join("agent").join("sessions")))
                })
                .unwrap_or_default();
            sessions_in_folder(candidates, folder)
        }
        CLIAgent::Codex => sessions_in_folder(candidates(agent), folder),
        CLIAgent::Kimi | CLIAgent::DeepSeekHarness => {
            let folder = ResolvedPath::new(folder);
            let mut sessions = directory_sessions(agent)
                .unwrap_or_default()
                .into_iter()
                .filter(|session| is_within_known(&session.cwd, None, &folder))
                .collect::<Vec<_>>();
            sessions.sort_by(|left, right| right.modified.cmp(&left.modified));
            resolve_folders(&mut sessions);
            sessions
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
        | CLIAgent::Unknown => Vec::new(),
    }
}

#[cfg(target_family = "wasm")]
fn sessions_for_agent_in_folder(_: CLIAgent, _: &Path) -> Vec<PastAgentSession> {
    Vec::new()
}

/// Resolves each session's folder on disk, which the scan does so the UI thread never has to.
pub(super) fn resolve_folders(sessions: &mut [PastAgentSession]) {
    for session in sessions {
        session.canonical_cwd = canonical_directory(&session.cwd);
    }
}

pub(crate) fn find_all(agent: CLIAgent) -> Vec<PastAgentSession> {
    scan(agent, None)
}

/// The agents whose past sessions the session picker lists. An agent without a resume command
/// is listed without a Resume action.
pub(crate) const RESUMABLE_AGENTS: [CLIAgent; 5] = [
    CLIAgent::Claude,
    CLIAgent::Codex,
    CLIAgent::Pi,
    CLIAgent::Kimi,
    CLIAgent::DeepSeekHarness,
];

/// Every resumable agent's past sessions, newest first.
pub(crate) fn find_all_resumable() -> Vec<PastAgentSession> {
    let mut sessions = RESUMABLE_AGENTS
        .into_iter()
        .flat_map(find_all)
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| right.modified.cmp(&left.modified));
    sessions
}

/// Every recorded resumable session in `folder`, without the global recent-session limit.
pub(crate) fn find_all_resumable_in(folder: &Path) -> Vec<PastAgentSession> {
    let mut sessions = RESUMABLE_AGENTS
        .into_iter()
        .flat_map(|agent| sessions_for_agent_in_folder(agent, folder))
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
        CLIAgent::Kimi => ["-S", "--session", "-r", "--resume"]
            .into_iter()
            .find_map(argument_after)
            .or_else(|| {
                words.iter().find_map(|word| {
                    word.strip_prefix("--session=")
                        .or_else(|| word.strip_prefix("--resume="))
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned)
                })
            }),
        CLIAgent::DeepSeekHarness => argument_after("--session-id"),
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
