use std::path::PathBuf;

use super::*;

#[test]
fn an_installed_executable_reports_its_version_directory() {
    let executable: PathBuf = [
        "C:\\",
        "Users",
        "someone",
        "AppData",
        "Local",
        "Programs",
        "WarpSpeed",
        "versions",
        "20260829-132618307-bfcd80cd7822-dirty",
        "warpspeed.exe",
    ]
    .iter()
    .collect();

    assert_eq!(
        installed_version(&executable).as_deref(),
        Some("20260829-132618307-bfcd80cd7822-dirty")
    );
}

#[test]
fn a_build_directory_executable_has_no_installed_version() {
    // The case that made this module necessary: several of these were running
    // at once with no way to tell them apart.
    let executable: PathBuf = ["T:\\", "projects", "warp", "target", "release", "warpspeed.exe"]
        .iter()
        .collect();

    assert_eq!(installed_version(&executable), None);
}

#[test]
fn a_versions_directory_one_level_up_does_not_count() {
    // `versions/<version>/warpspeed.exe` is the layout; anything deeper is not
    // something the installer produced, so claiming a version would be a lie.
    let executable: PathBuf = [
        "C:\\", "install", "versions", "20260829-1", "nested", "warpspeed.exe",
    ]
    .iter()
    .collect();

    assert_eq!(installed_version(&executable), None);
}

#[test]
fn the_report_names_the_build_and_every_directory_it_uses() {
    let report = report();

    for expected in [
        "release tag:",
        "revision:",
        "built at:",
        "profile:",
        "executable:",
        "installed as:",
        "launch configs:",
        "config:",
        "data:",
        "state:",
    ] {
        assert!(
            report.contains(expected),
            "report is missing {expected:?}:\n{report}"
        );
    }
}

#[test]
fn empty_build_stamps_read_as_unknown_rather_than_blank() {
    // A blank line in a bug report looks like a bug in the reporting.
    assert_eq!(unknown_if_empty(""), "unknown");
    assert_eq!(unknown_if_empty("   "), "unknown");
    assert_eq!(unknown_if_empty("bfcd80cd7822"), "bfcd80cd7822");
}
