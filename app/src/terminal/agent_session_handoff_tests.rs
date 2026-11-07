use std::fs;
use std::path::Path;

use chrono::{Local, TimeZone};
use command::blocking::Command;
use managed_agent_sessions::MAX_SESSION_FILE_BYTES;
use tempfile::TempDir;

use super::*;

const SESSION_ID: &str = "0b6c1234-ffff-4a4a-8b8b-0123456789ab";

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8 git output")
}

fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "Handoff Test"]);
    git(dir, &["config", "user.email", "handoff@example.invalid"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    fs::write(dir.join("tracked.txt"), "one\n").unwrap();
    fs::write(dir.join("staged.txt"), "one\n").unwrap();
    fs::write(dir.join(".gitignore"), "ignored.log\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "initial"]);
}

fn write_file(path: &Path, content: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn agent_ctrl_profiles() {
    assert_eq!(
        agent_ctrl_profile(CLIAgent::Claude),
        Some("staging-claude-interactive-rw")
    );
    assert_eq!(
        agent_ctrl_profile(CLIAgent::Pi),
        Some("staging-pi-interactive-rw")
    );
    assert_eq!(
        agent_ctrl_profile(CLIAgent::Codex),
        Some("staging-codex-interactive-rw")
    );
    assert_eq!(
        agent_ctrl_profile(CLIAgent::Kimi),
        Some("staging-kimi-interactive-rw")
    );
    assert_eq!(
        agent_ctrl_profile(CLIAgent::DeepSeekHarness),
        Some("staging-dsh-interactive-rw")
    );
    assert_eq!(agent_ctrl_profile(CLIAgent::Gemini), None);
}

#[test]
fn claude_project_slug_replaces_every_non_alphanumeric() {
    assert_eq!(
        claude_project_slug(Path::new(
            r"C:\Users\budel\AppData\Local\Programs\WarpSpeed"
        )),
        "C--Users-budel-AppData-Local-Programs-WarpSpeed"
    );
    assert_eq!(
        claude_project_slug(Path::new(
            r"T:\projects\SOTZ3N\stoz3n-agent-dash\.claude\worktrees\x"
        )),
        "T--projects-SOTZ3N-stoz3n-agent-dash--claude-worktrees-x"
    );
    assert_eq!(
        claude_project_slug(Path::new("/home/ben/src/my_app.rs")),
        "-home-ben-src-my-app-rs"
    );
}

#[test]
fn claude_session_path_uses_project_slug() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    for (cwd, slug) in [
        (r"T:\projects\dev-ops\warp", "T--projects-dev-ops-warp"),
        ("/home/ben/src/foo.bar", "-home-ben-src-foo-bar"),
    ] {
        let expected = home
            .path()
            .join(".claude")
            .join("projects")
            .join(slug)
            .join(format!("{SESSION_ID}.jsonl"));
        write_file(&expected, b"{}\n");
        let found =
            session_file_path(CLIAgent::Claude, SESSION_ID, Path::new(cwd), &roots).unwrap();
        assert_eq!(found, expected);
    }
}

#[test]
fn claude_session_path_honors_resolved_config_dir() {
    let home = TempDir::new().unwrap();
    let config = TempDir::new().unwrap();
    let roots = AgentSessionRoots {
        claude_config: config.path().to_path_buf(),
        ..AgentSessionRoots::under_home(home.path())
    };
    let expected = config
        .path()
        .join("projects")
        .join("-srv-app")
        .join(format!("{SESSION_ID}.jsonl"));
    write_file(&expected, b"{}\n");
    let found =
        session_file_path(CLIAgent::Claude, SESSION_ID, Path::new("/srv/app"), &roots).unwrap();
    assert_eq!(found, expected);
}

#[test]
fn pi_session_dir_slug_matches_observed_layout() {
    assert_eq!(
        pi_session_dir_name(Path::new(r"T:\projects\dev-ops")),
        "--T--projects-dev-ops--"
    );
    assert_eq!(
        pi_session_dir_name(Path::new(r"C:\Users\budel")),
        "--C--Users-budel--"
    );
    assert_eq!(
        pi_session_dir_name(Path::new("/home/ben/src")),
        "--home-ben-src--"
    );
}

#[test]
fn pi_session_path_matches_id_in_project_dir() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    let cwd = Path::new(r"T:\projects\dev-ops");
    let project = home
        .path()
        .join(".pi")
        .join("agent")
        .join("sessions")
        .join("--T--projects-dev-ops--");
    write_file(
        &project.join("2026-10-02T09-00-00-000Z_11111111-2222-3333-4444-555555555555.jsonl"),
        b"{}\n",
    );
    let expected = project.join(format!("2026-10-03T10-00-00-000Z_{SESSION_ID}.jsonl"));
    write_file(&expected, b"{}\n");

    let found = session_file_path(CLIAgent::Pi, SESSION_ID, cwd, &roots).unwrap();
    assert_eq!(found, expected);

    let missing = session_file_path(
        CLIAgent::Pi,
        "99999999-0000-0000-0000-000000000000",
        cwd,
        &roots,
    )
    .unwrap_err();
    assert!(is_missing_session_file(&missing));
    assert!(
        format!("{missing:#}").contains("no pi session file"),
        "{missing:#}"
    );
}

#[test]
fn pi_session_path_honors_session_dir_override() {
    let home = TempDir::new().unwrap();
    let sessions = TempDir::new().unwrap();
    let roots = AgentSessionRoots {
        pi_session_dir: Some(sessions.path().to_path_buf()),
        ..AgentSessionRoots::under_home(home.path())
    };
    let expected = sessions
        .path()
        .join(format!("2026-10-03T10-00-00-000Z_{SESSION_ID}.jsonl"));
    write_file(&expected, b"{}\n");
    let found = session_file_path(CLIAgent::Pi, SESSION_ID, Path::new("/srv/app"), &roots).unwrap();
    assert_eq!(found, expected);
}

#[test]
fn codex_session_path_found_by_id_anywhere_under_sessions() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    let sessions = home.path().join(".codex").join("sessions");
    write_file(
        &sessions.join(
            "2026/06/29/rollout-2026-06-29T08-00-00-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.jsonl",
        ),
        b"{}\n",
    );
    let expected = sessions.join(format!(
        "2026/06/30/rollout-2026-06-30T12-53-51-{SESSION_ID}.jsonl"
    ));
    write_file(&expected, b"{}\n");
    let found =
        session_file_path(CLIAgent::Codex, SESSION_ID, Path::new("/anywhere"), &roots).unwrap();
    assert_eq!(found, expected);
}

