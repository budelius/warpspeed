use std::path::Path;

use tempfile::TempDir;

use super::launch_with_executable;
use crate::window_process::bootstrap_file;
use crate::window_process::protocol::WorkerBootstrap;

#[test]
fn a_failed_launch_removes_its_bootstrap_capability() {
    let temporary = TempDir::new().unwrap();
    let bootstrap = WorkerBootstrap::new(
        "build-identity".to_owned(),
        "profile-identity".to_owned(),
        "coordinator-endpoint".to_owned(),
        200,
    );

    let error = launch_with_executable(
        Path::new("executable-that-does-not-exist-window-process-test"),
        temporary.path(),
        &bootstrap,
    )
    .err()
    .unwrap();

    assert!(
        error
            .to_string()
            .contains("failed to launch isolated window worker")
    );
    assert_eq!(
        std::fs::read_dir(bootstrap_file::directory(temporary.path()))
            .unwrap()
            .count(),
        0
    );
}
