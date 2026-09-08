use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{
    clone_checkpoints_in_dir, group_summaries, load_from_dir, read_catalog, save_to_dir,
    save_windows_to_dir,
};
use crate::ai::agent::conversation::AIConversationId;
use crate::app_state::{
    CliAgentResumeDescriptor, LeafContents, LeafSnapshot, PaneNodeSnapshot, PaneUuid, TabSnapshot,
    TerminalPaneSnapshot, WindowSnapshot,
};
use crate::launch_configs::launch_config::LaunchConfigWindowSource;
use crate::tab::SelectedTabColor;
use crate::terminal::ShellLaunchData;
use crate::terminal::cli_agent::CLIAgent;
use crate::terminal::model::block::SerializedBlock;
use crate::terminal::shell::ShellType;

fn window(source: LaunchConfigWindowSource) -> WindowSnapshot {
    let conversation_id = AIConversationId::new();
    WindowSnapshot {
        tabs: vec![TabSnapshot {
            custom_title: Some("Agent".to_string()),
            root: PaneNodeSnapshot::Leaf(LeafSnapshot {
                is_focused: true,
                custom_vertical_tabs_title: Some("DEVOPS agent".to_string()),
                contents: LeafContents::Terminal(TerminalPaneSnapshot {
                    uuid: vec![1, 2, 3],
                    persistent_session_id: Some(warp_core::SessionId::from(u64::MAX)),
                    cli_agent_resume_descriptor: Some(CliAgentResumeDescriptor {
                        agent: CLIAgent::Claude,
                        session_id: Some("cf44b8d2-9381-4016-8bfb-7aafc57062fd".to_owned()),
                        cwd: PathBuf::from("/workspace/devops"),
                        title: Some("wire up the supervisor".to_owned()),
                        captured_unix_seconds: Some(1_767_225_600),
                    }),
                    cwd: Some("/workspace/devops".to_string()),
                    shell_launch_data: Some(ShellLaunchData::Executable {
                        executable_path: PathBuf::from("/bin/zsh"),
                        shell_type: ShellType::Zsh,
                    }),
                    is_active: true,
                    is_read_only: false,
                    input_config: None,
                    llm_model_override: Some("agent-model".to_string()),
                    active_profile_id: None,
                    conversation_ids_to_restore: vec![conversation_id],
                    active_conversation_id: Some(conversation_id),
                }),
            }),
            default_directory_color: None,
            selected_color: SelectedTabColor::Cleared,
            left_panel: None,
            right_panel: None,
            group_id: None,
            pinned: true,
        }],
        active_tab_index: 0,
        team_uid: None,
        bounds: None,
        fullscreen_state: Default::default(),
        quake_mode: false,
        universal_search_width: Some(400.0),
        warp_ai_width: None,
        voltron_width: None,
        warp_drive_index_width: None,
        left_panel_open: true,
        vertical_tabs_panel_open: true,
        left_panel_width: Some(240.0),
        right_panel_width: None,
        agent_management_filters: None,
        launch_config_source: Some(source),
        tab_groups: vec![],
    }
}

#[test]
fn window_session_round_trip_preserves_shell_and_agent_state() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = LaunchConfigWindowSource {
        path: PathBuf::from("/home/warp/.warp/session_configurations/devops.toml"),
        document_index: 2,
        window_index: 0,
        config_name: "DEVOPS".to_string(),
    };
    let mut expected = window(source.clone());
    let block_lists = HashMap::from([(
        PaneUuid(vec![1, 2, 3]),
        vec![SerializedBlock::default().into()],
    )]);

    save_to_dir(
        dir.path(),
        &source,
        &expected,
        &block_lists,
        false,
        1_767_225_600,
    )
    .expect("window session should save");
    expected.tabs[0].custom_title = Some("Updated agent".to_string());
    save_to_dir(
        dir.path(),
        &source,
        &expected,
        &Default::default(),
        true,
        1_767_225_601,
    )
    .expect("window session should be replaced");
    let restored = load_from_dir(dir.path(), &source)
        .expect("window session should load")
        .expect("window session should exist");

    assert!(dir.path().join("devops/2/window-0.json").is_file());
    assert_eq!(restored.window, expected);
    assert_eq!(restored.block_lists, Arc::new(block_lists));
}

