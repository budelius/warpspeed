use std::collections::VecDeque;
use std::sync::Arc;

use uuid::Uuid;
use warpui_core::r#async::executor::Background;

use super::{RestoreReceiver, connect_and_receive_window};
use crate::window_process::coordinator::CoordinatorServer;
use crate::window_process::protocol::{Chunk, CoordinatorMessage};

#[test]
fn worker_receives_its_restore_snapshot_over_the_authenticated_channel() {
    let executor = Arc::new(Background::default());
    let server = CoordinatorServer::start(executor.clone()).unwrap();
    let transfer_id = Uuid::new_v4();
    let bootstrap = server.register_worker_with_commands(
        "build-identity".to_owned(),
        "profile-identity".to_owned(),
        u64::MAX,
        VecDeque::from([CoordinatorMessage::RestoreWindowChunk(Chunk {
            transfer_id,
            index: 0,
            count: 1,
            bytes: b"window snapshot".to_vec(),
        })]),
    );

    let (_, snapshot) =
        warpui_core::r#async::block_on(connect_and_receive_window(&bootstrap, executor)).unwrap();

    assert_eq!(snapshot, b"window snapshot");
}

#[test]
fn worker_rejects_out_of_order_restore_chunks() {
    let transfer_id = Uuid::new_v4();
    let mut receiver = RestoreReceiver::default();

    let error = receiver
        .push(Chunk {
            transfer_id,
            index: 1,
            count: 2,
            bytes: Vec::new(),
        })
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "window restore chunks arrived out of order"
    );
}

#[test]
fn worker_requires_a_complete_restore_snapshot() {
    let transfer_id = Uuid::new_v4();
    let mut receiver = RestoreReceiver::default();
    receiver
        .push(Chunk {
            transfer_id,
            index: 0,
            count: 2,
            bytes: Vec::new(),
        })
        .unwrap();

    let error = receiver.finish().unwrap_err();

    assert_eq!(error.to_string(), "window restore snapshot is incomplete");
}
