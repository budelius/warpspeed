use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use chrono::{Local, TimeZone as _};
use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::vec2f;

#[cfg(windows)]
use super::write_json_atomically;
use super::{
    NoCheckpointsToClone, clone_checkpoints_in_dir, group_summaries, load_from_dir, read_catalog,
    save_to_dir, save_windows_to_dir,
};
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::ai::blocklist::SerializedBlockListItem;
use crate::app_state::{
    AmbientAgentPaneSnapshot, BranchSnapshot, CliAgentResumeDescriptor, LeafContents, LeafSnapshot,
    PaneFlex, PaneNodeSnapshot, PaneUuid, SplitDirection, TabSnapshot, TerminalPaneSnapshot,
    WindowSnapshot,
};
use crate::launch_configs::launch_config::LaunchConfigWindowSource;
use crate::tab::SelectedTabColor;
use crate::terminal::ShellLaunchData;
use crate::terminal::cli_agent::CLIAgent;
use crate::terminal::model::block::SerializedBlock;
use crate::terminal::shell::ShellType;

fn oversized_output(byte: u8, additional_bytes: usize) -> Vec<u8> {
    vec![byte; super::MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES + additional_bytes]
}

fn dense_ansi_output(additional_bytes: usize) -> Vec<u8> {
    oversized_output(0x1b, additional_bytes)
}

fn source() -> LaunchConfigWindowSource {
    LaunchConfigWindowSource {
        path: PathBuf::from("/home/warp/.warp/session_configurations/devops.toml"),
        document_index: 2,
        window_index: 0,
        config_name: "DEVOPS".to_string(),
    }
}

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
        launch_config_source: Some(source),
        tab_groups: vec![],
    }
}

#[cfg(windows)]
#[test]
fn atomic_write_retries_a_temporarily_blocked_windows_replacement() {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use windows::Win32::Storage::FileSystem::FILE_SHARE_READ;

    let dir = tempfile::tempdir().expect("session directory should be created");
    let path = dir.path().join("stored-sessions.json");
    std::fs::write(&path, r#"["old"]"#).expect("the original catalog should be written");
    let locked_file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .open(&path)
        .expect("the original catalog should be opened without delete sharing");
    let (release_tx, release_rx) = mpsc::channel();
    let releaser = thread::spawn(move || {
        release_rx
            .recv()
            .expect("the replacement attempt should start");
        thread::sleep(Duration::from_millis(50));
        drop(locked_file);
    });

    release_tx
        .send(())
        .expect("the replacement attempt should be signaled");
    let result = write_json_atomically(&path, &vec!["new"]);
    releaser
        .join()
        .expect("the temporary catalog lock should be released");
    result.expect("the catalog should be replaced after its reader releases it");

    let stored = std::fs::read_to_string(path).expect("the replacement catalog should be readable");
    assert_eq!(stored, r#"["new"]"#);
}

#[test]
fn v2_checkpoint_uses_compact_bytes_and_bounds_oversized_output() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = source();
    let oversized = oversized_output(b'x', 4096);
    let block_lists = HashMap::from([(
        PaneUuid(vec![1, 2, 3]),
        vec![SerializedBlock::new_for_test(Vec::new(), oversized).into()],
    )]);

    save_to_dir(
        dir.path(),
        &source,
        &window(source.clone()),
        &block_lists,
        false,
        1_767_225_600,
    )
    .expect("window session should save");

    let path = dir.path().join("devops/2/window-0.json");
    let json = fs::read_to_string(&path).expect("checkpoint should be readable");
    let restored = load_from_dir(dir.path(), &source)
        .expect("window session should load")
        .expect("window session should exist");
    let item = &restored.block_lists[&PaneUuid(vec![1, 2, 3])][0];
    let SerializedBlockListItem::Command { block } = item;

    assert!(json.contains("\"version\":2"));
    assert!(json.contains("\"stylized_output\":\""));
    assert!(json.contains("\"stylized_output_truncated\":true"));
    assert!(json.contains(&format!(
        "\"stylized_output_original_bytes\":{}",
        super::MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES + 4096
    )));
    assert!(block.stylized_output.len() <= super::MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES);
    assert!(
        block
            .stylized_output
            .starts_with(b"\x1b[0m[older output truncated]\r\n")
    );
}

#[test]
fn loading_v1_checkpoint_migrates_atomically_and_retains_backup() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = source();
    let path = dir.path().join("devops/2/window-0.json");
    let stored = super::StoredWindowSession {
        version: super::LEGACY_SESSION_FORMAT_VERSION,
        source: source.clone(),
        profile: Some("WarpSpeed".to_owned()),
        window: window(source.clone()),
        block_lists: vec![super::StoredPaneBlocks {
            uuid: vec![1, 2, 3],
            blocks: vec![
                SerializedBlock::new_for_test(b"echo legacy".to_vec(), b"legacy output".to_vec())
                    .into(),
            ],
        }],
        saved_at_unix_seconds: 1_767_225_600,
    };
    super::write_json_atomically(&path, &stored).expect("v1 checkpoint should save");

    let restored = load_from_dir(dir.path(), &source)
        .expect("v1 checkpoint should load")
        .expect("v1 checkpoint should exist");
    let migrated_json = fs::read_to_string(&path).expect("migrated checkpoint should be readable");
    let backup_path = super::legacy_backup_path(&path);
    let backup_json = fs::read_to_string(&backup_path).expect("legacy backup should be retained");
    let checksum = fs::read_to_string(super::legacy_backup_checksum_path(&backup_path))
        .expect("legacy backup checksum should be retained");

    assert_eq!(restored.window, stored.window);
    assert!(migrated_json.contains("\"version\":2"));
    assert!(migrated_json.contains("\"stylized_output\":\""));
    assert!(backup_json.contains("\"version\":1"));
    assert!(backup_json.contains("\"stylized_output\":["));
    assert_eq!(checksum.trim(), super::file_sha256(&backup_path).unwrap());
}

