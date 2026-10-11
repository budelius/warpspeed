use std::fs;
use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
use serde_json::Value;

use super::{
    INSTRUCTIONS, MINIMUM_PLUGIN_VERSION, PACKAGE, PLUGIN_TARBALL, command_path, installed_version,
    plugin_add_args, profiles, tarball_file_name, write_tarball,
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
fn the_bundled_tarball_is_the_pinned_release() {
    let mut archive = tar::Archive::new(GzDecoder::new(PLUGIN_TARBALL));
    let mut manifest = None;
    let mut has_entry_point = false;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().into_owned();
        if path == Path::new("package/package.json") {
            let mut contents = String::new();
            entry.read_to_string(&mut contents).unwrap();
            manifest = Some(serde_json::from_str::<Value>(&contents).unwrap());
        } else if path == Path::new("package/dist/index.js") {
            has_entry_point = true;
        }
    }
    let manifest = manifest.expect("the tarball has package/package.json");
    assert_eq!(manifest["name"], PACKAGE);
    assert_eq!(manifest["version"], MINIMUM_PLUGIN_VERSION);
    assert!(has_entry_point, "the tarball has package/dist/index.js");
    assert_eq!(
        tarball_file_name(),
        format!("stozn-dsh-warp-{MINIMUM_PLUGIN_VERSION}.tgz")
    );
}

#[test]
fn the_tarball_is_written_to_a_stable_path_under_dsh_home() {
    let home = tempfile::tempdir().unwrap();
    let expected = home.path().join("warp").join(tarball_file_name());

    let written = write_tarball(home.path()).unwrap();
    assert_eq!(written, expected);
    assert_eq!(fs::read(&written).unwrap(), PLUGIN_TARBALL);

    assert_eq!(write_tarball(home.path()).unwrap(), expected);
    assert_eq!(fs::read(&expected).unwrap(), PLUGIN_TARBALL);
    let entries = fs::read_dir(home.path().join("warp")).unwrap().count();
    assert_eq!(entries, 1, "no temporary file is left behind");

    fs::write(&expected, b"stale").unwrap();
    write_tarball(home.path()).unwrap();
    assert_eq!(fs::read(&expected).unwrap(), PLUGIN_TARBALL);
}

#[test]
fn the_plugin_is_added_from_the_tarball_path() {
    assert_eq!(
        plugin_add_args("headless", "/home/me/.dsh/warp/plugin.tgz"),
        [
            "plugin",
            "--profile",
            "headless",
            "add",
            "/home/me/.dsh/warp/plugin.tgz"
        ]
    );
}

#[test]
fn a_tarball_path_the_shell_would_split_is_refused() {
    assert_eq!(
        command_path(Path::new("/home/me/.dsh/warp/plugin.tgz")).unwrap(),
        "/home/me/.dsh/warp/plugin.tgz"
    );
    for unsafe_path in [
        "/home/my name/.dsh/warp/plugin.tgz",
        "/home/me/.dsh/warp$x/plugin.tgz",
        "/home/me/a&b/plugin.tgz",
        "/home/me/50%/plugin.tgz",
        "/home/me/it's/plugin.tgz",
    ] {
        let error = command_path(Path::new(unsafe_path)).unwrap_err();
        assert!(
            error.message.contains(unsafe_path),
            "the refusal names {unsafe_path}: {}",
            error.message
        );
    }
}

#[cfg(windows)]
#[test]
fn windows_tarball_paths_use_forward_slashes() {
    assert_eq!(
        command_path(Path::new(r"C:\Users\me\.dsh\warp\plugin.tgz")).unwrap(),
        "C:/Users/me/.dsh/warp/plugin.tgz"
    );
}

#[test]
fn the_manual_install_command_uses_the_bundled_tarball() {
    let manual = INSTRUCTIONS
        .steps
        .iter()
        .map(|step| step.command)
        .find(|command| command.starts_with("dsh plugin"))
        .expect("the instructions show the install command");
    assert_eq!(
        manual,
        format!(
            "dsh plugin --profile headless add <dsh home>/warp/stozn-dsh-warp-{MINIMUM_PLUGIN_VERSION}.tgz"
        )
    );
    assert!(
        INSTRUCTIONS.steps.iter().all(|step| {
            !step.command.contains("registry") && !step.description.contains("registry")
        }),
        "the instructions no longer mention the registry"
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
