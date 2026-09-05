//! Starting the session supervisor as a process that outlives Warp.
//!
//! Two things have to be true of it. It must not die when Warp does, which is
//! what `setsid()` in a `pre_exec` hook buys: a new session with no controlling
//! terminal, so the `SIGHUP` that reaches Warp's process group does not reach
//! it. And it must not be reaped by Warp, which is why it is double-forked --
//! the intermediate child exits immediately, `init` adopts the supervisor, and
//! Warp is left with nothing to wait on.
//!
//! Standard streams go to `/dev/null`. Left attached to Warp's, they would keep
//! its pipes open after it exits, and anything the supervisor logged would land
//! in a terminal that is no longer there.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result, bail};

/// Starts `program` as a session leader that survives this process exiting.
///
/// Returns once the intermediate child has been reaped, so no zombie is left
/// behind; the supervisor itself is by then a child of `init`.
#[allow(clippy::disallowed_types)]
pub fn spawn_detached_supervisor<S>(program: &Path, args: &[S]) -> Result<()>
where
    S: AsRef<OsStr>,
{
    use std::os::unix::process::CommandExt;

    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Safety: `setsid` and `fork` are async-signal-safe, which is the bar for
    // anything running between fork and exec. Nothing here allocates or takes a
    // lock.
    unsafe {
        command.pre_exec(|| {
            // The second fork is what makes the supervisor un-reapable by Warp:
            // this intermediate process exits immediately and the grandchild it
            // leaves behind is adopted by init.
            match libc::fork() {
                -1 => return Err(std::io::Error::last_os_error()),
                0 => {}
                // The parent of the grandchild. Exiting with `_exit` rather
                // than unwinding: this is a forked copy of Warp, and running
                // any destructor here would act on state Warp still owns.
                _ => libc::_exit(0),
            }

            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let status = command
        .spawn()
        .context("Failed to start the terminal session supervisor")?
        .wait()
        .context("Failed to reap the terminal session supervisor's launcher")?;

    if !status.success() {
        bail!("The terminal session supervisor's launcher exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
#[path = "daemon_spawn_tests.rs"]
mod tests;
