//! Lending a PTY endpoint to another process on Windows.
//!
//! The supervisor owns the pseudoconsole and the pipe behind it, but the app
//! does its own I/O so the supervisor stays out of the hot path. On Unix that
//! is `SCM_RIGHTS`: the descriptor is passed over the control socket. Windows
//! has no equivalent, so the owner instead duplicates the handle *into* the
//! target process with [`DuplicateHandle`], and sends the resulting value over
//! the control channel as an ordinary number.
//!
//! The value is only meaningful inside the target process. A duplicated handle
//! is a real reference to the same kernel object, so the object outlives the
//! supervisor closing its own copy, and the target's copy is closed by Windows
//! when that process exits -- which is what makes a crashed client safe.

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE};

/// Duplicates `handle` into the process `target_pid`, returning the handle
/// value as seen by that process.
///
/// The caller keeps its own copy; this hands out a second reference rather than
/// moving the first.
pub fn lend_handle_to_process(handle: HANDLE, target_pid: u32) -> Result<i64> {
    if handle.is_invalid() {
        bail!("Refusing to lend an invalid handle to process {target_pid}");
    }

    // `PROCESS_DUP_HANDLE` is the narrowest right that permits duplication;
    // asking for more would fail against a target we have limited rights to.
    let target = unsafe { OpenProcess(PROCESS_DUP_HANDLE, false, target_pid) }
        .with_context(|| format!("Failed to open process {target_pid} to lend it a handle"))?;

    let mut duplicate = HANDLE::default();
    let result = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle,
            target,
            &mut duplicate,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )
    };

    // Close the process handle either way: it is our reference to the target
    // process, not to the duplicated object, and leaking it would pin the
    // process object for as long as the supervisor runs.
    let _ = unsafe { CloseHandle(target) };

    result.with_context(|| format!("Failed to duplicate a handle into process {target_pid}"))?;

    Ok(duplicate.0 as i64)
}

#[cfg(test)]
#[path = "handle_lending_tests.rs"]
mod tests;
