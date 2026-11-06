use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail, ensure};
use futures::lock::Mutex;
use instant::Instant;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warpui_core::r#async::Timer;
use warpui_core::r#async::executor::Background;
use warpui_core::{Entity, ModelContext, SingletonEntity};

use super::bootstrap_file;
use super::coordinator::WorkerClient;
use super::protocol::{Chunk, CoordinatorMessage, WorkerBootstrap, WorkerMessage};

const MAX_RESTORE_BYTES: usize = 16 * 1024 * 1024;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Serialize, Deserialize)]
pub(crate) struct StartupPayload {
    pub(crate) window: Option<crate::app_state::WindowSnapshot>,
    pub(crate) urls: Vec<url::Url>,
}

pub(crate) fn consume_bootstrap(
    path: &Path,
    expected_build_identity: &str,
    expected_profile_identity: &str,
    now_unix_seconds: u64,
) -> Result<WorkerBootstrap> {
    let bootstrap = bootstrap_file::consume(path)?;
    bootstrap.validate(
        expected_build_identity,
        expected_profile_identity,
        now_unix_seconds,
    )?;
    Ok(bootstrap)
}

pub(crate) async fn connect_and_receive_window(
    bootstrap: &WorkerBootstrap,
    background_executor: Arc<Background>,
) -> Result<(WorkerClient, Vec<u8>)> {
    let mut client = WorkerClient::connect(bootstrap, background_executor).await?;
    let welcome = client
        .send(WorkerMessage::Hello {
            challenge: bootstrap.challenge,
        })
        .await?;
    ensure!(
        welcome == CoordinatorMessage::Welcome,
        "window worker did not receive coordinator welcome"
    );

    let mut receiver = RestoreReceiver::default();
    loop {
        match client.send(WorkerMessage::NextCommand).await? {
            CoordinatorMessage::RestoreWindowChunk(chunk) => receiver.push(chunk)?,
            CoordinatorMessage::Noop => break,
            CoordinatorMessage::CoordinatorRetiring => {
                bail!("window-process coordinator retired during worker startup")
            }
            _ => bail!("window worker received an invalid startup command"),
        }
    }
    Ok((client, receiver.finish()?))
}

pub(crate) struct WorkerConnection {
    client: Arc<Mutex<WorkerClient>>,
    started_at: Instant,
}

impl WorkerConnection {
    pub(crate) fn new(client: WorkerClient, ctx: &mut ModelContext<Self>) -> Self {
        let mut connection = Self {
            client: Arc::new(Mutex::new(client)),
            started_at: Instant::now(),
        };
        connection.schedule_heartbeat(ctx);
        connection
    }

    pub(crate) fn first_frame_presented(&self, ctx: &mut ModelContext<Self>) {
        let client = self.client.clone();
        ctx.spawn(
            async move {
                client
                    .lock()
                    .await
                    .send(WorkerMessage::FirstFramePresented)
                    .await
            },
            |_, result, _| warp_errors::report_if_error!(result),
        );
    }

    pub(crate) fn open_window(&self, ctx: &mut ModelContext<Self>) {
        let client = self.client.clone();
        ctx.spawn(
            async move { client.lock().await.send(WorkerMessage::OpenWindow).await },
            |_, result, _| warp_errors::report_if_error!(result),
        );
    }