#[test]
fn session_file_path_rejects_unsafe_ids_and_unsupported_agents() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    for id in ["", "../escape", "a/b", &"a".repeat(129)] {
        assert!(session_file_path(CLIAgent::Claude, id, Path::new("/x"), &roots).is_err());
    }
    assert!(session_file_path(CLIAgent::Gemini, SESSION_ID, Path::new("/x"), &roots).is_err());
}

#[test]
fn handoff_branch_name_shape() {
    let now = Local.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap();
    let branch = handoff_branch_name(now, SESSION_ID);
    assert_eq!(branch, "handoff/20261003-1200-0b6c1234");

    let name = branch.strip_prefix("handoff/").unwrap();
    assert!((1..=100).contains(&name.len()));
    assert!(
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    );
    assert!(!branch.contains(".."));
}

#[test]
fn handoff_commit_includes_untracked_and_leaves_repo_untouched() {
    let repo = TempDir::new().unwrap();
    let repo = repo.path();
    init_repo(repo);
    fs::write(repo.join("tracked.txt"), "two\n").unwrap();
    fs::write(repo.join("untracked.txt"), "new\n").unwrap();
    fs::write(repo.join("ignored.log"), "noise\n").unwrap();
    fs::write(repo.join("staged.txt"), "staged\n").unwrap();
    git(repo, &["add", "staged.txt"]);

    let snapshot = |repo: &Path| {
        (
            git(repo, &["status", "--porcelain=v1"]),
            git(repo, &["rev-parse", "HEAD"]),
            git(repo, &["diff", "--cached"]),
            git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]),
        )
    };
    let before = snapshot(repo);

    let sha = create_handoff_commit(repo, SESSION_ID).unwrap();

    assert_eq!(snapshot(repo), before);
    let files = git(repo, &["show", "--name-only", "--format=", &sha]);
    let files = files.lines().collect::<Vec<_>>();
    assert!(files.contains(&"tracked.txt"), "{files:?}");
    assert!(files.contains(&"untracked.txt"), "{files:?}");
    assert!(files.contains(&"staged.txt"), "{files:?}");
    assert!(!files.contains(&"ignored.log"), "{files:?}");
    assert_eq!(
        git(repo, &["rev-parse", &format!("{sha}^")]),
        before.1,
        "handoff commit's parent is HEAD"
    );
    assert_eq!(
        git(repo, &["log", "-1", "--format=%s", &sha]).trim(),
        format!("WarpSpeed handoff of {SESSION_ID}")
    );
}

fn claude_source(home: &Path, repo: &Path, content: &[u8]) -> HandoffSource {
    let roots = AgentSessionRoots::under_home(home);
    write_file(
        &roots
            .claude_config
            .join("projects")
            .join(claude_project_slug(repo))
            .join(format!("{SESSION_ID}.jsonl")),
        content,
    );
    HandoffSource {
        agent: CLIAgent::Claude,
        session_id: SESSION_ID.to_owned(),
        cwd: repo.to_path_buf(),
        transcript_path: None,
    }
}

fn add_bare_remote(repo: &Path, name: &str, root: &Path) -> String {
    let bare = root.join(format!("{name}.git"));
    git(root, &["init", "-q", "--bare", bare.to_str().unwrap()]);
    let url = bare.to_string_lossy().into_owned();
    git(repo, &["remote", "add", name, &url]);
    url
}

