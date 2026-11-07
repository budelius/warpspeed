use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use instant::Instant;
use uuid::Uuid;
use warpui_core::r#async::Timer;
use warpui_core::r#async::executor::Background;
use warpui_core::{Entity, ModelContext, SingletonEntity};

use super::coordinator::{CoordinatorServer, WorkerEvent, WorkerStatus};
use super::protocol::{Chunk, CoordinatorMessage, MAX_CHUNK_BYTES, WorkerMessage};
use super::spawn::{self, StartingWorker};

const MAX_WINDOW_RESTORE_BYTES: usize = 16 * 1024 * 1024;
const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(60);
const BOOTSTRAP_LIFETIME: Duration = Duration::from_secs(2 * 60);
const MAX_PENDING_URLS: usize = 128;
const MAX_QUEUED_WINDOWS: usize = 128;
const WORKER_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);
const WORKER_CRASH_WINDOW: Duration = Duration::from_secs(5 * 60);
const MAX_WORKER_RESTARTS_PER_CRASH_WINDOW: usize = 3;

#[derive(Default)]
struct SnapshotTransfer {
    transfer_id: Option<Uuid>,
    expected_count: u32,
    next_index: u32,
    bytes: Vec<u8>,
}

impl SnapshotTransfer {
    fn push(&mut self, chunk: Chunk) -> Result<Option<Vec<u8>>> {
        if chunk.index == 0 {
            *self = Self {
                transfer_id: Some(chunk.transfer_id),
                expected_count: chunk.count,
                next_index: 0,
                bytes: Vec::new(),
            };
        }
        ensure!(
            self.transfer_id == Some(chunk.transfer_id)
                && self.expected_count == chunk.count
                && self.next_index == chunk.index,
            "invalid worker window snapshot transfer"
        );
        ensure!(
            self.bytes.len() + chunk.bytes.len() <= MAX_WINDOW_RESTORE_BYTES,
            "worker window snapshot exceeds the byte limit"
        );
        self.bytes.extend(chunk.bytes);
        self.next_index += 1;
        Ok((self.next_index == self.expected_count).then(|| std::mem::take(&mut self.bytes)))
    }
}

struct StartingRecord {
    worker_id: Uuid,
    process: StartingWorker,
    deadline: Instant,
}

struct RunningRecord {
    worker_id: Uuid,
    process: Child,
}

pub(crate) struct CoordinatorManager {
    server: CoordinatorServer,
    build_identity: String,
    profile_identity: String,
    state_dir: PathBuf,
    queued_windows: VecDeque<Vec<u8>>,
    starting: Option<StartingRecord>,
    running: Vec<RunningRecord>,
    worker_crashes: VecDeque<Instant>,
    unresponsive_workers: HashSet<Uuid>,
    window_order: Vec<Uuid>,
    windows: HashMap<Uuid, crate::app_state::WindowSnapshot>,
    snapshot_transfers: HashMap<Uuid, SnapshotTransfer>,
    aggregate_state: crate::app_state::AppState,
    persistence_sender: SyncSender<crate::persistence::ModelEvent>,
    pending_urls: VecDeque<url::Url>,
    halted: Option<String>,
}

impl CoordinatorManager {
    pub(crate) fn start(
        build_identity: String,
        profile_identity: String,
        state_dir: PathBuf,
        aggregate_state: crate::app_state::AppState,
        persistence_sender: SyncSender<crate::persistence::ModelEvent>,
        background_executor: Arc<Background>,
    ) -> Result<Self> {
        super::bootstrap_file::clean_stale(&state_dir, SystemTime::now())?;
        Ok(Self {
            server: CoordinatorServer::start(background_executor)?,
            build_identity,
            profile_identity,
            state_dir,
            queued_windows: VecDeque::new(),
            starting: None,
            running: Vec::new(),
            worker_crashes: VecDeque::new(),
            unresponsive_workers: HashSet::new(),
            window_order: Vec::new(),
            windows: HashMap::new(),
            snapshot_transfers: HashMap::new(),
            aggregate_state,
            persistence_sender,
            pending_urls: VecDeque::new(),
            halted: None,
        })
    }

