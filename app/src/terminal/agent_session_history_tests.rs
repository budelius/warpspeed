use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{
    PastAgentSession, ResolvedPath, UnidentifiedAgent, describe, is_within_known, kimi_sessions,
    match_for_restore, pi_candidates, resolve_folders, vibe_sessions,
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
        canonical_cwd: None,
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

#[test]
fn is_within_matches_the_folder_and_its_subfolders() {
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join(".claude").join("worktrees").join("fix");
    std::fs::create_dir_all(&worktree).unwrap();
    let is_within = |path: &Path, folder: &Path| {
        is_within_known(
            path,
            path.canonicalize().ok().as_deref(),
            &ResolvedPath::new(folder),
        )
    };
    assert!(is_within(root.path(), root.path()));
    assert!(is_within(&worktree, root.path()));
    assert!(!is_within(root.path(), &worktree));
    assert!(!is_within(Path::new("/elsewhere"), root.path()));
}

#[test]
fn nothing_is_within_an_empty_folder_and_an_empty_path_is_within_nothing() {
    let root = tempfile::tempdir().unwrap();
    let empty = ResolvedPath::new(Path::new(""));
    assert!(!is_within_known(root.path(), None, &empty));
    assert!(!is_within_known(Path::new(""), None, &empty));
    assert!(!is_within_known(
        Path::new(""),
        None,
        &ResolvedPath::new(root.path())
    ));
}

#[test]
fn resolving_folders_stores_each_sessions_canonical_folder() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    fs::create_dir(&project).unwrap();
    let mut sessions = vec![
        PastAgentSession {
            cwd: project.join(".").join("..").join("project"),
            ..past(CLIAgent::Claude, "here", "", None, 1)
        },
        past(CLIAgent::Claude, "gone", "/no/such/folder", None, 1),
        past(CLIAgent::Claude, "unknown", "", None, 1),
    ];

    resolve_folders(&mut sessions);

    assert_eq!(sessions[0].canonical_cwd, project.canonicalize().ok());
    assert_eq!(sessions[1].canonical_cwd, None);
    assert_eq!(sessions[2].canonical_cwd, None);
}

#[test]
fn a_known_canonical_folder_decides_membership_without_the_raw_path() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("nested");
    fs::create_dir(&nested).unwrap();
    let folder = ResolvedPath::new(root.path());

    let alias = Path::new("Z:/an/alias/that/does/not/exist");
    assert!(is_within_known(
        alias,
        nested.canonicalize().ok().as_deref(),
        &folder
    ));
    assert!(!is_within_known(alias, None, &folder));
}

#[test]
fn kimi_and_dsh_commands_name_their_session() {
    let id = "session_0b6c1234-ffff-4a4a-8b8b-0123456789ab";
    for (command, expected) in [
        (format!("kimi --session {id}"), Some(id)),
        (format!("kimi -S {id}"), Some(id)),
        (format!("kimi -r {id}"), Some(id)),
        (format!("kimi --resume {id} --yolo"), Some(id)),
        (format!("kimi --session={id}"), Some(id)),
        ("kimi -c".to_owned(), None),
        ("kimi --session".to_owned(), None),
        ("kimi --session --yolo".to_owned(), None),
        ("kimi".to_owned(), None),
    ] {
        assert_eq!(
            super::session_id_from_command(CLIAgent::Kimi, &command).as_deref(),
            expected,
            "{command}"
        );
    }
    assert_eq!(
        super::session_id_from_command(
            CLIAgent::DeepSeekHarness,
            "dsh --profile headless --session-id session-0b6c1234-ffff-4a4a-8b8b-0123456789ab \"go on\"",
        )
        .as_deref(),
        Some("session-0b6c1234-ffff-4a4a-8b8b-0123456789ab")
    );
    assert_eq!(
        super::session_id_from_command(CLIAgent::DeepSeekHarness, "dsh --profile headless \"hi\""),
        None
    );
}

fn write_file(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

const KIMI_ID: &str = "session_0b6c1234-ffff-4a4a-8b8b-0123456789ab";

#[test]
fn kimi_index_supplies_the_directory_and_state_the_details() {
    let home = tempfile::tempdir().unwrap();
    let dir = home
        .path()
        .join("sessions")
        .join("wd_proj_4045ff9b01fe")
        .join(KIMI_ID);
    write_file(
        &dir.join("state.json"),
        &format!(
            r#"{{"id":"{KIMI_ID}","version":2,"cwd":"C:/Users/me/proj","createdAt":1788240000000,"updatedAt":1788240666255,"archived":false,"agents":{{}},"custom":{{}},"title":"Fix the reboot bug","titleKind":"generated","lastPrompt":"and the tests too","isCustomTitle":false}}"#
        ),
    );
    let line =
        serde_json::json!({"sessionId": KIMI_ID, "sessionDir": dir, "workDir": "C:/Users/me/proj"});
    write_file(
        &home.path().join("session_index.jsonl"),
        &format!("{line}\n"),
    );

    let sessions = kimi_sessions(home.path());

    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.agent, CLIAgent::Kimi);
    assert_eq!(session.session_id, KIMI_ID);
    assert_eq!(session.cwd, Path::new("C:/Users/me/proj"));
    assert_eq!(session.title.as_deref(), Some("Fix the reboot bug"));
    assert_eq!(
        session.modified,
        SystemTime::UNIX_EPOCH + Duration::from_millis(1_788_240_666_255)
    );
}

#[test]
fn kimi_sessions_are_found_without_an_index() {
    let home = tempfile::tempdir().unwrap();
    write_file(
        &home
            .path()
            .join("sessions")
            .join("wd_my-project_123dafdbc83f")
            .join(KIMI_ID)
            .join("state.json"),
        &format!(
            r#"{{"id":"{KIMI_ID}","version":2,"workDir":"/home/me/my project","createdAt":"2026-09-01T09:00:00.000Z","updatedAt":"2026-09-01T10:00:00.000Z","archived":false,"lastPrompt":"Write the release notes"}}"#
        ),
    );

    let sessions = kimi_sessions(home.path());

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].cwd, Path::new("/home/me/my project"));
    assert_eq!(
        sessions[0].title.as_deref(),
        Some("Write the release notes")
    );
    assert_eq!(
        sessions[0].modified,
        SystemTime::UNIX_EPOCH + Duration::from_millis(1_788_256_800_000)
    );
}

#[test]
fn kimi_archived_and_unreadable_sessions_are_skipped() {
    let home = tempfile::tempdir().unwrap();
    let workspace = home.path().join("sessions").join("wd_proj_4045ff9b01fe");
    write_file(
        &workspace.join("session_archived").join("state.json"),
        r#"{"id":"session_archived","version":2,"cwd":"/p","updatedAt":1,"archived":true}"#,
    );
    write_file(&workspace.join("session_broken").join("state.json"), "{");
    fs::create_dir_all(workspace.join("session_empty")).unwrap();

    assert!(kimi_sessions(home.path()).is_empty());
}
