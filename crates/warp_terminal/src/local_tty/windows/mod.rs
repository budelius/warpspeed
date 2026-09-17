mod attachment_relay;
mod child;
mod conpty_api;
pub mod daemon_spawn;
mod environment;
pub mod handle_lending;
#[cfg(test)]
#[path = "iocp_reuse_tests.rs"]
mod iocp_reuse_tests;
pub mod pipe_security;
pub mod pipe_transport;
mod pipes;
mod proc_thread_attribute_list;
pub mod supervised_pty;
pub(crate) mod supervisor_client;
pub mod supervisor_daemon;
mod supervisor_host;

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use child::ChildExitWatcher;
pub use conpty_api::ConptyApi;
use conpty_api::ConptyApiError;
use environment::get_shell_environment_variables;
pub use environment::get_user_and_system_env_variable;
use thiserror::Error;
use warp_core::SessionId;
use warp_errors::{report_error, report_if_error};
use warpui_core::{AppContext, SingletonEntity};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Console::{COORD, HPCON};
use windows::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    EXTENDED_STARTUPINFO_PRESENT, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

use super::event_loop::{PTY_TOKEN, SIGNALS_TOKEN};
use super::shell::{DirectShellStarter, ShellStarter, WslShellStarter};
use super::spawner::PtyHandle;
use super::{EventedPty, EventedReadWrite, PtyOptions, SizeInfo, mio_channel};
use crate::local_tty::spawner::{PtySpawnInfo, PtySpawner};
use crate::local_tty::supervisor::message::SessionMetadata;
use crate::local_tty::windows::proc_thread_attribute_list::ProcThreadAttributeList;
use crate::writeable_pty;

trait ToCoord {
    fn to_coord(&self) -> COORD;
}

impl ToCoord for SizeInfo {
    fn to_coord(&self) -> COORD {
        COORD {
            X: self.columns as i16,
            Y: self.rows as i16,
        }
    }
}

/// A wrapper around [`HANDLE`] that is marked as being safe to share between
/// threads.
#[derive(Clone, Copy)]
struct ShareableHandle(pub HANDLE);

// Mark `ShareableHandle` as being safe to share between threads,
// even though `HANDLE` holds a `*mut c_void`, which isn't inherently
// safe to share.
unsafe impl Send for ShareableHandle {}
unsafe impl Sync for ShareableHandle {}

pub(super) struct PtySpawnResult {
    pub pid: u32,
    pub pipe: mio::windows::NamedPipe,
    pty_handle: Option<HPCON>,
    conpty_api: Option<ConptyApi>,
    child_exit_watcher: Option<ChildExitWatcher>,
    supervisor: Option<(
        Arc<supervisor_client::WindowsTerminalSupervisorClient>,
        SessionId,
    )>,
    replay: Option<Vec<u8>>,
}

impl PtySpawnResult {
    pub(super) fn from_supervisor(
        endpoint: crate::local_tty::supervisor::transport::Endpoint,
        client: Arc<supervisor_client::WindowsTerminalSupervisorClient>,
        session_id: SessionId,
    ) -> anyhow::Result<Self> {
        Self::from_attached(endpoint, client, session_id, Vec::new())
    }

    pub(super) fn from_attached(
        endpoint: crate::local_tty::supervisor::transport::Endpoint,
        client: Arc<supervisor_client::WindowsTerminalSupervisorClient>,
        session_id: SessionId,
        replay: Vec<u8>,
    ) -> anyhow::Result<Self> {
        if endpoint.raw == 0 {
            anyhow::bail!("Windows terminal supervisor returned an invalid PTY handle");
        }
        let pipe = unsafe {
            mio::windows::NamedPipe::from_raw_handle(endpoint.raw as usize as *mut std::ffi::c_void)
        };
        Ok(Self {
            pid: endpoint.pid,
            pipe,
            pty_handle: None,
            conpty_api: None,
            child_exit_watcher: None,
            supervisor: Some((client, session_id)),
            replay: (!replay.is_empty()).then_some(replay),
        })
    }
}

pub struct PseudoConsoleChild {
    process_info: PROCESS_INFORMATION,
}

