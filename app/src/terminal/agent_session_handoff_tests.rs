use command::blocking::Command;
use std::fs;
use std::path::Path;

use chrono::{Local, TimeZone};
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
fn moved_agent_names() {
    assert_eq!(
        moved_agent_name(CLIAgent::Claude),
        Some("staging-claude-moved")
    );
    assert_eq!(moved_agent_name(CLIAgent::Pi), Some("staging-pi-moved"));
    assert_eq!(
        moved_agent_name(CLIAgent::Codex),
        Some("staging-codex-moved")
    );
    assert_eq!(moved_agent_name(CLIAgent::Gemini), None);
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
    fs::write(repo.join("untracked.txt"), "new\n").unwrap();
    let source = claude_source(home.path(), repo, b"{\"line\":1}\n");
    let roots = AgentSessionRoots::under_home(home.path());
    let now = Local.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap();

    let prepared = prepare_handoff_with_roots(&source, &roots, now).unwrap();

    assert_eq!(prepared.agent_name, "staging-claude-moved");
    assert_eq!(prepared.session_file, b"{\"line\":1}\n");
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

    let error = prepare_handoff_with_roots(&source, &roots, now).unwrap_err();

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

    let error = prepare_handoff_with_roots(&source, &roots, now).unwrap_err();

    assert!(
        format!("{error:#}").contains("no `gitea` or `origin` remote"),
        "{error:#}"
    );
}
