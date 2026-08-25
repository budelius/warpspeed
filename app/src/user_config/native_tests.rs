use std::fs;

use serde::Deserialize;

use super::{load_launch_configs, overwrite_launch_config_window_in_dir};
use crate::launch_configs::launch_config::{
    LaunchConfig, LaunchConfigWindowSource, WindowTemplate,
};

fn window(cwd: &str) -> WindowTemplate {
    serde_yaml::from_str(&format!("tabs:\n  - layout:\n      cwd: {cwd}\n"))
        .expect("window template should parse")
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
