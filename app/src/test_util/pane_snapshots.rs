use uuid::Uuid;

use crate::app_state::{LeafContents, LeafSnapshot, PaneNodeSnapshot, TerminalPaneSnapshot};
use crate::terminal::ShellLaunchData;

/// One restored terminal pane, with no blocks and no conversations, carrying
/// the given `cwd` and shell.
///
/// `shell_launch_data: None` means "whatever the user's default is" -- the same
/// thing an un-customised pane records.
pub fn terminal_leaf_snapshot(
    cwd: Option<String>,
    shell_launch_data: Option<ShellLaunchData>,
) -> PaneNodeSnapshot {
    PaneNodeSnapshot::Leaf(LeafSnapshot {
        is_focused: true,
        custom_vertical_tabs_title: None,
        contents: LeafContents::Terminal(TerminalPaneSnapshot {
            uuid: Uuid::new_v4().as_bytes().to_vec(),
            persistent_session_id: None,
            cwd,
            shell_launch_data,
            is_active: true,
            is_read_only: false,
            input_config: None,
            llm_model_override: None,
            active_profile_id: None,
            conversation_ids_to_restore: vec![],
            active_conversation_id: None,
        }),
    })
}

/// One restored terminal pane, with no blocks, no conversations, and the
/// default shell.
pub fn single_terminal_leaf_snapshot() -> PaneNodeSnapshot {
    terminal_leaf_snapshot(Some("/".to_string()), None)
}
