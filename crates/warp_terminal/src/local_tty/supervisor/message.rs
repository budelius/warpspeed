//! The supervisor wire protocol, version 7.
//!
//! Data only: nothing here performs I/O, so the same messages travel over a
//! Unix socket, a Windows named pipe, or a network tunnel.

use serde::{Deserialize, Serialize};
use warp_core::SessionId;

use super::transport::Endpoint;
use crate::local_tty::screen::ScreenSnapshot;
use crate::shell::ShellLaunchData;

/// The protocol version, carried in the socket and pipe name so clients can select the matching
/// wire encoding. Older versions require an explicit adapter in [`super::compatibility`].
///
/// ## Bump this whenever anything below changes shape
///
/// A supervisor outlives the app by design, so at any moment the daemon
/// answering may have been started weeks ago by a build that is no longer
/// installed. The name is the only thing that keeps those two apart, and it
/// separates them only if this number moves.
///
/// v2 to v3 is the lesson. `SessionSummary` gained an `attached` flag without a
/// bump, so a new client and a still-running old supervisor shared one pipe
/// name and disagreed about the bytes on it. Every `ListSessions` came back as
/// `invalid u8 while decoding bool`, every attach found a session the other
/// supervisor was holding, and panes reported their shell as having exited
/// prematurely -- none of which named the protocol as the cause.
///
/// [`super::framing_tests`] pins the encoding so that changing it without
/// coming here fails a test rather than a user's terminal.
// v5 makes pre-attach Resize reflow the supervisor's headless screen before
// lending an endpoint. The wire shape is unchanged, but old v4 supervisors
// discard replay when the receiving pane has different dimensions.
// v6 carries whether the detached shell was waiting at its prompt, allowing
// the adopting pane to reactivate its editor without sourcing bootstrap again.
// v7 identifies the detached shell process and pane UUID so clients can name its agent exactly.
pub const PROTOCOL_VERSION: u32 = 7;

/// A message in either direction between a client and the supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    /// Starts a supervisor-owned terminal and lends its endpoint to the requesting client.
    Spawn {
        session_id: SessionId,
        /// A bincode-encoded `PtyOptions`. Kept opaque here so the protocol remains comparable in
        /// tests even though shell starters contain platform path types without `PartialEq`.
        options: Vec<u8>,
    },

    /// The result of a [`Message::Spawn`].
    SpawnResponse { result: Result<Endpoint, String> },

    /// The client is releasing a session. The screen travels with it, because
    /// the supervisor is blind while a client is attached and has no other way
    /// to know what the session was displaying.
    Detach {
        session_id: SessionId,
        snapshot: ScreenSnapshot,
        metadata: SessionMetadata,
    },

    /// The result of a [`Message::Detach`].
    DetachResponse { error: Option<String> },

    /// The client wants a session back, resuming output after `after_sequence`.
    /// `None` asks for whatever the supervisor still holds.
    Attach {
        session_id: SessionId,
        after_sequence: Option<u64>,
    },

    /// The result of a [`Message::Attach`].
    AttachResponse {
        result: Result<AttachedSession, String>,
    },

    /// Changes the size of a supervisor-owned terminal.
    Resize {
        session_id: SessionId,
        columns: u16,
        rows: u16,
    },

    /// The result of a [`Message::Resize`].
    ResizeResponse { error: Option<String> },

    /// Ends a supervisor-owned terminal.
    Kill { session_id: SessionId },

    /// The result of a [`Message::Kill`].
    KillResponse { error: Option<String> },

    /// Checks whether a supervisor-owned terminal is still alive.
    ChildStatus { session_id: SessionId },

    /// The result of a [`Message::ChildStatus`].
    ChildStatusResponse { running: bool },

    /// Asks for every session the supervisor is holding.
    ListSessions,

    /// The result of a [`Message::ListSessions`].
    ListSessionsResponse { sessions: Vec<SessionSummary> },

    /// Liveness check, also used to learn a supervisor's protocol version.
    Ping,

    /// The reply to [`Message::Ping`].
    Pong { protocol_version: u32 },

    /// Confirms that a response was read before the server closes its named-pipe instance.
    ResponseReceived,
}

/// What a client receives when it takes a session back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedSession {
    /// The session's screen as of `sequence`.
    pub snapshot: ScreenSnapshot,

    /// Output produced after that screen was rendered.
    pub buffered_output: Vec<u8>,

    /// The sequence number `buffered_output` ends at.
    pub sequence: u64,

    /// `true` when output was discarded because the ring wrapped, so the pane
    /// can say so rather than presenting a silent gap as continuous output.
    pub output_was_dropped: bool,

    /// The borrowed PTY endpoint, when the transport could carry one.
    ///
    /// `None` means output arrives as streamed messages instead -- the case for
    /// every transport that leaves the machine.
    pub endpoint: Option<Endpoint>,

    /// What the client that detached knew about the session.
    ///
    /// Travels back on the attach because the pane taking the session over has
    /// no other source for it: it is adopting a shell it never started, and
    /// [`SessionMetadata::bootstrap`] is what lets it do so without the shell
    /// having to introduce itself a second time.
    pub metadata: SessionMetadata,
}

/// What a client knows about a session that the supervisor cannot work out for
/// itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// The shell the session is running.
    pub shell: String,

    /// Where the session was last known to be.
    pub working_directory: String,

    /// The session's title, for telling two shells apart at a glance.
    pub title: String,

    /// Whether the shell was waiting at its prompt when handed over.
    ///
    /// Restoring this lifecycle marker reactivates the editor without sending
    /// the one-time bootstrap script to an already-running shell.
    pub input_editor_active: bool,

    /// What the shell said about itself when it started.
    ///
    /// `None` for a session detached before it finished bootstrapping, and for
    /// one handed over by a client that never saw the handshake.
    pub bootstrap: Option<SessionBootstrap>,
}

