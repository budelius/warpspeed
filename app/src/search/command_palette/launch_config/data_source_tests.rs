use std::path::PathBuf;

use super::*;
use crate::launch_configs::launch_config::LaunchConfigSource;

fn config_named(name: &str, path: &str) -> LaunchConfig {
    LaunchConfig {
        name: name.to_owned(),
        active_window_index: Some(0),
        windows: Vec::new(),
        source: Some(LaunchConfigSource {
            path: PathBuf::from(path),
            document_index: 1,
        }),
    }
}

#[test]
fn a_yaml_and_a_session_config_sharing_a_name_are_both_searchable() {
    let searcher = FuzzyLaunchConfigSearcher {
        configs: [
            config_named("OBACHAN", "/cfg/OBACHAN.yaml"),
            config_named("OBACHAN", "/cfg/OBACHAN.toml"),
        ]
        .into_iter()
        .map(|config| (super::config_key(&config), config))
        .collect(),
    };

    let results = searcher.search("obachan").expect("search should succeed");

    assert_eq!(results.len(), 2);
}