#[test]
fn preserving_blocks_does_not_replace_v1_when_migration_fails() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = source();
    let path = dir.path().join("devops/2/window-0.json");
    let stored = super::StoredWindowSession {
        version: super::LEGACY_SESSION_FORMAT_VERSION,
        source: source.clone(),
        profile: Some("WarpSpeed".to_owned()),
        window: window(source.clone()),
        block_lists: vec![super::StoredPaneBlocks {
            uuid: vec![1, 2, 3],
            blocks: vec![
                SerializedBlock::new_for_test(b"echo legacy".to_vec(), b"legacy output".to_vec())
                    .into(),
            ],
        }],
        saved_at_unix_seconds: 1_767_225_600,
    };
    super::write_json_atomically(&path, &stored).expect("v1 checkpoint should save");
    let original = fs::read(&path).unwrap();
    fs::write(super::legacy_backup_path(&path), b"{invalid backup")
        .expect("invalid rollback should be written");

    save_to_dir(
        dir.path(),
        &source,
        &window(source.clone()),
        &HashMap::new(),
        true,
        1_767_225_601,
    )
    .expect_err("a failed migration must stop the preserving save");

    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(
        super::read_checkpoint_file(&path).unwrap().version,
        super::LEGACY_SESSION_FORMAT_VERSION
    );
}

