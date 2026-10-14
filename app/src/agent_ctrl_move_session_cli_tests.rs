use std::path::PathBuf;
use std::time::SystemTime;

use clap::Parser as _;

use super::*;

fn past(agent: CLIAgent, session_id: &str, cwd: &str) -> PastAgentSession {
    PastAgentSession {
        agent,
        session_id: session_id.to_owned(),
        cwd: PathBuf::from(cwd),
        canonical_cwd: None,
        title: None,
        last_prompt: None,
        modified: SystemTime::UNIX_EPOCH,
        preview: Vec::new(),
    }
}

#[test]
fn only_dsh_sessions_are_moved_without_a_pane() {
    assert_eq!(paneless_agent("dsh"), Some(CLIAgent::DeepSeekHarness));
    for label in ["kimi", "claude", "pi", "codex", ""] {
        assert_eq!(paneless_agent(label), None, "{label}");
    }
}

#[test]
fn the_recorded_session_supplies_the_directory() {
    let history = [
        past(CLIAgent::DeepSeekHarness, "other", "/x"),
        past(CLIAgent::DeepSeekHarness, "0b6c", r"T:\src\app"),
    ];
    let source = recorded_source(CLIAgent::DeepSeekHarness, "0b6c", &history).unwrap();
    assert_eq!(source.cwd, PathBuf::from(r"T:\src\app"));
    assert_eq!(source.session_id, "0b6c");
    assert_eq!(source.transcript_path, None);

    let error = recorded_source(CLIAgent::DeepSeekHarness, "missing", &history).unwrap_err();
    assert!(error.to_string().contains("missing"), "{error}");
}

#[test]
fn move_session_is_the_only_command_this_module_runs() {
    let parse = |args: &[&str]| {
        let args =
            warp_cli::Args::try_parse_from(["warpspeed", "agent-ctrl"].iter().chain(args).copied())
                .unwrap();
        let Some(warp_cli::Command::AgentCtrl(managed)) = args.command() else {
            panic!("expected agent-ctrl");
        };
        managed.clone()
    };
    let request = move_session_request(&parse(&[
        "move-session",
        "--agent",
        "dsh",
        "--session",
        "0b6c",
    ]))
    .expect("move-session reaches this module");
    assert_eq!(request.agent, "dsh");
    assert_eq!(request.session_id, "0b6c");
    assert!(move_session_request(&parse(&["repos"])).is_none());
}
