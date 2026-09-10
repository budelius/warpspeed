//! The listener a running Warp window keeps open for the `sessions` command.
//!
//! ## Why requests take a round trip to the main thread
//!
//! Every request here is answered out of the app's own state: which panes
//! exist, what the supervisor owns, whether a detach succeeded. All of that
//! lives behind an `AppContext`, which is not `Send` and belongs to the main
//! thread. The IPC server, meanwhile, handles connections on the background
//! executor.
//!
//! So the service handler does not answer. It puts the request on a channel
//! with a one-shot sender for the reply and waits. A stream spawned on the main
//! thread takes each one, answers it with a real `AppContext`, and sends the
//! answer back. The background side stays a courier, which is the only shape
//! that lets a request both touch app state and produce a response.

use async_trait::async_trait;
use futures::channel::oneshot;
use ipc::{Server, ServerBuilder, Service, ServiceImpl};
use warp_errors::report_error;
use warpui::{Entity, ModelContext, SingletonEntity};

use super::address::{self, InstanceMarker};
use super::handler;
use super::protocol::{Request, Response};

/// The service the `sessions` command calls.
pub struct RemoteControlService;

impl Service for RemoteControlService {
    type Request = Request;
    type Response = Response;
}

/// One request, with somewhere to put the answer.
type Envelope = (Request, oneshot::Sender<Response>);

#[derive(Clone)]
struct RemoteControlServiceImpl {
    requests: async_channel::Sender<Envelope>,
}

#[async_trait]
impl ServiceImpl for RemoteControlServiceImpl {
    type Service = RemoteControlService;

    async fn handle_request(&self, request: Request) -> Response {
        let (reply, answer) = oneshot::channel();
        if self.requests.send((request, reply)).await.is_err() {
            return Response::Failed {
                error: "This Warp instance is shutting down".to_owned(),
            };
        }
        answer.await.unwrap_or_else(|_| Response::Failed {
            error: "This Warp instance did not answer the request".to_owned(),
        })
    }
}

/// Keeps this process reachable by the `sessions` command for as long as it runs.
pub struct RemoteControlServer {
    /// Dropping the server stops accepting connections.
    _server: Option<Server>,

    /// What was written to disk, so the same thing can be removed.
    marker: Option<InstanceMarker>,
}

impl RemoteControlServer {
    /// Binds the channel and advertises it, or leaves this instance
    /// unreachable.
    ///
    /// A failure here is reported and swallowed. Remote control is a way to
    /// drive Warp from outside it; an instance that could not open the channel
    /// still has to run.
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        match Self::start(ctx) {
            Ok(server) => server,
            Err(error) => {
                report_error!(error.context("Failed to open the Warp remote-control channel"));
                Self {
                    _server: None,
                    marker: None,
                }
            }
        }
    }

    fn start(ctx: &mut ModelContext<Self>) -> anyhow::Result<Self> {
        let pid = std::process::id();
        let address = address::listen_address(pid)?;

        // A predecessor with this pid is long gone -- the OS would not have
        // reissued the number otherwise -- so whatever it left behind is stale.
        // On Unix `bind` fails outright on an existing socket file, which would
        // make a crashed instance's leftovers lock this pid out for good.
        #[cfg(unix)]
        let _ = std::fs::remove_file(&address);

        let (requests, incoming) = async_channel::unbounded::<Envelope>();
        let (server, _) = ServerBuilder::default()
            .with_fixed_address(address.clone())
            .with_service(RemoteControlServiceImpl { requests })
            .build_and_run(ctx.background_executor())?;

        ctx.spawn_stream_local(
            incoming,
            |_server, (request, reply), ctx| {
                let response = handler::handle(request, ctx);
                // The caller giving up between asking and being answered is
                // ordinary: the `sessions` command has its own timeout.
                let _ = reply.send(response);
            },
            |_, _| {},
        );

        let marker = InstanceMarker {
            pid,
            address,
            version: super::build_identity(),
        };
        address::write_marker(&marker)?;

        log::info!("Remote control listening for the `sessions` command as pid {pid}");
        Ok(Self {
            _server: Some(server),
            marker: Some(marker),
        })
    }
}

impl Drop for RemoteControlServer {
    fn drop(&mut self) {
        if let Some(marker) = self.marker.take() {
            address::remove_marker(&marker);
        }
    }
}

impl Entity for RemoteControlServer {
    type Event = ();
}

impl SingletonEntity for RemoteControlServer {}