// Mark `ChildExitWatcher` as being safe to share between threads,
// even though `PROCESS_INFORMATION` holds `HANDLE`s, which each hold
// a `*mut c_void`, which isn't inherently safe to share.
unsafe impl Send for PseudoConsoleChild {}
unsafe impl Sync for PseudoConsoleChild {}

impl PseudoConsoleChild {
    /// How long to wait for the root shell process to exit after asking for its termination.
    const TERMINATE_WAIT_MS: u32 = 1_000;

    pub fn id(&self) -> u32 {
        self.process_info.dwProcessId
    }

    pub fn is_terminated(&self) -> bool {
        let wait_event = unsafe { WaitForSingleObject(self.process_info.hProcess, 0) };
        wait_event == WAIT_OBJECT_0
    }

    /// Forcefully terminates the root shell process.
    ///
    /// Descendant processes are intentionally not terminated here: closing the pseudoconsole (see
    /// [`EventedPty::kill`] for [`Pty`]) ends the console session, and the console host terminates
    /// every client process still attached to it. This mirrors Windows Terminal's
    /// `ConptyConnection::Close()`.
    pub fn kill(&mut self) -> anyhow::Result<()> {
        if self.is_terminated() {
            return Ok(());
        }

        unsafe {
            TerminateProcess(self.process_info.hProcess, 1)
                .map_err(anyhow::Error::from)
                .with_context(|| format!("Failed to terminate PTY root process {}", self.id()))?;
        }

        match unsafe { WaitForSingleObject(self.process_info.hProcess, Self::TERMINATE_WAIT_MS) } {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => {
                anyhow::bail!(
                    "Timed out waiting for PTY root process {} to exit",
                    self.id()
                )
            }
            wait_result => anyhow::bail!(
                "Unexpected wait result {wait_result:?} while terminating PTY root process {}",
                self.id()
            ),
        }
    }
}

impl Drop for PseudoConsoleChild {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.process_info.hThread);
            let _ = CloseHandle(self.process_info.hProcess);
        }
    }
}

