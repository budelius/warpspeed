use anyhow::{Context, Result};
use warp_core::SessionId;
use warp_core::features::FeatureFlag;
#[cfg(unix)]
use warp_errors::report_error;
use warpui_core::{AppContext, Entity, SingletonEntity};
#[cfg(unix)]
use {
    crate::local_tty::server::TerminalServer, anyhow::bail, std::cmp::Reverse,
    std::collections::HashMap, std::ffi::OsString, std::process::Child,
};

#[cfg(target_os = "windows")]
use super::PseudoConsoleChild;
#[cfg(unix)]
use super::unix_supervisor::UnixTerminalSupervisorClient;
#[cfg(windows)]
use super::windows::supervisor_client::WindowsTerminalSupervisorClient;
use super::{PtyOptions, PtySpawnResult};
use crate::local_tty::screen::ScreenSnapshot;
use crate::local_tty::supervisor::message::{SessionMetadata, SessionSummary};
use crate::local_tty::{self};

/// One session to hand to the supervisor, with the screen it was last showing.
///
/// The supervisor is blind while a client is attached, so the screen has to
/// travel with the handover or the session resumes from a blank one.
pub struct PendingDetach {
    pub session_id: SessionId,
    pub snapshot: ScreenSnapshot,
    pub metadata: SessionMetadata,
}

