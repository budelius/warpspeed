//! Where Kimi Code and DeepSeek Harness keep their data, as their own code resolves it.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::agent_session_history::is_safe_session_id;

/// Kimi Code's data root: `$KIMI_CODE_HOME`, else `~/.kimi-code`. Kimi's CLI treats an empty
/// value as unset.
pub(crate) fn kimi_home() -> Option<PathBuf> {
    std::env::var_os("KIMI_CODE_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".kimi-code")))
}

/// DeepSeek Harness's data root: `$DSH_HOME`, else `~/.dsh`. dsh treats a blank value as unset.
pub(crate) fn dsh_home() -> Option<PathBuf> {
    std::env::var("DSH_HOME")
        .ok()
        .filter(|home| !home.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".dsh")))
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

#[cfg(test)]
#[path = "agent_session_paths_tests.rs"]
mod tests;