#[derive(Error, Debug)]
pub enum PtySpawnError {
    #[error("Could not create anonymous pipes for PTY input/output: {0:#}")]
    CreateAnonymousPipesFailed(#[source] std::io::Error),
    #[error("Could not create pipe for PTY input/output: {0:#}")]
    CreatePipeFailed(#[source] pipes::CreatePipeError),
    #[error("Could not create a pseudoconsole: {0:#}")]
    CreatePseudoConsoleFailed(#[source] windows::core::Error),
    #[error("Failed to initialize the thread attribute list: {0:#}")]
    InitializeThreadAttributeListFailed(#[source] windows::core::Error),
    #[error("Failed to set psuedoconsole attribute to thread attribute list: {0:#}")]
    SetThreadAttributeListFailed(#[source] windows::core::Error),
    #[error("Failed to encode shell command: {0:#}")]
    CreateShellCommandFailed(#[from] EncodingError),
    #[error("Failed to create shell process {detail}: {error:#}")]
    CreateShellProcessFailed {
        detail: String,
        #[source]
        error: windows::core::Error,
    },
    #[error("Failed to load ConPTY functions: {0:#}")]
    LoadConPtyApiFailed(#[from] ConptyApiError),
    #[error("Failed to add Child Exit watcher: {0:#}")]
    ChildExitWatcherFailed(#[source] windows::core::Error),
    #[error("Failed to write shell command bytes to pty: {0:#}")]
    FailedToWriteCommandBytes(#[source] std::io::Error),
    #[error("Shell starter is not supported on this platform: {0}")]
    UnsupportedShellStarter(String),
}

pub(super) fn spawn(
    options: PtyOptions,
    event_loop_tx: mio_channel::Sender<writeable_pty::Message>,
) -> Result<PtySpawnInfo, PtySpawnError> {
    let conpty_api = unsafe { ConptyApi::load() }?;
    let environment_block = get_shell_environment_variables(&options);

    let PtyOptions {
        size,
        shell_starter,
        ..
    } = options;

    let pipes::DuplexPipe { client, server } =
        pipes::create_async_anonymous_pipe().map_err(PtySpawnError::CreatePipeFailed)?;

    // Create the pseudoconsole, giving it the handle to the client side of the pipe.
    let pty_handle = match unsafe { conpty_api.create(size.to_coord(), client, 0) } {
        Ok(pty_handle) => pty_handle,
        Err(err) => return Err(PtySpawnError::CreatePseudoConsoleFailed(err)),
    };

    // Tell the pseudoconsole that it is already visible.
    let _ = unsafe { conpty_api.show_hide(pty_handle, true) };

    // Spawn the child process, and tell it to communicate via the pseudoconsole.
    // The default zeros the memory.
    let mut startup_info = STARTUPINFOEXW::default();
    startup_info.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup_info.StartupInfo.dwFlags = STARTF_USESTDHANDLES;

    let mut attrs = unsafe {
        ProcThreadAttributeList::new()
            .map_err(PtySpawnError::InitializeThreadAttributeListFailed)?
    };
    attrs
        .set_pty_connection(pty_handle)
        .map_err(PtySpawnError::SetThreadAttributeListFailed)?;
    startup_info.lpAttributeList = attrs.as_mut_ptr();

    // Create the hosted shell process.
    let shell_command = match &shell_starter {
        ShellStarter::Direct(shell_starter) | ShellStarter::MSYS2(shell_starter) => {
            shell_command(shell_starter)?
        }
        ShellStarter::Wsl(wsl_shell_starter) => wsl_shell_command(wsl_shell_starter)?,
        ShellStarter::DockerSandbox(_) => {
            // Docker sandbox shells are only supported on Unix; they should
            // never reach the Windows PTY spawn path. Surface as an error
            // rather than panicking so a rogue persisted/round-tripped
            // sandbox starter degrades gracefully on Windows.
            report_error!("Docker sandbox shell starter reached the Windows PTY spawn path");
            return Err(PtySpawnError::UnsupportedShellStarter(
                "Docker sandbox shells are not supported on Windows".to_owned(),
            ));
        }
    };
    let mut process_information = PROCESS_INFORMATION::default();

    let start_directory = options
        .start_dir
        .filter(|start_dir| start_dir.is_dir())
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .filter(|path| path.is_dir())
        })
        .map(|path| HSTRING::from(path.as_os_str()));

    let base_flags =
        PROCESS_CREATION_FLAGS(0) | EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT;

    // Ask to leave Warp's job object, and start inside it if the job forbids
    // that. `CREATE_BREAKAWAY_FROM_JOB` keeps a shell alive when the job around
    // Warp is closed, but a job created without `JOB_OBJECT_LIMIT_BREAKAWAY_OK`
    // answers the request with `ERROR_ACCESS_DENIED` -- and Warp cannot change
    // that from the inside. Refusing to fall back means no shell starts at all
    // whenever Warp runs under something that makes such a job: a test runner,
    // a CI agent, some terminals. A shell inside the job is far better than
    // none. The same trade is made in `supervised_pty` and `daemon_spawn`.
    let mut spawn = |flags| unsafe {
        CreateProcessW(
            PCWSTR::null(), /* lpApplicationName */
            Some(PWSTR::from_raw(shell_command.as_ptr().cast_mut())),
            None,  /* lpProcessAttributes */
            None,  /* lpThreadAttributes */
            false, /* bInheritHandles */
            flags,
            Some(environment_block.as_ptr() as *const std::ffi::c_void),
            start_directory
                .as_ref()
                .map(|hstring| PCWSTR::from_raw(hstring.as_ptr()))
                .unwrap_or(PCWSTR::null()),
            &startup_info.StartupInfo as *const STARTUPINFOW,
            &mut process_information,
        )
    };

    let mut result = spawn(base_flags | CREATE_BREAKAWAY_FROM_JOB);
    if result
        .as_ref()
        .err()
        .is_some_and(|error| error.code().0 & 0xFFFF == ACCESS_DENIED)
    {
        log::info!(
            "The job object around Warp forbids breakaway; starting the shell inside it instead"
        );
        result = spawn(base_flags);
    }
    result.map_err(|error| {
        let detail = shell_starter.shell_detail();
        PtySpawnError::CreateShellProcessFailed { detail, error }
    })?;

    let _ = unsafe { conpty_api.release(pty_handle) };

    let pipe = unsafe { mio::windows::NamedPipe::from_raw_handle(server.0 as *mut _) };
    let child_exit_watcher = ChildExitWatcher::new(process_information.hProcess, event_loop_tx)
        .map_err(PtySpawnError::ChildExitWatcherFailed)?;
    let child = PseudoConsoleChild {
        process_info: process_information,
    };
    let result = PtySpawnResult {
        pid: process_information.dwProcessId,
        pipe,
        pty_handle: Some(pty_handle),
        conpty_api: Some(conpty_api),
        child_exit_watcher: Some(child_exit_watcher),
        supervisor: None,
        replay: None,
    };
    Ok(PtySpawnInfo { result, child })
}
#[derive(Error, Debug)]
pub enum EncodingError {
    #[error("Could not encode argument {arg:?}")]
    InvalidArgumentEncoding { arg: OsString },
    #[error(transparent)]
    Other(#[from] windows::core::Error),
}

fn wsl_shell_command(wsl_shell_starter: &WslShellStarter) -> Result<HSTRING, EncodingError> {
    let mut encoded_shell_command = Vec::<u16>::new();

    log::info!(
        "Starting WSL shell process: {:?}",
        wsl_shell_starter.distribution()
    );
    append_quoted(&WslShellStarter::wsl_command(), &mut encoded_shell_command);
    for arg in wsl_shell_starter.args() {
        encoded_shell_command.push(' ' as u16);
        if arg.encode_wide().any(|c| c == 0) {
            return Err(EncodingError::InvalidArgumentEncoding { arg: arg.clone() });
        }
        append_quoted(arg, &mut encoded_shell_command);
    }
    Ok(HSTRING::from_wide(&encoded_shell_command))
}

/// Constructs the shell command in a Windows-native encoding using 16-bit values.
///
/// Windows expects a single string for its command which includes all arguments, so we need to
/// string them together and escape them appropriately.
/// `ERROR_ACCESS_DENIED`, returned when the job forbids breakaway.
const ACCESS_DENIED: i32 = 5;

fn shell_command(shell_starter: &DirectShellStarter) -> Result<HSTRING, EncodingError> {
    let mut encoded_shell_command = Vec::<u16>::new();

    log::info!(
        "Starting direct shell process: {:?}",
        shell_starter.logical_shell_path()
    );
    append_quoted(
        shell_starter.logical_shell_path().as_os_str(),
        &mut encoded_shell_command,
    );

    for arg in shell_starter.args() {
        encoded_shell_command.push(' ' as u16);
        if arg.encode_wide().any(|c| c == 0) {
            return Err(EncodingError::InvalidArgumentEncoding { arg: arg.clone() });
        }
        append_quoted(arg, &mut encoded_shell_command);
    }
    Ok(HSTRING::from_wide(&encoded_shell_command))
}

/// Appends an argument and properly quotes it.
///
/// See: https://learn.microsoft.com/en-us/archive/blogs/twistylittlepassagesallalike/everyone-quotes-command-line-arguments-the-wrong-way
fn append_quoted(arg: &OsStr, cmdline: &mut Vec<u16>) {
    if !arg.is_empty()
        && !arg.encode_wide().any(|c| {
            c == ' ' as u16
                || c == '\t' as u16
                || c == '\n' as u16
                || c == '\x0b' as u16
                || c == '\"' as u16
        })
    {
        cmdline.extend(arg.encode_wide());
        return;
    }
    cmdline.push('"' as u16);

    let arg: Vec<_> = arg.encode_wide().collect();
    let mut i = 0;
    while i < arg.len() {
        let mut num_backslashes = 0;
        while i < arg.len() && arg[i] == '\\' as u16 {
            i += 1;
            num_backslashes += 1;
        }

        if i == arg.len() {
            for _ in 0..num_backslashes * 2 {
                cmdline.push('\\' as u16);
            }
            break;
        } else if arg[i] == b'"' as u16 {
            for _ in 0..num_backslashes * 2 + 1 {
                cmdline.push('\\' as u16);
            }
            cmdline.push(arg[i]);
        } else {
            for _ in 0..num_backslashes {
                cmdline.push('\\' as u16);
            }
            cmdline.push(arg[i]);
        }
        i += 1;
    }
    cmdline.push('"' as u16);
}

pub struct Pty {
    child_exit_watcher: Option<ChildExitWatcher>,
    handle: Box<dyn PtyHandle>,
    pty_handle: Option<HPCON>,
    pipe: mio::windows::NamedPipe,
    token: mio::Token,
    conpty_api: Option<ConptyApi>,
    supervisor: Option<(
        Arc<supervisor_client::WindowsTerminalSupervisorClient>,
        SessionId,
    )>,
    replay: Option<Vec<u8>>,
    pty_closed: bool,
    /// The last size handed to the console host, as `(columns, rows)`.
    last_applied_size: Option<(i16, i16)>,
}

/// Whether a resize to `next` is worth sending to the console host.
///
/// ConPTY is told a `COORD` and nothing else, so a resize that leaves the
/// column and row counts alone tells it nothing — but it still costs a full
/// buffer reflow in the console host and a prompt repaint in the attached
/// shell. Pane pixel sizes change on every frame of a window drag, which is
/// how a single drag used to storm every open session with resizes that said
/// the same thing.
fn resize_is_worth_sending(
    last_applied: Option<(i16, i16)>,
    next: (i16, i16),
    forced: bool,
) -> bool {
    forced || last_applied != Some(next)
}

impl Pty {
    pub fn new(
        options: PtyOptions,
        persistent_session_id: SessionId,
        hooks: &dyn super::spawner::PtySpawnHooks,
        event_loop_tx: mio_channel::Sender<writeable_pty::Message>,
        ctx: &mut AppContext,
    ) -> anyhow::Result<Self> {
        let size = options.size;
        let (result, handle) = PtySpawner::handle(ctx).update(ctx, |pty_spawner, ctx| {
            pty_spawner.spawn_pty(options, persistent_session_id, hooks, event_loop_tx, ctx)
        })?;
        let mut pty = Self::from_spawn_result(result, handle);
        // Forced: the console host has to learn the session's starting size.
        pty.on_resize(&size, true);
        Ok(pty)
    }

    pub fn attach(
        session_id: SessionId,
        size: &SizeInfo,
        include_snapshot: bool,
        ctx: &mut AppContext,
    ) -> anyhow::Result<Option<(Self, SessionMetadata)>> {
        let Some((result, handle, metadata)) =
            PtySpawner::handle(ctx).update(ctx, |spawner, _| {
                spawner.attach_pty(
                    session_id,
                    include_snapshot,
                    (size.columns() as u16, size.rows() as u16),
                )
            })?
        else {
            return Ok(None);
        };
        let mut pty = Self::from_spawn_result(result, handle);
        // Forced: a reattached session needs its size re-announced.
        pty.on_resize(size, true);
        Ok(Some((pty, metadata)))
    }

    fn from_spawn_result(result: PtySpawnResult, handle: Box<dyn PtyHandle>) -> Self {
        Self {
            child_exit_watcher: result.child_exit_watcher,
            handle,
            pty_handle: result.pty_handle,
            pipe: result.pipe,
            token: PTY_TOKEN,
            conpty_api: result.conpty_api,
            supervisor: result.supervisor,
            replay: result.replay,
            pty_closed: false,
            last_applied_size: None,
        }
    }

    pub fn get_pid(&self) -> u32 {
        self.handle.pid()
    }

    pub fn persistent_session_id(&self) -> Option<SessionId> {
        self.handle.persistent_session_id()
    }

    /// Closes the pseudoconsole and disconnects from the console host.
    ///
    /// Closing the pseudoconsole ends the console session: the console host terminates every client
    /// process still attached to it. Idempotent so that it can run in both [`EventedPty::kill`] and
    /// `Drop`.
    fn close_pseudoconsole(&mut self) {
        use std::io::Read as _;

        if self.pty_closed {
            return;
        }
        self.pty_closed = true;

        let (Some(conpty_api), Some(pty_handle)) = (&self.conpty_api, self.pty_handle) else {
            // The supervisor owns the pseudoconsole. Dropping this process's duplicated pipe
            // handle detaches the client without ending the console session.
            return;
        };
        unsafe {
            conpty_api.close(pty_handle);
        }

        // Drain all data in the pipe.
        let mut buffer = [0; 1000];
        while let Ok(num_byes_read) = self.pipe.read(&mut buffer) {
            if num_byes_read == 0 {
                break;
            }
        }

        // Finally, disconnect from the console host, which will ultimately get it to terminate.
        let _ = self.pipe.disconnect();
    }
}

impl EventedReadWrite for Pty {
    type Reader = mio::windows::NamedPipe;
    type Writer = mio::windows::NamedPipe;

    fn register(&mut self, poll: &mio::Poll, interest: mio::Interest) -> std::io::Result<()> {
        poll.registry()
            .register(&mut self.pipe, self.token, interest)?;
        if let Some(watcher) = self.child_exit_watcher.as_mut() {
            poll.registry()
                .register(watcher, SIGNALS_TOKEN, mio::Interest::READABLE)?;
        }
        Ok(())
    }

    fn reregister(&mut self, poll: &mio::Poll, interest: mio::Interest) -> std::io::Result<()> {
        poll.registry()
            .reregister(&mut self.pipe, self.token, interest)?;
        if let Some(watcher) = self.child_exit_watcher.as_mut() {
            poll.registry()
                .reregister(watcher, SIGNALS_TOKEN, mio::Interest::READABLE)?;
        }
        Ok(())
    }

    fn deregister(&mut self, poll: &mio::Poll) -> std::io::Result<()> {
        poll.registry().deregister(&mut self.pipe)?;
        if let Some(watcher) = self.child_exit_watcher.as_mut() {
            poll.registry().deregister(watcher)?;
        }
        Ok(())
    }

    fn reader(&mut self) -> &mut Self::Reader {
        &mut self.pipe
    }

    fn read_token(&self) -> mio::Token {
        self.token
    }

    fn writer(&mut self) -> &mut Self::Writer {
        &mut self.pipe
    }

    fn write_token(&self) -> mio::Token {
        self.token
    }
}

impl EventedPty for Pty {
    fn child_event_token(&self) -> mio::Token {
        SIGNALS_TOKEN
    }

    fn take_replay_reader(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
        self.replay
            .take()
            .map(|bytes| Box::new(std::io::Cursor::new(bytes)) as Box<dyn std::io::Read + Send>)
    }

    fn requires_exit_polling(&self) -> bool {
        self.handle.requires_exit_polling()
    }

    fn next_child_event(&mut self) -> Option<super::ChildEvent> {
        None
    }

    fn on_resize(&mut self, size: &crate::SizeInfo, forced: bool) {
        let coord = size.to_coord();
        let next = (coord.X, coord.Y);
        if !resize_is_worth_sending(self.last_applied_size, next, forced) {
            return;
        }
        // Only a size the console host actually accepted counts as applied, so
        // a resize that failed can be retried by the next one that repeats it.
        let applied = if let Some((client, session_id)) = self.supervisor.as_ref() {
            let result = client.resize(*session_id, size.columns as u16, size.rows as u16);
            let applied = result.is_ok();
            report_if_error!(result);
            applied
        } else if let (Some(conpty_api), Some(pty_handle)) = (&self.conpty_api, self.pty_handle) {
            let result = unsafe {
                conpty_api
                    .resize(pty_handle, coord)
                    .context("Failed to resize pseudoconsole")
            };
            let applied = result.is_ok();
            report_if_error!(result);
            applied
        } else {
            false
        };

        if applied {
            self.last_applied_size = Some(next);
        }
    }

    fn kill(mut self) -> anyhow::Result<()> {
        self.close_pseudoconsole();
        self.handle.kill()
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.close_pseudoconsole();
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
