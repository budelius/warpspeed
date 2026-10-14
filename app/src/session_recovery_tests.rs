use std::path::PathBuf;

use super::*;
use crate::ai::agent::conversation::AIConversationId;
use crate::app_state::{
    CliAgentResumeDescriptor, LeafContents, LeafSnapshot, PaneNodeSnapshot, TabSnapshot,
    TerminalPaneSnapshot,
};
use crate::tab::SelectedTabColor;
use crate::terminal::ShellLaunchData;
use crate::terminal::cli_agent::CLIAgent;
use crate::terminal::shell::ShellType;

fn window() -> WindowSnapshot {
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
                    session_holder: None,
                    cli_agent_resume_descriptor: Some(CliAgentResumeDescriptor {
                        agent: CLIAgent::Claude,
                        session_id: Some("cf44b8d2-9381-4016-8bfb-7aafc57062fd".to_owned()),
                        cwd: PathBuf::from("/workspace/devops"),
                        title: Some("wire up the supervisor".to_owned()),
                        captured_unix_seconds: Some(1_767_225_600),
                    }),
                    managed_session_job_id: None,
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
        launch_config_source: None,
        tab_groups: vec![],
    }
}

fn state() -> AppState {
    AppState {
        windows: vec![window(), window()],
        active_window_index: Some(1),
        block_lists: Default::default(),
        running_mcp_servers: Vec::new(),
    }
}

fn saved_run(dir: &Path, state: &AppState) -> Recovery {
    let mut recovery = Recovery::new(dir).unwrap();
    recovery.ready = true;
    recovery.save(state).unwrap();
    recovery
}

#[test]
fn recovers_every_window_of_a_crashed_instance_beside_a_live_instance() {
    let dir = tempfile::tempdir().unwrap();
    let expected = state();
    let crashed = saved_run(dir.path(), &expected);
    let live = saved_run(dir.path(), &state());
    drop(crashed);

    let mut next = Recovery::new(dir.path()).unwrap();
    let recovered = next.claim_interrupted().unwrap();

    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].windows, expected.windows);
    assert_eq!(recovered[0].active_window_index, Some(1));
    assert!(live.own.path.is_file());
}

#[test]
fn two_restarting_instances_cannot_restore_the_same_crash() {
    let dir = tempfile::tempdir().unwrap();
    drop(saved_run(dir.path(), &state()));
    let mut first = Recovery::new(dir.path()).unwrap();
    assert_eq!(first.claim_interrupted().unwrap().len(), 1);

    let mut second = Recovery::new(dir.path()).unwrap();
    assert!(second.claim_interrupted().unwrap().is_empty());
}

#[test]
fn a_second_crash_during_startup_keeps_the_original_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let original = saved_run(dir.path(), &state());
    let path = original.own.path.clone();
    drop(original);
    let mut failed_restart = Recovery::new(dir.path()).unwrap();
    failed_restart.claim_interrupted().unwrap();
    failed_restart.save(&state()).unwrap();
    drop(failed_restart);

    assert!(path.is_file());
    let mut next = Recovery::new(dir.path()).unwrap();
    assert_eq!(next.claim_interrupted().unwrap().len(), 1);
}

#[test]
fn successful_checkpoint_replaces_recovered_sources_without_duplicating_them() {
    let dir = tempfile::tempdir().unwrap();
    let original = saved_run(dir.path(), &state());
    let path = original.own.path.clone();
    drop(original);
    let mut next = Recovery::new(dir.path()).unwrap();
    let recovered = next.claim_interrupted().unwrap();
    next.ready = true;
    next.save(&AppState {
        windows: recovered
            .into_iter()
            .flat_map(|checkpoint| checkpoint.windows)
            .collect(),
        ..state()
    })
    .unwrap();
    assert!(!path.exists());
    drop(next);

    let mut third = Recovery::new(dir.path()).unwrap();
    let recovered = third.claim_interrupted().unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].windows.len(), 2);
}

#[test]
fn failed_checkpoint_write_does_not_consume_recoverable_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let original = saved_run(dir.path(), &state());
    let path = original.own.path.clone();
    drop(original);
    let mut next = Recovery::new(dir.path()).unwrap();
    next.claim_interrupted().unwrap();
    next.ready = true;
    next.own.path = path.join("cannot-write-below-a-file.json");

    assert!(next.save(&state()).is_err());
    assert!(path.is_file());
}

#[test]
fn corrupt_checkpoint_does_not_block_other_crashed_instances() {
    let dir = tempfile::tempdir().unwrap();
    let corrupt = dir.path().join("corrupt.json");
    fs::write(&corrupt, b"partial json").unwrap();
    drop(saved_run(dir.path(), &state()));

    let mut next = Recovery::new(dir.path()).unwrap();
    assert_eq!(next.claim_interrupted().unwrap().len(), 1);
    assert_eq!(fs::read(&corrupt).unwrap(), b"partial json");
}

#[test]
fn recovers_multiple_crashed_instances() {
    let dir = tempfile::tempdir().unwrap();
    let first = saved_run(dir.path(), &state());
    let second = saved_run(dir.path(), &state());
    drop(first);
    drop(second);

    let mut next = Recovery::new(dir.path()).unwrap();
    let recovered = next.claim_interrupted().unwrap();
    assert_eq!(recovered.len(), 2);
    assert_eq!(recovered[0].windows.len(), 2);
    assert_eq!(recovered[1].windows.len(), 2);
}

#[test]
fn newer_checkpoint_versions_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.json");
    let contents = serde_json::to_vec(&Checkpoint {
        version: 2,
        revision: "future-build".into(),
        supersedes: Vec::new(),
        windows: state().windows,
        active_window_index: Some(0),
    })
    .unwrap();
    fs::write(&path, &contents).unwrap();

    let mut next = Recovery::new(dir.path()).unwrap();
    assert!(next.claim_interrupted().unwrap().is_empty());
    assert_eq!(fs::read(path).unwrap(), contents);
}

#[test]
fn crash_after_replacement_write_does_not_duplicate_the_source_windows() {
    let dir = tempfile::tempdir().unwrap();
    let original = saved_run(dir.path(), &state());
    let original_path = original.own.path.clone();
    drop(original);
    let mut replacement = Recovery::new(dir.path()).unwrap();
    let recovered = replacement.claim_interrupted().unwrap();
    crate::launch_configs::session_store::write_json_atomically(
        &replacement.own.path,
        &Checkpoint {
            version: 1,
            revision: "replacement-build".into(),
            supersedes: vec![
                original_path
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
            ],
            windows: recovered
                .into_iter()
                .flat_map(|checkpoint| checkpoint.windows)
                .collect(),
            active_window_index: Some(1),
        },
    )
    .unwrap();
    drop(replacement);
    assert!(original_path.exists());

    let mut next = Recovery::new(dir.path()).unwrap();
    let recovered = next.claim_interrupted().unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].windows.len(), 2);
}

#[test]
fn clean_exit_is_not_reported_as_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let clean = saved_run(dir.path(), &state());
    clean.own.remove_checkpoint().unwrap();
    drop(clean);

    let mut next = Recovery::new(dir.path()).unwrap();
    assert!(next.claim_interrupted().unwrap().is_empty());
}