    pub(crate) fn queue_window(&mut self, serialized_window: Vec<u8>) -> Result<()> {
        ensure!(
            serialized_window.len() <= MAX_WINDOW_RESTORE_BYTES,
            "window restore snapshot exceeds the byte limit"
        );
        if let Some(reason) = &self.halted {
            bail!("window worker restoration is halted: {reason}");
        }
        ensure!(
            self.queued_windows.len() < MAX_QUEUED_WINDOWS,
            "window worker launch queue is full"
        );
        self.queued_windows.push_back(serialized_window);
        Ok(())
    }

    fn queue_new_window(&mut self, urls: Vec<url::Url>) -> Result<()> {
        let payload = super::worker::StartupPayload { window: None, urls };
        self.queue_window(
            serde_json::to_vec(&payload).context("failed to encode new window startup")?,
        )
    }

    pub(crate) fn poll(&mut self, now: Instant) -> Result<()> {
        while let Some(event) = self.server.try_next_event() {
            self.handle_event(event)?;
        }
        self.observe_unresponsive_workers(now);
        self.reap_running_workers(now)?;

        if let Some(starting) = self.starting.as_mut() {
            if let Some(status) = starting.process.try_wait()? {
                let worker_id = starting.worker_id;
                let starting = self.starting.take().unwrap();
                starting.process.abort();
                return self.halt(format!(
                    "window worker {worker_id} exited before readiness with {status}"
                ));
            }
            match self.server.worker_status(starting.worker_id) {
                Some(WorkerStatus::Presented) => {
                    let starting = self.starting.take().unwrap();
                    log::info!(
                        "Window isolation worker {} presented its first frame; {} window(s) remain queued",
                        starting.worker_id,
                        self.queued_windows.len()
                    );
                    while let Some(url) = self.pending_urls.pop_front() {
                        self.server.enqueue_worker_command(
                            starting.worker_id,
                            CoordinatorMessage::OpenUrl {
                                url: url.to_string(),
                            },
                        )?;
                    }
                    self.running.push(RunningRecord {
                        worker_id: starting.worker_id,
                        process: starting.process.into_running(),
                    });
                }
                Some(WorkerStatus::Closed) => {
                    let worker_id = starting.worker_id;
                    let starting = self.starting.take().unwrap();
                    starting.process.abort();
                    return self.halt(format!("window worker {worker_id} closed before readiness"));
                }
                Some(WorkerStatus::Registered | WorkerStatus::Connected | WorkerStatus::Ready) => {
                    if now >= starting.deadline {
                        let worker_id = starting.worker_id;
                        let starting = self.starting.take().unwrap();
                        starting.process.abort();
                        return self.halt(format!(
                            "window worker {worker_id} timed out before readiness"
                        ));
                    }
                    return Ok(());
                }
                None => {
                    let worker_id = starting.worker_id;
                    let starting = self.starting.take().unwrap();
                    starting.process.abort();
                    return self.halt(format!(
                        "window worker {worker_id} disappeared before readiness"
                    ));
                }
            }
        }

        if self.starting.is_none()
            && self.halted.is_none()
            && let Some(window) = self.queued_windows.pop_front()
            && let Err(error) = self.launch_window(window, now)
        {
            return self.halt(format!("failed to launch window worker: {error:#}"));
        }
        Ok(())
    }

    pub(crate) fn route_url(&mut self, url: url::Url) -> Result<()> {
        if let Some(worker) = self.running.last() {
            self.server.enqueue_worker_command(
                worker.worker_id,
                CoordinatorMessage::OpenUrl {
                    url: url.to_string(),
                },
            )
        } else if self.starting.is_none() && self.queued_windows.is_empty() {
            self.queue_new_window(vec![url])
        } else {
            ensure!(
                self.pending_urls.len() < MAX_PENDING_URLS,
                "window coordinator URL queue is full"
            );
            self.pending_urls.push_back(url);
            Ok(())
        }
    }

