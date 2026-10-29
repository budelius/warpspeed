use uuid::Uuid;

use super::{
    AuthenticatedPeer, Chunk, CoordinatorMessage, MAX_CHUNK_BYTES, PROTOCOL_VERSION,
    WorkerBootstrap, WorkerMessage,
};

fn bootstrap() -> WorkerBootstrap {
    WorkerBootstrap::new(
        "build-identity".to_owned(),
        "profile-identity".to_owned(),
        "coordinator-endpoint".to_owned(),
        200,
    )
}

#[test]
fn authenticated_peers_exchange_bounded_frames() {
    let bootstrap = bootstrap();
    let mut sender = AuthenticatedPeer::from_bootstrap(&bootstrap);
    let mut receiver = AuthenticatedPeer::from_bootstrap(&bootstrap);
    let encoded = sender.encode(WorkerMessage::Ready).unwrap();

    let (_, decoded) = receiver.decode::<WorkerMessage>(&encoded).unwrap();

    assert_eq!(decoded, WorkerMessage::Ready);
}

#[test]
fn frame_authentication_detects_modified_content() {
    let bootstrap = bootstrap();
    let mut sender = AuthenticatedPeer::from_bootstrap(&bootstrap);
    let mut receiver = AuthenticatedPeer::from_bootstrap(&bootstrap);
    let encoded = sender.encode(WorkerMessage::Ready).unwrap();
    let altered = String::from_utf8(encoded)
        .unwrap()
        .replace("\"Ready\"", "\"WindowClosed\"");

    let error = receiver
        .decode::<WorkerMessage>(altered.as_bytes())
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "window-process frame authentication failed"
    );
}

#[test]
fn authenticated_frames_cannot_be_replayed() {
    let bootstrap = bootstrap();
    let mut sender = AuthenticatedPeer::from_bootstrap(&bootstrap);
    let mut receiver = AuthenticatedPeer::from_bootstrap(&bootstrap);
    let encoded = sender.encode(WorkerMessage::Ready).unwrap();
    receiver.decode::<WorkerMessage>(&encoded).unwrap();

    let error = receiver.decode::<WorkerMessage>(&encoded).unwrap_err();

    assert_eq!(
        error.to_string(),
        "replayed or out-of-order window-process frame"
    );
}

#[test]
fn peers_reject_frames_from_a_different_build() {
    let bootstrap = bootstrap();
    let mut other_build = bootstrap.clone();
    other_build.build_identity = "other-build".to_owned();
    let mut sender = AuthenticatedPeer::from_bootstrap(&bootstrap);
    let mut receiver = AuthenticatedPeer::from_bootstrap(&other_build);
    let encoded = sender.encode(WorkerMessage::Ready).unwrap();

    let error = receiver.decode::<WorkerMessage>(&encoded).unwrap_err();

    assert_eq!(error.to_string(), "window-process build identity mismatch");
}

#[test]
fn persistence_chunks_are_rejected_before_oversized_frames_are_encoded() {
    let bootstrap = bootstrap();
    let mut sender = AuthenticatedPeer::from_bootstrap(&bootstrap);
    let body = WorkerMessage::PersistenceRecordChunk(Chunk {
        transfer_id: Uuid::new_v4(),
        index: 0,
        count: 1,
        bytes: vec![0; MAX_CHUNK_BYTES + 1],
    });

    let error = sender.encode(body).unwrap_err();

    assert_eq!(
        error.to_string(),
        "window-process transfer chunk exceeds the byte limit"
    );
}

#[test]
fn coordinator_messages_use_the_same_authenticated_channel() {
    let bootstrap = bootstrap();
    let mut sender = AuthenticatedPeer::from_bootstrap(&bootstrap);
    let mut receiver = AuthenticatedPeer::from_bootstrap(&bootstrap);
    let encoded = sender.encode(CoordinatorMessage::Welcome).unwrap();

    let (_, decoded) = receiver.decode::<CoordinatorMessage>(&encoded).unwrap();

    assert_eq!(decoded, CoordinatorMessage::Welcome);
}

#[test]
fn bootstrap_validation_requires_the_same_build_and_profile() {
    let bootstrap = bootstrap();

    bootstrap
        .validate("build-identity", "profile-identity", 100)
        .unwrap();
    assert_eq!(bootstrap.protocol_version, PROTOCOL_VERSION);
}

#[test]
fn expired_bootstraps_are_rejected() {
    let bootstrap = bootstrap();

    let error = bootstrap
        .validate("build-identity", "profile-identity", 201)
        .unwrap_err();

    assert_eq!(error.to_string(), "window worker bootstrap has expired");
}

#[test]
fn bootstrap_secrets_are_redacted_from_debug_output() {
    let bootstrap = bootstrap();
    let output = format!("{bootstrap:?}");

    assert!(output.contains("[redacted]"));
    assert!(!output.contains(&format!("{:?}", bootstrap.secret.0)));
}