#[test]
fn failed_temporary_checkpoint_validation_leaves_the_active_v1_unchanged() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = source();
    let path = dir.path().join("devops/2/window-0.json");
    let stored = super::StoredWindowSession {
        version: super::LEGACY_SESSION_FORMAT_VERSION,
        source: source.clone(),
        profile: Some("WarpSpeed".to_owned()),
        window: window(source),
        block_lists: Vec::new(),
        saved_at_unix_seconds: 1_767_225_600,
    };
    super::write_json_atomically(&path, &stored).expect("v1 checkpoint should save");
    let original = fs::read(&path).unwrap();

    super::write_checkpoint_atomically(&path, &stored)
        .expect_err("a temporary v1 checkpoint must fail v2 validation");

    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn backup_without_checksum_recovers_after_an_interrupted_migration() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = source();
    let path = dir.path().join("devops/2/window-0.json");
    let stored = super::StoredWindowSession {
        version: super::LEGACY_SESSION_FORMAT_VERSION,
        source: source.clone(),
        profile: Some("WarpSpeed".to_owned()),
        window: window(source.clone()),
        block_lists: Vec::new(),
        saved_at_unix_seconds: 1_767_225_600,
    };
    super::write_json_atomically(&path, &stored).expect("v1 checkpoint should save");
    let backup_path = super::ensure_legacy_backup(&path).unwrap();
    fs::remove_file(super::legacy_backup_checksum_path(&backup_path)).unwrap();
    fs::write(&path, b"{interrupted").unwrap();

    let restored = load_from_dir(dir.path(), &source)
        .expect("backup recovery should succeed")
        .expect("recovered checkpoint should exist");

    assert_eq!(restored.window, stored.window);
    assert!(super::legacy_backup_checksum_path(&backup_path).is_file());
    assert!(fs::read_to_string(path).unwrap().contains("\"version\":2"));
}

#[test]
fn every_migration_boundary_retains_a_valid_checkpoint_or_rollback() {
    for interruption in [
        super::MigrationBoundary::RollbackPrepared,
        super::MigrationBoundary::TemporaryValidated,
        super::MigrationBoundary::ActiveReplaced,
    ] {
        let dir = tempfile::tempdir().expect("session directory should be created");
        let source = source();
        let path = dir.path().join("devops/2/window-0.json");
        let stored = super::StoredWindowSession {
            version: super::LEGACY_SESSION_FORMAT_VERSION,
            source: source.clone(),
            profile: Some("WarpSpeed".to_owned()),
            window: window(source),
            block_lists: Vec::new(),
            saved_at_unix_seconds: 1_767_225_600,
        };
        super::write_json_atomically(&path, &stored).expect("v1 checkpoint should save");
        let mut parsed = super::read_checkpoint_file(&path).unwrap();

        super::migrate_legacy_checkpoint_with_hook(&path, &mut parsed, |boundary| {
            if boundary == interruption {
                anyhow::bail!("injected interruption at {boundary:?}");
            }
            Ok(())
        })
        .expect_err("injected interruption should stop migration");

        let backup_path = super::legacy_backup_path(&path);
        super::validate_legacy_backup(&backup_path).expect("rollback should remain valid");
        let active = super::read_checkpoint_file(&path).expect("active checkpoint should be valid");
        let expected_active_version = if interruption == super::MigrationBoundary::ActiveReplaced {
            super::SESSION_FORMAT_VERSION
        } else {
            super::LEGACY_SESSION_FORMAT_VERSION
        };
        assert_eq!(active.version, expected_active_version);
    }
}

#[test]
fn oversized_v1_integer_array_is_bounded_during_migration() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = source();
    let path = dir.path().join("devops/2/window-0.json");
    let original_len = super::MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES + 128 * 1024;
    let stored = super::StoredWindowSession {
        version: super::LEGACY_SESSION_FORMAT_VERSION,
        source: source.clone(),
        profile: Some("WarpSpeed".to_owned()),
        window: window(source.clone()),
        block_lists: vec![super::StoredPaneBlocks {
            uuid: vec![1, 2, 3],
            blocks: vec![
                SerializedBlock::new_for_test(
                    b"echo dense".to_vec(),
                    dense_ansi_output(128 * 1024),
                )
                .into(),
            ],
        }],
        saved_at_unix_seconds: 1_767_225_600,
    };
    super::write_json_atomically(&path, &stored).expect("v1 checkpoint should save");

    let restored = load_from_dir(dir.path(), &source)
        .expect("v1 checkpoint should load")
        .expect("v1 checkpoint should exist");
    let SerializedBlockListItem::Command { block } =
        &restored.block_lists[&PaneUuid(vec![1, 2, 3])][0];

    assert!(block.stylized_output.len() <= super::MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES);
    assert!(block.stylized_output_truncated);
    assert_eq!(block.stylized_output_original_bytes, Some(original_len));
    assert!(
        block
            .stylized_output
            .starts_with(b"\x1b[0m[older output truncated]\r\n")
    );
    assert!(super::legacy_backup_path(&path).is_file());
}

