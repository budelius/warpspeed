//! A pseudoconsole owned by the supervisor rather than by the app.
//!
//! The app's own PTY (`super::Pty`) is wired into a mio event loop and an
//! `AppContext`, neither of which exists in the supervisor process. What the
//! supervisor needs is smaller: create the pseudoconsole, start the shell under
//! it, and hold the pipe so it can read output while no client is attached and
//! lend the handle when one arrives.
//!
//! ## Why the reads are overlapped
//!
//! `pipes::create_async_anonymous_pipe` deliberately does not set
//! `FILE_SYNCHRONOUS_IO_NONALERT`, so the pipe is asynchronous. That is not an
//! accident to be worked around: the client wraps this same handle in
//! `mio::windows::NamedPipe`, which requires an overlapped handle. Making a
//! synchronous pipe for the supervisor's convenience would break every client
//! it lends to, so the supervisor does overlapped reads instead.

use std::path::Path;

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{COORD, HPCON};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateEventW, CreateProcessW, EXTENDED_STARTUPINFO_PRESENT,
    PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, ResetEvent, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    TerminateProcess, WaitForSingleObject,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

use super::conpty_api::ConptyApi;
use super::pipes;
use super::proc_thread_attribute_list::ProcThreadAttributeList;

/// `CREATE_BREAKAWAY_FROM_JOB`, so the shell is not killed when Warp's job
/// object closes.
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

/// `ERROR_ACCESS_DENIED`, returned when the job forbids breakaway.
const ACCESS_DENIED: i32 = 5;

/// A pseudoconsole and the shell running under it, owned by the supervisor.
pub struct SupervisedPty {
    /// The shell's process id.
    pid: u32,

    /// The shell process, for waiting and killing.
    process: HANDLE,

    /// The pseudoconsole.
    pty: HPCON,

    /// The supervisor's end of the duplex pipe. This is the handle lent to a
    /// client.
    pipe: HANDLE,

    /// Signals completion of an overlapped read.
    read_event: HANDLE,

    /// Signals completion of an overlapped write. Separate from the read event
    /// so a write cannot consume a read's completion signal.
    write_event: HANDLE,

    conpty_api: ConptyApi,

    /// Whether the shell escaped Warp's job object; see
    /// [`super::daemon_spawn`] for why this is a preference rather than a
    /// requirement.
    escaped_job_object: bool,
}

impl SupervisedPty {
    /// The shell's process id.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The handle a client reads and writes, for lending with
    /// [`super::handle_lending::lend_handle_to_process`].
    pub fn pipe_handle(&self) -> HANDLE {
        self.pipe
    }

    /// Whether the shell left Warp's job object.
    pub fn escaped_job_object(&self) -> bool {
        self.escaped_job_object
    }

    /// Reads output, waiting up to `timeout_ms` for some to arrive.
    ///
    /// Returns the number of bytes read, which is zero on timeout.
    pub fn read(&mut self, buffer: &mut [u8], timeout_ms: u32) -> Result<usize> {
        let mut overlapped = OVERLAPPED {
            hEvent: self.read_event,
            ..Default::default()
        };
        unsafe { ResetEvent(self.read_event) }.context("Failed to reset the pty read event")?;

        let pending =
            match unsafe { ReadFile(self.pipe, Some(buffer), None, Some(&mut overlapped)) } {
                Ok(()) => false,
                Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => true,
                Err(error) => return Err(error).context("Failed to read from the supervised pty"),
            };

        if pending {
            match unsafe { WaitForSingleObject(self.read_event, timeout_ms) } {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => {
                    // The read is still outstanding and the kernel still owns
                    // `overlapped`, which is about to go out of scope. Cancel
                    // it and wait for the cancellation to land before letting
                    // that happen: returning here would leave the kernel
                    // writing into a dead stack frame, and would leave the pipe
                    // with a pending operation that makes every later read
                    // return nothing.
                    let _ = unsafe { CancelIoEx(self.pipe, Some(&overlapped)) };
                    let mut cancelled = 0u32;
                    let _ = unsafe {
                        GetOverlappedResult(self.pipe, &overlapped, &mut cancelled, true)
                    };
                    return Ok(0);
                }
                other => bail!("Waiting on the supervised pty read failed: {other:?}"),
            }
        }

        let mut read = 0u32;
        unsafe { GetOverlappedResult(self.pipe, &overlapped, &mut read, false) }
            .context("Failed to complete a read from the supervised pty")?;

        Ok(read as usize)
    }

    /// Writes input to the shell.
    ///
    /// Overlapped like the reads, and for the same reason: this pipe is
    /// asynchronous, so a synchronous `WriteFile` on it has no defined
    /// behaviour.
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let mut overlapped = OVERLAPPED {
            hEvent: self.write_event,
            ..Default::default()
        };
        unsafe { ResetEvent(self.write_event) }.context("Failed to reset the pty write event")?;

        match unsafe { WriteFile(self.pipe, Some(bytes), None, Some(&mut overlapped)) } {
            Ok(()) => {}
            Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => {}
            Err(error) => return Err(error).context("Failed to write to the supervised pty"),
        }

        let mut written = 0u32;
        unsafe { GetOverlappedResult(self.pipe, &overlapped, &mut written, true) }
            .context("Failed to complete a write to the supervised pty")?;

        if written as usize != bytes.len() {
            bail!(
                "Wrote {written} of {} bytes to the supervised pty",
                bytes.len()
            );
        }

