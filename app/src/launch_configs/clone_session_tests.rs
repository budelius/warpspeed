use std::path::{Path, PathBuf};

use super::{clone_config_file, free_identity};

fn write_config(dir: &Path, file_stem: &str, id: &str, name: &str) -> PathBuf {
    let path = dir.join(format!("{file_stem}.toml"));
    std::fs::write(
        &path,
        format!(
            "# keep me\nversion = 1\nid = \"{id}\"\nname = \"{name}\"\ngeneration = 3\nactive_tab = \"shield\"\n\n[[tabs]]\nid = \"shield\"\nconfig = \"sessions/OBACHAN/05-shield.toml\"\n"
        ),
    )
    .expect("the config should be written");
    path
}

#[test]
fn the_first_free_identity_is_suffixed_with_two() {
    let dir = tempfile::tempdir().expect("config directory should be created");
    write_config(dir.path(), "OBACHAN", "obachan", "OBACHAN");

    let identity = free_identity(dir.path(), "OBACHAN").expect("an identity should be free");

    assert_eq!(identity.file_stem, "OBACHAN-2");
    assert_eq!(identity.id, "obachan-2");
    assert_eq!(identity.name, "OBACHAN (2)");
}

#[test]
fn a_taken_identity_advances_to_the_next_free_one() {
    let dir = tempfile::tempdir().expect("config directory should be created");
    write_config(dir.path(), "OBACHAN", "obachan", "OBACHAN");
    write_config(dir.path(), "OBACHAN-2", "obachan-2", "OBACHAN (2)");

    let identity = free_identity(dir.path(), "OBACHAN").expect("an identity should be free");

    assert_eq!(identity.file_stem, "OBACHAN-3");
    assert_eq!(identity.id, "obachan-3");
    assert_eq!(identity.name, "OBACHAN (3)");
}

#[test]
fn copying_rewrites_the_identity_and_keeps_everything_else() {
    let dir = tempfile::tempdir().expect("config directory should be created");
    let from = write_config(dir.path(), "OBACHAN", "obachan", "OBACHAN");
    let identity = free_identity(dir.path(), "OBACHAN").expect("an identity should be free");

    let path = clone_config_file(&from, dir.path(), &identity).expect("the copy should succeed");

    let contents = std::fs::read_to_string(&path).expect("the copy should be readable");
    assert_eq!(path, dir.path().join("OBACHAN-2.toml"));
    assert!(contents.contains("id = \"obachan-2\""));
    assert!(contents.contains("name = \"OBACHAN (2)\""));
    assert!(contents.contains("generation = 1"));
    assert!(contents.contains("active_tab = \"shield\""));
    assert!(contents.contains("config = \"sessions/OBACHAN/05-shield.toml\""));
    assert!(contents.contains("# keep me"));
}
