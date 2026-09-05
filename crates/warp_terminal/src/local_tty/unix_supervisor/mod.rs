//! The Unix backend for the v2 terminal session supervisor.
//!
//! The counterpart of `local_tty::windows`'s supervisor modules, against the
//! same cross-platform `supervisor::{serve, session_table, message}` core. Only
//! two things differ from Windows and both live here: the transport is an
//! `AF_UNIX` socket rather than a named pipe, and an endpoint is lent as a file
//! descriptor over `SCM_RIGHTS` rather than duplicated into the client process.
//!
//! This supersedes `local_tty::server`'s v1 supervisor, which keeps sessions
//! alive across app exit but spools their output as raw bytes -- correct for
//! line-oriented output, visibly wrong for anything on the alternate screen --
//! and cannot enumerate what it is holding.

pub mod client_process;
pub mod daemon_spawn;
pub mod socket;
pub mod supervised_pty;
pub mod supervisor_client;
pub mod supervisor_daemon;
pub mod supervisor_host;
pub mod transport;

pub use supervisor_client::UnixTerminalSupervisorClient;
pub use supervisor_host::UnixSessionHost;
