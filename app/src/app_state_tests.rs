use super::*;

fn descriptor(agent: CLIAgent, session_id: Option<&str>) -> CliAgentResumeDescriptor {
    CliAgentResumeDescriptor {
        agent,
        session_id: session_id.map(str::to_owned),
        cwd: PathBuf::from(r"T:\projects\dev-ops\system"),
        title: None,
        captured_unix_seconds: None,
    }
}

/// Only syntax verified against the shipped CLI belongs here. A guessed flag runs, starts a fresh
/// session, and scrolls away the ID it was supposed to recover.
#[test]
fn agents_with_verified_syntax_generate_a_resume_command() {
    for (agent, expected) in [
        (CLIAgent::Claude, "claude --resume session-1"),
        (CLIAgent::Codex, "codex resume session-1"),
        (CLIAgent::Vibe, "vibe --resume session-1"),
        (CLIAgent::Pi, "pi --session session-1"),
        (CLIAgent::Kimi, "kimi --session session-1"),
    ] {
        assert_eq!(
            descriptor(agent, Some("session-1"))
                .resume_command()
                .as_deref(),
            Some(expected),
            "{} must resume with the syntax its CLI documents",
            agent.display_name()
        );
    }
}

/// DeepSeek Harness ships no interactive terminal UI to reopen a session in.
#[test]
fn deepseek_harness_has_no_resume_command() {
    assert_eq!(
        descriptor(CLIAgent::DeepSeekHarness, Some("session-1")).resume_command(),
        None
    );
}

/// `gemini --resume` takes "latest" or a per-project index, and `opencode session` can list and
/// delete by ID but not open one. Both would otherwise look resumable because Warp has their
/// session ID.
#[test]
fn unsupported_cli_agent_resume_descriptor_does_not_generate_command() {
    assert_eq!(
        descriptor(CLIAgent::Gemini, Some("session-1")).resume_command(),
        None
    );
    assert_eq!(
        descriptor(CLIAgent::OpenCode, Some("session-1")).resume_command(),
        None
    );
}

/// An agent detected from its command line alone never reports a session ID, and there is nothing
/// to resume.
#[test]
fn a_descriptor_without_a_session_id_does_not_generate_command() {
    assert_eq!(descriptor(CLIAgent::Claude, None).resume_command(), None);
}

#[test]
fn test_has_horizontal_split() {
    let single_leaf = PaneNodeSnapshot::Leaf(LeafSnapshot {
        is_focused: false,
        custom_vertical_tabs_title: None,
        contents: LeafContents::Code(CodePaneSnapShot::Local {
            tabs: vec![CodePaneTabSnapshot {
                path: Some(PathBuf::new()),
            }],
            active_tab_index: 0,
            source: None,
        }),
    });
    assert!(!single_leaf.has_horizontal_split());

    let horizontal_split = PaneNodeSnapshot::Branch(BranchSnapshot {
        direction: SplitDirection::Horizontal,
        children: vec![
            (
                PaneFlex(1.),
                PaneNodeSnapshot::Leaf(LeafSnapshot {
                    is_focused: false,
                    custom_vertical_tabs_title: None,
                    contents: LeafContents::Code(CodePaneSnapShot::Local {
                        tabs: vec![CodePaneTabSnapshot {
                            path: Some(PathBuf::new()),
                        }],
                        active_tab_index: 0,
                        source: None,
                    }),
                }),
            ),
            (
                PaneFlex(1.),
                PaneNodeSnapshot::Leaf(LeafSnapshot {
                    is_focused: false,
                    custom_vertical_tabs_title: None,
                    contents: LeafContents::Code(CodePaneSnapShot::Local {
                        tabs: vec![CodePaneTabSnapshot {
                            path: Some(PathBuf::new()),
                        }],
                        active_tab_index: 0,
                        source: None,
                    }),
                }),
            ),
        ],
    });
    assert!(horizontal_split.has_horizontal_split());
}

#[test]
fn test_code_pane_snapshot_single_tab() {
    let snapshot = CodePaneSnapShot::Local {
        tabs: vec![CodePaneTabSnapshot {
            path: Some(PathBuf::from("/tmp/test.rs")),
        }],
        active_tab_index: 0,
        source: Some(CodeSource::FileTree {
            location: crate::code::buffer_location::LocalOrRemotePath::Local(PathBuf::from(
                "/tmp/test.rs",
            )),
        }),
    };
    let CodePaneSnapShot::Local {
        tabs,
        active_tab_index,
        source,
    } = &snapshot;
    assert_eq!(tabs.len(), 1);
    assert_eq!(*active_tab_index, 0);
    assert_eq!(tabs[0].path, Some(PathBuf::from("/tmp/test.rs")));
    assert!(matches!(source, Some(CodeSource::FileTree { .. })));
}

#[test]
fn test_code_pane_snapshot_with_multiple_tabs() {
    let snapshot = CodePaneSnapShot::Local {
        tabs: vec![
            CodePaneTabSnapshot {
                path: Some(PathBuf::from("/tmp/main.rs")),
            },
            CodePaneTabSnapshot {
                path: Some(PathBuf::from("/tmp/lib.rs")),
            },
            CodePaneTabSnapshot { path: None },
        ],
        active_tab_index: 1,
        source: Some(CodeSource::Link {
            path: PathBuf::from("/tmp/main.rs"),
            range_start: None,
            range_end: None,
        }),
    };
    let CodePaneSnapShot::Local {
        tabs,
        active_tab_index,
        source,
    } = &snapshot;
    assert_eq!(tabs.len(), 3);
    assert_eq!(*active_tab_index, 1);
    assert_eq!(tabs[0].path, Some(PathBuf::from("/tmp/main.rs")));
    assert_eq!(tabs[1].path, Some(PathBuf::from("/tmp/lib.rs")));
    assert_eq!(tabs[2].path, None);
    assert!(matches!(source, Some(CodeSource::Link { .. })));
}

#[test]
fn a_snapshot_without_optional_session_metadata_still_deserializes() {
    let json = r#"{"uuid":[1,2],"persistent_session_id":7,"cwd":null,"shell_launch_data":null,
        "is_active":true,"is_read_only":false,"input_config":null,"llm_model_override":null,
        "active_profile_id":null,"conversation_ids_to_restore":[],"active_conversation_id":null}"#;
    let snapshot: TerminalPaneSnapshot = serde_json::from_str(json).unwrap();
    assert_eq!(snapshot.session_holder, None);
    assert_eq!(snapshot.cli_agent_resume_descriptor, None);
    assert_eq!(snapshot.managed_session_job_id, None);
}

#[test]
fn a_session_holder_round_trips_through_json() {
    let holder = SessionHolder {
        channel: r"\\.\pipe\warp-terminal-supervisor-v6-abcdef0123456789".to_owned(),
        version: "20261004-173607861-268f56926594".to_owned(),
        pid: 27784,
    };
    let json = serde_json::to_string(&holder).unwrap();
    assert_eq!(
        serde_json::from_str::<SessionHolder>(&json).unwrap(),
        holder
    );
}
