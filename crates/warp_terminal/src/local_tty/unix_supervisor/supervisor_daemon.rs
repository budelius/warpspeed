//! The Unix supervisor process: accept a request, answer it, keep the detached
//! terminals fed.
//!
//! Single-threaded on purpose. The listener is non-blocking and each accepted
//! connection carries exactly one request, so the loop always comes back round
//! to [`Supervisor::pump`] -- which is what stops a detached shell from
//! blocking on a terminal nobody is reading. Every read and write on an
//! accepted connection is bounded, so one silent peer cannot park the daemon.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use instant::Instant;

use super::socket::{SocketGuard, bind_listener, peer_pid, prepare_socket_directory, socket_path};
use super::supervisor_host::UnixSessionHost;
use super::transport::UnixSocketTransport;
use crate::event_listener::ChannelEventListener;
use crate::local_tty::supervisor::serve::{DEFAULT_SESSION_TTL_SECONDS, Supervisor};
use crate::local_tty::supervisor::transport::Transport;
use crate::local_tty::supervisor::{SupervisorExit, SupervisorExitReason};

/// How long one request/response exchange may take before the peer is abandoned.
const IO_TIMEOUT: Duration = Duration::from_secs(15);

/// How often the loop comes back round when no client is talking to it.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How long the supervisor lingers with nothing to supervise before exiting.
///
/// Without this the daemon is immortal: it is `setsid()`'d and nothing reaps
/// it, so an app crash would leave it running past logout.
const IDLE_SHUTDOWN_DELAY: Duration = Duration::from_secs(60);

/// Runs the Unix terminal supervisor until it has been idle with no sessions.
pub fn run() -> Result<()> {
    run_with_exit(|| {}).map(|_| ())
}

pub fn run_with_exit(on_listening: impl FnOnce()) -> Result<SupervisorExit> {
    let path = socket_path();
    let directory = path
        .parent()
        .ok_or_else(|| anyhow!("Terminal supervisor socket has no parent directory"))?;
    prepare_socket_directory(directory)?;

    let listener = bind_listener(&path)?;
    listener
        .set_nonblocking(true)
        .context("Failed to make the terminal supervisor listener non-blocking")?;
    let _socket_guard = SocketGuard(path);
    on_listening();

    let events = ChannelEventListener::new_headless();
    let mut supervisor = Supervisor::new(UnixSessionHost::default(), true);
    let mut idle_since = Some(Instant::now());

    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(error) = serve_one_request(&mut supervisor, stream, &events) {
                    log::error!("Terminal supervisor request failed: {error:#}");
                }
                idle_since = None;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error).context("Terminal supervisor listener failed"),
        }

        let now = unix_seconds();
        supervisor.recover_orphaned_sessions(now, &events);
        supervisor.pump(now, DEFAULT_SESSION_TTL_SECONDS);

        if supervisor.has_host_sessions() {
            idle_since = None;
        } else if idle_since.get_or_insert_with(Instant::now).elapsed() >= IDLE_SHUTDOWN_DELAY {
            log::info!("Terminal supervisor exiting: nothing left to supervise");
            return Ok(SupervisorExit {
                reason: SupervisorExitReason::Idle,
                had_live_sessions: false,
            });
        }
    }
}

/// Reads one request off `stream`, answers it, and lets the connection go.
fn serve_one_request(
    supervisor: &mut Supervisor<UnixSessionHost>,
    stream: std::os::unix::net::UnixStream,
    events: &ChannelEventListener,
) -> Result<()> {
    // The listener is non-blocking and on some platforms the accepted socket
    // inherits that, while the exchange below is written against a blocking
    // one. The timeouts are what stop a silent peer from parking the daemon.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;

    let client_pid = peer_pid(&stream)?;
    let mut transport = UnixSocketTransport::new(stream);
    let request = transport.receive_timeout(IO_TIMEOUT)?;

    let Some(response) = supervisor.handle_message(&request, client_pid, unix_seconds(), events)
    else {
        return Ok(());
    };
    let lends_descriptor = matches!(
        response,
        crate::local_tty::supervisor::message::Message::SpawnResponse { .. }
            | crate::local_tty::supervisor::message::Message::AttachResponse { .. }
    );
    transport.send(&response)?;

    if lends_descriptor {
        // Closing a Unix socket with a lent descriptor still in flight can discard the control
        // message, and the client would attach to nothing.
        let _ = transport.receive_timeout(IO_TIMEOUT);
    }
    Ok(())
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
