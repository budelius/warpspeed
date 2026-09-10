use std::fs;

use super::load_session_configs;
use crate::launch_configs::launch_config::PaneTemplateType;

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

use std::collections::HashMap;

use super::{SessionClaim, claim_in_dir, read_owner};

#[test]
fn claiming_a_free_lease_records_the_owner() {
    let dir = tempfile::tempdir().expect("lease directory should be created");
    let mut leases = HashMap::new();

    let claim = claim_in_dir(dir.path(), "obachan", 1, &mut leases)
        .expect("claiming a free lease should succeed");

    assert!(matches!(claim, SessionClaim::Claimed));
    let owner = read_owner(dir.path(), "obachan", 1).expect("owner record should be written");
    assert_eq!(owner.pid, std::process::id());
    assert!(owner.claimed_at_unix_seconds > 0);
}

#[test]
fn a_second_holder_sees_the_recorded_owner() {
    let dir = tempfile::tempdir().expect("lease directory should be created");
    let mut first = HashMap::new();
    let mut second = HashMap::new();

    claim_in_dir(dir.path(), "obachan", 1, &mut first).expect("first claim should succeed");
    let claim = claim_in_dir(dir.path(), "obachan", 1, &mut second)
        .expect("a contended claim should not error");

    let SessionClaim::HeldBy(Some(owner)) = claim else {
        panic!("a held lease should report its owner");
    };
    assert_eq!(owner.pid, std::process::id());
}

#[test]
fn an_already_held_lease_is_reentrant_for_its_holder() {
    let dir = tempfile::tempdir().expect("lease directory should be created");
    let mut leases = HashMap::new();

    claim_in_dir(dir.path(), "obachan", 1, &mut leases).expect("first claim should succeed");
    let claim =
        claim_in_dir(dir.path(), "obachan", 1, &mut leases).expect("reclaiming should succeed");

    assert!(matches!(claim, SessionClaim::Claimed));
}

#[test]
fn a_corrupt_owner_record_still_reports_the_lease_as_held() {
    let dir = tempfile::tempdir().expect("lease directory should be created");
    let mut first = HashMap::new();
    let mut second = HashMap::new();
    claim_in_dir(dir.path(), "obachan", 1, &mut first).expect("first claim should succeed");
    std::fs::write(
        dir.path().join("obachan").join("1").join("owner.json"),
        "{ not json",
    )
    .expect("owner record should be overwritable");

    let claim = claim_in_dir(dir.path(), "obachan", 1, &mut second)
        .expect("a contended claim should not error");

    assert!(matches!(claim, SessionClaim::HeldBy(None)));
}

#[test]
fn a_failed_owner_write_does_not_throw_away_the_lease() {
    let dir = tempfile::tempdir().expect("lease directory should be created");
    let mut leases = HashMap::new();
    // A directory at the owner record's path makes `fs::write` fail there
    // without disturbing the lock file itself.
    std::fs::create_dir_all(dir.path().join("obachan").join("1").join("owner.json"))
        .expect("owner.json should be creatable as a directory to force the write to fail");

    let claim = claim_in_dir(dir.path(), "obachan", 1, &mut leases).expect(
        "a lock already held should not fail just because the owner record could not be written",
    );

    assert!(matches!(claim, SessionClaim::Claimed));
    assert!(
        leases.contains_key(&("obachan".to_owned(), 1)),
        "the acquired lease must not be dropped when only the owner record fails"
    );
}

#[test]
fn different_generations_do_not_contend() {
    let dir = tempfile::tempdir().expect("lease directory should be created");
    let mut first = HashMap::new();
    let mut second = HashMap::new();

    claim_in_dir(dir.path(), "obachan", 1, &mut first).expect("first claim should succeed");
    let claim = claim_in_dir(dir.path(), "obachan", 2, &mut second)
        .expect("a fresh generation should be claimable");

    assert!(matches!(claim, SessionClaim::Claimed));
}
