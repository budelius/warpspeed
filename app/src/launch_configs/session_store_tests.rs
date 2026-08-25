use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::{load_from_dir, save_to_dir};
use crate::ai::agent::conversation::AIConversationId;
use crate::app_state::{
    LeafContents, LeafSnapshot, PaneNodeSnapshot, PaneUuid, TabSnapshot, TerminalPaneSnapshot,
    WindowSnapshot,
};
use crate::launch_configs::launch_config::LaunchConfigWindowSource;
use crate::tab::SelectedTabColor;
use crate::terminal::ShellLaunchData;
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
        path: PathBuf::from("/home/warp/.warp/launch_configurations/devops.yaml"),
        document_index: 0,
        window_index: 0,
        config_name: "DEVOPS".to_string(),
    };
    let mut expected = window(source.clone());
    let block_lists = HashMap::from([(
        PaneUuid(vec![1, 2, 3]),
        vec![SerializedBlock::default().into()],
    )]);

    save_to_dir(dir.path(), &source, &expected, &block_lists, false)
        .expect("window session should save");
    expected.tabs[0].custom_title = Some("Updated agent".to_string());
    save_to_dir(dir.path(), &source, &expected, &Default::default(), true)
        .expect("window session should be replaced");
    let restored = load_from_dir(dir.path(), &source)
        .expect("window session should load")
        .expect("window session should exist");

    assert_eq!(restored.window, expected);
    assert_eq!(restored.block_lists, Arc::new(block_lists));
}
