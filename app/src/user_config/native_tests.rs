use std::fs;

use serde::Deserialize;

use super::{
    Overwrite, SaveLaunchConfigError, load_launch_configs, overwrite_launch_config_window_in_dir,
    write_launch_config_file,
};
use crate::launch_configs::launch_config::{
    LaunchConfig, LaunchConfigWindowSource, WindowTemplate,
};

fn window(cwd: &str) -> WindowTemplate {
    serde_yaml::from_str(&format!("tabs:\n  - layout:\n      cwd: {cwd}\n"))
        .expect("window template should parse")
}

fn config(name: &str, cwd: &str) -> LaunchConfig {
    LaunchConfig {
        name: name.to_string(),
        active_window_index: Some(0),
        windows: vec![window(cwd)],
        source: None,
    }
}

fn read_configs(path: &std::path::Path) -> Vec<LaunchConfig> {
    let file = fs::File::open(path).expect("launch config should be readable");
    serde_yaml::Deserializer::from_reader(file)
        .map(LaunchConfig::deserialize)
        .collect::<Result<Vec<_>, _>>()
        .expect("launch config documents should parse")
}

#[test]
fn loading_launch_configs_records_each_document_source() {
    let dir = tempfile::tempdir().expect("launch config directory should be created");
    let path = dir.path().join("devops.yaml");
    fs::write(
        &path,
        "name: DEVOPS\nwindows: []\n---\nname: OTHER\nwindows: []\n",
    )
    .expect("launch config should be written");

    let configs = load_launch_configs(dir.path());

    assert_eq!(configs.len(), 2);
    assert_eq!(configs[0].source.as_ref().unwrap().path, path);
    assert_eq!(configs[0].source.as_ref().unwrap().document_index, 0);
    assert_eq!(configs[1].source.as_ref().unwrap().document_index, 1);
}

#[test]
fn overwriting_window_preserves_other_windows_and_documents() {
    let dir = tempfile::tempdir().expect("launch config directory should be created");
    let path = dir.path().join("devops.yaml");
    fs::write(
        &path,
        "name: DEVOPS\nwindows:\n  - tabs:\n      - layout:\n          cwd: /devops/one\n  - tabs:\n      - layout:\n          cwd: /devops/two\n---\nname: OTHER\nwindows:\n  - tabs:\n      - layout:\n          cwd: /other\n",
    )
    .expect("launch config should be written");
    let source = LaunchConfigWindowSource {
        path: path.clone(),
        document_index: 0,
        window_index: 1,
        config_name: "DEVOPS".to_string(),
    };

    overwrite_launch_config_window_in_dir(dir.path(), &source, window("/devops/updated"))
        .expect("window should be overwritten");

    let file = fs::File::open(path).expect("launch config should be readable");
    let configs = serde_yaml::Deserializer::from_reader(file)
        .map(LaunchConfig::deserialize)
        .collect::<Result<Vec<_>, _>>()
        .expect("launch config documents should parse");
    assert_eq!(configs.len(), 2);
    assert_eq!(configs[0].windows[0], window("/devops/one"));
    assert_eq!(configs[0].windows[1], window("/devops/updated"));
    assert_eq!(configs[1].windows[0], window("/other"));
}

#[test]
fn overwriting_window_accepts_a_case_only_config_name_change() {
    let dir = tempfile::tempdir().expect("launch config directory should be created");
    let path = dir.path().join("SYS.yaml");
    fs::write(
        &path,
        "name: Sys\nwindows:\n  - tabs:\n      - layout:\n          cwd: /system/old\n",
    )
    .expect("launch config should be written");
    let source = LaunchConfigWindowSource {
        path: path.clone(),
        document_index: 0,
        window_index: 0,
        config_name: "SYS".to_string(),
    };

    let updated = overwrite_launch_config_window_in_dir(dir.path(), &source, window("/system/new"))
        .expect("case-only name changes should not detach an open window from its config");

    assert_eq!(updated.name, "Sys");
    assert_eq!(updated.windows[0], window("/system/new"));
    let stored = read_configs(&path);
    assert_eq!(stored[0].name, "Sys");
    assert_eq!(stored[0].windows[0], window("/system/new"));
}