#[test]
fn autosaved_session_catalog_describes_each_agent_pane() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = LaunchConfigWindowSource {
        path: PathBuf::from("/home/warp/.warp/session_configurations/devops.toml"),
        document_index: 2,
        window_index: 0,
        config_name: "DEVOPS".to_string(),
    };

    save_windows_to_dir(
        dir.path(),
        &[window(source)],
        &Default::default(),
        true,
        1_767_225_600,
        None,
    )
    .expect("window session should save");
    let sessions = group_summaries(super::read_catalog(dir.path()).expect("catalog should load"));

    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.name(), "DEVOPS");
    assert_eq!(session.saved_at_unix_seconds(), 1_767_225_600);
    assert_eq!(session.pane_count(), 1);
    assert_eq!(session.agent_count(), 1);
    let pane = &session.tabs().next().expect("tab should exist").panes[0];
    assert_eq!(pane.shell.as_deref(), Some("/bin/zsh"));
    assert_eq!(pane.cwd.as_deref(), Some("/workspace/devops"));
    let agent = pane.agent.as_ref().expect("agent should be described");
    assert_eq!(agent.agent, CLIAgent::Claude);
    assert_eq!(
        agent.resume_command.as_deref(),
        Some("claude --resume cf44b8d2-9381-4016-8bfb-7aafc57062fd")
    );
    assert_eq!(agent.title.as_deref(), Some("wire up the supervisor"));
    let searchable_text = session.searchable_text();
    assert!(searchable_text.contains("/home/warp/.warp/session_configurations/devops.toml"));
    assert!(searchable_text.contains("Claude Code"));
    assert!(searchable_text.contains("wire up the supervisor"));
}

#[test]
fn close_time_save_retains_an_openable_history_entry() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = LaunchConfigWindowSource {
        path: PathBuf::from("/home/warp/.warp/session_configurations/devops.toml"),
        document_index: 2,
        window_index: 0,
        config_name: "DEVOPS".to_string(),
    };
    let expected = window(source);

    save_windows_to_dir(
        dir.path(),
        std::slice::from_ref(&expected),
        &Default::default(),
        false,
        1_767_225_600,
        Some("saved-run"),
    )
    .expect("window session should be retained");
    let sessions = group_summaries(super::read_catalog(dir.path()).expect("catalog should load"));

    assert_eq!(sessions.len(), 2);
    let history = sessions
        .iter()
        .find(|session| !session.is_current())
        .expect("stored session history should be cataloged");
    assert!(history.id.starts_with("history:"));
    assert!(history.id.ends_with(":saved-run"));
    let restored = super::load_stored_session_from_dir(dir.path(), history)
        .expect("stored session should load");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].window, expected);
}

#[test]
fn reopening_a_configuration_starts_a_distinct_history_run() {
    let source = LaunchConfigWindowSource {
        path: PathBuf::from("/history-run-test/devops.yaml"),
        document_index: 0,
        window_index: 0,
        config_name: "DEVOPS".to_string(),
    };
    let snapshot = window(source.clone());

    super::begin_run(&source);
    let first = super::active_history_id(std::slice::from_ref(&snapshot))
        .expect("active run should have a history ID");
    assert_eq!(
        super::active_history_id(std::slice::from_ref(&snapshot)).as_deref(),
        Some(first.as_str())
    );

    super::begin_run(&source);
    let second = super::active_history_id(std::slice::from_ref(&snapshot))
        .expect("reopened run should have a history ID");
    assert_ne!(first, second);
}

#[test]
fn closing_one_window_retains_the_complete_multi_window_session() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let first_source = LaunchConfigWindowSource {
        path: PathBuf::from("/home/warp/.warp/launch_configurations/devops.yaml"),
        document_index: 0,
        window_index: 0,
        config_name: "DEVOPS".to_string(),
    };
    let second_source = LaunchConfigWindowSource {
        window_index: 1,
        ..first_source.clone()
    };
    let first = window(first_source);
    let second = window(second_source);

    save_windows_to_dir(
        dir.path(),
        &[first.clone(), second.clone()],
        &Default::default(),
        true,
        1_767_225_500,
        None,
    )
    .expect("multi-window session should autosave");
    save_windows_to_dir(
        dir.path(),
        std::slice::from_ref(&first),
        &Default::default(),
        false,
        1_767_225_600,
        Some("saved-run"),
    )
    .expect("complete session should be retained when one window closes");
    save_windows_to_dir(
        dir.path(),
        std::slice::from_ref(&second),
        &Default::default(),
        false,
        1_767_225_601,
        Some("saved-run"),
    )
    .expect("the same history entry should update when the other window closes");

    let sessions = group_summaries(super::read_catalog(dir.path()).expect("catalog should load"));
    let history = sessions
        .iter()
        .find(|session| !session.is_current())
        .expect("stored session history should be cataloged");
    assert_eq!(history.windows().len(), 2);
    let restored = super::load_stored_session_from_dir(dir.path(), history)
        .expect("multi-window session should restore");
    assert_eq!(restored.len(), 2);
    assert_eq!(restored[0].window, first);
    assert_eq!(restored[1].window, second);
}

