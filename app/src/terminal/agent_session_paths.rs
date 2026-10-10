//! Where Kimi Code and DeepSeek Harness keep their data, as their own code resolves it.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::agent_session_history::is_safe_session_id;

/// Kimi Code's data root: `$KIMI_CODE_HOME`, else `~/.kimi-code`.
pub(crate) fn kimi_home() -> Option<PathBuf> {
    kimi_home_override(std::env::var_os("KIMI_CODE_HOME"))
        .or_else(|| dirs::home_dir().map(|home| home.join(".kimi-code")))
}

/// `KIMI_CODE_HOME`'s value as a path, or `None` where Kimi's CLI treats it as unset (empty).
pub(crate) fn kimi_home_override(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|home| !home.is_empty()).map(PathBuf::from)
}

/// DeepSeek Harness's data root: `$DSH_HOME`, else `~/.dsh`.
pub(crate) fn dsh_home() -> Option<PathBuf> {
    dsh_home_override(std::env::var_os("DSH_HOME"))
        .or_else(|| dirs::home_dir().map(|home| home.join(".dsh")))
}

/// `DSH_HOME`'s value as a path, or `None` where dsh treats it as unset (blank or not Unicode).
pub(crate) fn dsh_home_override(value: Option<OsString>) -> Option<PathBuf> {
    value
        .filter(|home| home.to_str().is_some_and(|home| !home.trim().is_empty()))
        .map(PathBuf::from)
}

/// Each live session's directory from Kimi's `session_index.jsonl`, or `None` without an index.
/// Kimi appends a line per created session and a `deleted` tombstone per deletion, so the last
/// line for an id wins.
pub(crate) fn kimi_indexed_session_dirs(home: &Path) -> Option<Vec<(String, PathBuf)>> {
    let file = File::open(home.join("session_index.jsonl")).ok()?;
    let mut dirs: HashMap<String, Option<PathBuf>> = HashMap::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(session_id) = entry["sessionId"]
            .as_str()
            .filter(|id| is_safe_session_id(id))
        else {
            continue;
        };
        let dir = if entry["deleted"] == true {
            None
        } else {
            let Some(dir) = entry["sessionDir"].as_str() else {
                continue;
            };
            Some(PathBuf::from(dir))
        };
        dirs.insert(session_id.to_owned(), dir);
    }
    Some(
        dirs.into_iter()
            .filter_map(|(session_id, dir)| Some((session_id, dir?)))
            .collect(),
    )
}

/// The directory Kimi names a working directory's sessions after (`encodeWorkDirKey` in
/// `agent-core-v2/src/_base/utils/workdir-slug.ts`): `wd_<slug>_<first 12 hex of sha256>`.
/// Kimi reuses an existing workspace whose root differs only in case, so its session index is
/// the better source when it has the session.
pub(crate) fn kimi_work_dir_key(work_dir: &str) -> String {
    let normalized = work_dir.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    let base = normalized.rsplit('/').next().unwrap_or(normalized);
    let hash = hex::encode(Sha256::digest(normalized.as_bytes()));
    format!("wd_{}_{}", kimi_work_dir_slug(base), &hash[..12])
}

fn kimi_work_dir_slug(name: &str) -> String {
    const MAX_SLUG_LEN: usize = 40;
    let mut slug = String::new();
    let mut in_replaced_run = false;
    for character in name.to_lowercase().chars() {
        if character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || matches!(character, '.' | '_' | '-')
        {
            slug.push(character);
            in_replaced_run = false;
        } else if !in_replaced_run {
            slug.push('-');
            in_replaced_run = true;
        }
    }
    let slug = slug.trim_matches('-');
    let slug = slug.get(..MAX_SLUG_LEN).unwrap_or(slug).trim_matches('-');
    match slug {
        "" | "." | ".." => "workspace".to_owned(),
        slug => slug.to_owned(),
    }
}

#[cfg(test)]
#[path = "agent_session_paths_tests.rs"]
mod tests;