#[test]
fn prepare_handoff_pushes_to_gitea_when_present() {
    let home = TempDir::new().unwrap();
    let remotes = TempDir::new().unwrap();
    let repo = TempDir::new().unwrap();
    let repo = repo.path();
    init_repo(repo);
    add_bare_remote(repo, "origin", remotes.path());
    let gitea_url = add_bare_remote(repo, "gitea", remotes.path());
    // A signing setup that cannot work must not block the handoff commit.
    git(repo, &["config", "commit.gpgsign", "true"]);
    git(
        repo,
        &[
            "config",
            "gpg.program",
            "warpspeed-handoff-test-missing-gpg",
        ],
    );
    fs::write(repo.join("untracked.txt"), "new\n").unwrap();
    let source = claude_source(home.path(), repo, b"{\"line\":1}\n");
    let roots = AgentSessionRoots::under_home(home.path());
    let now = Local.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap();

    let prepared = prepare_handoff_with_roots(&source, &roots, now).unwrap();

    assert_eq!(prepared.agent_name, "staging-claude-interactive-rw");
    assert_eq!(prepared.session_file, b"{\"line\":1}\n");
    assert_eq!(
        prepared.session_file_path,
        session_file_path(source.agent, &source.session_id, &source.cwd, &roots).unwrap()
    );
    assert_eq!(prepared.remote, "gitea");
    assert_eq!(prepared.repo_url, gitea_url);
    assert_eq!(prepared.branch, "handoff/20261003-1200-0b6c1234");
    let pushed = git(
        &remotes.path().join("gitea.git"),
        &["rev-parse", "refs/heads/handoff/20261003-1200-0b6c1234"],
    );
    assert_eq!(pushed.trim(), prepared.commit);
    let origin_refs = git(&remotes.path().join("origin.git"), &["for-each-ref"]);
    assert!(origin_refs.trim().is_empty(), "{origin_refs}");
}

#[test]
fn prepare_handoff_falls_back_to_origin() {
    let home = TempDir::new().unwrap();
    let remotes = TempDir::new().unwrap();
    let repo = TempDir::new().unwrap();
    let repo = repo.path();
    init_repo(repo);
    let origin_url = add_bare_remote(repo, "origin", remotes.path());
    let source = claude_source(home.path(), repo, b"{}\n");
    let roots = AgentSessionRoots::under_home(home.path());
    let now = Local.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap();

    let prepared = prepare_handoff_with_roots(&source, &roots, now).unwrap();

    assert_eq!(prepared.remote, "origin");
    assert_eq!(prepared.repo_url, origin_url);
}

#[test]
fn prepare_handoff_refuses_oversized_file_before_any_git_work() {
    let home = TempDir::new().unwrap();
    let remotes = TempDir::new().unwrap();
    let repo = TempDir::new().unwrap();
    let repo = repo.path();
    init_repo(repo);
    add_bare_remote(repo, "gitea", remotes.path());
    let source = claude_source(home.path(), repo, &vec![b'x'; MAX_SESSION_FILE_BYTES + 1]);
    let roots = AgentSessionRoots::under_home(home.path());
    let now = Local.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap();

    let Err(error) = prepare_handoff_with_roots(&source, &roots, now) else {
        panic!("prepare_handoff must fail");
    };

    assert!(format!("{error:#}").contains("too large"), "{error:#}");
    let refs = git(&remotes.path().join("gitea.git"), &["for-each-ref"]);
    assert!(refs.trim().is_empty(), "{refs}");
}

#[test]
fn prepare_handoff_without_remote_fails() {
    let home = TempDir::new().unwrap();
    let repo = TempDir::new().unwrap();
    let repo = repo.path();
    init_repo(repo);
    let source = claude_source(home.path(), repo, b"{}\n");
    let roots = AgentSessionRoots::under_home(home.path());
    let now = Local.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap();

    let Err(error) = prepare_handoff_with_roots(&source, &roots, now) else {
        panic!("prepare_handoff must fail");
    };

    assert!(
        format!("{error:#}").contains("no `gitea`, `origin`, or `gitlab` remote"),
        "{error:#}"
    );
}

#[test]
fn move_session_failure_before_cut_over_says_the_local_session_is_still_running() {
    let steps = [
        MoveStep::Preparing,
        MoveStep::Importing,
        MoveStep::WaitingForResume {
            job_id: "job-1".to_owned(),
        },
    ];
    for step in &steps {
        let message = move_failure_message(step, "boom", None);
        assert!(
            message.contains("local session is still running"),
            "{step:?}: {message}"
        );
        assert!(message.contains("boom"), "{step:?}: {message}");
        assert!(!message.contains("handoff/"), "{step:?}: {message}");

        let message =
            move_failure_message(step, "boom", Some("gitea/handoff/20261003-1200-0b6c1234"));
        assert!(
            message.contains("local session is still running"),
            "{step:?}: {message}"
        );
        assert!(
            message.contains("gitea/handoff/20261003-1200-0b6c1234"),
            "{step:?}: {message}"
        );
    }
}

#[test]
fn move_session_failure_during_cut_over_asks_to_close_the_agent_manually() {
    let message = move_failure_message(
        &MoveStep::CuttingOver {
            job_id: "job-1".to_owned(),
            pushed_branch: Some("gitea/handoff/x".to_owned()),
        },
        "timed out",
        Some("gitea/handoff/x"),
    );
    assert!(
        message.contains("local agent did not exit; close it manually"),
        "{message}"
    );
    assert!(message.contains("job-1"), "{message}");
    assert!(
        message
            .contains("the cloud run job-1 is live; open it with: warpspeed agent-ctrl open job-1"),
        "{message}"
    );
    assert!(!message.contains("still running"), "{message}");
}