#[test]
fn unreadable_active_checkpoint_recovers_from_validated_legacy_backup() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = source();
    let path = dir.path().join("devops/2/window-0.json");
    let stored = super::StoredWindowSession {
        version: super::LEGACY_SESSION_FORMAT_VERSION,
        source: source.clone(),
        profile: Some("WarpSpeed".to_owned()),
        window: window(source.clone()),
        block_lists: Vec::new(),
        saved_at_unix_seconds: 1_767_225_600,
    };
    super::write_json_atomically(&path, &stored).expect("v1 checkpoint should save");
    load_from_dir(dir.path(), &source).expect("initial migration should succeed");
    fs::write(&path, b"{corrupt").expect("active checkpoint should be corruptible");

    let restored = load_from_dir(dir.path(), &source)
        .expect("backup recovery should succeed")
        .expect("recovered checkpoint should exist");

    assert_eq!(restored.window, stored.window);
    assert!(fs::read_to_string(path).unwrap().contains("\"version\":2"));
}

#[test]
fn pane_and_window_output_budgets_prioritize_recent_blocks() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = source();
    let mut snapshot = window(source.clone());
    let terminal = match &snapshot.tabs[0].root {
        PaneNodeSnapshot::Leaf(leaf) => leaf.clone(),
        PaneNodeSnapshot::Branch(_) => unreachable!(),
    };
    let pane_uuids = [vec![1, 2, 3], vec![4, 5, 6], vec![7, 8, 9]];
    snapshot.tabs[0].root = PaneNodeSnapshot::Branch(BranchSnapshot {
        direction: SplitDirection::Vertical,
        children: pane_uuids
            .iter()
            .map(|uuid| {
                let mut leaf = terminal.clone();
                let LeafContents::Terminal(terminal) = &mut leaf.contents else {
                    unreachable!();
                };
                terminal.uuid = uuid.clone();
                (PaneFlex(1.0), PaneNodeSnapshot::Leaf(leaf))
            })
            .collect(),
    });
    let mut block_lists = HashMap::new();
    for (pane_index, uuid) in pane_uuids.iter().enumerate() {
        let blocks = (0..8)
            .map(|block_index| {
                let mut block = SerializedBlock::new_for_test(
                    format!("pane {pane_index} block {block_index}").into_bytes(),
                    vec![b'a' + pane_index as u8; super::MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES],
                );
                block.start_ts = Local
                    .timestamp_opt((pane_index * 8 + block_index) as i64, 0)
                    .single();
                block.into()
            })
            .collect();
        block_lists.insert(PaneUuid(uuid.clone()), blocks);
    }

    save_to_dir(
        dir.path(),
        &source,
        &snapshot,
        &block_lists,
        false,
        1_767_225_600,
    )
    .expect("window session should save");
    let restored = load_from_dir(dir.path(), &source)
        .expect("window session should load")
        .expect("window session should exist");
    let pane_sizes = pane_uuids
        .iter()
        .map(|uuid| {
            restored.block_lists[&PaneUuid(uuid.clone())]
                .iter()
                .map(|item| match item {
                    SerializedBlockListItem::Command { block } => block.stylized_output.len(),
                })
                .sum::<usize>()
        })
        .collect::<Vec<_>>();

    assert!(pane_sizes[0] > 0);
    assert!(pane_sizes[0] < pane_sizes[1]);
    assert!(pane_sizes[1] < pane_sizes[2]);
    assert_eq!(pane_sizes[2], super::MAX_PANE_STYLIZED_OUTPUT_BYTES);
    assert!(
        restored
            .block_lists
            .values()
            .flatten()
            .all(|item| !super::command_block(item).stylized_output.is_empty())
    );
    assert!(pane_sizes.iter().sum::<usize>() <= super::MAX_WINDOW_STYLIZED_OUTPUT_BYTES);
}

