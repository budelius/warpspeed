//! A PTY owned by the supervisor rather than by the app.
//!
//! The supervisor holds the leader descriptor for the whole life of the
//! session. While a client is attached it does no I/O at all -- the client has
//! been lent a copy of that descriptor and reads it directly -- and while
//! nothing is attached the supervisor reads it itself, which is what keeps a
//! detached shell from blocking on a full pipe.

use std::fs::File;
use std::io::Read;
use std::os::unix::prelude::*;
use std::process::Child;

use anyhow::{Context, Result};

use crate::local_tty::{self, PtyOptions};

/// A shell process and the terminal it is attached to.
pub struct SupervisedPty {
    child: Child,
    leader: File,
}

impl SupervisedPty {
    /// Starts a shell on a new PTY that this process owns.
    pub fn spawn(mut options: PtyOptions) -> Result<Self> {
        // The supervisor outlives the app, so anything it inherits leaks for as
        // long as it runs.
        options.close_fds = true;

        let spawned = local_tty::spawn(options).context("Failed to start a supervised terminal")?;
        let leader_fd = spawned.result.leader_fd;
        set_nonblocking(leader_fd)
            .context("Failed to make a supervised terminal's leader non-blocking")?;

        Ok(Self {
            child: spawned.child,
            // Safety: `spawn` hands over ownership of `leader_fd` in its result,
            // and nothing else in this process holds it.
            leader: unsafe { File::from_raw_fd(leader_fd) },
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The leader descriptor, for lending to a client over `SCM_RIGHTS`.
    ///
    /// Borrowed rather than handed over: the supervisor keeps reading it once
    /// the client lets go.
    pub fn leader_fd(&self) -> RawFd {
        self.leader.as_raw_fd()
    }

    /// Reads whatever output is available without waiting for any.
    ///
    /// The descriptor is non-blocking, so an idle terminal reads as zero bytes
    /// rather than parking the supervisor's single-threaded loop.
    pub fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        loop {
            return match self.leader.read(buffer) {
                Ok(bytes_read) => Ok(bytes_read),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(0),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                // The follower side being gone reads as an I/O error on Linux
                // rather than end-of-file. The session is over either way, and
                // the caller learns that from `is_running`.
                Err(_) => Ok(0),
            };
        }
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn resize(&mut self, columns: u16, rows: u16) -> Result<()> {
        let size = libc::winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let result =
            unsafe { libc::ioctl(self.leader.as_raw_fd(), libc::TIOCSWINSZ, &size as *const _) };
        if result < 0 {
            return Err(std::io::Error::last_os_error())
                .context("Failed to resize a supervised terminal");
        }
        Ok(())
    }

    /// Ends the shell and reaps it.
    pub fn kill(&mut self) -> Result<()> {
        if !self.is_running() {
            return Ok(());
        }
        self.child
            .kill()
            .context("Failed to end a supervised terminal")?;
        self.child
            .wait()
            .context("Failed to reap a supervised terminal")?;
        Ok(())
    }
}

impl Drop for SupervisedPty {
    fn drop(&mut self) {
        // Closing the leader before waiting: on macOS a child with unread PTY
        // output can wedge in an unkillable exiting state otherwise. See the
        // same note on `local_tty::unix::Pty::kill`.
        let _ = self.kill();
    }
}

fn set_nonblocking(fd: RawFd) -> Result<()> {
    let flags = nix::fcntl::fcntl(fd, nix::fcntl::F_GETFL)?;
    let mut flags = nix::fcntl::OFlag::from_bits_truncate(flags);
    flags.insert(nix::fcntl::OFlag::O_NONBLOCK);
    nix::fcntl::fcntl(fd, nix::fcntl::F_SETFL(flags))?;
    Ok(())
}

#[cfg(test)]
#[path = "supervised_pty_tests.rs"]
mod tests;