    pub(crate) fn start_polling(&mut self, ctx: &mut ModelContext<Self>) {
        ctx.spawn(
            async {
                Timer::after(Duration::from_millis(50)).await;
                Instant::now()
            },
            |manager, now, ctx| {
                warp_errors::report_if_error!(manager.poll(now));
                manager.start_polling(ctx);
            },
        );
    }

    fn launch_window(&mut self, serialized_window: Vec<u8>, now: Instant) -> Result<()> {
        let payload: super::worker::StartupPayload = serde_json::from_slice(&serialized_window)
            .context("failed to decode queued window restore")?;
        let initial_window = payload.window;
        let commands = restore_commands(serialized_window)?;
        let expires_at = SystemTime::now()
            .checked_add(BOOTSTRAP_LIFETIME)
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .context("failed to compute window worker bootstrap expiration")?;
        let bootstrap = self.server.register_worker_with_commands(
            self.build_identity.clone(),
            self.profile_identity.clone(),
            expires_at,
            commands,
        );
        let process = spawn::launch(&self.state_dir, &bootstrap)?;
        log::info!(
            "Window isolation launched worker {} (restore position {}, {} window(s) queued)",
            bootstrap.worker_id,
            self.window_order.len(),
            self.queued_windows.len()
        );
        self.window_order.push(bootstrap.worker_id);
        if let Some(window) = initial_window {
            self.windows.insert(bootstrap.worker_id, window);
        }
        self.starting = Some(StartingRecord {
            worker_id: bootstrap.worker_id,
            process,
            deadline: now + WORKER_READY_TIMEOUT,
        });
        Ok(())
    }

    fn handle_event(&mut self, event: WorkerEvent) -> Result<()> {
        match event.message {
            WorkerMessage::AppStateChunk(chunk) => {
                let transfer = self.snapshot_transfers.entry(event.worker_id).or_default();
                if let Some(bytes) = transfer.push(chunk)? {
                    let window = serde_json::from_slice(&bytes)
                        .context("failed to decode worker window snapshot")?;
                    self.windows.insert(event.worker_id, window);
                    self.snapshot_transfers.remove(&event.worker_id);
                    self.persist_aggregate_state()?;
                }
            }
            WorkerMessage::OpenWindow => self.queue_new_window(Vec::new())?,
            WorkerMessage::Hello { .. }
            | WorkerMessage::Ready
            | WorkerMessage::FirstFramePresented
            | WorkerMessage::Heartbeat { .. }
            | WorkerMessage::NextCommand
            | WorkerMessage::PersistenceRecordChunk(_)
            | WorkerMessage::WindowClosed => {}
        }
        Ok(())
    }

    fn persist_aggregate_state(&mut self) -> Result<()> {
        let windows = self
            .window_order
            .iter()
            .map(|worker_id| self.windows.get(worker_id).cloned())
            .collect::<Option<Vec<_>>>();
        let Some(windows) = windows else {
            return Ok(());
        };
        self.aggregate_state.windows = windows;
        self.aggregate_state.active_window_index = self
            .aggregate_state
            .active_window_index
            .filter(|index| *index < self.aggregate_state.windows.len())
            .or((!self.aggregate_state.windows.is_empty()).then_some(0));
        self.persistence_sender
            .send(crate::persistence::ModelEvent::Snapshot(
                self.aggregate_state.clone(),
            ))
            .context("failed to queue aggregated window snapshot")
    }

    fn observe_unresponsive_workers(&mut self, now: Instant) {
        let stale = self
            .server
            .stale_worker_ids(now, WORKER_HEARTBEAT_TIMEOUT)
            .into_iter()
            .collect::<HashSet<_>>();
        self.unresponsive_workers
            .retain(|worker_id| stale.contains(worker_id));
        for worker_id in stale {
            if self.unresponsive_workers.insert(worker_id) {
                log::warn!("Window worker {worker_id} stopped sending heartbeats");
            }
        }
    }