/// The shell's own account of itself, as it gave it once and will not give
/// again.
///
/// A warpified shell announces who and what it is exactly once, on the prompt
/// after startup: an `InitShell` hook naming the shell, user and host, then a
/// `Bootstrapped` hook carrying everything the client needs for completions.
/// A pane that misses those two never leaves
/// `TerminalInputState::NotBootstrapped` -- it renders no blocks and refuses
/// every keystroke.
///
/// Which is precisely the position an attaching pane is in. The shell it takes
/// over said all of this to a Warp process that has since exited, and it has no
/// reason to say it twice. So the pane that hands the session over records the
/// announcement here, and the pane that picks it up replays it.
///
/// A plain mirror of the two hook payloads rather than the payloads themselves,
/// because those deserialize through hand-written impls that accept the shell's
/// JSON shape -- tolerating empty strings for absent values, and numbers
/// written as strings. Sending them through bincode would ask those impls to
/// read a wire format they were never written for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBootstrap {
    /// The id the shell stamps on every hook it sends.
    ///
    /// Not the supervisor's [`SessionId`]: this one is baked into the shell's
    /// prompt function at startup, and the shell keeps using it. A pane that
    /// does not register it rejects every later hook from the shell as coming
    /// from an unknown session.
    pub hook_session_id: u64,

    /// The shell's name, as `ShellType::from_name` understands it.
    pub shell: String,

    /// Who the shell is running as.
    pub user: String,

    /// The host it is running on, which is how a local session is told from a
    /// remote one.
    pub hostname: String,

    /// The WSL distribution, when the shell is inside one.
    pub wsl_name: Option<String>,

    /// Whether the announcement came from a subshell.
    pub is_subshell: bool,

    pub histfile: Option<String>,
    pub home_dir: Option<String>,
    pub path: Option<String>,
    /// `CDPATH`, colon-separated.
    pub cdpath: Option<String>,
    pub editor: Option<String>,
    pub aliases: Option<String>,
    pub abbreviations: Option<String>,
    pub function_names: Option<String>,
    pub env_var_names: Option<String>,
    pub builtins: Option<String>,
    pub keywords: Option<String>,
    pub shell_version: Option<String>,

    /// Whether the shell's own vi mode is on.
    pub vi_mode_enabled: Option<String>,

    /// The operating system category, e.g. `MacOS`, `Linux`, `Windows`.
    pub os_category: Option<String>,

    pub linux_distribution: Option<String>,

    /// The full path to the running shell binary.
    pub shell_path: Option<String>,

    /// The shell options in effect. A sorted list rather than the set it comes
    /// from, so that two encodings of the same bootstrap are the same bytes.
    pub shell_options: Option<Vec<String>>,

    /// Tags for known shell configurations and plugins, sorted for the same
    /// reason as `shell_options`.
    pub shell_plugins: Option<Vec<String>>,
}

/// A session as shown in the detached-sessions drawer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Identifies the session for a later attach.
    pub session_id: SessionId,

    /// What the client told us about it when it detached.
    pub metadata: SessionMetadata,

    /// How long the session has been detached. Computed by the supervisor, not
    /// supplied by the client.
    pub detached_for_seconds: u64,

    /// `true` while another client holds this session. Such a session is listed
    /// but cannot be attached, because a second client would read the same PTY.
    pub attached: bool,

    /// The pid of the shell the supervisor spawned for this session.
    pub shell_pid: u32,

    /// `WARP_TERMINAL_SESSION_UUID` as hex, or empty when the spawn did not provide one.
    pub session_uuid_hex: String,
}

impl SessionMetadata {
    /// Whether a pane adopting this session should treat its shell as waiting at the prompt.
    ///
    /// A shell running a program is never at its prompt, whatever the handover says. Without a
    /// recorded bootstrap there was no real handover -- the client died, or never attached the
    /// session -- so `input_editor_active` is only a default, and an idle shell is at its prompt.
    pub fn adopted_shell_is_at_prompt(&self, has_running_child: bool) -> bool {
        if has_running_child {
            return false;
        }
        self.bootstrap.is_none() || self.input_editor_active
    }
}

impl SessionBootstrap {
    /// The announcement of a local shell supervised under `session_id`, rebuilt for a session
    /// whose client vanished without handing it over.
    ///
    /// Only what a pane needs to accept the shell can be recovered. A supervised shell stamps its
    /// hooks with the id it was spawned under, which is its supervisor session id, and a local
    /// shell reports this machine's hostname, which is what makes the pane treat it as local.
    /// Everything else the shell announced -- aliases, history file, `PATH` -- is unknown and left
    /// empty. `None` for shells whose identity cannot be inferred this way, such as those inside
    /// WSL or a container.
    pub fn for_orphaned_local_shell(
        session_id: SessionId,
        launch: &ShellLaunchData,
        user: String,
        hostname: String,
    ) -> Option<Self> {
        let shell_type = match launch {
            ShellLaunchData::Executable { shell_type, .. }
            | ShellLaunchData::MSYS2 { shell_type, .. } => *shell_type,
            ShellLaunchData::WSL { .. } | ShellLaunchData::DockerSandbox { .. } => return None,
        };
        Some(Self {
            hook_session_id: session_id.as_u64(),
            shell: shell_type.name().to_owned(),
            user,
            hostname,
            ..Self::default()
        })
    }
}

#[cfg(test)]
#[path = "message_tests.rs"]
mod tests;
