use std::fmt;

use anyhow::{Context, Result, bail, ensure};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(crate) const PROTOCOL_VERSION: u16 = 1;
pub(crate) const MAX_FRAME_BYTES: usize = 320 * 1024;
pub(crate) const MAX_CHUNK_BYTES: usize = 256 * 1024;
const MAX_TRANSFER_CHUNKS: u32 = 256;
const MAX_IDENTITY_BYTES: usize = 256;
const MAX_ENDPOINT_BYTES: usize = 4096;
const MAC_BYTES: usize = 32;
const HMAC_BLOCK_BYTES: usize = 64;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChannelSecret([u8; MAC_BYTES]);

impl ChannelSecret {
    pub(crate) fn generate() -> Self {
        let mut bytes = [0; MAC_BYTES];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }
}

impl fmt::Debug for ChannelSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ChannelSecret([redacted])")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkerBootstrap {
    pub(crate) protocol_version: u16,
    pub(crate) pool_id: Uuid,
    pub(crate) worker_id: Uuid,
    pub(crate) build_identity: String,
    pub(crate) profile_identity: String,
    pub(crate) endpoint: String,
    pub(crate) expires_at_unix_seconds: u64,
    pub(crate) challenge: [u8; 16],
    pub(crate) secret: ChannelSecret,
}

impl WorkerBootstrap {
    #[cfg(test)]
    pub(crate) fn new(
        build_identity: String,
        profile_identity: String,
        endpoint: String,
        expires_at_unix_seconds: u64,
    ) -> Self {
        Self::for_pool(
            Uuid::new_v4(),
            build_identity,
            profile_identity,
            endpoint,
            expires_at_unix_seconds,
        )
    }

    pub(crate) fn for_pool(
        pool_id: Uuid,
        build_identity: String,
        profile_identity: String,
        endpoint: String,
        expires_at_unix_seconds: u64,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            pool_id,
            worker_id: Uuid::new_v4(),
            build_identity,
            profile_identity,
            endpoint,
            expires_at_unix_seconds,
            challenge: random_bytes(),
            secret: ChannelSecret::generate(),
        }
    }

    pub(crate) fn validate(
        &self,
        expected_build_identity: &str,
        expected_profile_identity: &str,
        now_unix_seconds: u64,
    ) -> Result<()> {
        ensure!(
            self.protocol_version == PROTOCOL_VERSION,
            "unsupported window-process protocol version"
        );
        validate_identity("build", &self.build_identity)?;
        validate_identity("profile", &self.profile_identity)?;
        ensure!(
            !self.endpoint.is_empty() && self.endpoint.len() <= MAX_ENDPOINT_BYTES,
            "invalid coordinator endpoint"
        );
        ensure!(
            self.build_identity == expected_build_identity,
            "window worker build identity does not match coordinator"
        );
        ensure!(
            self.profile_identity == expected_profile_identity,
            "window worker profile identity does not match coordinator"
        );
        ensure!(
            now_unix_seconds <= self.expires_at_unix_seconds,
            "window worker bootstrap has expired"
        );
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Chunk {
    pub(crate) transfer_id: Uuid,
    pub(crate) index: u32,
    pub(crate) count: u32,
    #[serde(with = "serde_bytes")]
    pub(crate) bytes: Vec<u8>,
}

impl fmt::Debug for Chunk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Chunk")
            .field("transfer_id", &self.transfer_id)
            .field("index", &self.index)
            .field("count", &self.count)
            .field("byte_len", &self.bytes.len())
            .finish()
    }
}