#[test]
fn move_session_failure_while_waiting_leaves_the_stop_outcome_to_the_stop_request() {
    let message = move_failure_message(
        &MoveStep::WaitingForResume {
            job_id: "job-1".to_owned(),
        },
        "did not resume within 120s",
        None,
    );
    assert!(message.contains("waiting for job-1 to resume"), "{message}");
    assert!(message.contains("still running"), "{message}");
    assert!(!message.contains("stop"), "{message}");
}

#[test]
fn move_session_stop_outcome_says_whether_the_stop_was_requested() {
    let asked = stop_request_outcome("job-1", None);
    assert!(asked.contains("run job-1 was asked to stop"), "{asked}");

    let failed = stop_request_outcome(
        "job-1",
        Some("dashboard returned 401 Unauthorized: https://ben:s3cret@dash.example/x"),
    );
    assert!(
        failed.contains("could not ask run job-1 to stop (dashboard returned 401 Unauthorized"),
        "{failed}"
    );
    assert!(
        failed.contains("stop it with: warpspeed agent-ctrl stop job-1"),
        "{failed}"
    );
    assert!(!failed.contains("was asked to stop"), "{failed}");
    assert!(!failed.contains("s3cret"), "{failed}");
}

#[test]
fn move_session_changed_message_names_the_branch() {
    let message = session_changed_message(Some("gitea/handoff/x"));
    assert!(
        message.contains("local session changed during the move"),
        "{message}"
    );
    assert!(!message.contains("stop"), "{message}");
    assert!(message.contains("gitea/handoff/x"), "{message}");
}

#[test]
fn move_session_repo_url_drops_userinfo() {
    for (configured, sent) in [
        (
            "https://bob:t0ken@git.example.com/org/repo.git",
            "https://git.example.com/org/repo.git",
        ),
        (
            "https://bob@git.example.com/org/repo.git",
            "https://git.example.com/org/repo.git",
        ),
        (
            "ssh://git@git.example.com:2222/org/repo.git",
            "ssh://git.example.com:2222/org/repo.git",
        ),
        (
            "https://git.example.com/org/repo.git",
            "https://git.example.com/org/repo.git",
        ),
        (
            "git@git.example.com:org/repo.git",
            "git@git.example.com:org/repo.git",
        ),
        ("/srv/git/repo.git", "/srv/git/repo.git"),
        (r"C:\repos\repo.git", r"C:\repos\repo.git"),
    ] {
        assert_eq!(strip_url_userinfo(configured), sent, "{configured}");
    }
}

#[test]
fn move_session_cut_over_exits_only_the_unchanged_moved_session() {
    let moved = SESSION_ID;
    let cases = [
        (
            "same session, unchanged file",
            Some(moved),
            Some(100),
            CutOver::ExitLocalAgent,
        ),
        (
            "another session in the pane",
            Some("other-session"),
            Some(100),
            CutOver::LeaveLocalAgent,
        ),
        (
            "no session in the pane",
            None,
            Some(100),
            CutOver::LeaveLocalAgent,
        ),
        (
            "same session, file grew",
            Some(moved),
            Some(101),
            CutOver::SessionChanged,
        ),
        (
            "another session, moved file grew",
            Some("other-session"),
            Some(150),
            CutOver::SessionChanged,
        ),
        (
            "session file unreadable",
            Some(moved),
            None,
            CutOver::SessionChanged,
        ),
    ];
    for (description, current_id, current_len, expected) in cases {
        assert_eq!(
            cut_over_decision(current_id, moved, current_len, 100),
            expected,
            "{description}"
        );
    }
}

#[test]
fn move_session_failure_message_strips_credentials_from_urls() {
    let message = move_failure_message(
        &MoveStep::Preparing,
        "git push failed: fatal: unable to access 'https://bob:s3cret@git.example.com/a/b.git/': 403",
        None,
    );
    assert!(!message.contains("s3cret"), "{message}");
    assert!(!message.contains("bob"), "{message}");
    assert!(
        message.contains("https://git.example.com/a/b.git/"),
        "{message}"
    );
}

#[test]
fn move_session_idempotency_key_matches_the_server_pattern() {
    let commit = "0123456789abcdef0123456789abcdef01234567";
    let key = move_idempotency_key(SESSION_ID, commit);
    assert_eq!(key, format!("move-{SESSION_ID}-0123456789ab"));

    let long_id = "a".repeat(128);
    let key = move_idempotency_key(&long_id, commit);
    assert!(key.len() <= 128, "{}", key.len());
    assert!(key.len() >= 16);
    assert!(
        key.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-')),
        "{key}"
    );
    assert_eq!(key, format!("move-{}-0123456789ab", "a".repeat(100)));
}

#[test]
fn move_session_exit_command_matches_each_agent() {
    assert_eq!(agent_exit_command(CLIAgent::Claude), Some("/exit"));
    assert_eq!(agent_exit_command(CLIAgent::Codex), Some("/exit"));
    assert_eq!(agent_exit_command(CLIAgent::Kimi), Some("/exit"));
    assert_eq!(agent_exit_command(CLIAgent::Pi), Some("/quit"));
    assert_eq!(agent_exit_command(CLIAgent::DeepSeekHarness), None);
    assert_eq!(agent_exit_command(CLIAgent::Gemini), None);
}