    pub(crate) fn save_window(
        &self,
        window: &crate::app_state::WindowSnapshot,
        ctx: &mut ModelContext<Self>,
    ) -> Result<()> {
        let bytes = serde_json::to_vec(window)?;
        ensure!(
            bytes.len() <= MAX_RESTORE_BYTES,
            "window snapshot exceeds the byte limit"
        );
        let transfer_id = Uuid::new_v4();
        let count = bytes
            .len()
            .max(1)
            .div_ceil(super::protocol::MAX_CHUNK_BYTES) as u32;
        let chunks = if bytes.is_empty() {
            vec![Chunk {
                transfer_id,
                index: 0,
                count: 1,
                bytes: Vec::new(),
            }]
        } else {
            bytes
                .chunks(super::protocol::MAX_CHUNK_BYTES)
                .enumerate()
                .map(|(index, bytes)| Chunk {
                    transfer_id,
                    index: index as u32,
                    count,
                    bytes: bytes.to_vec(),
                })
                .collect()
        };
        let client = self.client.clone();
        ctx.spawn(
            async move {
                let mut client = client.lock().await;
                for chunk in chunks {
                    client.send(WorkerMessage::AppStateChunk(chunk)).await?;
                }
                Ok::<(), anyhow::Error>(())
            },
            |_, result, _| warp_errors::report_if_error!(result),
        );
        Ok(())
    }

    fn schedule_heartbeat(&mut self, ctx: &mut ModelContext<Self>) {
        let client = self.client.clone();
        let started_at = self.started_at;
        ctx.spawn(
            async move {
                Timer::after(HEARTBEAT_INTERVAL).await;
                let monotonic_millis = started_at.elapsed().as_millis() as u64;
                client
                    .lock()
                    .await
                    .send(WorkerMessage::Heartbeat { monotonic_millis })
                    .await
            },
            |connection, result, ctx| match result {
                Ok(CoordinatorMessage::OpenUrl { url }) => {
                    match url::Url::parse(&url) {
                        Ok(url) if crate::uri::requests_new_window(&url) => {
                            // A worker may host exactly one window. Route a runtime new-window
                            // request back to the coordinator instead of panicking at the process
                            // window limit.
                            connection.open_window(ctx);
                        }
                        Ok(url) => crate::uri::handle_incoming_uri(&url, ctx),
                        Err(error) => warp_errors::report_error!(anyhow::anyhow!(
                            "Coordinator routed an invalid URL: {error}"
                        )),
                    }
                    connection.schedule_heartbeat(ctx);
                }
                Ok(CoordinatorMessage::Ack { .. } | CoordinatorMessage::Noop) => {
                    connection.schedule_heartbeat(ctx);
                }
                Ok(_) => {
                    warp_errors::report_error!("Window worker received an invalid runtime command");
                    connection.schedule_heartbeat(ctx);
                }
                Err(error) => warp_errors::report_error!(error),
            },
        );
    }
}

impl Entity for WorkerConnection {
    type Event = ();
}

impl SingletonEntity for WorkerConnection {}

#[derive(Default)]
struct RestoreReceiver {
    transfer_id: Option<Uuid>,
    expected_count: Option<u32>,
    next_index: u32,
    bytes: Vec<u8>,
}

impl RestoreReceiver {
    fn push(&mut self, chunk: Chunk) -> Result<()> {
        chunk.validate()?;
        match (self.transfer_id, self.expected_count) {
            (None, None) => {
                self.transfer_id = Some(chunk.transfer_id);
                self.expected_count = Some(chunk.count);
            }
            (Some(transfer_id), Some(count)) => {
                ensure!(
                    transfer_id == chunk.transfer_id && count == chunk.count,
                    "window restore transfer changed during startup"
                );
            }
            _ => bail!("invalid window restore receiver state"),
        }
        ensure!(
            chunk.index == self.next_index,
            "window restore chunks arrived out of order"
        );
        ensure!(
            self.bytes.len() + chunk.bytes.len() <= MAX_RESTORE_BYTES,
            "window restore snapshot exceeds the byte limit"
        );
        self.bytes.extend(chunk.bytes);
        self.next_index += 1;
        Ok(())
    }

    fn finish(self) -> Result<Vec<u8>> {
        let Some(expected_count) = self.expected_count else {
            bail!("window worker received no restore snapshot")
        };
        ensure!(
            self.next_index == expected_count,
            "window restore snapshot is incomplete"
        );
        Ok(self.bytes)
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
