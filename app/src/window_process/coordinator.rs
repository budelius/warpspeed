use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use instant::Instant;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warpui_core::r#async::executor::Background;

use super::protocol::{
    AuthenticatedPeer, BoundedMessage, CoordinatorMessage, WorkerBootstrap, WorkerMessage,
};

const EVENT_QUEUE_CAPACITY: usize = 256;

#[derive(Clone, Serialize, Deserialize)]
struct TransportRequest {
    worker_id: Uuid,
    frame: Vec<u8>,
}

impl fmt::Debug for TransportRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransportRequest")
            .field("worker_id", &self.worker_id)
            .field("frame_byte_len", &self.frame.len())
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
enum TransportResponse {
    Frame(Vec<u8>),
    Rejected { code: String },
}

impl fmt::Debug for TransportResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frame(frame) => formatter
                .debug_struct("Frame")
                .field("byte_len", &frame.len())
                .finish(),
            Self::Rejected { code } => formatter
                .debug_struct("Rejected")
                .field("code", code)
                .finish(),
        }
    }
}

struct CoordinatorTransport;

#[async_trait]
impl ipc::Service for CoordinatorTransport {
    type Request = TransportRequest;
    type Response = TransportResponse;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerStatus {
    Registered,
    Connected,
    Ready,
    Presented,
    Closed,
}

pub(crate) struct WorkerEvent {
    pub(crate) worker_id: Uuid,
    pub(crate) message: WorkerMessage,
}

struct RegisteredWorker {
    authenticator: AuthenticatedPeer,
    challenge: [u8; 16],
    status: WorkerStatus,
    commands: VecDeque<CoordinatorMessage>,
    last_seen: Instant,
}

struct CoordinatorState {
    workers: HashMap<Uuid, RegisteredWorker>,
    events: async_channel::Sender<WorkerEvent>,
}

#[derive(Clone)]
struct CoordinatorTransportImpl {
    state: Arc<Mutex<CoordinatorState>>,
}

#[async_trait]
impl ipc::ServiceImpl for CoordinatorTransportImpl {
    type Service = CoordinatorTransport;

    async fn handle_request(&self, request: TransportRequest) -> TransportResponse {
        self.handle_authenticated_request(request)
            .unwrap_or_else(|code| TransportResponse::Rejected { code })
    }
}

impl CoordinatorTransportImpl {
    fn handle_authenticated_request(
        &self,
        request: TransportRequest,
    ) -> std::result::Result<TransportResponse, String> {
        let mut state = self.state.lock();
        let events = state.events.clone();
        let worker = state
            .workers
            .get_mut(&request.worker_id)
            .ok_or_else(|| "unknown_worker".to_owned())?;
        let (request_id, message) = worker
            .authenticator
            .decode::<WorkerMessage>(&request.frame)
            .map_err(|_| "authentication_failed".to_owned())?;
        let response = advance_worker(worker, &message, request_id)?;
        let response = worker
            .authenticator
            .encode(response)
            .map_err(|_| "response_failed".to_owned())?;
        events
            .try_send(WorkerEvent {
                worker_id: request.worker_id,
                message,
            })
            .map_err(|_| "coordinator_busy".to_owned())?;
        Ok(TransportResponse::Frame(response))
    }
}

fn advance_worker(
    worker: &mut RegisteredWorker,
    message: &WorkerMessage,
    request_id: Uuid,
) -> std::result::Result<CoordinatorMessage, String> {
    let response = match (worker.status, message) {
        (WorkerStatus::Registered, WorkerMessage::Hello { challenge })
            if challenge == &worker.challenge =>
        {
            worker.status = WorkerStatus::Connected;
            CoordinatorMessage::Welcome
        }
        (WorkerStatus::Connected, WorkerMessage::NextCommand) => worker
            .commands
            .pop_front()
            .unwrap_or(CoordinatorMessage::Noop),
        (WorkerStatus::Connected, WorkerMessage::Ready) if worker.commands.is_empty() => {
            worker.status = WorkerStatus::Ready;
            CoordinatorMessage::Ack { request_id }
        }
        (WorkerStatus::Ready, WorkerMessage::FirstFramePresented) => {
            worker.status = WorkerStatus::Presented;
            CoordinatorMessage::Ack { request_id }
        }
        (
            WorkerStatus::Connected | WorkerStatus::Ready | WorkerStatus::Presented,
            WorkerMessage::Heartbeat { .. },
        ) => worker
            .commands
            .pop_front()
            .unwrap_or(CoordinatorMessage::Ack { request_id }),
        (
            WorkerStatus::Ready | WorkerStatus::Presented,
            WorkerMessage::AppStateChunk(_)
            | WorkerMessage::PersistenceRecordChunk(_)
            | WorkerMessage::OpenWindow,
        ) => CoordinatorMessage::Ack { request_id },
        (
            WorkerStatus::Connected | WorkerStatus::Ready | WorkerStatus::Presented,
            WorkerMessage::WindowClosed,
        ) => {
            worker.status = WorkerStatus::Closed;
            CoordinatorMessage::Ack { request_id }
        }
        (status, _) => return Err(format!("invalid_worker_state_{status:?}")),
    };
    worker.last_seen = Instant::now();
    Ok(response)
}

pub(crate) struct CoordinatorServer {
    _server: ipc::Server,
    address: ipc::ConnectionAddress,
    pool_id: Uuid,
    state: Arc<Mutex<CoordinatorState>>,
    events: async_channel::Receiver<WorkerEvent>,
}

impl CoordinatorServer {
    pub(crate) fn start(background_executor: Arc<Background>) -> Result<Self> {
        let (events, event_rx) = async_channel::bounded(EVENT_QUEUE_CAPACITY);
        let state = Arc::new(Mutex::new(CoordinatorState {
            workers: HashMap::new(),
            events,
        }));
        let transport = CoordinatorTransportImpl {
            state: state.clone(),
        };
        let (server, address) = ipc::ServerBuilder::default()
            .with_service(transport)
            .build_and_run(background_executor)
            .map_err(|error| anyhow::anyhow!("{error:?}"))
            .context("failed to start window-process coordinator")?;
        restrict_endpoint(&address)?;
        Ok(Self {
            _server: server,
            address,
            pool_id: Uuid::new_v4(),
            state,
            events: event_rx,
        })
    }

