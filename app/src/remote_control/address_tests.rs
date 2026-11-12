//! [`write_marker`] and [`read_markers`] work against the real state directory,
//! which a unit test must not write into: this suite runs beside a developer's
//! own Warp, and a stray marker there would be offered to the `sessions` command as a
//! live instance. Reading is tested against a temporary directory instead.

use super::*;

fn marker_json(pid: u32, address: &str) -> String {
    serde_json::to_string(&InstanceMarker {
        pid,
        address: address.to_owned(),
        version: format!("build-of-{pid}"),
    })
    .unwrap()
}

#[test]
fn a_marker_file_name_declares_its_protocol_version() {
    assert_eq!(marker_protocol_version("v7-1234.json"), Some(7));
    assert_eq!(marker_protocol_version("v12-1.json"), Some(12));
    assert_eq!(marker_protocol_version("1234.json"), None);
    assert_eq!(marker_protocol_version("vx-1234.json"), None);
    assert_eq!(
        marker_protocol_version(&marker_path(1).file_name().unwrap().to_string_lossy()),
        Some(PROTOCOL_VERSION)
    );
}

/// An older build's marker records an older build's channel. Sending this build's bytes there is
/// how a `sessions panes` once got `InvalidBoolEncoding` back from a window on protocol 6.
#[test]
fn only_markers_of_this_protocol_are_offered_for_requests() {
    let directory = tempfile::tempdir().unwrap();
    let older = PROTOCOL_VERSION - 1;
    for (name, contents) in [
        (
            format!("v{PROTOCOL_VERSION}-20.json"),
            marker_json(20, "current"),
        ),
        (format!("v{older}-10.json"), marker_json(10, "older")),
        ("30.json".to_owned(), marker_json(30, "unversioned")),
        ("notes.txt".to_owned(), "not a marker".to_owned()),
    ] {
        std::fs::write(directory.path().join(name), contents).unwrap();
    }

    let found = read_markers_in(directory.path());

    let summary: Vec<(u32, Option<u32>, bool)> = found
        .iter()
        .map(|found| {
            (
                found.marker.pid,
                found.protocol_version,
                found.speaks_this_protocol(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (10, Some(older), false),
            (20, Some(PROTOCOL_VERSION), true),
            (30, None, false),
        ]
    );
}

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
