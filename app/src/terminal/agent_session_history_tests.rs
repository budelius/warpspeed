use std::fs;
use std::time::{Duration, SystemTime};

use super::{describe, pi_candidates, session_id_from_command, vibe_sessions};
use crate::terminal::CLIAgent;

#[test]
fn pi_transcript_supplies_session_identity_directory_and_title() {
    let root = tempfile::tempdir().expect("temporary root should be created");
    let project = root.path().join("--project--");
    fs::create_dir(&project).expect("project directory should be created");
    fs::write(
        project.join("2026-09-28T09-53-21-590Z_filename-id.jsonl"),
        concat!(
            "{\"type\":\"session\",\"id\":\"actual-id\",\"cwd\":\"/work/repo\"}\n",
            "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Fix the reboot bug\"}]}}\n"
        ),
    )
    .expect("transcript should be written");

    let session = describe(
        pi_candidates(root.path())
            .into_iter()
            .next()
            .expect("Pi transcript should be found"),
    );

    assert_eq!(session.agent, CLIAgent::Pi);
    assert_eq!(session.session_id, "actual-id");
    assert_eq!(session.cwd, std::path::Path::new("/work/repo"));
    assert_eq!(session.title.as_deref(), Some("Fix the reboot bug"));
}

#[test]
fn vibe_index_supplies_session_identity_directory_and_title() {
    let root = tempfile::tempdir().expect("temporary root should be created");
    fs::write(
        root.path().join(".session_index.json"),
        r#"{
            "session_directory": {
                "mtime_ns": 123,
                "session_id": "vibe-session-id",
                "cwd": "/work/repo",
                "title": "Fix the reboot bug"
            }
        }"#,
    )
    .expect("session index should be written");

    let session = vibe_sessions(root.path())
        .into_iter()
        .next()
        .expect("Vibe session should be found");

    assert_eq!(session.agent, CLIAgent::Vibe);
    assert_eq!(session.session_id, "vibe-session-id");
    assert_eq!(session.cwd, std::path::Path::new("/work/repo"));
    assert_eq!(session.title.as_deref(), Some("Fix the reboot bug"));
    assert_eq!(
        session.modified,
        SystemTime::UNIX_EPOCH + Duration::from_nanos(123)
    );
}

#[test]
fn resume_commands_expose_their_session_ids() {
    assert_eq!(
        session_id_from_command(CLIAgent::Claude, "claude --resume claude-id"),
        Some("claude-id".to_owned())
    );
    assert_eq!(
        session_id_from_command(CLIAgent::Codex, "codex resume codex-id"),
        Some("codex-id".to_owned())
    );
    assert_eq!(
        session_id_from_command(CLIAgent::Pi, "pi --session pi-id"),
        Some("pi-id".to_owned())
    );
    assert_eq!(
        session_id_from_command(CLIAgent::Vibe, "vibe --resume vibe-id"),
        Some("vibe-id".to_owned())
    );
}

#[test]
fn plain_agent_commands_have_no_session_id() {
    assert_eq!(session_id_from_command(CLIAgent::Claude, "claude"), None);
    assert_eq!(session_id_from_command(CLIAgent::Codex, "codex"), None);
    assert_eq!(session_id_from_command(CLIAgent::Pi, "pi"), None);
}
