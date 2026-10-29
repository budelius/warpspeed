use std::ffi::OsString;
use std::path::PathBuf;

use super::GuiProcessRole;

#[test]
fn disabled_isolation_uses_the_legacy_profile_owner() {
    assert_eq!(
        GuiProcessRole::resolve(false, None).unwrap(),
        GuiProcessRole::LegacyOwner
    );
}

#[test]
fn enabled_isolation_without_a_bootstrap_uses_the_coordinator() {
    assert_eq!(
        GuiProcessRole::resolve(true, None).unwrap(),
        GuiProcessRole::Coordinator
    );
}

#[test]
fn enabled_isolation_with_a_bootstrap_uses_a_window_worker() {
    assert_eq!(
        GuiProcessRole::resolve(true, Some(OsString::from("worker.json"))).unwrap(),
        GuiProcessRole::WindowWorker {
            bootstrap_path: PathBuf::from("worker.json")
        }
    );
}

#[test]
fn a_worker_bootstrap_cannot_bypass_disabled_isolation() {
    let error = GuiProcessRole::resolve(false, Some(OsString::from("worker.json"))).unwrap_err();

    assert_eq!(
        error.to_string(),
        "WarpSpeed window workers cannot start while window process isolation is disabled"
    );
}

#[test]
fn a_worker_requires_a_nonempty_bootstrap_path() {
    let error = GuiProcessRole::resolve(true, Some(OsString::new())).unwrap_err();

    assert_eq!(
        error.to_string(),
        "WarpSpeed window worker bootstrap path is empty"
    );
}

#[test]
fn only_profile_owners_report_ownership() {
    assert!(GuiProcessRole::LegacyOwner.owns_profile());
    assert!(GuiProcessRole::Coordinator.owns_profile());
    assert!(
        !GuiProcessRole::WindowWorker {
            bootstrap_path: PathBuf::from("worker.json")
        }
        .owns_profile()
    );
}
