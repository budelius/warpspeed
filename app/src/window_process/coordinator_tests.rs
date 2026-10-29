use std::sync::Arc;
use std::time::Duration;

use instant::Instant;
use warpui_core::r#async::executor::Background;

use super::{CoordinatorServer, WorkerClient, WorkerStatus};
use crate::window_process::protocol::{CoordinatorMessage, WorkerMessage};

fn server_and_bootstrap() -> (
    CoordinatorServer,
    crate::window_process::protocol::WorkerBootstrap,
    Arc<Background>,
) {
    let executor = Arc::new(Background::default());
    let server = CoordinatorServer::start(executor.clone()).unwrap();
    let bootstrap = server.register_worker(
        "build-identity".to_owned(),
        "profile-identity".to_owned(),
        u64::MAX,
    );
    (server, bootstrap, executor)
}

#[test]
fn worker_handshake_advances_through_first_frame_readiness() {
    let (server, bootstrap, executor) = server_and_bootstrap();
    let worker_id = bootstrap.worker_id;

    warpui_core::r#async::block_on(async {
        let mut client = WorkerClient::connect(&bootstrap, executor).await.unwrap();
        assert_eq!(
            client
                .send(WorkerMessage::Hello {
                    challenge: bootstrap.challenge
                })
                .await
                .unwrap(),
            CoordinatorMessage::Welcome
        );
        assert!(matches!(
            client.send(WorkerMessage::Ready).await.unwrap(),
            CoordinatorMessage::Ack { .. }
        ));
        assert!(matches!(
            client
                .send(WorkerMessage::FirstFramePresented)
                .await
                .unwrap(),
            CoordinatorMessage::Ack { .. }
        ));
    });

    assert_eq!(
        server.worker_status(worker_id),
        Some(WorkerStatus::Presented)
    );
}

#[test]
fn worker_handshake_rejects_the_wrong_challenge() {
    let (server, bootstrap, executor) = server_and_bootstrap();
    let worker_id = bootstrap.worker_id;

    let error = warpui_core::r#async::block_on(async {
        let mut client = WorkerClient::connect(&bootstrap, executor).await.unwrap();
        client
            .send(WorkerMessage::Hello { challenge: [0; 16] })
            .await
            .unwrap_err()
    });

    assert!(
        error
            .to_string()
            .contains("invalid_worker_state_Registered")
    );
    assert_eq!(
        server.worker_status(worker_id),
        Some(WorkerStatus::Registered)
    );
}

#[test]
fn coordinator_exposes_authenticated_worker_events() {
    let (server, bootstrap, executor) = server_and_bootstrap();

    warpui_core::r#async::block_on(async {
        let mut client = WorkerClient::connect(&bootstrap, executor).await.unwrap();
        client
            .send(WorkerMessage::Hello {
                challenge: bootstrap.challenge,
            })
            .await
            .unwrap();
    });

    let event = server.try_next_event().unwrap();
    assert_eq!(event.worker_id, bootstrap.worker_id);
    assert_eq!(
        event.message,
        WorkerMessage::Hello {
            challenge: bootstrap.challenge
        }
    );
}

#[test]
fn presented_workers_can_request_another_window() {
    let (server, bootstrap, executor) = server_and_bootstrap();

    warpui_core::r#async::block_on(async {
        let mut client = WorkerClient::connect(&bootstrap, executor).await.unwrap();
        client
            .send(WorkerMessage::Hello {
                challenge: bootstrap.challenge,
            })
            .await
            .unwrap();
        client.send(WorkerMessage::Ready).await.unwrap();
        client
            .send(WorkerMessage::FirstFramePresented)
            .await
            .unwrap();
        assert!(matches!(
            client.send(WorkerMessage::OpenWindow).await.unwrap(),
            CoordinatorMessage::Ack { .. }
        ));
    });

    assert!(matches!(
        server.try_next_event().unwrap().message,
        WorkerMessage::Hello { .. }
    ));
    assert_eq!(
        server.try_next_event().unwrap().message,
        WorkerMessage::Ready
    );
    assert_eq!(
        server.try_next_event().unwrap().message,
        WorkerMessage::FirstFramePresented
    );
    assert_eq!(
        server.try_next_event().unwrap().message,
        WorkerMessage::OpenWindow
    );
}

#[test]
fn coordinator_identifies_and_removes_stale_workers() {
    let (server, bootstrap, _) = server_and_bootstrap();

    let stale = server.stale_worker_ids(
        Instant::now() + Duration::from_secs(31),
        Duration::from_secs(30),
    );

    assert_eq!(stale, vec![bootstrap.worker_id]);
    server.remove_worker(bootstrap.worker_id);
    assert_eq!(server.worker_status(bootstrap.worker_id), None);
}