#[test]
fn move_session_file_prefers_the_reported_transcript_path() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    let launch_dir = Path::new("/home/ben/src/app");
    let by_launch_dir = roots
        .claude_config
        .join("projects")
        .join(claude_project_slug(launch_dir))
        .join(format!("{SESSION_ID}.jsonl"));
    write_file(
        &by_launch_dir,
        b"{}
",
    );
    let reported = roots
        .claude_config
        .join("projects")
        .join("-home-ben-src-app-reported")
        .join(format!("{SESSION_ID}.jsonl"));
    let source = |transcript_path: Option<PathBuf>| HandoffSource {
        agent: CLIAgent::Claude,
        session_id: SESSION_ID.to_owned(),
        cwd: launch_dir.to_path_buf(),
        transcript_path,
    };

    // Reported, but the file does not exist: fall back to the shell-cwd lookup.
    assert_eq!(
        resolve_session_file(&source(Some(reported.clone())), &roots).unwrap(),
        by_launch_dir
    );

    write_file(
        &reported, b"{}
",
    );
    assert_eq!(
        resolve_session_file(&source(Some(reported.clone())), &roots).unwrap(),
        reported
    );

    assert_eq!(
        resolve_session_file(&source(None), &roots).unwrap(),
        by_launch_dir
    );
}

#[test]
fn move_session_file_ignores_a_reported_path_that_is_not_this_session() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    let launch_dir = Path::new("/srv/app");
    let by_launch_dir = roots
        .claude_config
        .join("projects")
        .join(claude_project_slug(launch_dir))
        .join(format!("{SESSION_ID}.jsonl"));
    write_file(
        &by_launch_dir,
        b"{}
",
    );
    let other_session = roots
        .claude_config
        .join("projects")
        .join("-srv-app")
        .join("another-session.jsonl");
    write_file(
        &other_session,
        b"{}
",
    );
    let outside_roots = home.path().join(format!("{SESSION_ID}.jsonl"));
    write_file(
        &outside_roots,
        b"secret
",
    );
    let escaping = roots
        .claude_config
        .join("projects")
        .join("..")
        .join("..")
        .join(format!("{SESSION_ID}.jsonl"));

    for reported in [other_session, outside_roots, escaping] {
        let source = HandoffSource {
            agent: CLIAgent::Claude,
            session_id: SESSION_ID.to_owned(),
            cwd: launch_dir.to_path_buf(),
            transcript_path: Some(reported.clone()),
        };
        assert_eq!(
            resolve_session_file(&source, &roots).unwrap(),
            by_launch_dir,
            "{}",
            reported.display()
        );
    }
}

#[test]
fn move_session_progress_message_names_the_agent_when_known() {
    assert_eq!(
        move_progress_message(Some(CLIAgent::Claude)),
        "Moving Claude Code session to AgentCtrl… don't use the agent until this finishes."
    );
    assert_eq!(
        move_progress_message(None),
        "Moving the agent session to AgentCtrl… don't use the agent until this finishes."
    );
}

#[test]
fn repo_url_for_prefers_gitea_and_strips_userinfo() {
    let repo = TempDir::new().unwrap();
    let repo = repo.path();
    init_repo(repo);
    git(
        repo,
        &["remote", "add", "origin", "https://github.com/acme/app.git"],
    );
    git(
        repo,
        &["remote", "add", "gitlab", "https://gitlab.com/acme/app.git"],
    );
    git(
        repo,
        &[
            "remote",
            "add",
            "gitea",
            "https://bot:s3cret@gitea.example.com/acme/app.git",
        ],
    );
    let subdirectory = repo.join("src");
    fs::create_dir(&subdirectory).unwrap();

    assert_eq!(
        repo_url_for(&subdirectory).unwrap(),
        (
            "gitea".to_owned(),
            "https://gitea.example.com/acme/app.git".to_owned()
        )
    );
}

#[test]
fn repo_url_for_falls_back_to_origin() {
    let repo = TempDir::new().unwrap();
    let repo = repo.path();
    init_repo(repo);
    git(
        repo,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );

    assert_eq!(
        repo_url_for(repo).unwrap(),
        (
            "origin".to_owned(),
            "git@github.com:acme/app.git".to_owned()
        )
    );
}

#[test]
fn repo_url_for_prefers_gitlab_over_origin() {
    let repo = TempDir::new().unwrap();
    let repo = repo.path();
    init_repo(repo);
    git(
        repo,
        &["remote", "add", "origin", "https://github.com/acme/app.git"],
    );
    git(
        repo,
        &["remote", "add", "gitlab", "https://gitlab.com/acme/app.git"],
    );

    assert_eq!(
        repo_url_for(repo).unwrap(),
        (
            "gitlab".to_owned(),
            "https://gitlab.com/acme/app.git".to_owned()
        )
    );
}

#[test]
fn repo_url_for_supports_an_explicit_gitlab_remote() {
    let repo = TempDir::new().unwrap();
    let repo = repo.path();
    init_repo(repo);
    git(
        repo,
        &["remote", "add", "gitlab", "https://gitlab.com/acme/app.git"],
    );

    assert_eq!(
        repo_url_for(repo).unwrap(),
        (
            "gitlab".to_owned(),
            "https://gitlab.com/acme/app.git".to_owned()
        )
    );
}

