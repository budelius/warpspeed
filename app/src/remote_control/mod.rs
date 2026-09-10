//! Driving a running Warp from outside it.
//!
//! A terminal session lives in three places at once: the supervisor owns the
//! shell process, a window owns the pane showing it, and the user owns the
//! decision to move it between them. Two of those can already be reached from a
//! script -- the supervisor answers `sessions list`, and the shell itself
//! is just a process -- but the middle one could not be reached at all, which
//! made the one behaviour that matters most impossible to test: quit Warp with
//! an agent running, start it again, and get the agent back.
//!
//! This module is the missing third. A running instance listens on a channel
//! named after its own process id, the `sessions` command finds it through a marker
//! file, and requests are answered by the app itself with its real state in
//! hand.
//!
//! Deliberately small. It carries the operations a person performs on a session
//! -- list, detach, attach, quit -- and nothing else. It is not a general
//! automation surface, and every request it does carry corresponds to something
//! reachable from the UI, so nothing here can put a window into a state a user
//! could not have reached themselves.

pub(crate) mod address;
pub(crate) mod client;
mod handler;
pub(crate) mod protocol;
mod server;

pub(crate) use server::RemoteControlServer;

/// The name this executable was invoked as.
///
/// So that advice in an error message is a command the reader can actually run.
/// This binary ships as `warpspeed` in this fork and as `warp` upstream, and a
/// message naming the wrong one sends someone to a program they do not have.
pub(crate) fn program_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "warp".to_owned())
}

/// Which build this process is, for telling an installed Warp apart from one
/// started out of a build directory.
fn build_identity() -> String {
    crate::build_info::changelog_version().unwrap_or_else(|| "unknown".to_owned())
}

/// Withdraws this process's advertisement, on the way out.
///
/// [`RemoteControlServer`] also does this when it is dropped, and on Windows it
/// never is: the event loop ends by calling `std::process::exit`, which runs no
/// destructors. A marker left behind is not fatal -- nothing is believed until
/// it answers a `Ping` -- but it makes a quit look unfinished to anything
/// watching for the instance to go, so the exit path says so explicitly rather
/// than relying on a drop that may not happen.
pub(crate) fn withdraw_this_process() {
    let pid = std::process::id();
    if let Ok(listen_address) = address::listen_address(pid) {
        address::remove_marker(&address::InstanceMarker {
            pid,
            address: listen_address,
            version: String::new(),
        });
    } else {
        address::forget(pid);
    }
}