#[test]
fn checkpoint_cardinality_limits_retain_the_most_recent_history() {
    let mut block_lists = (0..3)
        .map(|pane_index| {
            let blocks = (0..(super::MAX_RESTORED_BLOCKS_PER_PANE + 2))
                .map(|block_index| {
                    let sequence =
                        pane_index * (super::MAX_RESTORED_BLOCKS_PER_PANE + 2) + block_index;
                    let mut block = SerializedBlock::new_for_test(
                        format!("command {sequence}").into_bytes(),
                        Vec::new(),
                    );
                    block.start_ts = Local.timestamp_opt(sequence as i64, 0).single();
                    block.into()
                })
                .collect();
            super::StoredPaneBlocks {
                uuid: vec![pane_index as u8],
                blocks,
            }
        })
        .collect::<Vec<_>>();

    super::enforce_checkpoint_budgets(&mut block_lists);

    let retained = block_lists
        .iter()
        .flat_map(|pane| &pane.blocks)
        .collect::<Vec<_>>();
    assert_eq!(retained.len(), super::MAX_RESTORED_BLOCKS_PER_WINDOW);
    assert!(
        block_lists
            .iter()
            .all(|pane| pane.blocks.len() <= super::MAX_RESTORED_BLOCKS_PER_PANE)
    );
    let oldest_retained = retained
        .iter()
        .filter_map(|block| block.start_ts())
        .min()
        .unwrap();
    assert_eq!(
        oldest_retained.timestamp(),
        (super::MAX_RESTORED_BLOCKS_PER_PANE + 4) as i64
    );
}

#[test]
fn checkpoint_cardinality_limits_drop_old_empty_pane_histories() {
    let mut block_lists = (0..=super::MAX_STORED_PANE_BLOCK_LISTS)
        .map(|pane_index| {
            let mut block = SerializedBlock::new_for_test(Vec::new(), Vec::new());
            block.start_ts = Local.timestamp_opt(pane_index as i64, 0).single();
            super::StoredPaneBlocks {
                uuid: pane_index.to_le_bytes().to_vec(),
                blocks: vec![block.into()],
            }
        })
        .collect::<Vec<_>>();

    super::enforce_checkpoint_budgets(&mut block_lists);

    assert_eq!(block_lists.len(), super::MAX_STORED_PANE_BLOCK_LISTS);
    assert!(
        block_lists
            .iter()
            .all(|pane| pane.uuid != 0usize.to_le_bytes())
    );
}

#[test]
fn checkpoint_file_size_is_rejected_before_deserialization() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let path = dir.path().join("oversized.json");
    let file = fs::File::create(&path).expect("checkpoint should be created");
    file.set_len(super::MAX_CHECKPOINT_FILE_BYTES + 1)
        .expect("logical checkpoint length should be set");

    let error = match super::read_checkpoint_file(&path) {
        Ok(_) => panic!("oversized checkpoint should be rejected before parsing"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("exceeding"));
}

