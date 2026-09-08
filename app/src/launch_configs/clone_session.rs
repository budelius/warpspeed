use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::launch_configs::launch_config::LaunchConfigWindowSource;
use crate::launch_configs::session_store;
use crate::session_configs::normalized_session_id;

/// The identity a clone will take: a file stem, the session id derived from it,
/// and the display name.
pub(crate) struct CloneIdentity {
    pub file_stem: String,
    pub id: String,
    pub name: String,
}

pub(crate) struct ClonedSession {
    pub name: String,
    pub config_path: PathBuf,
    pub windows: Vec<LaunchConfigWindowSource>,
}

/// The highest suffix worth trying before giving up. A user with this many
/// clones of one session has a different problem.
const MAX_CLONE_SUFFIX: usize = 100;

/// The session ids already spoken for in this directory. Ids are normalized, so
/// distinct file names can name the same session: `OBACHAN 2.toml` already holds
/// `obachan-2`, which is the id `OBACHAN-2.toml` would take.
fn taken_session_ids(session_configs_dir: &Path) -> HashSet<String> {
    let Ok(entries) = std::fs::read_dir(session_configs_dir) else {
        return HashSet::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()? != "toml" {
                return None;
            }
            Some(normalized_session_id(path.file_stem()?.to_str()?))
        })
        .collect()
}

fn free_identity(session_configs_dir: &Path, from_stem: &str) -> Result<CloneIdentity> {
    let taken_ids = taken_session_ids(session_configs_dir);
    for suffix in 2..=MAX_CLONE_SUFFIX {
        let file_stem = format!("{from_stem}-{suffix}");
        let id = normalized_session_id(&file_stem);
        // A colliding id is worse than a colliding file name: the checkpoint
        // directory and the lease are both derived from the id, so the clone
        // would write into -- and contend for -- an unrelated session's state.
        if taken_ids.contains(&id)
            || session_configs_dir
                .join(format!("{file_stem}.toml"))
                .exists()
        {
            continue;
        }
        return Ok(CloneIdentity {
            id,
            name: format!("{from_stem} ({suffix})"),
            file_stem,
        });
    }
    bail!("No free name for a clone of {from_stem:?}")
}

fn clone_config_file(
    from: &Path,
    session_configs_dir: &Path,
    identity: &CloneIdentity,
) -> Result<PathBuf> {
    let contents = std::fs::read_to_string(from)
        .with_context(|| format!("Failed to read session config {from:?}"))?;
    let mut document = contents
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("Failed to parse session config {from:?}"))?;
    document["id"] = toml_edit::value(identity.id.as_str());
    document["name"] = toml_edit::value(identity.name.as_str());
    // A clone starts its own history rather than inheriting the generation the
    // original is on, so its lease and checkpoints never alias the original's.
    document["generation"] = toml_edit::value(1_i64);

    // Create-new so the identity check and this write cannot race another
    // process into overwriting a config that appeared in between.
    let path = session_configs_dir.join(format!("{}.toml", identity.file_stem));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("Failed to create cloned session config {path:?}"))?;
    file.write_all(document.to_string().as_bytes())
        .with_context(|| format!("Failed to write cloned session config {path:?}"))?;
    Ok(path)
}

pub(crate) fn clone_session(from: &LaunchConfigWindowSource) -> Result<ClonedSession> {
    let session_configs_dir = crate::user_config::session_configs_dir();
    let from_stem = from
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("Session config path has no file name")?;

    let identity = free_identity(&session_configs_dir, from_stem)?;
    let config_path = clone_config_file(&from.path, &session_configs_dir, &identity)?;
    let windows = session_store::clone_checkpoints(from, &config_path, &identity.name)?;

    Ok(ClonedSession {
        name: identity.name,
        config_path,
        windows,
    })
}

#[cfg(test)]
#[path = "clone_session_tests.rs"]
mod tests;
