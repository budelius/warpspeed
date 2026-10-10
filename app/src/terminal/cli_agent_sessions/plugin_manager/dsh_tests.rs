use std::fs;
use std::path::Path;

use super::{
    INSTRUCTIONS, MINIMUM_PLUGIN_VERSION, PACKAGE, REGISTRY_LINE, installed_version, profiles,
    with_registry_line,
};

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn installs_into_every_profile_or_the_headless_one() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(profiles(home.path()), vec!["headless".to_owned()]);

    for profile in ["web", "acp", ".cache", "with space"] {
        fs::create_dir_all(home.path().join("profiles").join(profile)).unwrap();
    }
    write(&home.path().join("profiles").join("notes.txt"), "");
    assert_eq!(
        profiles(home.path()),
        vec!["acp".to_owned(), "web".to_owned()]
    );
}

#[test]
fn registry_line_is_added_once() {
    assert_eq!(
        with_registry_line("").as_deref(),
        Some(format!("{REGISTRY_LINE}\n").as_str())
    );
    assert_eq!(
        with_registry_line("save-exact=true").as_deref(),
        Some(format!("save-exact=true\n{REGISTRY_LINE}\n").as_str())
    );
    assert_eq!(with_registry_line(&format!("{REGISTRY_LINE}\n")), None);
    assert_eq!(
        with_registry_line("  @stozn:registry=https://mirror.example/npm/\n"),
        None
    );
}

#[test]
fn the_manual_install_command_pins_the_installers_version() {
    let manual = INSTRUCTIONS
        .steps
        .iter()
        .map(|step| step.command)
        .find(|command| command.starts_with("dsh plugin"))
        .expect("the instructions show the install command");
    assert_eq!(
        manual,
        format!("dsh plugin --profile headless add {PACKAGE}@{MINIMUM_PLUGIN_VERSION}")
    );
}

#[test]
fn installed_version_reads_the_profiles_dependency() {
    let profile = tempfile::tempdir().unwrap();
    assert_eq!(installed_version(profile.path()), None);

    write(
        &profile.path().join("package.json"),
        r#"{"dependencies":{"left-pad":"1.0.0"}}"#,
    );
    assert_eq!(installed_version(profile.path()), None);

    write(
        &profile.path().join("package.json"),
        r#"{"dependencies":{"@stozn/dsh-warp":"^0.1.0"}}"#,
    );
    assert_eq!(installed_version(profile.path()).as_deref(), Some("0.0.0"));

    write(
        &profile
            .path()
            .join("node_modules")
            .join("@stozn")
            .join("dsh-warp")
            .join("package.json"),
        &format!(r#"{{"name":"@stozn/dsh-warp","version":"{MINIMUM_PLUGIN_VERSION}"}}"#),
    );
    assert_eq!(
        installed_version(profile.path()).as_deref(),
        Some(MINIMUM_PLUGIN_VERSION)
    );
}
