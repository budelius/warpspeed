use warp_core::AppId;

use super::*;
use crate::LaunchMode;

#[test]
fn warpspeed_app_ids_require_profile_ownership() {
    assert!(is_warpspeed_app_id(&AppId::new("dev", "warp", "WarpSpeed")));
    assert!(is_warpspeed_app_id(&AppId::new(
        "dev",
        "warp",
        "WarpSpeed-project-a"
    )));
    assert!(is_warpspeed_app_id(&AppId::new(
        "dev",
        "warp",
        "WarpSpeedAdmin"
    )));
    assert!(!is_warpspeed_app_id(&AppId::new("dev", "warp", "Warp")));
    assert!(!is_warpspeed_app_id(&AppId::new(
        "dev",
        "warp",
        "WarpSpeedy"
    )));
    assert!(!permits_window_process_handoff_for_app_id(&AppId::new(
        "dev",
        "warp",
        "WarpSpeed"
    )));
    assert!(permits_window_process_handoff_for_app_id(&AppId::new(
        "dev", "warp", "Warp"
    )));
}

#[test]
fn only_gui_launches_participate_in_profile_ownership() {
    let gui = LaunchMode::App {
        args: warp_cli::AppArgs::default(),
        api_key: None,
        gui_process_role: crate::window_process::role::GuiProcessRole::LegacyOwner,
    };
    let coordinator = LaunchMode::App {
        args: warp_cli::AppArgs::default(),
        api_key: None,
        gui_process_role: crate::window_process::role::GuiProcessRole::Coordinator,
    };
    let worker = LaunchMode::App {
        args: warp_cli::AppArgs::default(),
        api_key: None,
        gui_process_role: crate::window_process::role::GuiProcessRole::WindowWorker {
            bootstrap_path: "worker.json".into(),
        },
    };
    let test_helper = LaunchMode::Test {
        driver: Box::new(None),
        is_integration_test: false,
    };
    let remote_proxy = LaunchMode::RemoteServerProxy;
    let remote_daemon = LaunchMode::RemoteServerDaemon {
        identity_key: "test".to_owned(),
    };

    assert!(gui.participates_in_gui_profile_ownership());
    assert!(coordinator.participates_in_gui_profile_ownership());
    assert!(!worker.participates_in_gui_profile_ownership());
    assert!(!test_helper.participates_in_gui_profile_ownership());
    assert!(!remote_proxy.participates_in_gui_profile_ownership());
    assert!(!remote_daemon.participates_in_gui_profile_ownership());
}

#[test]
fn non_gui_warpspeed_launches_do_not_initialize_gui_app_services() {
    let warpspeed = AppId::new("dev", "warp", "WarpSpeed-test");
    let warp = AppId::new("dev", "warp", "Warp");

    assert!(should_initialize_for_app_id(&warpspeed, true));
    assert!(!should_initialize_for_app_id(&warpspeed, false));
    assert!(should_initialize_for_app_id(&warp, true));
    assert!(should_initialize_for_app_id(&warp, false));
}

#[test]
fn forwarded_launch_metadata_is_read_from_any_url() {
    let urls = [
        url::Url::parse("warpspeed://action/new_window").unwrap(),
        url::Url::parse("warpspeed://action/new_tab?warp_forwarded_source_version=2026-test")
            .unwrap(),
    ];

    assert_eq!(
        forwarded_source_version(&urls).as_deref(),
        Some("2026-test")
    );
}

#[test]
fn profile_scopes_are_distinct_and_safe_for_platform_names() {
    let default = profile_ownership_scope_for_app_id(&AppId::new("dev", "warp", "WarpSpeed"));
    let project =
        profile_ownership_scope_for_app_id(&AppId::new("dev", "warp", "WarpSpeed-project-a"));
    let admin = profile_ownership_scope_for_app_id(&AppId::new("dev", "warp", "WarpSpeedAdmin"));
    let collapsed_without_encoding =
        profile_ownership_scope_for_app_id(&AppId::new("dev", "warp", "WarpSpeed-projecta"));

    assert_ne!(default, project);
    assert_ne!(default, admin);
    assert_ne!(project, collapsed_without_encoding);
    assert!(
        project
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    );
}
