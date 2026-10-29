use std::time::SystemTime;

use tempfile::TempDir;

use super::{clean_stale, consume, directory, write};
use crate::window_process::protocol::WorkerBootstrap;

fn bootstrap() -> WorkerBootstrap {
    WorkerBootstrap::new(
        "build-identity".to_owned(),
        "profile-identity".to_owned(),
        "coordinator-endpoint".to_owned(),
        200,
    )
}

#[test]
fn bootstrap_files_are_consumed_once() {
    let temporary = TempDir::new().unwrap();
    let bootstrap = bootstrap();
    let path = write(temporary.path(), &bootstrap).unwrap();

    let restored = consume(&path).unwrap();

    assert_eq!(restored, bootstrap);
    assert!(!path.exists());
}

#[test]
fn bootstrap_files_cannot_replace_an_existing_capability() {
    let temporary = TempDir::new().unwrap();
    let bootstrap = bootstrap();
    write(temporary.path(), &bootstrap).unwrap();

    let error = write(temporary.path(), &bootstrap).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("failed to create worker bootstrap")
    );
}

#[test]
fn oversized_bootstrap_files_are_rejected_before_decode() {
    let temporary = TempDir::new().unwrap();
    let bootstrap = bootstrap();
    let path = directory(temporary.path()).join(format!("worker-{}.json", bootstrap.worker_id));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, vec![0; 16 * 1024 + 1]).unwrap();

    let error = consume(&path).unwrap_err();

    assert_eq!(error.to_string(), "invalid window worker bootstrap file");
}

#[test]
fn unrelated_files_are_not_cleaned() {
    let temporary = TempDir::new().unwrap();
    let path = directory(temporary.path()).join("notes.txt");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"keep").unwrap();

    clean_stale(temporary.path(), SystemTime::now()).unwrap();

    assert!(path.exists());
}

#[cfg(unix)]
#[test]
fn bootstrap_directory_and_file_are_private() {
    use std::os::unix::fs::PermissionsExt as _;

    let temporary = TempDir::new().unwrap();
    let path = write(temporary.path(), &bootstrap()).unwrap();
    let directory_mode = std::fs::metadata(path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode();
    let file_mode = std::fs::metadata(path).unwrap().permissions().mode();

    assert_eq!(directory_mode & 0o077, 0);
    assert_eq!(file_mode & 0o077, 0);
}

#[cfg(unix)]
#[test]
fn bootstrap_symlinks_are_rejected() {
    use std::os::unix::fs::symlink;

    let temporary = TempDir::new().unwrap();
    let bootstrap = bootstrap();
    let path = directory(temporary.path()).join(format!("worker-{}.json", bootstrap.worker_id));
    let target = temporary.path().join("target.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&target, serde_json::to_vec(&bootstrap).unwrap()).unwrap();
    symlink(target, &path).unwrap();

    let error = consume(&path).unwrap_err();

    assert_eq!(
        error.to_string(),
        "window worker bootstrap cannot be a symlink"
    );
}
