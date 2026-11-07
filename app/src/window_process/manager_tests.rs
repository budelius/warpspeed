use std::sync::Arc;
use std::sync::mpsc::sync_channel;

use tempfile::TempDir;
use uuid::Uuid;
use warpui_core::r#async::executor::Background;

use super::{CoordinatorManager, MAX_WINDOW_RESTORE_BYTES, SnapshotTransfer, restore_commands};
use crate::app_state::AppState;
use crate::window_process::protocol::{Chunk, CoordinatorMessage, MAX_CHUNK_BYTES};
use crate::window_process::worker::StartupPayload;

#[test]
fn restore_snapshots_are_split_into_bounded_ordered_chunks() {
    let snapshot = vec![7; MAX_CHUNK_BYTES * 2 + 5];

    let commands = restore_commands(snapshot.clone()).unwrap();

    assert_eq!(commands.len(), 3);
    let mut restored = Vec::new();
    for (expected_index, command) in commands.into_iter().enumerate() {
        let CoordinatorMessage::RestoreWindowChunk(chunk) = command else {
            panic!("unexpected coordinator command");
        };
        assert_eq!(chunk.index, expected_index as u32);
        assert_eq!(chunk.count, 3);
        assert!(chunk.bytes.len() <= MAX_CHUNK_BYTES);
        restored.extend(chunk.bytes);
    }
    assert_eq!(restored, snapshot);
}

#[test]
fn empty_restore_snapshots_still_have_one_chunk() {
    let commands = restore_commands(Vec::new()).unwrap();

    assert_eq!(commands.len(), 1);
    let CoordinatorMessage::RestoreWindowChunk(chunk) = &commands[0] else {
        panic!("unexpected coordinator command");
    };
    assert_eq!(chunk.index, 0);
    assert_eq!(chunk.count, 1);
    assert!(chunk.bytes.is_empty());
}

#[test]
fn restore_snapshots_have_an_aggregate_byte_limit() {
    let error = restore_commands(vec![0; MAX_WINDOW_RESTORE_BYTES + 1]).unwrap_err();

    assert_eq!(
        error.to_string(),
        "window restore snapshot exceeds the byte limit"
    );
}

#[test]
fn worker_window_snapshot_chunks_are_reassembled_in_order() {
    let transfer_id = Uuid::new_v4();
    let mut transfer = SnapshotTransfer::default();

    let first = transfer
        .push(Chunk {
            transfer_id,
            index: 0,
            count: 2,
            bytes: b"window ".to_vec(),
        })
        .unwrap();
    let second = transfer
        .push(Chunk {
            transfer_id,
            index: 1,
            count: 2,
            bytes: b"snapshot".to_vec(),
        })
        .unwrap();

    assert!(first.is_none());
    assert_eq!(second, Some(b"window snapshot".to_vec()));
}

#[test]
fn worker_window_snapshot_chunks_reject_out_of_order_transfers() {
    let error = SnapshotTransfer::default()
        .push(Chunk {
            transfer_id: Uuid::new_v4(),
            index: 1,
            count: 2,
            bytes: Vec::new(),
        })
        .unwrap_err();

    assert_eq!(error.to_string(), "invalid worker window snapshot transfer");
}

#[test]
fn routed_url_starts_a_worker_after_the_last_window_closes() {
    let state_dir = TempDir::new().unwrap();
    let (persistence_sender, _persistence_receiver) = sync_channel(4);
    let mut manager = CoordinatorManager::start(
        "build-identity".to_owned(),
        "profile-identity".to_owned(),
        state_dir.path().to_path_buf(),
        AppState {
            windows: Vec::new(),
            active_window_index: None,
            block_lists: Arc::default(),
            running_mcp_servers: Vec::new(),
        },
        persistence_sender,
        Arc::new(Background::default()),
    )
    .unwrap();
    let first_url =
        url::Url::parse("warpspeed://action/new_window?path=C%3A%5CUsers%5Cbudel").unwrap();
    let second_url =
        url::Url::parse("warpspeed://action/new_window?path=C%3A%5CUsers%5Cbudel%5Cprojects")
            .unwrap();

    manager.route_url(first_url.clone()).unwrap();

    assert!(manager.pending_urls.is_empty());
    assert_eq!(manager.queued_windows.len(), 1);
    let payload: StartupPayload =
        serde_json::from_slice(manager.queued_windows.front().unwrap()).unwrap();
    assert!(payload.window.is_none());
    assert_eq!(payload.urls, vec![first_url]);

    manager.route_url(second_url.clone()).unwrap();

    assert_eq!(manager.queued_windows.len(), 1);
    assert_eq!(manager.pending_urls.len(), 1);
    assert_eq!(manager.pending_urls.front(), Some(&second_url));
}
