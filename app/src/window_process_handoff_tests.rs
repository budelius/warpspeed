use super::*;
use crate::app_state::WindowSnapshot;

fn window_snapshot() -> WindowSnapshot {
    WindowSnapshot {
        tabs: Vec::new(),
        active_tab_index: 0,
        team_uid: None,
        bounds: None,
        fullscreen_state: Default::default(),
        quake_mode: false,
        universal_search_width: None,
        warp_ai_width: None,
        voltron_width: None,
        warp_drive_index_width: None,
        left_panel_open: false,
        vertical_tabs_panel_open: false,
        left_panel_width: None,
        right_panel_width: None,
        agent_management_filters: None,
        launch_config_source: None,
        tab_groups: Vec::new(),
    }
}

#[test]
fn handoff_payload_round_trip_preserves_window_and_agent_commands() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("handoff.json");
    let payload = HandoffPayload {
        protocol_version: PROTOCOL_VERSION,
        window: window_snapshot(),
        agent_resume_commands: vec![AgentResumeCommand {
            pane_uuid: vec![1, 2, 3],
            command: "claude --resume test".to_owned(),
        }],
        transfers_session_lease: true,
    };

    write_request(&path, &payload).unwrap();
    let restored: HandoffPayload = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();

    assert_eq!(restored.protocol_version, PROTOCOL_VERSION);
    assert_eq!(restored.window, payload.window);
    assert_eq!(
        restored.agent_resume_commands,
        payload.agent_resume_commands
    );
    assert!(restored.transfers_session_lease);
}

#[test]
fn legacy_handoff_payload_does_not_transfer_a_session_lease() {
    let payload = HandoffPayload {
        protocol_version: PROTOCOL_VERSION,
        window: window_snapshot(),
        agent_resume_commands: Vec::new(),
        transfers_session_lease: false,
    };
    let mut encoded = serde_json::to_value(payload).unwrap();
    encoded
        .as_object_mut()
        .unwrap()
        .remove("transfers_session_lease");

    let restored: HandoffPayload = serde_json::from_value(encoded).unwrap();

    assert!(!restored.transfers_session_lease);
}

#[test]
fn ready_and_release_handshake_cleans_up_its_markers() {
    let directory = tempfile::tempdir().unwrap();
    let request_path = directory.path().join("handoff.json");
    let payload = HandoffPayload {
        protocol_version: PROTOCOL_VERSION,
        window: window_snapshot(),
        agent_resume_commands: Vec::new(),
        transfers_session_lease: false,
    };
    write_request(&request_path, &payload).unwrap();

    let incoming = IncomingWindowHandoff::from_path(request_path.clone()).unwrap();
    let (ready_path, release_path) = marker_paths(&request_path);
    assert!(!request_path.exists());

    incoming.mark_ready().unwrap();
    assert!(ready_path.exists());
    write_marker(&release_path).unwrap();
    assert!(warpui::r#async::block_on(incoming.wait_for_release()));

    assert!(!ready_path.exists());
    assert!(!release_path.exists());
}

#[test]
fn installed_replacement_keeps_the_current_channel_binary() {
    let directory = tempfile::tempdir().unwrap();
    let install_root = directory.path();
    let version = "20260827-test";
    fs::write(install_root.join("current.txt"), version).unwrap();
    let version_dir = install_root.join("versions").join(version);
    fs::create_dir_all(&version_dir).unwrap();
    let installed = version_dir.join(if cfg!(windows) {
        "warp-oss.exe"
    } else {
        "warp-oss"
    });
    fs::write(&installed, []).unwrap();
    let current = Path::new(if cfg!(windows) {
        "target/release/warp-oss.exe"
    } else {
        "target/release/warp-oss"
    });

    assert_eq!(
        installed_replacement_executable(current, install_root).unwrap(),
        Some(installed)
    );
}

#[test]
fn missing_local_install_falls_back_to_the_current_executable() {
    let directory = tempfile::tempdir().unwrap();
    let current = Path::new("target/release/warpspeed.exe");

    assert_eq!(
        installed_replacement_executable(current, directory.path()).unwrap(),
        None
    );
}

#[test]
fn handoff_markers_share_the_request_stem() {
    let request = Path::new("handoff-id.json");
    let (ready, release) = marker_paths(request);

    assert_eq!(ready, Path::new("handoff-id.ready"));
    assert_eq!(release, Path::new("handoff-id.release"));
}

/// The original process kills only the process it launched, which on Windows is a crash-recovery
/// wrapper around the real app. The replacement has to learn about an abort some other way, or it
/// carries on as if handed the window and starts fresh shells next to the original's.
#[test]
fn an_aborted_handoff_is_not_mistaken_for_a_release() {
    let directory = tempfile::tempdir().unwrap();
    let request_path = directory.path().join("handoff.json");
    let payload = HandoffPayload {
        protocol_version: PROTOCOL_VERSION,
        window: window_snapshot(),
        agent_resume_commands: Vec::new(),
        transfers_session_lease: false,
    };
    write_request(&request_path, &payload).unwrap();
    let incoming = IncomingWindowHandoff::from_path(request_path.clone()).unwrap();
    incoming.mark_ready().unwrap();

    write_marker(&abort_path(&request_path)).unwrap();
    let started = Instant::now();
    let released = warpui::r#async::block_on(incoming.wait_for_release());

    assert!(!released);
    assert!(started.elapsed() < RELEASE_TIMEOUT);
    assert!(!abort_path(&request_path).exists());
}

#[test]
fn the_abort_marker_shares_the_request_stem() {
    assert_eq!(
        abort_path(Path::new("handoff-id.json")),
        Path::new("handoff-id.abort")
    );
}