#[test]
#[ignore = "set WARPSPEED_LEGACY_CHECKPOINT to benchmark a copied real checkpoint"]
fn representative_legacy_checkpoint_migration_benchmark() {
    let source_path = std::env::var_os("WARPSPEED_LEGACY_CHECKPOINT")
        .map(PathBuf::from)
        .expect("WARPSPEED_LEGACY_CHECKPOINT must point to a V1 checkpoint");
    let temp_dir = tempfile::tempdir().expect("benchmark directory should be created");
    let path = temp_dir.path().join("window-0.json");
    fs::copy(&source_path, &path).expect("legacy fixture should be copied");

    let migration_started = instant::Instant::now();
    let mut stored = super::read_checkpoint_file(&path).expect("legacy fixture should parse");
    let parse_duration = migration_started.elapsed();
    super::migrate_legacy_checkpoint(&path, &mut stored).expect("legacy fixture should migrate");
    let migration_duration = migration_started.elapsed();
    let migrated_bytes = fs::metadata(&path).unwrap().len();
    let reread_started = instant::Instant::now();
    let migrated = super::read_checkpoint_file(&path).expect("migrated fixture should parse");
    let reread_duration = reread_started.elapsed();
    let retained_output_bytes = migrated
        .block_lists
        .iter()
        .flat_map(|pane| &pane.blocks)
        .map(|item| match item {
            SerializedBlockListItem::Command { block } => block.stylized_output.len(),
        })
        .sum::<usize>();

    eprintln!(
        "legacy_bytes={} parse_ms={} migration_ms={} migrated_bytes={} reread_ms={} retained_output_bytes={}",
        fs::metadata(source_path).unwrap().len(),
        parse_duration.as_millis(),
        migration_duration.as_millis(),
        migrated_bytes,
        reread_duration.as_millis(),
        retained_output_bytes
    );
    assert!(migrated_bytes <= 25 * 1024 * 1024);
    assert!(retained_output_bytes <= super::MAX_WINDOW_STYLIZED_OUTPUT_BYTES);
    assert!(super::legacy_backup_path(&path).is_file());
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

fn seed_source_checkpoint(
    dir: &Path,
    block_lists: &HashMap<PaneUuid, Vec<SerializedBlockListItem>>,
) -> LaunchConfigWindowSource {
    let source = session_source("/cfg/OBACHAN.toml", 1, 0);
    save_to_dir(
        dir,
        &source,
        &window(source.clone()),
        block_lists,
        false,
        1_767_225_600,
    )
    .expect("the source checkpoint should be written");
    source
}

#[test]
fn cloning_rewrites_the_source_and_keeps_the_layout() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = seed_source_checkpoint(dir.path(), &HashMap::new());

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
    let block_lists = HashMap::from([(
        PaneUuid(vec![1, 2, 3]),
        vec![SerializedBlock::default().into()],
    )]);
    let source = seed_source_checkpoint(dir.path(), &block_lists);

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
    assert!(
        !original.block_lists.is_empty(),
        "cloning must not destroy the original's block lists"
    );
}

#[test]
fn cloning_registers_the_clone_in_the_catalog() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = seed_source_checkpoint(dir.path(), &HashMap::new());

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

#[test]
fn cloning_a_session_with_no_checkpoints_yet_fails_distinguishably() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = session_source("/cfg/OBACHAN.toml", 1, 0);
    // `claim_session` creates the lease directory before any checkpoint is
    // ever written, so this state -- present but empty -- is reachable, not
    // hypothetical.
    std::fs::create_dir_all(dir.path().join("obachan").join("1"))
        .expect("the empty checkpoint directory should be created");

    let error = clone_checkpoints_in_dir(
        dir.path(),
        &source,
        Path::new("/cfg/OBACHAN-2.toml"),
        "OBACHAN (2)",
    )
    .expect_err("cloning a session with no checkpoints should fail");

    assert!(
        error.downcast_ref::<NoCheckpointsToClone>().is_some(),
        "the failure must be distinguishable so the dialog can show an actionable message \
         instead of a generic one"
    );
}