impl Chunk {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.count > 0 && self.count <= MAX_TRANSFER_CHUNKS,
            "invalid window-process transfer chunk count"
        );
        ensure!(
            self.index < self.count,
            "invalid window-process transfer chunk index"
        );
        ensure!(
            self.bytes.len() <= MAX_CHUNK_BYTES,
            "window-process transfer chunk exceeds the byte limit"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum WorkerMessage {
    Hello { challenge: [u8; 16] },
    Ready,
    FirstFramePresented,
    Heartbeat { monotonic_millis: u64 },
    NextCommand,
    AppStateChunk(Chunk),
    PersistenceRecordChunk(Chunk),
    OpenWindow,
    WindowClosed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum CoordinatorMessage {
    Welcome,
    Ack { request_id: Uuid },
    RequestAppState { transfer_id: Uuid },
    RestoreWindowChunk(Chunk),
    Noop,
    SettingsChanged { revision: u64 },
    OpenUrl { url: String },
    CloseWindow,
    CoordinatorRetiring,
    Reject { code: String },
}

pub(crate) trait BoundedMessage: Serialize + DeserializeOwned {
    fn validate_bounds(&self) -> Result<()>;
}

impl BoundedMessage for WorkerMessage {
    fn validate_bounds(&self) -> Result<()> {
        match self {
            Self::AppStateChunk(chunk) | Self::PersistenceRecordChunk(chunk) => chunk.validate(),
            Self::Hello { .. }
            | Self::Ready
            | Self::FirstFramePresented
            | Self::Heartbeat { .. }
            | Self::NextCommand
            | Self::OpenWindow
            | Self::WindowClosed => Ok(()),
        }
    }
}

impl BoundedMessage for CoordinatorMessage {
    fn validate_bounds(&self) -> Result<()> {
        match self {
            Self::RestoreWindowChunk(chunk) => chunk.validate(),
            Self::OpenUrl { url } => {
                ensure!(
                    !url.is_empty() && url.len() <= MAX_ENDPOINT_BYTES,
                    "invalid routed URL"
                );
                Ok(())
            }
            Self::Reject { code } => {
                ensure!(
                    !code.is_empty() && code.len() <= MAX_IDENTITY_BYTES,
                    "invalid rejection code"
                );
                Ok(())
            }
            Self::Welcome
            | Self::Ack { .. }
            | Self::RequestAppState { .. }
            | Self::Noop
            | Self::SettingsChanged { .. }
            | Self::CloseWindow
            | Self::CoordinatorRetiring => Ok(()),
        }
    }
}

#[derive(Serialize)]
struct UnsignedFrame<'a, T> {
    protocol_version: u16,
    pool_id: Uuid,
    worker_id: Uuid,
    build_identity: &'a str,
    profile_identity: &'a str,
    sequence: u64,
    request_id: Uuid,
    body: &'a T,
}

#[derive(Serialize, Deserialize)]
struct WireFrame<T> {
    protocol_version: u16,
    pool_id: Uuid,
    worker_id: Uuid,
    build_identity: String,
    profile_identity: String,
    sequence: u64,
    request_id: Uuid,
    body: T,
    mac: [u8; MAC_BYTES],
}

pub(crate) struct AuthenticatedPeer {
    pool_id: Uuid,
    worker_id: Uuid,
    build_identity: String,
    profile_identity: String,
    secret: ChannelSecret,
    next_outbound_sequence: u64,
    highest_inbound_sequence: u64,
}

impl AuthenticatedPeer {
    pub(crate) fn from_bootstrap(bootstrap: &WorkerBootstrap) -> Self {
        Self {
            pool_id: bootstrap.pool_id,
            worker_id: bootstrap.worker_id,
            build_identity: bootstrap.build_identity.clone(),
            profile_identity: bootstrap.profile_identity.clone(),
            secret: bootstrap.secret.clone(),
            next_outbound_sequence: 1,
            highest_inbound_sequence: 0,
        }
    }

    pub(crate) fn encode<T: BoundedMessage>(&mut self, body: T) -> Result<Vec<u8>> {
        body.validate_bounds()?;
        let sequence = self.next_outbound_sequence;
        self.next_outbound_sequence = self
            .next_outbound_sequence
            .checked_add(1)
            .context("window-process sequence exhausted")?;
        let request_id = Uuid::new_v4();
        let unsigned = UnsignedFrame {
            protocol_version: PROTOCOL_VERSION,
            pool_id: self.pool_id,
            worker_id: self.worker_id,
            build_identity: &self.build_identity,
            profile_identity: &self.profile_identity,
            sequence,
            request_id,
            body: &body,
        };
        let authenticated = serde_json::to_vec(&unsigned)
            .context("failed to encode authenticated window-process frame")?;
        let frame = WireFrame {
            protocol_version: unsigned.protocol_version,
            pool_id: unsigned.pool_id,
            worker_id: unsigned.worker_id,
            build_identity: unsigned.build_identity.to_owned(),
            profile_identity: unsigned.profile_identity.to_owned(),
            sequence,
            request_id,
            body,
            mac: hmac_sha256(&self.secret.0, &authenticated),
        };
        let bytes = serde_json::to_vec(&frame).context("failed to encode window-process frame")?;
        ensure!(
            bytes.len() <= MAX_FRAME_BYTES,
            "window-process frame exceeds the byte limit"
        );
        Ok(bytes)
    }

    pub(crate) fn decode<T: BoundedMessage>(&mut self, bytes: &[u8]) -> Result<(Uuid, T)> {
        ensure!(
            bytes.len() <= MAX_FRAME_BYTES,
            "window-process frame exceeds the byte limit"
        );
        let frame: WireFrame<T> =
            serde_json::from_slice(bytes).context("invalid window-process frame")?;
        ensure!(
            frame.protocol_version == PROTOCOL_VERSION,
            "unsupported window-process protocol version"
        );
        ensure!(
            frame.pool_id == self.pool_id && frame.worker_id == self.worker_id,
            "window-process peer identity mismatch"
        );
        ensure!(
            frame.build_identity == self.build_identity,
            "window-process build identity mismatch"
        );
        ensure!(
            frame.profile_identity == self.profile_identity,
            "window-process profile identity mismatch"
        );
        validate_identity("build", &frame.build_identity)?;
        validate_identity("profile", &frame.profile_identity)?;
        frame.body.validate_bounds()?;

        let unsigned = UnsignedFrame {
            protocol_version: frame.protocol_version,
            pool_id: frame.pool_id,
            worker_id: frame.worker_id,
            build_identity: &frame.build_identity,
            profile_identity: &frame.profile_identity,
            sequence: frame.sequence,
            request_id: frame.request_id,
            body: &frame.body,
        };
        let authenticated = serde_json::to_vec(&unsigned)
            .context("failed to verify authenticated window-process frame")?;
        let expected_mac = hmac_sha256(&self.secret.0, &authenticated);
        ensure!(
            constant_time_equal(&frame.mac, &expected_mac),
            "window-process frame authentication failed"
        );
        ensure!(
            frame.sequence > self.highest_inbound_sequence,
            "replayed or out-of-order window-process frame"
        );
        self.highest_inbound_sequence = frame.sequence;
        Ok((frame.request_id, frame.body))
    }
}

fn random_bytes<const SIZE: usize>() -> [u8; SIZE] {
    let mut bytes = [0; SIZE];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

fn validate_identity(kind: &str, identity: &str) -> Result<()> {
    if identity.is_empty() || identity.len() > MAX_IDENTITY_BYTES {
        bail!("invalid window-process {kind} identity");
    }
    Ok(())
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; MAC_BYTES] {
    let mut normalized_key = [0; HMAC_BLOCK_BYTES];
    if key.len() > HMAC_BLOCK_BYTES {
        normalized_key[..MAC_BYTES].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized_key[..key.len()].copy_from_slice(key);
    }

    let mut inner_pad = [0x36; HMAC_BLOCK_BYTES];
    let mut outer_pad = [0x5c; HMAC_BLOCK_BYTES];
    for ((inner, outer), key_byte) in inner_pad
        .iter_mut()
        .zip(outer_pad.iter_mut())
        .zip(normalized_key)
    {
        *inner ^= key_byte;
        *outer ^= key_byte;
    }

    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    outer.finalize().into()
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0, |difference, (left, right)| difference | (left ^ right))
        == 0
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
