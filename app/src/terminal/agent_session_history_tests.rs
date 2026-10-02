use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use super::{
    PastAgentSession, UnidentifiedAgent, describe, match_for_restore, pi_candidates, vibe_sessions,
};
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

fn past(
    agent: CLIAgent,
    id: &str,
    cwd: &str,
    title: Option<&str>,
    hours_before: u64,
) -> PastAgentSession {
    PastAgentSession {
        agent,
        session_id: id.to_owned(),
        cwd: PathBuf::from(cwd),
        title: title.map(str::to_owned),
        modified: saved_at() - Duration::from_secs(hours_before * 60 * 60),
    }
}

fn saved_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000)
}

fn unidentified(agent: CLIAgent, cwd: &str, title: Option<&str>) -> UnidentifiedAgent {
    UnidentifiedAgent {
        agent,
        cwd: PathBuf::from(cwd),
        title: title.map(str::to_owned),
    }
}

#[test]
fn restore_picks_the_newest_session_in_the_panes_directory() {
    let candidates = [
        past(CLIAgent::Claude, "older", "/work/a", None, 30),
        past(CLIAgent::Claude, "newer", "/work/a", None, 2),
        past(CLIAgent::Claude, "elsewhere", "/work/b", None, 1),
    ];

    let matches = match_for_restore(
        &[unidentified(CLIAgent::Claude, "/work/a", None)],
        saved_at(),
        &candidates,
        &HashSet::new(),
    );

    assert_eq!(matches, vec![Some("newer".to_owned())]);
}

#[test]
fn restore_never_gives_one_session_to_two_panes_or_reuses_a_held_one() {
    let candidates = [
        past(CLIAgent::Claude, "held", "/work/a", None, 1),
        past(CLIAgent::Claude, "first", "/work/a", None, 2),
        past(CLIAgent::Claude, "second", "/work/a", None, 3),
    ];
    let excluded = HashSet::from(["held".to_owned()]);

    let matches = match_for_restore(
        &[
            unidentified(CLIAgent::Claude, "/work/a", None),
            unidentified(CLIAgent::Claude, "/work/a", None),
            unidentified(CLIAgent::Claude, "/work/a", None),
        ],
        saved_at(),
        &candidates,
        &excluded,
    );

    assert_eq!(
        matches,
        vec![Some("first".to_owned()), Some("second".to_owned()), None]
    );
}

#[test]
fn restore_prefers_the_session_whose_title_the_pane_recorded() {
    let candidates = [
        past(CLIAgent::Claude, "newest", "/work/a", Some("other work"), 1),
        past(
            CLIAgent::Claude,
            "titled",
            "/work/a",
            Some("fix the mail server"),
            5,
        ),
    ];

    let matches = match_for_restore(
        &[
            unidentified(CLIAgent::Claude, "/work/a", None),
            unidentified(CLIAgent::Claude, "/work/a", Some("fix the mail server")),
        ],
        saved_at(),
        &candidates,
        &HashSet::new(),
    );

    assert_eq!(
        matches,
        vec![Some("newest".to_owned()), Some("titled".to_owned())]
    );
}

#[test]
fn restore_ignores_sessions_from_long_before_or_after_the_snapshot_and_other_agents() {
    let mut after = past(CLIAgent::Claude, "after", "/work/a", None, 0);
    after.modified = saved_at() + Duration::from_secs(5 * 60 * 60);
    let candidates = [
        past(CLIAgent::Claude, "ancient", "/work/a", None, 24 * 30),
        after,
        past(CLIAgent::Pi, "pi", "/work/a", None, 1),
    ];

    let matches = match_for_restore(
        &[unidentified(CLIAgent::Claude, "/work/a", None)],
        saved_at(),
        &candidates,
        &HashSet::new(),
    );

    assert_eq!(matches, vec![None]);
}

#[test]
fn resume_commands_expose_their_session_ids() {
    assert_eq!(
        super::session_id_from_command(CLIAgent::Claude, "claude --resume claude-id"),
        Some("claude-id".to_owned())
    );
    assert_eq!(
        super::session_id_from_command(CLIAgent::Codex, "codex resume codex-id"),
        Some("codex-id".to_owned())
    );
    assert_eq!(
        super::session_id_from_command(CLIAgent::Pi, "pi --session pi-id"),
        Some("pi-id".to_owned())
    );
    assert_eq!(
        super::session_id_from_command(CLIAgent::Vibe, "vibe --resume vibe-id"),
        Some("vibe-id".to_owned())
    );
}

#[test]
fn plain_agent_commands_have_no_session_id() {
    assert_eq!(
        super::session_id_from_command(CLIAgent::Claude, "claude"),
        None
    );
    assert_eq!(
        super::session_id_from_command(CLIAgent::Codex, "codex"),
        None
    );
    assert_eq!(super::session_id_from_command(CLIAgent::Pi, "pi"), None);
}