#[test]
fn repo_url_for_fails_without_a_usable_remote() {
    let repo = TempDir::new().unwrap();
    init_repo(repo.path());
    let not_a_repo = TempDir::new().unwrap();

    assert!(repo_url_for(repo.path()).is_err());
    assert!(repo_url_for(not_a_repo.path()).is_err());
}

fn repository(name: &str, url: &str, branch: &str) -> RepositoryInfo {
    RepositoryInfo {
        name: name.to_owned(),
        url: url.to_owned(),
        branch: branch.to_owned(),
        reachable: true,
        error: None,
    }
}

#[test]
fn start_agent_on_agent_ctrl_finds_the_repository_by_normalized_url() {
    let repositories = [
        repository("other", "https://gitea.example.com/acme/other", "main"),
        repository("app", "https://Gitea.Example.com/acme/app", "dev"),
    ];

    for local_url in [
        "https://gitea.example.com/acme/app.git",
        "https://gitea.example.com/acme/app/",
        "git@gitea.example.com:acme/app.git",
    ] {
        assert_eq!(
            configured_repository(local_url, &repositories).map(|repository| &repository.name),
            Some(&"app".to_owned()),
            "{local_url}"
        );
    }
    assert_eq!(
        configured_repository("https://gitea.example.com/acme/missing.git", &repositories),
        None
    );
}

#[test]
fn start_agent_on_agent_ctrl_prompt_names_the_remote_repo_and_branch() {
    assert_eq!(
        start_agent_prompt(
            "gitea",
            &repository("warp", "https://gitea.example.com/acme/warp", "dev")
        ),
        "Start which agent on gitea warp (dev)?"
    );
}

#[test]
fn start_agent_on_agent_ctrl_failure_says_when_the_node_is_at_capacity() {
    let at_capacity = anyhow::Error::new(managed_agent_sessions::HttpStatusError {
        status: reqwest::StatusCode::CONFLICT,
        body: r#"{"code":"node_at_capacity","message":"too many runs"}"#.to_owned(),
    })
    .context("failed to launch the run");
    assert_eq!(
        start_agent_failure_message(&at_capacity),
        "AgentCtrl node is at capacity; stop a run or try later"
    );

    let other = anyhow!(
        "dashboard request failed: https://bob:s3cret@dash.example.com/api/agent-control/runs"
    );
    let message = start_agent_failure_message(&other);
    assert!(!message.contains("s3cret"), "{message}");
    assert!(!message.contains("bob"), "{message}");
    assert!(
        message.contains("https://dash.example.com/api/agent-control/runs"),
        "{message}"
    );
}

const KIMI_SESSION_ID: &str = "session_0b6c1234-ffff-4a4a-8b8b-0123456789ab";

#[test]
fn kimi_and_dsh_session_ids_are_valid_but_path_characters_are_not() {
    for id in [
        KIMI_SESSION_ID,
        "session-0b6c1234-ffff-4a4a-8b8b-0123456789ab",
        SESSION_ID,
    ] {
        assert!(is_valid_session_id(id), "{id}");
    }
    for id in ["", "a.b", "../x", r"a\b", "a/b", &"a".repeat(129)] {
        assert!(!is_valid_session_id(id), "{id}");
    }
}

#[test]
fn handoff_branch_name_skips_the_session_prefix() {
    let now = Local.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap();
    assert_eq!(
        handoff_branch_name(now, KIMI_SESSION_ID),
        "handoff/20261003-1200-0b6c1234"
    );
    assert_eq!(
        handoff_branch_name(now, "session-0b6c1234-ffff-4a4a-8b8b-0123456789ab"),
        "handoff/20261003-1200-0b6c1234"
    );
    assert_eq!(
        handoff_branch_name(now, "session_"),
        "handoff/20261003-1200-session_"
    );
}

fn kimi_session(roots: &AgentSessionRoots, workspace: &str, id: &str) -> PathBuf {
    let dir = roots.kimi_home.join("sessions").join(workspace).join(id);
    write_file(&dir.join("state.json"), br#"{"id":"x","cwd":"/old"}"#);
    write_file(&dir.join("agents/main/wire.jsonl"), b"{}\n");
    dir
}

#[test]
fn kimi_session_dir_comes_from_the_last_index_line_for_the_id() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    let stale = kimi_session(&roots, "wd_app_aaaaaaaaaaaa", KIMI_SESSION_ID);
    let current = kimi_session(&roots, "wd_app_bbbbbbbbbbbb", KIMI_SESSION_ID);
    let line = |dir: &Path| {
        serde_json::json!({"sessionId": KIMI_SESSION_ID, "sessionDir": dir, "workDir": "/app"})
            .to_string()
    };
    write_file(
        &roots.kimi_home.join("session_index.jsonl"),
        format!(
            "{}\n{{\"sessionId\":\"other\",\"sessionDir\":\"/x\",\"workDir\":\"/x\"}}\n{}\n",
            line(&stale),
            line(&current)
        )
        .as_bytes(),
    );
    let found =
        session_file_path(CLIAgent::Kimi, KIMI_SESSION_ID, Path::new("/app"), &roots).unwrap();
    assert_eq!(found, current);
}