        Ok(())
    }

    /// Tells the pseudoconsole its new size.
    pub fn resize(&mut self, columns: u16, rows: u16) -> Result<()> {
        unsafe {
            self.conpty_api.resize(
                self.pty,
                COORD {
                    X: columns as i16,
                    Y: rows as i16,
                },
            )
        }
        .context("Failed to resize the supervised pty")
    }

    /// Whether the shell is still running.
    pub fn is_running(&self) -> bool {
        (unsafe { WaitForSingleObject(self.process, 0) }) == WAIT_TIMEOUT
    }

    /// Waits for the shell to exit, up to `timeout_ms`.
    pub fn wait(&self, timeout_ms: u32) -> bool {
        (unsafe { WaitForSingleObject(self.process, timeout_ms) }) == WAIT_OBJECT_0
    }

    /// Ends the session.
    pub fn kill(&mut self) -> Result<()> {
        unsafe { TerminateProcess(self.process, 1) }
            .context("Failed to terminate the supervised shell")
    }
}

impl Drop for SupervisedPty {
    fn drop(&mut self) {
        // Closing the pseudoconsole ends the console session, which terminates
        // whatever is still attached to it.
        unsafe { self.conpty_api.close(self.pty) };
        for handle in [self.pipe, self.process, self.read_event, self.write_event] {
            if !handle.is_invalid() {
                let _ = unsafe { CloseHandle(handle) };
            }
        }
    }
}

/// Starts `command_line` under a new pseudoconsole owned by this process.
pub fn spawn_supervised_pty(
    command_line: &str,
    columns: u16,
    rows: u16,
    working_directory: Option<&Path>,
) -> Result<SupervisedPty> {
    let conpty_api = unsafe { ConptyApi::load() }?;

    let pipes::DuplexPipe { client, server } = pipes::create_async_anonymous_pipe()
        .map_err(|error| anyhow::anyhow!("Failed to create the supervised pty pipe: {error}"))?;

    let size = COORD {
        X: columns as i16,
        Y: rows as i16,
    };
    let pty = unsafe { conpty_api.create(size, client, 0) }
        .context("Failed to create the supervised pseudoconsole")?;

    // Tell the pseudoconsole it is already visible, matching what the app's own
    // spawn path does.
    let _ = unsafe { conpty_api.show_hide(pty, true) };

    let mut startup_info = STARTUPINFOEXW::default();
    startup_info.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup_info.StartupInfo.dwFlags = STARTF_USESTDHANDLES;

    let mut attributes = unsafe { ProcThreadAttributeList::new() }
        .context("Failed to allocate the attribute list")?;
    attributes
        .set_pty_connection(pty)
        .context("Failed to attach the child to the pseudoconsole")?;
    startup_info.lpAttributeList = attributes.as_mut_ptr();

    let mut command: Vec<u16> = command_line
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let directory = working_directory
        .filter(|path| path.is_dir())
        .map(|path| HSTRING::from(path.as_os_str()));

    let mut process_information = PROCESS_INFORMATION::default();
    let base_flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT;

    // Ask for job breakaway, and fall back without it if the job forbids that.
    // A shell inside Warp's job is far better than no shell; see
    // `super::daemon_spawn` for the full reasoning.
    let mut escaped_job_object = true;
    let mut result = create_process(
        &mut command,
        base_flags | PROCESS_CREATION_FLAGS(CREATE_BREAKAWAY_FROM_JOB),
        directory.as_ref(),
        &startup_info,
        &mut process_information,
    );

    if result
        .as_ref()
        .err()
        .is_some_and(|error| error.code().0 & 0xFFFF == ACCESS_DENIED)
    {
        escaped_job_object = false;
        result = create_process(
            &mut command,
            base_flags,
            directory.as_ref(),
            &startup_info,
            &mut process_information,
        );
    }

    if let Err(error) = result {
        unsafe { conpty_api.close(pty) };
        let _ = unsafe { CloseHandle(server) };
        return Err(error).context("Failed to start the supervised shell");
    }

    // The thread handle is not needed; only the process is waited on.
    let _ = unsafe { CloseHandle(process_information.hThread) };

    let read_event = unsafe { CreateEventW(None, true, false, None) }
        .context("Failed to create the supervised pty read event")?;
    let write_event = unsafe { CreateEventW(None, true, false, None) }
        .context("Failed to create the supervised pty write event")?;

    Ok(SupervisedPty {
        pid: process_information.dwProcessId,
        process: process_information.hProcess,
        pty,
        pipe: server,
        read_event,
        write_event,
        conpty_api,
        escaped_job_object,
    })
}

/// The `CreateProcessW` call, factored out so it can be retried with different
/// creation flags.
fn create_process(
    command: &mut [u16],
    flags: PROCESS_CREATION_FLAGS,
    directory: Option<&HSTRING>,
    startup_info: &STARTUPINFOEXW,
    process_information: &mut PROCESS_INFORMATION,
) -> windows::core::Result<()> {
    unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR::from_raw(command.as_mut_ptr())),
            None,
            None,
            false,
            flags,
            None,
            directory
                .map(|dir| PCWSTR(dir.as_ptr()))
                .unwrap_or(PCWSTR::null()),
            &startup_info.StartupInfo,
            process_information,
        )
    }
}

#[cfg(test)]
#[path = "supervised_pty_tests.rs"]
mod tests;
