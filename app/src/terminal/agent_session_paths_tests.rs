use std::fs;

use super::kimi_indexed_session_dirs;

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
