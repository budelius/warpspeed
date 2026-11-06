//! Explicit wire compatibility for supervisors that outlive their originating build.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use warp_core::SessionId;

use super::message::{Message, PROTOCOL_VERSION, SessionMetadata, SessionSummary};

// v7 only appended identity fields to each ListSessionsResponse row. All other messages,
// including snapshot, bootstrap, endpoint, detach and resize, retain their v6 encoding.
const LIST_SESSIONS_RESPONSE: u32 = 13;

/// Whether the client has an explicitly verified codec for this wire version.
pub fn supports_protocol(version: u32) -> bool {
    matches!(version, 6 | PROTOCOL_VERSION)
}

#[derive(Serialize, Deserialize)]
struct SessionSummaryV6 {
    session_id: SessionId,
    metadata: SessionMetadata,
    detached_for_seconds: u64,
    attached: bool,
}

pub(crate) fn encode(message: &Message, version: u32) -> Result<Vec<u8>> {
    ensure!(
        supports_protocol(version),
        "Unsupported supervisor protocol v{version}"
    );
    if version == 6
        && let Message::ListSessionsResponse { sessions } = message
    {
        let sessions = sessions
            .iter()
            .map(|session| SessionSummaryV6 {
                session_id: session.session_id,
                metadata: session.metadata.clone(),
                detached_for_seconds: session.detached_for_seconds,
                attached: session.attached,
            })
            .collect::<Vec<_>>();
        return Ok(bincode::serialize(&(LIST_SESSIONS_RESPONSE, sessions))?);
    }
    Ok(bincode::serialize(message)?)
}

pub(crate) fn decode(payload: &[u8], version: u32) -> Result<Message> {
    ensure!(
        supports_protocol(version),
        "Unsupported supervisor protocol v{version}"
    );
    if version == 6 && payload.starts_with(&LIST_SESSIONS_RESPONSE.to_le_bytes()) {
        let (_, sessions): (u32, Vec<SessionSummaryV6>) = bincode::deserialize(payload)?;
        return Ok(Message::ListSessionsResponse {
            sessions: sessions
                .into_iter()
                .map(|session| SessionSummary {
                    session_id: session.session_id,
                    metadata: session.metadata,
                    detached_for_seconds: session.detached_for_seconds,
                    attached: session.attached,
                    shell_pid: 0,
                    session_uuid_hex: String::new(),
                })
                .collect(),
        });
    }
    Ok(bincode::deserialize(payload)?)
}

#[cfg(test)]
#[path = "compatibility_tests.rs"]
mod tests;
