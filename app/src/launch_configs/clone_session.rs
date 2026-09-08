use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::launch_configs::launch_config::LaunchConfigWindowSource;
use crate::launch_configs::session_store;

/// The identity a clone will take: a file stem, the session id derived from it,
/// and the display name.
pub(crate) struct CloneIdentity {
    pub file_stem: String,
    pub id: String,
    pub name: String,
}

#[expect(
    dead_code,
    reason = "consumed by the session-in-use dialog in a later task"
)]
pub(crate) struct ClonedSession {
    pub name: String,
    pub config_path: PathBuf,
    pub windows: Vec<LaunchConfigWindowSource>,
}

/// The highest suffix worth trying before giving up. A user with this many
/// clones of one session has a different problem.
const MAX_CLONE_SUFFIX: usize = 100;

fn free_identity(session_configs_dir: &Path, from_stem: &str) -> Result<CloneIdentity> {
    for suffix in 2..=MAX_CLONE_SUFFIX {
        let file_stem = format!("{from_stem}-{suffix}");
        if !session_configs_dir
            .join(format!("{file_stem}.toml"))
            .exists()
        {
            return Ok(CloneIdentity {
                id: file_stem.to_lowercase(),
                name: format!("{from_stem} ({suffix})"),
                file_stem,
            });
        }
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

    let path = session_configs_dir.join(format!("{}.toml", identity.file_stem));
    std::fs::write(&path, document.to_string())
        .with_context(|| format!("Failed to write cloned session config {path:?}"))?;
    Ok(path)
}

#[expect(
    dead_code,
    reason = "consumed by the session-in-use dialog in a later task"
)]
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
