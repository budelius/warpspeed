//! The token that lets an agent hook outside the terminal address the pane its agent runs in.
//!
//! Derived from the shell's session id rather than stored with the pane: a pane that adopts a
//! supervised shell -- after a detach, a quit, or a crash of the window that started it -- knows
//! the session id but has no other way to learn what the shell's environment was given.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::sync::OnceLock;

use rand::RngCore;
use sha2::{Digest, Sha256};
use warp_core::SessionId;
use warp_core::cli_agent_protocol::{WARP_AGENT_EVENT_BIN_ENV, WARP_AGENT_EVENT_TOKEN_ENV};

const SECRET_FILE_NAME: &str = "agent-event-secret";
const SECRET_LEN: usize = 32;

/// The token for the shell session `session_id`, the same in every process of this profile.
pub(crate) fn token_for(session_id: SessionId) -> Option<String> {
    secret().map(|secret| token_from_secret(secret, session_id))
}

/// Adds the token and this binary's path for `session_id` to a shell's environment.
pub(crate) fn add_env_vars(env_vars: &mut HashMap<OsString, OsString>, session_id: SessionId) {
    let (Some(token), Ok(binary)) = (token_for(session_id), std::env::current_exe()) else {
        return;
    };
    insert_env_vars(env_vars, token, &binary);
}

fn insert_env_vars(env_vars: &mut HashMap<OsString, OsString>, token: String, binary: &Path) {
    env_vars.insert(WARP_AGENT_EVENT_TOKEN_ENV.into(), token.into());
    env_vars.insert(
        WARP_AGENT_EVENT_BIN_ENV.into(),
        binary.as_os_str().to_owned(),
    );
}

fn token_from_secret(secret: &[u8], session_id: SessionId) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret);
    hasher.update(session_id.as_u64().to_be_bytes());
    hex::encode(&hasher.finalize()[..16])
}

fn secret() -> Option<&'static [u8]> {
    static SECRET: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    SECRET
        .get_or_init(|| {
            let dir = warp_core::paths::state_dir();
            // An empty path would put the secret in whatever directory the app was started from.
            if dir.as_os_str().is_empty() {
                return None;
            }
            load_or_create_secret(&dir)
                .inspect_err(|error| {
                    log::warn!("Agent-event secret unavailable: {:?}", error.kind());
                })
                .ok()
        })
        .as_deref()
}

/// Reads the profile's secret, creating it on first use. Two processes creating it at once agree
/// on one: each writes its own temporary file and only the first hard link to the real name wins.
fn load_or_create_secret(dir: &Path) -> io::Result<Vec<u8>> {
    let path = dir.join(SECRET_FILE_NAME);
    match std::fs::read(&path) {
        Ok(secret) => return checked(secret),
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        Err(_) => {}
    }
    std::fs::create_dir_all(dir)?;
    let mut secret = vec![0u8; SECRET_LEN];
    rand::thread_rng().fill_bytes(&mut secret);
    let temporary = dir.join(format!("{SECRET_FILE_NAME}.{}.tmp", std::process::id()));
    let written = create_user_only(&temporary)
        .and_then(|mut file| file.write_all(&secret).and_then(|()| file.sync_all()));
    let linked = written.and_then(|()| std::fs::hard_link(&temporary, &path));
    let _ = std::fs::remove_file(&temporary);
    match linked {
        Ok(()) => Ok(secret),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            checked(std::fs::read(&path)?)
        }
        Err(error) => Err(error),
    }
}

/// On Windows the file inherits the state directory's ACL, which is already per-user.
fn create_user_only(path: &Path) -> io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn checked(secret: Vec<u8>) -> io::Result<Vec<u8>> {
    if secret.len() == SECRET_LEN {
        Ok(secret)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the agent-event secret has the wrong length",
        ))
    }
}

#[cfg(test)]
#[path = "agent_event_token_tests.rs"]
mod tests;