#[test]
fn cloning_clears_the_original_bounds_so_the_clone_is_not_placed_on_top_of_it() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = session_source("/cfg/OBACHAN.toml", 1, 0);
    let mut original = window(source.clone());
    original.bounds = Some(RectF::new(vec2f(100., 200.), vec2f(800., 600.)));
    save_to_dir(
        dir.path(),
        &source,
        &original,
        &HashMap::new(),
        false,
        1_767_225_600,
    )
    .expect("the source checkpoint should be written");

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
    assert_eq!(
        restored.window.bounds, None,
        "a cloned window must get default placement, not sit on top of the original"
    );
}

fn ambient_task_id() -> AmbientAgentTaskId {
    AmbientAgentTaskId::from_str("2f2b0a5c-6f2f-4a3b-9a1b-0f8f8b1c2d3e")
        .expect("the fixture task id should parse")
}

/// A window whose tab root is a split rather than a bare leaf, with one
/// terminal pane and one ambient agent pane. Both the `Branch` recursion and a
/// non-terminal leaf go unexercised by the single-leaf fixture.
fn split_window(source: LaunchConfigWindowSource) -> WindowSnapshot {
    let mut window = window(source);
    let terminal = window.tabs[0].root.clone();
    window.tabs[0].root = PaneNodeSnapshot::Branch(BranchSnapshot {
        direction: SplitDirection::Vertical,
        children: vec![
            (PaneFlex(1.0), terminal),
            (
                PaneFlex(1.0),
                PaneNodeSnapshot::Leaf(LeafSnapshot {
                    is_focused: false,
                    custom_vertical_tabs_title: None,
                    contents: LeafContents::AmbientAgent(AmbientAgentPaneSnapshot {
                        uuid: vec![4, 5, 6],
                        task_id: Some(ambient_task_id()),
                    }),
                }),
            ),
        ],
    });
    window
}

fn split_panes(window: &WindowSnapshot) -> (&TerminalPaneSnapshot, &AmbientAgentPaneSnapshot) {
    let PaneNodeSnapshot::Branch(branch) = &window.tabs[0].root else {
        panic!("the split fixture window has a branch root");
    };
    let mut terminal = None;
    let mut ambient = None;
    for (_, child) in &branch.children {
        let PaneNodeSnapshot::Leaf(leaf) = child else {
            panic!("the split fixture branch holds only leaves");
        };
        match &leaf.contents {
            LeafContents::Terminal(pane) => terminal = Some(pane),
            LeafContents::AmbientAgent(pane) => ambient = Some(pane),
            other => panic!("unexpected leaf contents in the split fixture: {other:?}"),
        }
    }
    (
        terminal.expect("the split fixture has a terminal pane"),
        ambient.expect("the split fixture has an ambient agent pane"),
    )
}

#[test]
fn cloning_releases_identities_inside_a_split_including_the_ambient_agent_pane() {
    let dir = tempfile::tempdir().expect("session directory should be created");
    let source = session_source("/cfg/OBACHAN.toml", 1, 0);
    save_to_dir(
        dir.path(),
        &source,
        &split_window(source.clone()),
        &HashMap::new(),
        false,
        1_767_225_600,
    )
    .expect("the source checkpoint should be written");

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
    let (clone_terminal, clone_ambient) = split_panes(&restored.window);
    let (original_terminal, original_ambient) = split_panes(&original.window);

    assert_ne!(clone_terminal.uuid, original_terminal.uuid);
    assert!(!clone_terminal.uuid.is_empty());
    assert_ne!(
        clone_ambient.uuid, original_ambient.uuid,
        "the clone must not share the original's ambient pane uuid: block lists are keyed by it"
    );
    assert!(!clone_ambient.uuid.is_empty());
    assert_eq!(
        clone_ambient.task_id, None,
        "the clone must start its own ambient conversation, not join the original's"
    );
    assert_eq!(original_ambient.task_id, Some(ambient_task_id()));
}