impl PendingDetach {
    /// A handover from a caller that has no live grid to render -- a session
    /// named only by a persisted snapshot, or a window already torn down.
    pub fn without_screen(session_id: SessionId) -> Self {
        Self {
            session_id,
            snapshot: ScreenSnapshot::default(),
            metadata: SessionMetadata::default(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum PtySpawnMode {
    TerminalServer,
    FallbackToDirect,
    Direct,
}

pub trait PtySpawnHooks {
    fn before_spawn(&self);
    fn after_spawn(&self);
    fn spawned(&self, mode: PtySpawnMode, ctx: &mut AppContext);
}
/// A handle that can be used to interact with a pty process.
pub trait PtyHandle: Send + Sync {
    /// Returns the pty's process ID.
    fn pid(&self) -> u32;

    /// Returns whether or not the child process has terminated.  This may
    /// return false for an exited child (e.g.: for a server-hosted pty), but
    /// will never return true for a living child.
    fn has_process_terminated(&mut self) -> Result<bool>;

    /// The stable identifier used by an external terminal supervisor.
    fn persistent_session_id(&self) -> Option<SessionId> {
        None
    }

    /// Whether child exit must be polled because it cannot signal the app process.
    fn requires_exit_polling(&self) -> bool {
        false
    }

    /// Kills the pty process and waits for its successful termination.
    fn kill(&mut self) -> Result<()>;
}

type AttachedPty = (PtySpawnResult, Box<dyn PtyHandle>, SessionMetadata);

/// A handle for a pty that is a direct child of the current process.
#[cfg(unix)]
struct DirectPtyHandle {
    child: Child,
}

#[cfg(unix)]
impl PtyHandle for DirectPtyHandle {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn has_process_terminated(&mut self) -> Result<bool> {
        // If the child has exited, try_wait will return Ok(Some(exit_status)).
        self.child
            .try_wait()
            .map(|inner| inner.is_some())
            .map_err(anyhow::Error::from)
    }

    fn kill(&mut self) -> Result<()> {
        self.child.kill()?;
        match self.child.wait() {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }
}

#[cfg(target_os = "windows")]
struct DirectPtyHandle {
    child: PseudoConsoleChild,
}

#[cfg(target_os = "windows")]
impl PtyHandle for DirectPtyHandle {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn has_process_terminated(&mut self) -> Result<bool> {
        Ok(self.child.is_terminated())
    }

    fn kill(&mut self) -> Result<()> {
        self.child.kill()
    }
}

/// A handle for a PTY the Unix session supervisor owns.
///
/// Killing it is conditional: a session that was detached belongs to the
/// supervisor now, and ending it here would defeat the detach.
#[cfg(unix)]
struct SupervisorOwnedPtyHandle {
    client: std::sync::Arc<UnixTerminalSupervisorClient>,
    session_id: SessionId,
    pid: u32,
}

#[cfg(unix)]
impl PtyHandle for SupervisorOwnedPtyHandle {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn has_process_terminated(&mut self) -> Result<bool> {
        self.client
            .is_running(self.session_id)
            .map(|running| !running)
    }

    fn persistent_session_id(&self) -> Option<SessionId> {
        Some(self.session_id)
    }

    /// The shell is not this process's child, so `SIGCHLD` never fires for it.
    fn requires_exit_polling(&self) -> bool {
        true
    }

    fn kill(&mut self) -> Result<()> {
        if self.client.take_preserved(self.session_id) {
            Ok(())
        } else {
            self.client.kill(self.session_id)
        }
    }
}

#[cfg(windows)]
struct SupervisorOwnedPtyHandle {
    client: std::sync::Arc<WindowsTerminalSupervisorClient>,
    session_id: SessionId,
    pid: u32,
}

#[cfg(windows)]
impl PtyHandle for SupervisorOwnedPtyHandle {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn has_process_terminated(&mut self) -> Result<bool> {
        self.client
            .is_running(self.session_id)
            .map(|running| !running)
    }

    fn persistent_session_id(&self) -> Option<SessionId> {
        Some(self.session_id)
    }

    fn requires_exit_polling(&self) -> bool {
        true
    }

    fn kill(&mut self) -> Result<()> {
        if self.client.take_preserved(self.session_id) {
            Ok(())
        } else {
            self.client.kill(self.session_id)
        }
    }
}
/// Puts a session's replay bytes somewhere the PTY event loop can read them.
///
/// The Unix event loop takes its replay as a descriptor, and the v2 supervisor
/// hands the bytes over in its attach response. An unlinked temp file is what
/// bridges the two: a pipe would deadlock, since the ring holds far more than a
/// pipe's buffer and nothing drains it until the event loop starts.
#[cfg(unix)]
fn replay_descriptor(replay: &[u8]) -> Result<Option<std::os::fd::RawFd>> {
    use std::io::{Seek, Write};
    use std::os::fd::IntoRawFd;

    if replay.is_empty() {
        return Ok(None);
    }
    let mut file = tempfile::tempfile().context("Failed to stage a terminal session's replay")?;
    file.write_all(replay)
        .context("Failed to write a terminal session's replay")?;
    file.rewind()
        .context("Failed to rewind a terminal session's replay")?;
    Ok(Some(file.into_raw_fd()))
}

/// Invokes the provided callback function without crash reporting enabled.
fn invoke_without_crash_reporting<T>(hooks: &dyn PtySpawnHooks, func: impl FnOnce() -> T) -> T {
    // Uninitialize cocoa-sentry before spawning the shell process to avoid passing any custom state
    // (such as BSD signal handlers and mach exception handlers) into the shell process. This means
    // we lose all Cocoa crash reports from now until when the session is successfully spawned,
    // which is not ideal but allows us to fully ensure that we don't improperly leak any Sentry state
    // into the child processes.
    hooks.before_spawn();

    let retval = func();

    // Now that the child has spawned--reinitialize cocoa sentry.
    hooks.after_spawn();

    retval
}

pub(super) struct PtySpawnInfo {
    pub result: PtySpawnResult,
    #[cfg(unix)]
    pub child: Child,
    #[cfg(windows)]
    pub child: PseudoConsoleChild,
}

/// A global singleton that provides the ability to spawn ptys.
///
/// This abstracts away from callers the manner in which the pty is spawned -
/// depending on configuration, the pty might be spawned as a child of the
/// current process, or it may be spawned by a subprocess that is responsible
/// for owning and managing ptys.
pub struct PtySpawner {
    #[cfg(unix)]
    server: Option<TerminalServer>,
    #[cfg(unix)]
    supervisor: Option<std::sync::Arc<UnixTerminalSupervisorClient>>,
    #[cfg(windows)]
    supervisor: Option<std::sync::Arc<WindowsTerminalSupervisorClient>>,
}

impl PtySpawner {
    /// Creates a new PtySpawner.
    ///
    /// This should be called extremely early in the application startup
    /// process - we want to minimize the number of already-obtained resources
    /// that could leak into forked subprocesses (e.g.: file descriptors).
    pub fn new() -> Result<Self> {
        cfg_if::cfg_if! {
            if #[cfg(unix)] {
                let supervisor = FeatureFlag::PersistentTerminalSessions
                    .is_enabled()
                    .then(UnixTerminalSupervisorClient::new)
                    .transpose()?
                    .map(std::sync::Arc::new);
                // With the v2 supervisor running, the older persistent server
                // would be a second daemon supervising nothing: every session
                // is spawned through the supervisor below. The server stays as
                // the fallback spawn path, which needs no supervision.
                let server = if supervisor.is_some() {
                    super::server::TerminalServer::new_ephemeral()?
                } else {
                    super::server::TerminalServer::new()?
                };
                Ok(Self {
                    server: Some(server),
                    supervisor,
                })
            } else if #[cfg(target_os = "windows")] {
                let supervisor = FeatureFlag::PersistentTerminalSessions
                    .is_enabled()
                    .then(WindowsTerminalSupervisorClient::new)
                    .transpose()?
                    .map(std::sync::Arc::new);
                Ok(Self { supervisor })
            } else {
                unreachable!("Spawning a PTY is not supported on this platform.");
            }
        }
    }

    /// Creates a new PtySpanwer that is configured for unit test purposes.
    pub fn new_for_test() -> Self {
        cfg_if::cfg_if! {
            if #[cfg(unix)] {
                Self { server: None, supervisor: None }
            } else if #[cfg(target_os = "windows")] {
                Self { supervisor: None }
            } else {
                unreachable!("Spawning a PTY for tests is not supported on this platform.");
            }
        }
    }

    /// Marks selected supervised sessions for transfer without relinquishing this process's
    /// terminal-server client, since other windows may continue creating shells here.
    pub fn prepare_sessions_for_handoff(&self, handovers: Vec<PendingDetach>) -> Result<()> {
        #[cfg(unix)]
        if let Some(supervisor) = self.supervisor.as_ref() {
            for handover in handovers {
                supervisor.detach(handover.session_id, handover.snapshot, handover.metadata)?;
            }
            return Ok(());
        }
        #[cfg(unix)]
        if let Some(server) = self.server.as_ref()
            && server.client().is_persistent()
        {
            let session_ids = handovers
                .iter()
                .map(|handover| handover.session_id)
                .collect();
            server.prepare_for_app_termination(session_ids)?;
        }
        #[cfg(windows)]
        if let Some(supervisor) = self.supervisor.as_ref() {
            for handover in handovers {
                supervisor.detach(handover.session_id, handover.snapshot, handover.metadata)?;
            }
        }
        Ok(())
    }

    /// Does any work necessary to clean up state in advance of the app
    /// terminating.
    ///
    /// Every session in `handovers` outlives this process; everything else the
    /// supervisor holds for it is killed as its PTY handle is dropped.
    pub fn prepare_for_app_termination(&mut self, handovers: Vec<PendingDetach>) {
        #[cfg(unix)]
        let handovers = match self.supervisor.as_ref() {
            Some(supervisor) => {
                for handover in handovers {
                    let session_id = handover.session_id;
                    if let Err(error) =
                        supervisor.detach(session_id, handover.snapshot, handover.metadata)
                    {
                        log::error!(
                            "Failed to detach supervised terminal session {session_id:?}: {error:#}"
                        );
                    }
                }
                // The v1 server never owned these sessions, so it has nothing
                // left to be told about; it still has to be torn down below.
                Vec::new()
            }
            None => handovers,
        };

        #[cfg(unix)]
        if let Some(server) = self.server.take() {
            if server.client().is_persistent() {
                let session_ids = handovers
                    .iter()
                    .map(|handover| handover.session_id)
                    .collect();
                if let Err(error) = server.prepare_for_app_termination(session_ids) {
                    log::error!(
                        "Failed to prepare supervised terminal sessions for app termination: {error:#}"
                    );
                }
            } else {
                log::info!("Tearing down terminal server...");
            }
            drop(server);
        }

        #[cfg(windows)]
        if let Some(supervisor) = self.supervisor.as_ref() {
            for handover in handovers {
                let session_id = handover.session_id;
                if let Err(error) =
                    supervisor.detach(session_id, handover.snapshot, handover.metadata)
                {
                    log::error!(
                        "Failed to detach supervised terminal session {session_id:?}: {error:#}"
                    );
                }
            }
        }
    }

    /// Whether a session can be handed to a supervisor and later found again
    /// from a list, which is what a user-driven detach and attach needs.
    ///
    /// The Unix supervisor is still on protocol v1, which can preserve a
    /// session across app exit but cannot enumerate what it is holding. A
    /// detach the user cannot undo is worse than no detach at all, so the
    /// feature stays off there until v1 is migrated.
    pub fn supports_detached_sessions(&self) -> bool {
        #[cfg(any(unix, windows))]
        {
            self.supervisor.is_some()
        }
        #[cfg(not(any(unix, windows)))]
        {
            false
        }
    }

    /// Hands one session to the supervisor and leaves it running there.
    ///
    /// The session's PTY handle stops being this process's to kill, so closing
    /// the pane that held it leaves the shell and everything under it alive.
    #[cfg_attr(not(any(unix, windows)), allow(unused_variables))]
    pub fn detach_session(
        &self,
        session_id: SessionId,
        snapshot: ScreenSnapshot,
        metadata: SessionMetadata,
    ) -> Result<()> {
        #[cfg(any(unix, windows))]
        {
            let supervisor = self
                .supervisor
                .as_ref()
                .context("No terminal supervisor is running to detach this session to")?;
            supervisor.detach(session_id, snapshot, metadata)
        }
        #[cfg(not(any(unix, windows)))]
        {
            anyhow::bail!("Detaching a terminal session is not supported on this platform")
        }
    }

    /// Ends a session the supervisor is holding, killing its shell.
    ///
    /// Only meaningful for a detached session. A session a client is attached
    /// to is that client's to end, by closing the pane showing it.
    #[cfg_attr(not(any(unix, windows)), allow(unused_variables))]
    pub fn kill_detached_session(&self, session_id: SessionId) -> Result<()> {
        #[cfg(any(unix, windows))]
        {
            let supervisor = self
                .supervisor
                .as_ref()
                .context("No terminal supervisor is running to end this session")?;
            supervisor.kill(session_id)
        }
        #[cfg(not(any(unix, windows)))]
        {
            anyhow::bail!("Ending a supervised terminal session is not supported on this platform")
        }
    }

    /// The sessions the supervisor is holding with no client attached.
    pub fn list_detached_sessions(&self) -> Result<Vec<SessionSummary>> {
        #[cfg(any(unix, windows))]
        {
            let Some(supervisor) = self.supervisor.as_ref() else {
                return Ok(Vec::new());
            };
            supervisor.list_sessions()
        }
        #[cfg(not(any(unix, windows)))]
        {
            Ok(Vec::new())
        }
    }

    /// `pub`, not `pub(super)`: the app crate's `TerminalManager` calls this to
    /// hold on to the supervisor client, and that is a different crate now that
    /// this module lives in `warp_terminal`.
    #[cfg(unix)]
    pub fn persistent_server_client(
        &self,
    ) -> Option<std::sync::Arc<crate::local_tty::server::TerminalServerClient>> {
        let client = self.server.as_ref()?.client();
        client.is_persistent().then(|| client.clone())
    }

    /// Spawns a pty, returning information about the pty and a handle that can
    /// be used to interact with the pty process.
    pub(super) fn spawn_pty(
        &self,
        options: PtyOptions,
        persistent_session_id: SessionId,
        hooks: &dyn PtySpawnHooks,
        #[cfg(windows)] event_loop_tx: super::mio_channel::Sender<crate::writeable_pty::Message>,
        ctx: &mut AppContext,
    ) -> Result<(PtySpawnResult, Box<dyn PtyHandle>)> {
        #[cfg(not(unix))]
        let is_fallback = false;
        #[cfg(unix)]
        let mut is_fallback = false;

        // The v2 supervisor first where it is running: it owns the shell
        // outright, which is what lets the session outlive this process. The
        // terminal server below is the older path and only preserves sessions
        // for launch-config windows.
        #[cfg(unix)]
        if let Some(client) = &self.supervisor {
            let result = invoke_without_crash_reporting(hooks, || {
                client
                    .spawn(persistent_session_id, &options)
                    .map(|endpoint| PtySpawnResult {
                        pid: endpoint.pid,
                        leader_fd: endpoint.raw as std::os::fd::RawFd,
                        replay_fd: None,
                    })
            });
            match result {
                Ok(result) => {
                    hooks.spawned(PtySpawnMode::TerminalServer, ctx);
                    let pid = result.pid;
                    return Ok((
                        result,
                        Box::new(SupervisorOwnedPtyHandle {
                            client: client.clone(),
                            session_id: persistent_session_id,
                            pid,
                        }),
                    ));
                }
                Err(error) => {
                    log_env_var_diagnostics(&options.env_vars);
                    log::warn!(
                        "Failed to spawn a supervised terminal; falling back to the terminal                          server: {error:#}"
                    );
                }
            }
        }

        #[cfg(unix)]
        if let Some(server) = &self.server {
            let result = Self::spawn_pty_via_server(server, options.clone(), persistent_session_id);
            if let Err(err) = result {
                // Log env var names + sizes for any terminal server failure.
                // Large env vars are the most common cause (E2BIG on Linux,
                // socket overflow on macOS), so logging them on every failure
                // makes both cases diagnosable from the logs.
                log_env_var_diagnostics(&options.env_vars);

                // E2BIG is deterministic — retrying from the main process with
                // the same PtyOptions would hit the same limit — so fail
                // immediately rather than falling back.
                if is_e2big(&err) {
                    // err already contains the original message. We add some context on how to fix.
                    return Err(err.context(
                        "This can happen when env vars in the image or Oz secrets are \
                         too long.",
                    ));
                }
                report_error!(err.context(
                    "Failed to spawn pty via terminal server; falling back to spawning locally...",
                ));
                is_fallback = true;
            } else {
                hooks.spawned(PtySpawnMode::TerminalServer, ctx);
                return result;
            }
        }

        #[cfg(windows)]
        if let Some(client) = &self.supervisor {
            let result = invoke_without_crash_reporting(hooks, || {
                client
                    .spawn(persistent_session_id, &options)
                    .and_then(|endpoint| {
                        super::windows::PtySpawnResult::from_supervisor(
                            endpoint,
                            client.clone(),
                            persistent_session_id,
                        )
                    })
            });
            match result {
                Ok(result) => {
                    hooks.spawned(PtySpawnMode::TerminalServer, ctx);
                    let pid = result.pid;
                    return Ok((
                        result,
                        Box::new(SupervisorOwnedPtyHandle {
                            client: client.clone(),
                            session_id: persistent_session_id,
                            pid,
                        }),
                    ));
                }
                Err(error) => {
                    log::warn!(
                        "Failed to spawn a supervised Windows terminal; falling back to direct: {error:#}"
                    );
                }
            }
        }

        let mode = if is_fallback {
            PtySpawnMode::FallbackToDirect
        } else {
            PtySpawnMode::Direct
        };
        hooks.spawned(mode, ctx);

        Self::spawn_pty_directly(
            options,
            #[cfg(windows)]
            event_loop_tx,
            hooks,
        )
    }

    fn spawn_pty_directly(
        options: PtyOptions,
        #[cfg(windows)] event_loop_tx: super::mio_channel::Sender<crate::writeable_pty::Message>,
        hooks: &dyn PtySpawnHooks,
    ) -> Result<(PtySpawnResult, Box<dyn PtyHandle>)> {
        let pty_spawn_info = invoke_without_crash_reporting(hooks, move || {
            local_tty::spawn(
                options,
                #[cfg(windows)]
                event_loop_tx,
            )
        })?;
        let direct_pty_handle = Box::new(DirectPtyHandle {
            child: pty_spawn_info.child,
        });
        Ok((pty_spawn_info.result, direct_pty_handle))
    }

    #[cfg(unix)]
    fn spawn_pty_via_server(
        server: &TerminalServer,
        options: PtyOptions,
        persistent_session_id: SessionId,
    ) -> Result<(PtySpawnResult, Box<dyn PtyHandle>)> {
        use crate::local_tty::server::ServerOwnedPtyHandle;

        let client = server.client().clone();
        let supervised_session_id = client.is_persistent().then_some(persistent_session_id);
        let result = client.spawn_pty(options, supervised_session_id)?;
        let handle = Box::new(ServerOwnedPtyHandle {
            pid: result.pid,
            persistent_session_id: supervised_session_id,
            client,
        });
        Ok((result, handle))
    }

    /// Takes an existing supervisor-owned session back.
    ///
    /// `include_snapshot` decides whether the session's screen is replayed
    /// alongside the output buffered since it was rendered. A restored pane has
    /// already painted its own scrollback from sqlite and would show the screen
    /// twice; a pane opening fresh onto a detached session has nothing and
    /// needs it.
    /// Takes a session back from the supervisor, with what the client that let
    /// it go knew about it.
    ///
    /// The metadata travels with the endpoint because the pane adopting this
    /// shell has no other way to learn what the shell already announced -- see
    /// [`SessionMetadata::bootstrap`].
    pub(super) fn attach_pty(
        &self,
        session_id: SessionId,
        #[cfg_attr(unix, allow(unused_variables))] include_snapshot: bool,
        pane_size: (u16, u16),
    ) -> Result<Option<AttachedPty>> {
        #[cfg(unix)]
        if let Some(client) = self.supervisor.as_ref() {
            // Resize while the supervisor is still the sole reader so its
            // headless screen and the PTY can be repainted for this pane.
            client.resize(session_id, pane_size.0, pane_size.1)?;
            let attached = client.attach(session_id)?;
            let endpoint = attached
                .endpoint
                .context("Terminal supervisor did not lend a PTY endpoint")?;
            // A wrapped ring means the buffered output no longer reaches back
            // to the snapshot, so the screen has to be repainted whatever the
            // caller asked for -- otherwise the gap reads as continuous output.
            let mut replay = if snapshot_fits_pane(&attached.snapshot, pane_size)
                && (include_snapshot || attached.output_was_dropped)
            {
                attached.snapshot.repaint
            } else {
                Vec::new()
            };
            replay.extend(attached.buffered_output);

            let result = PtySpawnResult {
                pid: endpoint.pid,
                leader_fd: endpoint.raw as std::os::fd::RawFd,
                replay_fd: replay_descriptor(&replay)?,
            };
            let handle = Box::new(SupervisorOwnedPtyHandle {
                client: client.clone(),
                session_id,
                pid: endpoint.pid,
            });
            return Ok(Some((result, handle, attached.metadata)));
        }

        #[cfg(unix)]
        {
            use crate::local_tty::server::ServerOwnedPtyHandle;

            let Some(server) = self.server.as_ref() else {
                return Ok(None);
            };
            let client = server.client().clone();
            let Some(result) = client.attach_session(session_id)? else {
                return Ok(None);
            };
            let handle = Box::new(ServerOwnedPtyHandle {
                pid: result.pid,
                persistent_session_id: Some(session_id),
                client,
            });
            // The v1 server keeps a raw output spool and nothing else, so there
            // is no announcement to hand back. A pane attaching through it is
            // left where it has always been left.
            Ok(Some((result, handle, SessionMetadata::default())))
        }

        #[cfg(windows)]
        {
            let Some(client) = self.supervisor.as_ref() else {
                return Ok(None);
            };
            // Windows relays are fresh per attachment. Resize before borrowing
            // one so replay is rendered at this pane's dimensions instead of
            // being discarded as an unsafe old-size snapshot.
            client.resize(session_id, pane_size.0, pane_size.1)?;
            let attached = client.attach(session_id)?;
            let endpoint = attached
                .endpoint
                .context("Windows terminal supervisor did not lend a PTY endpoint")?;
            // A wrapped ring means the buffered output no longer reaches back
            // to the snapshot, so the screen has to be repainted whatever the
            // caller asked for -- otherwise the gap reads as continuous output.
            let mut replay = if snapshot_fits_pane(&attached.snapshot, pane_size)
                && (include_snapshot || attached.output_was_dropped)
            {
                attached.snapshot.repaint
            } else {
                Vec::new()
            };
            replay.extend(attached.buffered_output);
            let result = super::windows::PtySpawnResult::from_attached(
                endpoint,
                client.clone(),
                session_id,
                replay,
            )?;
            let handle = Box::new(SupervisorOwnedPtyHandle {
                client: client.clone(),
                session_id,
                pid: endpoint.pid,
            });
            Ok(Some((result, handle, attached.metadata)))
        }
    }
}

/// Whether a snapshot describes a screen the size of the pane about to show it.
///
/// A snapshot is a repaint sequence, and a repaint positions the cursor with
/// absolute coordinates taken from the screen it was rendered from. Replaying
/// one into a smaller pane addresses rows that pane does not have, which the
/// grid asserts against in a debug build and silently mis-indexes in a release
/// one -- the `assertion failed: requested < self.len` and `index out of bounds`
/// panics that a detached split, reattached as a full-width tab, produced every
/// time.
///
/// Skipping the repaint costs the screen as it was; the session's buffered
/// output still arrives, and the pane is live and usable. Restoring a screen
/// across a size change needs the supervisor to re-render at the new size,
/// which is a change to the attach round trip rather than a guard here.
fn snapshot_fits_pane(snapshot: &ScreenSnapshot, pane_size: (u16, u16)) -> bool {
    let fits = (snapshot.columns, snapshot.rows) == pane_size;
    if !fits {
        log::info!(
            "Not repainting an attached session's screen: it was {}x{} and this pane is {}x{}",
            snapshot.columns,
            snapshot.rows,
            pane_size.0,
            pane_size.1
        );
    }
    fits
}

impl Entity for PtySpawner {
    type Event = ();
}

impl SingletonEntity for PtySpawner {}

/// Returns `true` if the error (or any cause in its chain) indicates that
/// `execve` failed with `ENAMETOOLONG` / E2BIG ("Argument list too long").
///
/// The terminal-server IPC path stringifies the raw `io::Error`, so we cannot
/// reliably downcast; instead we check the formatted error message as a
/// fallback.
#[cfg(unix)]
fn is_e2big(err: &anyhow::Error) -> bool {
    // Try the "real" io::Error in the chain first (direct-spawn path).
    if err.chain().any(|e| {
        e.downcast_ref::<std::io::Error>()
            .and_then(|e| e.raw_os_error())
            .is_some_and(|code| code == libc::E2BIG)
    }) {
        return true;
    }
    // Fall back to string matching for the IPC path where the io::Error
    // was serialised to a String before being sent back to the client.
    let msg = format!("{err:#}");
    msg.contains("os error 7") || msg.contains("Argument list too long")
}

/// Logs the names and byte-lengths (not values) of env vars passed to the
/// shell. Called on any terminal server failure to aid diagnosis of oversized
/// env var / secret configurations (E2BIG on Linux, socket overflow on macOS).
#[cfg(unix)]
fn log_env_var_diagnostics(extra_env_vars: &HashMap<OsString, OsString>) {
    log::error!("Shell spawn env var diagnostics (names and sizes only, no values):");

    // Log the additional env vars supplied via PtyOptions.
    let mut extra: Vec<(&OsString, usize)> = extra_env_vars
        .iter()
        .map(|(k, v)| (k, k.len() + v.len() + 2))
        .collect();
    extra.sort_by_key(|(_, size)| Reverse(*size));
    log::error!("PtyOptions env_vars entries={}", extra_env_vars.len());
    for (key, size) in extra.iter().take(20) {
        log::error!("PtyOptions env var entry key={key:?} bytes={size}");
    }

    // Log the largest vars from the inherited process environment.
    let mut inherited: Vec<(OsString, usize)> = std::env::vars_os()
        .map(|(k, v)| {
            let size = k.len() + v.len() + 2;
            (k, size)
        })
        .collect();
    inherited.sort_by_key(|(_, size)| Reverse(*size));
    let total: usize = inherited.iter().map(|(_, s)| s).sum();
    log::error!(
        "Inherited process env summary vars={} bytes={total}",
        inherited.len()
    );
    for (key, size) in inherited.iter().take(20) {
        log::error!("Inherited process env entry key={key:?} bytes={size}");
    }
}