fn session_source(path: &str, generation: usize, window_index: usize) -> LaunchConfigWindowSource {
    LaunchConfigWindowSource {
        path: PathBuf::from(path),
        document_index: generation,
        window_index,
        config_name: "OBACHAN".to_owned(),
    }
}

fn only_terminal(window: &WindowSnapshot) -> &TerminalPaneSnapshot {
    let PaneNodeSnapshot::Leaf(leaf) = &window.tabs[0].root else {
        panic!("fixture window has a single leaf pane");
    };
    let LeafContents::Terminal(terminal) = &leaf.contents else {
        panic!("fixture leaf is a terminal");
    };
    terminal
}

fn seed_source_checkpoint(dir: &Path) -> LaunchConfigWindowSource {
    let source = session_source("/cfg/OBACHAN.toml", 1, 0);
    save_to_dir(
        dir,
        &source,
        &window(source.clone()),
        &HashMap::new(),
        false,
        1_767_225_600,
    )
    .expect("the source checkpoint should be written");
    source
}

#[test]
fn cloning_rewrites_the_source_and_keeps_the_layout() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = seed_source_checkpoint(dir.path());

    let cloned = clone_checkpoints_in_dir(
        dir.path(),
        &source,
        Path::new("/cfg/OBACHAN-2.toml"),
        "OBACHAN (2)",
    )
    .expect("cloning should succeed");

    assert_eq!(cloned.len(), 1);
    assert_eq!(cloned[0].path, PathBuf::from("/cfg/OBACHAN-2.toml"));
    assert_eq!(cloned[0].document_index, 1);
    assert_eq!(cloned[0].window_index, 0);
    assert_eq!(cloned[0].config_name, "OBACHAN (2)");

    let restored = load_from_dir(dir.path(), &cloned[0])
        .expect("the clone should load")
        .expect("the clone should exist");
    let original = load_from_dir(dir.path(), &source)
        .expect("the original should load")
        .expect("the original should exist");
    assert_eq!(restored.window.tabs.len(), original.window.tabs.len());
    assert_eq!(
        only_terminal(&restored.window).cwd,
        only_terminal(&original.window).cwd
    );
    assert_eq!(
        only_terminal(&restored.window).shell_launch_data,
        only_terminal(&original.window).shell_launch_data
    );
}

#[test]
fn cloning_releases_every_identity_owned_by_the_live_window() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = seed_source_checkpoint(dir.path());

    let cloned = clone_checkpoints_in_dir(
        dir.path(),
        &source,
        Path::new("/cfg/OBACHAN-2.toml"),
        "OBACHAN (2)",
    )
    .expect("cloning should succeed");

    let restored = load_from_dir(dir.path(), &cloned[0])
        .expect("the clone should load")
        .expect("the clone should exist");
    let original = load_from_dir(dir.path(), &source)
        .expect("the original should load")
        .expect("the original should exist");
    let clone_pane = only_terminal(&restored.window);
    let original_pane = only_terminal(&original.window);

    assert_ne!(clone_pane.uuid, original_pane.uuid);
    assert!(!clone_pane.uuid.is_empty());
    assert_eq!(clone_pane.persistent_session_id, None);
    assert_eq!(clone_pane.cli_agent_resume_descriptor, None);
    assert!(clone_pane.conversation_ids_to_restore.is_empty());
    assert_eq!(clone_pane.active_conversation_id, None);
    assert!(restored.block_lists.is_empty());

    assert!(original_pane.persistent_session_id.is_some());
    assert!(original_pane.cli_agent_resume_descriptor.is_some());
}

#[test]
fn cloning_registers_the_clone_in_the_catalog() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = seed_source_checkpoint(dir.path());

    clone_checkpoints_in_dir(
        dir.path(),
        &source,
        Path::new("/cfg/OBACHAN-2.toml"),
        "OBACHAN (2)",
    )
    .expect("cloning should succeed");

    let catalog = read_catalog(dir.path()).expect("the catalog should be readable");
    let names = catalog
        .iter()
        .map(|window| window.source.config_name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"OBACHAN"));
    assert!(names.contains(&"OBACHAN (2)"));
    assert_eq!(catalog.len(), 2);
}
