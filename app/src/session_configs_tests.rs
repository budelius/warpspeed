use std::fs;

use fs4::fs_std::FileExt as _;

use super::{
    OWNER_FILE_NAME, SESSION_LEASES, SessionLease, claim_session, load_session_configs,
    release_session, was_open_when_process_ended_in_dir,
};
use crate::launch_configs::launch_config::{LaunchConfigWindowSource, PaneTemplateType};

fn write_tab_config(dir: &std::path::Path, file_name: &str, name: &str, directory: &str) {
    fs::write(
        dir.join(file_name),
        format!(
            "name = {name:?}\ntitle = {name:?}\n\n[[panes]]\nid = \"main\"\ntype = \"terminal\"\ndirectory = {directory:?}\n"
        ),
    )
    .expect("tab config should be written");
}

#[test]
fn session_lease_is_released_after_the_last_local_claim() {
    let id = uuid::Uuid::new_v4().to_string();
    let source = LaunchConfigWindowSource {
        path: format!("{id}.toml").into(),
        document_index: 3,
        window_index: 0,
        config_name: "Test".to_owned(),
    };
    let key = (id, 3);
    SESSION_LEASES.lock().insert(
        key.clone(),
        SessionLease {
            _file: tempfile::tempfile().expect("temporary lease should be created"),
            claim_count: 1,
        },
    );

    assert!(claim_session(&source).expect("second local claim should succeed"));
    assert_eq!(SESSION_LEASES.lock()[&key].claim_count, 2);

    release_session(&source);
    assert_eq!(SESSION_LEASES.lock()[&key].claim_count, 1);

    release_session(&source);
    assert!(!SESSION_LEASES.lock().contains_key(&key));
}

#[test]
fn owner_marker_with_unlocked_lease_is_interrupted() {
    let dir = tempfile::tempdir().expect("lease directory should be created");
    let lease_dir = dir.path().join("workspace").join("3");
    fs::create_dir_all(&lease_dir).expect("lease directory should be writable");
    fs::write(lease_dir.join(OWNER_FILE_NAME), "{}").expect("owner marker should be writable");
    fs::File::create(lease_dir.join("owner.lock")).expect("lease file should be writable");

    assert!(was_open_when_process_ended_in_dir(
        dir.path(),
        "workspace",
        3
    ));
}

#[test]
fn owner_marker_with_locked_lease_is_still_running() {
    let dir = tempfile::tempdir().expect("lease directory should be created");
    let lease_dir = dir.path().join("workspace").join("3");
    fs::create_dir_all(&lease_dir).expect("lease directory should be writable");
    fs::write(lease_dir.join(OWNER_FILE_NAME), "{}").expect("owner marker should be writable");
    let lease =
        fs::File::create(lease_dir.join("owner.lock")).expect("lease file should be writable");
    assert!(lease.try_lock_exclusive().expect("lease should lock"));

    assert!(!was_open_when_process_ended_in_dir(
        dir.path(),
        "workspace",
        3
    ));
}

#[test]
fn session_config_materializes_referenced_tabs_in_order() {
    let root = tempfile::tempdir().expect("temporary directory should be created");
    let tab_dir = root.path().join("tab_configs");
    let session_dir = root.path().join("session_configurations");
    fs::create_dir_all(&tab_dir).expect("tab config directory should be created");
    fs::create_dir_all(&session_dir).expect("session config directory should be created");
    write_tab_config(&tab_dir, "first.toml", "First", "/first");
    write_tab_config(&tab_dir, "second.toml", "Second", "/second");
    fs::write(
        session_dir.join("workspace.toml"),
        r#"version = 1
id = "workspace"
name = "Workspace"
generation = 3
active_tab = "second"

[[tabs]]
id = "first"
config = "first.toml"

[[tabs]]
id = "second"
config = "second.toml"
"#,
    )
    .expect("session config should be written");

    let (configs, errors) = load_session_configs(&session_dir, &tab_dir);

    assert!(errors.is_empty());
    assert_eq!(configs.len(), 1);
    let config = &configs[0];
    assert_eq!(config.name, "Workspace");
    assert_eq!(config.windows[0].active_tab_index, Some(1));
    assert_eq!(config.windows[0].tabs[0].title.as_deref(), Some("First"));
    assert_eq!(config.windows[0].tabs[1].title.as_deref(), Some("Second"));
    let source = config.source.as_ref().expect("source should be recorded");
    assert_eq!(source.document_index, 3);
    let window_source = config.window_source(0).expect("window source should exist");
    assert!(window_source.is_session_config());
    assert_eq!(window_source.session_generation(), Some(3));
    assert_eq!(window_source.session_id().as_deref(), Some("workspace"));
    assert!(matches!(
        &config.windows[0].tabs[0].layout,
        PaneTemplateType::PaneTemplate { cwd, .. } if cwd == std::path::Path::new("/first")
    ));
}

#[test]
fn session_config_rejects_missing_tab_config() {
    let root = tempfile::tempdir().expect("temporary directory should be created");
    let tab_dir = root.path().join("tab_configs");
    let session_dir = root.path().join("session_configurations");
    fs::create_dir_all(&tab_dir).expect("tab config directory should be created");
    fs::create_dir_all(&session_dir).expect("session config directory should be created");
    fs::write(
        session_dir.join("workspace.toml"),
        r#"version = 1
id = "workspace"
name = "Workspace"

[[tabs]]
id = "missing"
config = "missing.toml"
"#,
    )
    .expect("session config should be written");

    let (configs, errors) = load_session_configs(&session_dir, &tab_dir);

    assert!(configs.is_empty());
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("missing or invalid tab config"));
}

#[test]
fn session_config_rejects_duplicate_tab_ids() {
    let root = tempfile::tempdir().expect("temporary directory should be created");
    let tab_dir = root.path().join("tab_configs");
    let session_dir = root.path().join("session_configurations");
    fs::create_dir_all(&tab_dir).expect("tab config directory should be created");
    fs::create_dir_all(&session_dir).expect("session config directory should be created");
    write_tab_config(&tab_dir, "tab.toml", "Tab", "/tab");
    fs::write(
        session_dir.join("workspace.toml"),
        r#"version = 1
id = "workspace"
name = "Workspace"

[[tabs]]
id = "same"
config = "tab.toml"

[[tabs]]
id = "same"
config = "tab.toml"
"#,
    )
    .expect("session config should be written");

    let (configs, errors) = load_session_configs(&session_dir, &tab_dir);

    assert!(configs.is_empty());
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("duplicate tab id"));
}
