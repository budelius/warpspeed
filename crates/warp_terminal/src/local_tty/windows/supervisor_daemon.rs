use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use instant::Instant;

use super::pipe_transport::{PipeListener, supervisor_pipe_name};
use super::supervisor_host::WindowsSessionHost;
use crate::event_listener::ChannelEventListener;
use crate::local_tty::supervisor::message::Message;
use crate::local_tty::supervisor::serve::{DEFAULT_SESSION_TTL_SECONDS, Supervisor};
use crate::local_tty::supervisor::transport::Transport;

const CONTROL_TIMEOUT: Duration = Duration::from_secs(15);
const PUMP_INTERVAL: Duration = Duration::from_millis(20);
const IDLE_SHUTDOWN_DELAY: Duration = Duration::from_secs(60);

struct Request {
    message: Message,
    client_pid: u32,
    response: mpsc::SyncSender<Option<Message>>,
}

/// Runs the Windows terminal supervisor until it has been idle with no live sessions.
///
/// `on_listening` runs once this process owns the supervisor pipe. A supervisor that loses the
/// race for the pipe exits before it, so it never touches state the winner owns, such as its log.
pub fn run(on_listening: impl FnOnce()) -> Result<()> {
    run_at_pipe(supervisor_pipe_name(), on_listening)
}

pub(super) fn run_at_pipe(pipe_name: String, on_listening: impl FnOnce()) -> Result<()> {
    let (request_tx, request_rx) = mpsc::channel::<Request>();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);

    std::thread::Builder::new()
        .name("terminal-supervisor-control".to_owned())
        .spawn(move || {
            let mut listener = match PipeListener::bind(pipe_name) {
                Ok(listener) => {
                    let _ = ready_tx.send(Ok(()));
                    listener
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return;
                }
            };

            loop {
                let mut transport = match listener.accept() {
                    Ok(transport) => transport,
                    Err(_) => return,
                };
                let client_pid = match transport.client_process_id() {
                    Ok(client_pid) => client_pid,
                    Err(_) => continue,
                };
                let message = match transport.receive_timeout(CONTROL_TIMEOUT) {
                    Ok(message) => message,
                    Err(_) => continue,
                };
                let (response_tx, response_rx) = mpsc::sync_channel(1);
                if request_tx
                    .send(Request {
                        message,
                        client_pid,
                        response: response_tx,
                    })
                    .is_err()
                {
                    return;
                }
                if let Ok(Some(response)) = response_rx.recv_timeout(CONTROL_TIMEOUT)
                    && transport.send(&response).is_ok()
                {
                    let _ = transport.receive_timeout(CONTROL_TIMEOUT);
                }
            }
        })
        .context("Failed to start the terminal supervisor control thread")?;

    match ready_rx.recv_timeout(CONTROL_TIMEOUT) {
        Ok(Ok(())) => on_listening(),
        Ok(Err(error)) => bail!(error),
        Err(error) => return Err(error).context("Terminal supervisor control pipe did not start"),
    }

    let listener = ChannelEventListener::new_headless();
    let mut supervisor = Supervisor::new(WindowsSessionHost::default(), true);
    let mut idle_since = Some(Instant::now());

    loop {
        match request_rx.recv_timeout(PUMP_INTERVAL) {
            Ok(request) => {
                let response = supervisor.handle_message(
                    &request.message,
                    request.client_pid,
                    unix_seconds(),
                    &listener,
                );
                let _ = request.response.send(response);
                idle_since = None;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }

        let now = unix_seconds();
        supervisor.recover_orphaned_sessions(now, &listener);
        supervisor.pump(now, DEFAULT_SESSION_TTL_SECONDS);
        if supervisor.has_host_sessions() {
            idle_since = None;
        } else if idle_since.get_or_insert_with(Instant::now).elapsed() >= IDLE_SHUTDOWN_DELAY {
            return Ok(());
        }
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