    fn reap_running_workers(&mut self, now: Instant) -> Result<()> {
        let mut index = 0;
        while index < self.running.len() {
            let Some(status) = self.running[index]
                .process
                .try_wait()
                .context("failed to inspect isolated window worker")?
            else {
                index += 1;
                continue;
            };

            let worker = self.running.swap_remove(index);
            let coordinator_status = self.server.worker_status(worker.worker_id);
            self.server.remove_worker(worker.worker_id);
            self.unresponsive_workers.remove(&worker.worker_id);
            if status.success() || coordinator_status == Some(WorkerStatus::Closed) {
                log::info!(
                    "Window isolation worker {} closed cleanly with {}",
                    worker.worker_id,
                    status
                );
                self.remove_window(worker.worker_id);
                self.persist_aggregate_state()?;
                continue;
            }

            let window = self.windows.get(&worker.worker_id).cloned();
            self.remove_window(worker.worker_id);
            while self
                .worker_crashes
                .front()
                .is_some_and(|crash| now.saturating_duration_since(*crash) >= WORKER_CRASH_WINDOW)
            {
                self.worker_crashes.pop_front();
            }
            if self.worker_crashes.len() >= MAX_WORKER_RESTARTS_PER_CRASH_WINDOW {
                log::error!(
                    "Window worker restart budget exhausted after worker {} exited with {}",
                    worker.worker_id,
                    status
                );
                self.persist_aggregate_state()?;
                continue;
            }
            self.worker_crashes.push_back(now);
            log::warn!(
                "Window isolation worker {} exited with {}; scheduling isolated recovery attempt {}/{}",
                worker.worker_id,
                status,
                self.worker_crashes.len(),
                MAX_WORKER_RESTARTS_PER_CRASH_WINDOW
            );
            if let Some(window) = window {
                let payload = super::worker::StartupPayload {
                    window: Some(window),
                    urls: Vec::new(),
                };
                ensure!(
                    self.queued_windows.len() < MAX_QUEUED_WINDOWS,
                    "window worker launch queue is full"
                );
                self.queued_windows.push_front(
                    serde_json::to_vec(&payload)
                        .context("failed to encode crashed window for restart")?,
                );
            } else {
                self.persist_aggregate_state()?;
            }
        }
        Ok(())
    }

    fn remove_window(&mut self, worker_id: Uuid) {
        self.window_order.retain(|id| *id != worker_id);
        self.windows.remove(&worker_id);
        self.snapshot_transfers.remove(&worker_id);
    }

    fn halt(&mut self, reason: String) -> Result<()> {
        self.queued_windows.clear();
        self.halted = Some(reason.clone());
        log::error!("Window isolation restore halted without shared-process fallback: {reason}");
        bail!(reason)
    }
}

impl Entity for CoordinatorManager {
    type Event = ();
}

impl SingletonEntity for CoordinatorManager {}

fn restore_commands(serialized_window: Vec<u8>) -> Result<VecDeque<CoordinatorMessage>> {
    ensure!(
        serialized_window.len() <= MAX_WINDOW_RESTORE_BYTES,
        "window restore snapshot exceeds the byte limit"
    );
    let transfer_id = Uuid::new_v4();
    let count = serialized_window.len().max(1).div_ceil(MAX_CHUNK_BYTES) as u32;
    let mut commands = VecDeque::with_capacity(count as usize);
    if serialized_window.is_empty() {
        commands.push_back(CoordinatorMessage::RestoreWindowChunk(Chunk {
            transfer_id,
            index: 0,
            count: 1,
            bytes: Vec::new(),
        }));
        return Ok(commands);
    }
    for (index, bytes) in serialized_window.chunks(MAX_CHUNK_BYTES).enumerate() {
        commands.push_back(CoordinatorMessage::RestoreWindowChunk(Chunk {
            transfer_id,
            index: index as u32,
            count,
            bytes: bytes.to_vec(),
        }));
    }
    Ok(commands)
}

#[cfg(test)]
#[path = "manager_tests.rs"]
mod tests;
