use std::fs;

use super::{kimi_indexed_session_dirs, kimi_work_dir_key};

#[test]
fn kimi_index_last_line_wins_and_tombstones_remove() {
    let home = tempfile::tempdir().unwrap();
    let kept = home
        .path()
        .join("sessions")
        .join("wd_proj_4045ff9b01fe")
        .join("session_a");
    let moved = home
        .path()
        .join("sessions")
        .join("wd_other_123dafdbc83f")
        .join("session_b");
    let lines = [
        serde_json::json!({"sessionId": "session_a", "sessionDir": kept, "workDir": "C:/Users/me/proj"}),
        serde_json::json!({"sessionId": "session_b", "sessionDir": "/old/place", "workDir": "/old"}),
        serde_json::json!({"sessionId": "session_b", "sessionDir": moved, "workDir": "/home/me/other"}),
        serde_json::json!({"sessionId": "session_c", "sessionDir": "/gone", "workDir": "/gone"}),
        serde_json::json!({"sessionId": "session_c", "deleted": true}),
        serde_json::json!({"sessionId": "../escape", "sessionDir": "/x", "workDir": "/x"}),
    ];
    let index = lines
        .iter()
        .map(|line| format!("{line}\n"))
        .collect::<String>()
        + "not json\n";
    fs::write(home.path().join("session_index.jsonl"), index).unwrap();

    let mut dirs = kimi_indexed_session_dirs(home.path()).expect("the index exists");
    dirs.sort();
    assert_eq!(
        dirs,
        vec![
            ("session_a".to_owned(), kept),
            ("session_b".to_owned(), moved),
        ]
    );
}

#[test]
fn kimi_index_missing_is_none() {
    let home = tempfile::tempdir().unwrap();
    assert!(kimi_indexed_session_dirs(home.path()).is_none());
}

/// Vectors from Kimi's `encodeWorkDirKey` (`workdir-slug.ts`): sha256 over the path with `\`
/// turned into `/` and trailing `/` removed, case kept; the first one is Kimi's own test.
#[test]
fn kimi_work_dir_key_matches_kimi() {
    for (work_dir, key) in [
        ("/Users/me/Developer/proj", "wd_proj_76bfce3667dc"),
        (r"C:\Users\me\proj", "wd_proj_4045ff9b01fe"),
        (r"c:\users\me\proj", "wd_proj_80e537bce8d6"),
        ("/home/me/my project/", "wd_my-project_123dafdbc83f"),
        ("/", "wd_workspace_e3b0c44298fc"),
    ] {
        assert_eq!(kimi_work_dir_key(work_dir), key, "{work_dir}");
    }
}

#[test]
fn kimi_work_dir_slug_follows_kimi() {
    let slug = |name: &str| {
        let key = kimi_work_dir_key(&format!("/x/{name}"));
        key["wd_".len()..key.len() - "_0123456789ab".len()].to_owned()
    };
    assert_eq!(slug("Some Folder With Spaces"), "some-folder-with-spaces");
    assert_eq!(slug("--Hello__World!!--"), "hello__world");
    assert_eq!(slug(&"a".repeat(50)), "a".repeat(40));
    assert_eq!(slug(&format!("{}-{}", "a".repeat(39), "b")), "a".repeat(39));
    assert_eq!(slug(".."), "workspace");
    assert_eq!(slug("..."), "...");
}
