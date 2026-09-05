//! `bind_listener` flips the process-wide umask around its `bind`, so any
//! test that binds -- or that asserts on a mode a concurrent bind would
//! mask -- has to run alone. The daemon binds once, on one thread, which is
//! why the umask is safe there and only a hazard for a parallel test run.

use std::os::unix::net::UnixStream;

use serial_test::serial;

use super::*;

/// A supervisor must never speak to a daemon from a different protocol
/// generation, so the version is part of the name rather than a field inside
/// the first message.
#[test]
fn the_socket_name_carries_the_protocol_version() {
    let path = socket_path();
    let name = path
        .file_name()
        .expect("the socket path must name a file")
        .to_string_lossy()
        .into_owned();

    assert!(
        name.starts_with(&format!("v{PROTOCOL_VERSION}-")),
        "socket name was {name}"
    );
}

/// The name is per-user and per-state-directory, so two profiles never share a
/// supervisor and one user's daemon is never reachable by another's.
#[test]
fn the_socket_name_is_scoped_to_this_user() {
    let name = socket_path()
        .file_name()
        .expect("the socket path must name a file")
        .to_string_lossy()
        .into_owned();

    assert!(
        name.contains(&nix::unistd::getuid().as_raw().to_string()),
        "socket name was {name}"
    );
}

#[test]
#[serial]
fn a_prepared_socket_directory_is_private_to_this_user() {
    let base = tempfile::tempdir().expect("a temp dir must be creatable");
    let directory = base.path().join("supervisor");

    prepare_socket_directory(&directory).expect("preparing must succeed");

    let mode = std::fs::symlink_metadata(&directory).unwrap().mode();
    assert_eq!(mode & 0o777, 0o700, "mode was {:o}", mode & 0o777);
}

/// A directory left group- or world-writable by an earlier run is tightened
/// rather than accepted: `DirBuilder`'s mode only applies to directories it
/// creates itself.
#[test]
#[serial]
fn an_over_permissive_socket_directory_is_tightened() {
    let base = tempfile::tempdir().expect("a temp dir must be creatable");
    let directory = base.path().join("supervisor");
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o777)).unwrap();

    prepare_socket_directory(&directory).expect("preparing must succeed");

    let mode = std::fs::symlink_metadata(&directory).unwrap().mode();
    assert_eq!(mode & 0o077, 0, "mode was {:o}", mode & 0o777);
}

/// Displacing a live supervisor would strand every session it is holding, so
/// the second one refuses the name instead of taking it.
#[test]
#[serial]
fn binding_over_a_live_supervisor_is_refused() {
    let base = tempfile::tempdir().expect("a temp dir must be creatable");
    let path = base.path().join("supervisor.sock");
    let _first = bind_listener(&path).expect("the first bind must succeed");

    let second = bind_listener(&path);

    assert!(second.is_err(), "a second bind must not take the name");
}

/// A socket left behind by a supervisor that died is not a reason to refuse to
/// start: nothing is listening on it.
#[test]
#[serial]
fn a_stale_socket_is_replaced() {
    let base = tempfile::tempdir().expect("a temp dir must be creatable");
    let path = base.path().join("supervisor.sock");
    drop(bind_listener(&path).expect("the first bind must succeed"));

    let second = bind_listener(&path);

    assert!(second.is_ok(), "a stale socket must be replaceable");
}

#[test]
#[serial]
fn a_bound_socket_is_not_reachable_by_other_users() {
    let base = tempfile::tempdir().expect("a temp dir must be creatable");
    let path = base.path().join("supervisor.sock");

    let _listener = bind_listener(&path).expect("binding must succeed");

    let mode = std::fs::symlink_metadata(&path).unwrap().mode();
    assert_eq!(mode & 0o077, 0, "mode was {:o}", mode & 0o777);
}

/// `sockaddr_un` holds a little over 100 bytes. Going over it fails at bind
/// with `SUN_LEN`, which names nothing useful, so the name is kept short enough
/// to fit under any plausible state directory.
#[test]
fn the_socket_path_fits_in_a_unix_socket_address() {
    let path = socket_path();

    assert!(
        check_socket_path_length(&path).is_ok(),
        "socket path {path:?} is {} bytes",
        path.as_os_str().as_bytes().len()
    );
}

#[test]
fn an_over_long_socket_path_is_refused_by_name() {
    let path = PathBuf::from(format!("/tmp/{}/supervisor.sock", "x".repeat(120)));

    let error = check_socket_path_length(&path).expect_err("an over-long path must be refused");

    assert!(
        error.to_string().contains("XDG_RUNTIME_DIR"),
        "error was {error}"
    );
}

#[test]
#[serial]
fn a_connected_peer_reports_its_own_pid() {
    let base = tempfile::tempdir().expect("a temp dir must be creatable");
    let path = base.path().join("supervisor.sock");
    let listener = bind_listener(&path).expect("binding must succeed");
    let _client = UnixStream::connect(&path).expect("connecting must succeed");

    let (accepted, _) = listener.accept().expect("accepting must succeed");

    assert_eq!(peer_pid(&accepted).unwrap(), std::process::id());
}
