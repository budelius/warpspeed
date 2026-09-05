//! Identifying an attached client by more than its process id.

/// A client process, identified by more than its PID.
///
/// PIDs are recycled. A session whose client died is detected as detached by
/// checking whether the PID is still alive, so once the kernel hands that
/// number to an unrelated process the session looks permanently attached: every
/// reattach is refused and the shell can never be reached again. Pairing the
/// PID with the process's start time makes a recycled PID distinguishable from
/// the process that actually attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientProcess {
    pub pid: u32,

    /// The kernel's start time for `pid`, read when the client attached.
    ///
    /// `None` on platforms where it cannot be read, where this degrades to the
    /// previous PID-only behaviour rather than refusing to work.
    start_time: Option<u64>,
}

impl ClientProcess {
    pub fn new(pid: u32) -> Self {
        Self {
            pid,
            start_time: process_start_time(pid),
        }
    }

    /// Whether this exact process is still running -- not merely something
    /// holding its PID.
    pub fn is_running(&self) -> bool {
        if !process_is_running(self.pid) {
            return false;
        }
        match (self.start_time, process_start_time(self.pid)) {
            (Some(attached), Some(current)) => attached == current,
            // Unknown on one side or the other: fall back to liveness alone.
            _ => true,
        }
    }
}

/// The kernel's start time for a process.
///
/// Linux only for now: field 22 of `/proc/<pid>/stat`, in clock ticks since
/// boot. Elsewhere this returns `None` and callers fall back to a bare liveness
/// check.
#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The second field is the executable name in parentheses and may itself
    // contain spaces and parentheses, so fields are counted from the last ')'.
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn process_start_time(_pid: u32) -> Option<u64> {
    None
}

pub fn process_is_running(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as i32, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
#[path = "client_process_tests.rs"]
mod tests;