    #[cfg(test)]
    pub(crate) fn register_worker(
        &self,
        build_identity: String,
        profile_identity: String,
        expires_at_unix_seconds: u64,
    ) -> WorkerBootstrap {
        self.register_worker_with_commands(
            build_identity,
            profile_identity,
            expires_at_unix_seconds,
            VecDeque::new(),
        )
    }

    pub(crate) fn register_worker_with_commands(
        &self,
        build_identity: String,
        profile_identity: String,
        expires_at_unix_seconds: u64,
        commands: VecDeque<CoordinatorMessage>,
    ) -> WorkerBootstrap {
        let bootstrap = WorkerBootstrap::for_pool(
            self.pool_id,
            build_identity,
            profile_identity,
            self.address.to_string(),
            expires_at_unix_seconds,
        );
        self.state.lock().workers.insert(
            bootstrap.worker_id,
            RegisteredWorker {
                authenticator: AuthenticatedPeer::from_bootstrap(&bootstrap),
                challenge: bootstrap.challenge,
                status: WorkerStatus::Registered,
                commands,
                last_seen: Instant::now(),
            },
        );
        bootstrap
    }

    pub(crate) fn enqueue_worker_command(
        &self,
        worker_id: Uuid,
        command: CoordinatorMessage,
    ) -> Result<()> {
        command.validate_bounds()?;
        let mut state = self.state.lock();
        let worker = state
            .workers
            .get_mut(&worker_id)
            .context("window worker is not registered")?;
        ensure!(
            matches!(worker.status, WorkerStatus::Ready | WorkerStatus::Presented),
            "window worker is not ready for routed commands"
        );
        worker.commands.push_back(command);
        Ok(())
    }

    pub(crate) fn worker_status(&self, worker_id: Uuid) -> Option<WorkerStatus> {
        self.state
            .lock()
            .workers
            .get(&worker_id)
            .map(|worker| worker.status)
    }

    pub(crate) fn stale_worker_ids(&self, now: Instant, timeout: std::time::Duration) -> Vec<Uuid> {
        self.state
            .lock()
            .workers
            .iter()
            .filter_map(|(worker_id, worker)| {
                (!matches!(worker.status, WorkerStatus::Closed)
                    && now.saturating_duration_since(worker.last_seen) >= timeout)
                    .then_some(*worker_id)
            })
            .collect()
    }

    pub(crate) fn remove_worker(&self, worker_id: Uuid) {
        self.state.lock().workers.remove(&worker_id);
    }

    pub(crate) fn try_next_event(&self) -> Option<WorkerEvent> {
        self.events.try_recv().ok()
    }
}

pub(crate) struct WorkerClient {
    worker_id: Uuid,
    authenticator: AuthenticatedPeer,
    caller: Box<dyn ipc::ServiceCaller<CoordinatorTransport>>,
}

impl WorkerClient {
    pub(crate) async fn connect(
        bootstrap: &WorkerBootstrap,
        background_executor: Arc<Background>,
    ) -> Result<Self> {
        let client = Arc::new(
            ipc::Client::connect(
                ipc::ConnectionAddress::from(bootstrap.endpoint.clone()),
                background_executor,
            )
            .await
            .context("failed to connect window worker to coordinator")?,
        );
        Ok(Self {
            worker_id: bootstrap.worker_id,
            authenticator: AuthenticatedPeer::from_bootstrap(bootstrap),
            caller: ipc::service_caller(client),
        })
    }

    pub(crate) async fn send(&mut self, message: WorkerMessage) -> Result<CoordinatorMessage> {
        let frame = self.authenticator.encode(message)?;
        let response = self
            .caller
            .call(TransportRequest {
                worker_id: self.worker_id,
                frame,
            })
            .await
            .context("window-process coordinator request failed")?;
        match response {
            TransportResponse::Frame(frame) => self
                .authenticator
                .decode::<CoordinatorMessage>(&frame)
                .map(|(_, message)| message),
            TransportResponse::Rejected { code } => {
                bail!("window-process coordinator rejected request: {code}")
            }
        }
    }
}

#[cfg(unix)]
fn restrict_endpoint(address: &ipc::ConnectionAddress) -> Result<()> {
    use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};

    let path = std::path::Path::new(&address.to_string()).to_path_buf();
    let metadata = std::fs::symlink_metadata(&path)
        .context("failed to inspect window-process coordinator endpoint")?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
        bail!("window-process coordinator endpoint is not a socket");
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .context("failed to restrict window-process coordinator endpoint")
}

#[cfg(not(unix))]
fn restrict_endpoint(_: &ipc::ConnectionAddress) -> Result<()> {
    Ok(())
}

#[cfg(test)]
#[path = "coordinator_tests.rs"]
mod tests;