#[test]
fn kimi_session_dir_is_scanned_for_without_an_index_or_with_an_outside_entry() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    let dir = kimi_session(&roots, "wd_app_aaaaaaaaaaaa", KIMI_SESSION_ID);
    assert_eq!(
        session_file_path(CLIAgent::Kimi, KIMI_SESSION_ID, Path::new("/app"), &roots).unwrap(),
        dir
    );

    let outside = home.path().join("elsewhere").join(KIMI_SESSION_ID);
    fs::create_dir_all(&outside).unwrap();
    write_file(
        &roots.kimi_home.join("session_index.jsonl"),
        serde_json::json!({"sessionId": KIMI_SESSION_ID, "sessionDir": outside, "workDir": "/app"})
            .to_string()
            .as_bytes(),
    );
    assert_eq!(
        session_file_path(CLIAgent::Kimi, KIMI_SESSION_ID, Path::new("/app"), &roots).unwrap(),
        dir
    );
    assert!(
        session_file_path(CLIAgent::Kimi, "session_missing", Path::new("/app"), &roots).is_err()
    );
}

#[test]
fn dsh_session_file_is_the_highest_generation_under_any_project() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    let dir = roots
        .dsh_home
        .join("sessions/--C-Users-budel-src-app--")
        .join(SESSION_ID);
    for name in [
        "session.jsonl",
        "session.v3.jsonl.zstd",
        "session.v4.jsonl.zstd",
        "session.v04.jsonl.zstd",
        "session.v9.jsonl.tmp",
    ] {
        write_file(&dir.join(name), b"x");
    }
    let found = session_file_path(
        CLIAgent::DeepSeekHarness,
        SESSION_ID,
        Path::new("/x"),
        &roots,
    )
    .unwrap();
    assert_eq!(found, dir.join("session.v4.jsonl.zstd"));
    assert!(
        session_file_path(
            CLIAgent::DeepSeekHarness,
            "missing",
            Path::new("/x"),
            &roots
        )
        .is_err()
    );
}

#[test]
fn reported_kimi_dir_and_dsh_file_are_trusted_only_under_their_roots() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    let kimi_dir = kimi_session(&roots, "wd_app_aaaaaaaaaaaa", KIMI_SESSION_ID);
    let kimi_source = |transcript_path: PathBuf| HandoffSource {
        agent: CLIAgent::Kimi,
        session_id: KIMI_SESSION_ID.to_owned(),
        cwd: PathBuf::from("/app"),
        transcript_path: Some(transcript_path),
    };
    assert!(is_reported_session_file(
        &kimi_dir,
        &kimi_source(kimi_dir.clone()),
        &roots
    ));
    let other = kimi_session(&roots, "wd_app_aaaaaaaaaaaa", "session_other");
    assert!(!is_reported_session_file(
        &other,
        &kimi_source(other.clone()),
        &roots
    ));
    let escaped = kimi_dir.join("..").join(KIMI_SESSION_ID);
    assert!(!is_reported_session_file(
        &escaped,
        &kimi_source(escaped.clone()),
        &roots
    ));

    let dsh_file = roots
        .dsh_home
        .join("sessions/--app--")
        .join(SESSION_ID)
        .join("session.v4.jsonl.zstd");
    write_file(&dsh_file, b"x");
    let dsh_source = HandoffSource {
        agent: CLIAgent::DeepSeekHarness,
        session_id: SESSION_ID.to_owned(),
        cwd: PathBuf::from("/app"),
        transcript_path: Some(dsh_file.clone()),
    };
    assert!(is_reported_session_file(&dsh_file, &dsh_source, &roots));
    let outside = home.path().join(SESSION_ID).join("session.v4.jsonl.zstd");
    write_file(&outside, b"x");
    assert!(!is_reported_session_file(&outside, &dsh_source, &roots));
}

#[test]
fn session_roots_default_under_home() {
    let roots = AgentSessionRoots::under_home(Path::new("/home/ben"));
    assert_eq!(roots.kimi_home, Path::new("/home/ben/.kimi-code"));
    assert_eq!(roots.dsh_home, Path::new("/home/ben/.dsh"));
}

fn tar_entries(archive: &[u8]) -> Vec<(String, Vec<u8>)> {
    use std::io::Read as _;
    tar::Archive::new(archive)
        .entries()
        .unwrap()
        .map(|entry| {
            let mut entry = entry.unwrap();
            assert_eq!(entry.header().entry_type(), tar::EntryType::Regular);
            let path = entry.path().unwrap().to_string_lossy().replace('\\', "/");
            let mut content = Vec::new();
            entry.read_to_end(&mut content).unwrap();
            (path, content)
        })
        .collect()
}

#[test]
fn kimi_session_is_packed_as_a_tar_of_every_file_under_its_id() {
    let home = TempDir::new().unwrap();
    let roots = AgentSessionRoots::under_home(home.path());
    let dir = kimi_session(&roots, "wd_app_aaaaaaaaaaaa", KIMI_SESSION_ID);
    write_file(&dir.join("agents/sub-1/wire.jsonl"), b"{\"a\":1}\n");
    write_file(&dir.join("session-meta/state.json"), b"{}");

    let payload = pack_kimi_session(&dir, KIMI_SESSION_ID).unwrap();

    let entries = tar_entries(&payload.bytes);
    let names: Vec<_> = entries.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            format!("{KIMI_SESSION_ID}/agents/main/wire.jsonl"),
            format!("{KIMI_SESSION_ID}/agents/sub-1/wire.jsonl"),
            format!("{KIMI_SESSION_ID}/session-meta/state.json"),
            format!("{KIMI_SESSION_ID}/state.json"),
        ]
    );
    assert_eq!(entries[1].1, b"{\"a\":1}\n");
    let total: u64 = entries
        .iter()
        .map(|(_, content)| content.len() as u64)
        .sum();
    assert_eq!(payload.content_len, total);
    assert_eq!(session_content_len(&dir), Some(total));
}