#[test]
fn saving_a_new_launch_config_writes_a_single_document() {
    let dir = tempfile::tempdir().expect("launch config directory should be created");

    let saved = write_launch_config_file(
        dir.path(),
        "devops".to_string(),
        config("DEVOPS", "/devops"),
        Overwrite::No,
    )
    .expect("launch config should be saved");

    assert_eq!(saved.file_name, "devops.yaml");
    assert_eq!(saved.document_index, 0);
    let configs = read_configs(&dir.path().join("devops.yaml"));
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].windows[0], window("/devops"));
}

#[test]
fn saving_a_new_launch_config_refuses_an_existing_name() {
    let dir = tempfile::tempdir().expect("launch config directory should be created");
    fs::write(
        dir.path().join("devops.yaml"),
        "name: DEVOPS\nwindows: []\n",
    )
    .expect("launch config should be written");

    let error = write_launch_config_file(
        dir.path(),
        "devops.yaml".to_string(),
        config("DEVOPS", "/devops"),
        Overwrite::No,
    )
    .expect_err("saving over an existing name should fail");

    assert!(matches!(
        error,
        SaveLaunchConfigError::FileAlreadyExists(name) if name == "devops.yaml"
    ));
}

#[test]
fn overwriting_replaces_a_single_document_file() {
    let dir = tempfile::tempdir().expect("launch config directory should be created");
    let path = dir.path().join("devops.yaml");
    fs::write(
        &path,
        "name: DEVOPS\nwindows:\n  - tabs:\n      - layout:\n          cwd: /devops/old\n",
    )
    .expect("launch config should be written");

    let saved = write_launch_config_file(
        dir.path(),
        "devops.yaml".to_string(),
        config("DEVOPS", "/devops/new"),
        Overwrite::Yes,
    )
    .expect("launch config should be overwritten");

    assert_eq!(saved.document_index, 0);
    let configs = read_configs(&path);
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].windows[0], window("/devops/new"));
}

#[test]
fn overwriting_keeps_the_other_documents_in_a_multi_document_file() {
    let dir = tempfile::tempdir().expect("launch config directory should be created");
    let path = dir.path().join("devops.yaml");
    fs::write(
        &path,
        "name: OTHER\nwindows:\n  - tabs:\n      - layout:\n          cwd: /other\n---\nname: DEVOPS\nwindows:\n  - tabs:\n      - layout:\n          cwd: /devops/old\n",
    )
    .expect("launch config should be written");

    let saved = write_launch_config_file(
        dir.path(),
        "devops.yaml".to_string(),
        config("DEVOPS", "/devops/new"),
        Overwrite::Yes,
    )
    .expect("launch config should be overwritten");

    assert_eq!(saved.document_index, 1);
    let configs = read_configs(&path);
    assert_eq!(configs.len(), 2);
    assert_eq!(configs[0].windows[0], window("/other"));
    assert_eq!(configs[1].windows[0], window("/devops/new"));
}

#[test]
fn overwriting_adds_a_document_when_no_name_in_the_file_matches() {
    let dir = tempfile::tempdir().expect("launch config directory should be created");
    let path = dir.path().join("devops.yaml");
    fs::write(
        &path,
        "name: ONE\nwindows: []\n---\nname: TWO\nwindows: []\n",
    )
    .expect("launch config should be written");

    let saved = write_launch_config_file(
        dir.path(),
        "devops.yaml".to_string(),
        config("THREE", "/three"),
        Overwrite::Yes,
    )
    .expect("launch config should be overwritten");

    assert_eq!(saved.document_index, 2);
    let configs = read_configs(&path);
    assert_eq!(configs.len(), 3);
    assert_eq!(configs[2].windows[0], window("/three"));
}

#[test]
fn overwriting_creates_the_file_when_the_name_was_changed_first() {
    let dir = tempfile::tempdir().expect("launch config directory should be created");

    let saved = write_launch_config_file(
        dir.path(),
        "renamed".to_string(),
        config("RENAMED", "/renamed"),
        Overwrite::Yes,
    )
    .expect("launch config should be saved");

    assert_eq!(saved.file_name, "renamed.yaml");
    let configs = read_configs(&dir.path().join("renamed.yaml"));
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].windows[0], window("/renamed"));
}

#[test]
fn saving_rejects_a_file_name_that_is_only_a_suffix() {
    let dir = tempfile::tempdir().expect("launch config directory should be created");

    let error = write_launch_config_file(
        dir.path(),
        ".yaml".to_string(),
        config("DEVOPS", "/devops"),
        Overwrite::No,
    )
    .expect_err("a name that is only a suffix should fail");

    assert!(matches!(error, SaveLaunchConfigError::EmptyFileName));
}
