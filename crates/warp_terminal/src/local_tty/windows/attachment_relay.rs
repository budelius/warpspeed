use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_IO_PENDING, HANDLE, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, ResetEvent, WaitForSingleObject,
};

use super::pipes;

const IO_POLL_MS: u32 = 20;
const BUFFER_SIZE: usize = 64 * 1024;

pub struct AttachmentRelay {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl AttachmentRelay {
    pub fn start(pty: HANDLE) -> Result<(Self, HANDLE)> {
        let pipes::DuplexPipe { client, server } = pipes::create_async_anonymous_pipe()
            .map_err(|error| anyhow::anyhow!("Failed to create attachment relay: {error}"))?;
        let pty = duplicate_local(pty)?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let pty_value = pty.0 as usize;
        let client_value = client.0 as usize;
        let worker = match thread::Builder::new()
            .name("ConPTY attachment relay".to_owned())
            .spawn(move || {
                let pty = HANDLE(pty_value as *mut _);
                let client = HANDLE(client_value as *mut _);
                if let Err(error) = relay(pty, client, &worker_stop) {
                    log::debug!("Attachment relay ended: {error:#}");
                }
                for handle in [pty, client] {
                    let _ = unsafe { CloseHandle(handle) };
                }
            }) {
            Ok(worker) => worker,
            Err(error) => {
                for handle in [pty, client, server] {
                    let _ = unsafe { CloseHandle(handle) };
                }
                return Err(error).context("Failed to start the attachment relay");
            }
        };
        Ok((
            Self {
                stop,
                worker: Some(worker),
            },
            server,
        ))
    }
}

impl Drop for AttachmentRelay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn relay(pty: HANDLE, client: HANDLE, stop: &AtomicBool) -> Result<()> {
    let pty_read = Event::new()?;
    let pty_write = Event::new()?;
    let client_read = Event::new()?;
    let client_write = Event::new()?;
    let mut buffer = [0u8; BUFFER_SIZE];

    while !stop.load(Ordering::Acquire) {
        let read = read_with_timeout(pty, pty_read.0, &mut buffer, IO_POLL_MS)?;
        if read > 0 {
            write_unless_stopped(client, client_write.0, &buffer[..read], stop)?;
        }
        let read = read_with_timeout(client, client_read.0, &mut buffer, IO_POLL_MS)?;
        if read > 0 {
            write_unless_stopped(pty, pty_write.0, &buffer[..read], stop)?;
        }
    }
    Ok(())
}

fn read_with_timeout(
    handle: HANDLE,
    event: HANDLE,
    buffer: &mut [u8],
    timeout_ms: u32,
) -> Result<usize> {
    unsafe { ResetEvent(event) }.context("Failed to reset relay read event")?;
    let mut overlapped = OVERLAPPED {
        hEvent: event,
        ..Default::default()
    };
    let pending = match unsafe { ReadFile(handle, Some(buffer), None, Some(&mut overlapped)) } {
        Ok(()) => false,
        Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => true,
        Err(error) => return Err(error).context("Attachment relay read failed"),
    };
    if pending {
        match unsafe { WaitForSingleObject(event, timeout_ms) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => {
                cancel_and_wait(handle, &overlapped);
                return Ok(0);
            }
            other => bail!("Waiting for attachment relay read failed: {other:?}"),
        }
    }
    let mut read = 0;
    unsafe { GetOverlappedResult(handle, &overlapped, &mut read, false) }
        .context("Failed to complete attachment relay read")?;
    Ok(read as usize)
}

/// Writes all of `bytes`, waiting for as long as the reader takes to make room.
///
/// A slow reader is back-pressure, not a failure: a GUI client stops draining output while it
/// reflows a large scrollback, and giving up on it would close the pipe under a live pane. A
/// reader that has gone away breaks the pipe instead, which fails the write.
fn write_unless_stopped(
    handle: HANDLE,
    event: HANDLE,
    bytes: &[u8],
    stop: &AtomicBool,
) -> Result<()> {
    unsafe { ResetEvent(event) }.context("Failed to reset relay write event")?;
    let mut overlapped = OVERLAPPED {
        hEvent: event,
        ..Default::default()
    };
    let pending = match unsafe { WriteFile(handle, Some(bytes), None, Some(&mut overlapped)) } {
        Ok(()) => false,
        Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => true,
        Err(error) => return Err(error).context("Attachment relay write failed"),
    };
    if pending {
        loop {
            match unsafe { WaitForSingleObject(event, IO_POLL_MS) } {
                WAIT_OBJECT_0 => break,
                WAIT_TIMEOUT if stop.load(Ordering::Acquire) => {
                    cancel_and_wait(handle, &overlapped);
                    bail!("Attachment relay stopped during a write");
                }
                WAIT_TIMEOUT => {}
                other => bail!("Waiting for attachment relay write failed: {other:?}"),
            }
        }
    }
    let mut written = 0;
    unsafe { GetOverlappedResult(handle, &overlapped, &mut written, false) }
        .context("Failed to complete attachment relay write")?;
    if written as usize != bytes.len() {
        bail!("Attachment relay wrote {written} of {} bytes", bytes.len());
    }
    Ok(())
}

fn cancel_and_wait(handle: HANDLE, overlapped: &OVERLAPPED) {
    let _ = unsafe { CancelIoEx(handle, Some(overlapped)) };
    let mut transferred = 0;
    let _ = unsafe { GetOverlappedResult(handle, overlapped, &mut transferred, true) };
}

fn duplicate_local(handle: HANDLE) -> Result<HANDLE> {
    let mut duplicate = HANDLE::default();
    unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle,
            GetCurrentProcess(),
            &mut duplicate,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )
    }
    .context("Failed to duplicate the ConPTY handle for an attachment relay")?;
    Ok(duplicate)
}

struct Event(HANDLE);

impl Event {
    fn new() -> Result<Self> {
        unsafe { CreateEventW(None, true, false, None) }
            .map(Self)
            .context("Failed to create attachment relay event")
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

#[cfg(test)]
#[path = "attachment_relay_tests.rs"]
mod tests;