#[test]
fn kimi_session_without_state_or_over_the_limit_is_refused() {
    let home = TempDir::new().unwrap();
    let bare = home.path().join(KIMI_SESSION_ID);
    write_file(&bare.join("agents/main/wire.jsonl"), b"{}\n");
    let error = pack_kimi_session(&bare, KIMI_SESSION_ID).unwrap_err();
    assert!(format!("{error:#}").contains("state.json"), "{error:#}");

    let roots = AgentSessionRoots::under_home(home.path());
    let dir = kimi_session(&roots, "wd_app_aaaaaaaaaaaa", KIMI_SESSION_ID);
    fs::File::create(dir.join("agents/main/big.bin"))
        .unwrap()
        .set_len(MAX_SESSION_FILE_BYTES as u64 + 1)
        .unwrap();
    let error = pack_kimi_session(&dir, KIMI_SESSION_ID).unwrap_err();
    assert!(format!("{error:#}").contains("too large"), "{error:#}");
}

#[test]
fn session_content_len_follows_appends_to_a_file_or_a_directory() {
    let home = TempDir::new().unwrap();
    let file = home.path().join("session.jsonl");
    write_file(&file, b"12345");
    assert_eq!(session_content_len(&file), Some(5));
    let dir = home.path().join("dir");
    write_file(&dir.join("a"), b"12");
    write_file(&dir.join("b/c"), b"345");
    assert_eq!(session_content_len(&dir), Some(5));
    write_file(&dir.join("b/c"), b"3456");
    assert_eq!(session_content_len(&dir), Some(6));
    assert_eq!(session_content_len(&home.path().join("missing")), None);
}

#[test]
fn prepare_handoff_uploads_a_kimi_tar_and_a_dsh_file_as_is() {
    let home = TempDir::new().unwrap();
    let remotes = TempDir::new().unwrap();
    let repo = TempDir::new().unwrap();
    let repo = repo.path();
    init_repo(repo);
    add_bare_remote(repo, "gitea", remotes.path());
    let roots = AgentSessionRoots::under_home(home.path());
    let now = Local.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap();

    let kimi_dir = kimi_session(&roots, "wd_app_aaaaaaaaaaaa", KIMI_SESSION_ID);
    let kimi = HandoffSource {
        agent: CLIAgent::Kimi,
        session_id: KIMI_SESSION_ID.to_owned(),
        cwd: repo.to_path_buf(),
        transcript_path: None,
    };
    let prepared = prepare_handoff_with_roots(&kimi, &roots, now).unwrap();
    assert_eq!(prepared.agent_name, "staging-kimi-interactive-rw");
    assert_eq!(prepared.session_file_path, kimi_dir);
    assert_eq!(
        prepared.session_len,
        session_content_len(&kimi_dir).unwrap()
    );
    assert!(
        tar_entries(&prepared.session_file)
            .iter()
            .any(|(name, _)| *name == format!("{KIMI_SESSION_ID}/state.json"))
    );
    assert_eq!(prepared.branch, "handoff/20261003-1200-0b6c1234");

    let dsh_file = roots
        .dsh_home
        .join("sessions/--app--")
        .join(SESSION_ID)
        .join("session.v4.jsonl.zstd");
    write_file(&dsh_file, b"\x28\xb5\x2f\xfdrest");
    let dsh = HandoffSource {
        agent: CLIAgent::DeepSeekHarness,
        session_id: SESSION_ID.to_owned(),
        cwd: repo.to_path_buf(),
        transcript_path: None,
    };
    // Both ids shorten to `0b6c1234`, so the same minute would reuse the pushed Kimi branch.
    let later = now + chrono::Duration::minutes(1);
    let prepared = prepare_handoff_with_roots(&dsh, &roots, later).unwrap();
    assert_eq!(prepared.agent_name, "staging-dsh-interactive-rw");
    assert_eq!(prepared.session_file, b"\x28\xb5\x2f\xfdrest");
    assert_eq!(prepared.session_len, 8);
}

#[test]
fn blank_agent_homes_resolve_to_the_defaults() {
    let home = Path::new("/home/user");
    let saved = ["DSH_HOME", "KIMI_CODE_HOME"].map(|key| (key, std::env::var_os(key)));
    unsafe { std::env::set_var("DSH_HOME", "  \t") };
    unsafe { std::env::set_var("KIMI_CODE_HOME", "") };

    let roots = AgentSessionRoots::from_env(home);

    for (key, value) in saved {
        match value {
            Some(value) => unsafe { std::env::set_var(key, value) },
            None => unsafe { std::env::remove_var(key) },
        }
    }
    assert_eq!(roots.dsh_home, home.join(".dsh"));
    assert_eq!(roots.kimi_home, home.join(".kimi-code"));
}
