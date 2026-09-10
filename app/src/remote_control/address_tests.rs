//! Only the naming is tested here.
//!
//! [`write_marker`] and [`read_markers`] work against the real state directory,
//! which a unit test must not write into: this suite runs beside a developer's
//! own Warp, and a stray marker there would be offered to the `sessions` command as a
//! live instance. What those two functions do beyond `serde_json` and
//! `std::fs` is nothing, and the part worth pinning -- that a name identifies
//! one process, one protocol version, and one profile -- is all in the naming.

use super::*;

#[test]
fn a_marker_path_identifies_one_process() {
    let one = marker_path(4321);
    let other = marker_path(4322);

    assert_ne!(one, other);
    assert!(
        one.to_string_lossy().contains("4321"),
        "the pid must be findable in the path, got {one:?}"
    );
    assert_eq!(one.parent(), Some(marker_directory().as_path()));
}

#[test]
fn a_marker_path_carries_the_protocol_version() {
    let path = marker_path(1);
    let name = path.file_name().expect("a marker path has a file name");

    assert!(
        name.to_string_lossy()
            .contains(&format!("v{PROTOCOL_VERSION}")),
        "a marker of one protocol version must not be read as another, got {name:?}"
    );
}

/// Two instances must never be handed the same channel, or a request meant for
/// one window would be answered by the other.
#[test]
fn two_processes_get_different_addresses() {
    let one = listen_address(4321).expect("an address should be derivable");
    let other = listen_address(4322).expect("an address should be derivable");

    assert_ne!(one, other);
}

#[test]
fn an_address_is_stable_for_one_process() {
    let first = listen_address(4321).expect("an address should be derivable");
    let second = listen_address(4321).expect("an address should be derivable");

    assert_eq!(first, second);
}

/// The channel is scoped to a profile, so the `sessions` command run against one state
/// directory cannot reach into another's windows.
#[test]
fn an_address_carries_the_profile() {
    let address = listen_address(4321).expect("an address should be derivable");

    assert!(
        address.contains(&state_directory_hash()),
        "the address must be scoped to this state directory, got {address}"
    );
}
