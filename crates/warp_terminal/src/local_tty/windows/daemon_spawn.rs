//! Starting the session supervisor as a process that outlives Warp.
//!
//! The supervisor exists to keep shells alive after the app exits, so it must
//! not be killed along with it. On Unix that is `setsid()`. On Windows there
//! are two things to escape:
//!
//! - **The console.** `DETACHED_PROCESS` gives the child no console, so it does
//!   not receive the Ctrl+C or Ctrl+Break that reach Warp's console group, and
//!   `CREATE_NEW_PROCESS_GROUP` keeps it out of that group as well.
//! - **The job object.** If Warp runs inside a job object -- and it often does,
//!   under a terminal, a test runner, or a CI agent -- every process in it dies
//!   when the job is closed, children included.
//!
//! ## Why breakaway is attempted rather than required
//!
//! `CREATE_BREAKAWAY_FROM_JOB` leaves the job, but it fails with
//! `ERROR_ACCESS_DENIED` unless the job was created with
//! `JOB_OBJECT_LIMIT_BREAKAWAY_OK`. A job that forbids breakaway is common and
//! is not something Warp can change from the inside.
//!
//! Refusing to spawn in that case would be the wrong trade: a supervisor that
//! dies with its job is still worth having, because the job usually outlives a
//! single Warp window, and the alternative is no persistence at all. So the
//! flag is attempted, the failure is caught, and the caller is told plainly
//! whether the process it got will actually outlive the job.
//!
//! `crates/command/src/blocking.rs:84` sets this flag unconditionally, which is
//! why every spawn through that path fails inside such a job -- the cause of
//! the long-standing failure in `local_tty::windows::child`'s test. This module
//! deliberately does not use that path.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Child, Command};

use anyhow::{Context, Result};

/// Windows creation flags, from `processthreadsapi.h`.
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

/// `ERROR_ACCESS_DENIED`, which is what a job that forbids breakaway returns.
const ACCESS_DENIED: i32 = 5;

/// A supervisor process that was started.
pub struct DaemonProcess {
    /// The running child.
    pub child: Child,

    /// Whether the process left Warp's job object.
    ///
    /// `false` means the job forbade breakaway, so this process will still be
    /// killed when that job is closed. Sessions survive Warp exiting, but not
    /// the job around it.
    pub escaped_job_object: bool,
}

impl DaemonProcess {
    /// The supervisor's process id.
    pub fn id(&self) -> u32 {
        self.child.id()
    }
}

/// Starts `program` detached from Warp's console and, where permitted, from its
/// job object.
///
/// Tries the strongest isolation first and degrades rather than failing: a
/// supervisor inside the job is far better than no supervisor.
pub fn spawn_detached_supervisor<S>(program: &Path, args: &[S]) -> Result<DaemonProcess>
where
    S: AsRef<OsStr>,
{
    let detached = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;

    match spawn_with_flags(program, args, detached | CREATE_BREAKAWAY_FROM_JOB) {
        Ok(child) => Ok(DaemonProcess {
            child,
            escaped_job_object: true,
        }),
        Err(error) if is_access_denied(&error) => {
            // The job forbids breakaway. Start inside it and say so, rather
            // than leaving the user with no persistence at all.
            let child = spawn_with_flags(program, args, detached).context(
                "Failed to start the terminal session supervisor even without job breakaway",
            )?;
            Ok(DaemonProcess {
                child,
                escaped_job_object: false,
            })
        }
        Err(error) => {
            Err(error).context("Failed to start the terminal session supervisor")
        }
    }
}

/// Spawns with the given creation flags and no inherited standard streams.
fn spawn_with_flags<S>(program: &Path, args: &[S], flags: u32) -> std::io::Result<Child>
where
    S: AsRef<OsStr>,
{
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;

    Command::new(program)
        .args(args)
        // A detached process has no console to inherit these from, and leaving
        // them attached to Warp's would keep its pipes alive after it exits.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(flags)
        .spawn()
}

/// Whether an error is the job object refusing breakaway.
fn is_access_denied(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(ACCESS_DENIED)
}

#[cfg(test)]
#[path = "daemon_spawn_tests.rs"]
mod tests;
