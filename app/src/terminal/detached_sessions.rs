//! Sessions the terminal supervisor is holding with no pane attached.
//!
//! A session is detached by handing its screen and its identity to the
//! supervisor, which owns the shell process and keeps it running. The pane that
//! held it can then be closed without killing anything, and the session is
//! picked back up later through [`list`] and an attach.
//!
//! Everything here answers falsely -- unsupported, empty -- when there is no
//! supervisor to talk to, so callers can ask unconditionally.

use warp_core::features::FeatureFlag;
use warpui::{AppContext, SingletonEntity};

#[cfg(feature = "local_tty")]
use crate::terminal::local_tty::spawner::PtySpawner;

/// A detached session as the picker shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetachedSession {
    /// Identifies the session for a later attach.
    pub session_id: warp_core::SessionId,

    /// What the session was running and where, for telling two apart.
    pub label: String,
}

/// Whether sessions can be detached and later found again on this machine.
///
/// Both halves have to hold: the feature has to be on, and a supervisor that
/// can enumerate what it holds has to be reachable. A detach with no way back
/// is worse than no detach at all.
pub fn are_supported(ctx: &AppContext) -> bool {
    if !FeatureFlag::PersistentTerminalSessions.is_enabled() {
        return false;
    }
    #[cfg(feature = "local_tty")]
    {
        PtySpawner::as_ref(ctx).supports_detached_sessions()
    }
    #[cfg(not(feature = "local_tty"))]
    {
        let _ = ctx;
        false
    }
}

/// The sessions currently available to attach, most recently detached first.
pub fn list(ctx: &AppContext) -> Vec<DetachedSession> {
    if !are_supported(ctx) {
        return Vec::new();
    }
    #[cfg(feature = "local_tty")]
    {
        let mut sessions = match PtySpawner::as_ref(ctx).list_detached_sessions() {
            Ok(sessions) => sessions,
            Err(error) => {
                log::warn!("Failed to list detached terminal sessions: {error:#}");
                return Vec::new();
            }
        };
        sessions.sort_by_key(|session| session.detached_for_seconds);
        sessions
            .into_iter()
            // A session another pane is already using would refuse a second
            // attach, so offering it would promise something that cannot happen.
            .filter(|session| !session.attached)
            .map(|session| DetachedSession {
                session_id: session.session_id,
                label: label_for(
                    &session.metadata.title,
                    &session.metadata.shell,
                    &session.metadata.working_directory,
                    session.detached_for_seconds,
                ),
            })
            .collect()
    }
    #[cfg(not(feature = "local_tty"))]
    {
        Vec::new()
    }
}

/// A shell's name as the picker shows it.
///
/// `ShellLaunchData::shell_detail` is a full executable path for everything but
/// WSL, and a picker row that leads with `C:\Program Files\Gitinash.exe`
/// buries the part that tells two sessions apart.
pub fn shell_label(detail: &str) -> String {
    std::path::Path::new(detail)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| detail.to_owned())
}

/// Composes what the picker shows for one session.
///
/// The metadata a session detached with can be entirely empty -- an app that
/// died without handing anything over -- so every part is optional and the
/// result never collapses to a blank row.
fn label_for(
    title: &str,
    shell: &str,
    working_directory: &str,
    detached_for_seconds: u64,
) -> String {
    let what = [title, shell]
        .into_iter()
        .map(str::trim)
        .find(|part| !part.is_empty())
        .unwrap_or("Terminal session");
    let mut label = what.to_owned();
    let directory = working_directory.trim();
    if !directory.is_empty() {
        label.push_str(" — ");
        label.push_str(directory);
    }
    label.push_str(&format!(" ({})", humanize_age(detached_for_seconds)));
    label
}

/// A detach age in the coarsest unit that still says something useful.
pub(crate) fn humanize_age(seconds: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    match seconds {
        seconds if seconds < MINUTE => "just now".to_owned(),
        seconds if seconds < HOUR => format!("{}m ago", seconds / MINUTE),
        seconds if seconds < DAY => format!("{}h ago", seconds / HOUR),
        seconds => format!("{}d ago", seconds / DAY),
    }
}

#[cfg(test)]
#[path = "detached_sessions_tests.rs"]
mod tests;
