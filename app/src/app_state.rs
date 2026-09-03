use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use pathfinder_geometry::rect::RectF;
use serde::{Deserialize, Serialize};
use warp_core::SessionId;
use warpui::platform::FullscreenState;
use warpui::{AppContext, SingletonEntity as _};

use crate::ai::agent::conversation::AIConversationId;
use crate::ai::agent_conversations_model::AgentManagementFilters;
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::ai::blocklist::{InputConfig, SerializedBlockListItem};
use crate::code::editor_management::CodeSource;
use crate::drive::OpenWarpDriveObjectSettings;
use crate::launch_configs::launch_config::LaunchConfigWindowSource;
use crate::root_view::quake_mode_window_id;
use crate::server::ids::{ServerId, SyncId};
use crate::settings_view::SettingsSection;
use crate::settings_view::environments_page::EnvironmentsPage;
use crate::tab::SelectedTabColor;
use crate::terminal::ShellLaunchData;
use crate::terminal::cli_agent::CLIAgent;
use crate::themes::theme::AnsiColorIdentifier;
use crate::workspace::WorkspaceRegistry;
use crate::workspace::tab_group::TabGroupId;
use crate::workspace::view::left_panel::ToolPanelView;

#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub windows: Vec<WindowSnapshot>,
    pub active_window_index: Option<usize>,
    pub block_lists: Arc<HashMap<PaneUuid, Vec<SerializedBlockListItem>>>,
    pub running_mcp_servers: Vec<uuid::Uuid>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PaneUuid(pub Vec<u8>);

/// Wrapper for persisting agent management filters to restore.
#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedAgentManagementFilters {
    pub filters: AgentManagementFilters,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowSnapshot {
    pub tabs: Vec<TabSnapshot>,
    pub active_tab_index: usize,
    pub team_uid: Option<ServerId>,
    #[serde(default, with = "optional_rect")]
    pub bounds: Option<RectF>,
    pub fullscreen_state: FullscreenState,
    pub quake_mode: bool,
    pub universal_search_width: Option<f32>,
    pub warp_ai_width: Option<f32>,
    pub voltron_width: Option<f32>,
    pub warp_drive_index_width: Option<f32>,
    pub left_panel_open: bool,
    pub vertical_tabs_panel_open: bool,
    pub left_panel_width: Option<f32>,
    pub right_panel_width: Option<f32>,
    pub agent_management_filters: Option<PersistedAgentManagementFilters>,
    #[serde(default)]
    pub launch_config_source: Option<LaunchConfigWindowSource>,
    /// Tab groups defined in this window. Group order is implicit from
    /// member tabs' positions, so no explicit ordering is persisted.
    pub tab_groups: Vec<TabGroupSnapshot>,
}

impl WindowSnapshot {
    #[cfg(feature = "local_tty")]
    pub(crate) fn persistent_session_ids(&self) -> Vec<SessionId> {
        fn collect(node: &PaneNodeSnapshot, ids: &mut Vec<SessionId>) {
            match node {
                PaneNodeSnapshot::Branch(branch) => {
                    for (_, child) in &branch.children {
                        collect(child, ids);
                    }
                }
                PaneNodeSnapshot::Leaf(LeafSnapshot {
                    contents: LeafContents::Terminal(terminal),
                    ..
                }) => ids.extend(terminal.persistent_session_id),
                PaneNodeSnapshot::Leaf(_) => {}
            }
        }

        let mut ids = Vec::new();
        for tab in &self.tabs {
            collect(&tab.root, &mut ids);
        }
        ids
    }

    /// Returns the agent conversations referenced by this window's terminal panes.
    pub(crate) fn conversation_ids_to_restore(&self) -> Vec<AIConversationId> {
        fn collect(node: &PaneNodeSnapshot, ids: &mut HashSet<AIConversationId>) {
            match node {
                PaneNodeSnapshot::Branch(branch) => {
                    for (_, child) in &branch.children {
                        collect(child, ids);
                    }
                }
                PaneNodeSnapshot::Leaf(LeafSnapshot {
                    contents: LeafContents::Terminal(terminal),
                    ..
                }) => {
                    ids.extend(terminal.conversation_ids_to_restore.iter().copied());
                    ids.extend(terminal.active_conversation_id);
                }
                PaneNodeSnapshot::Leaf(_) => {}
            }
        }

        let mut ids = HashSet::new();
        for tab in &self.tabs {
            collect(&tab.root, &mut ids);
        }
        ids.into_iter().collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabGroupSnapshot {
    pub id: TabGroupId,
    pub name: Option<String>,
    pub color: SelectedTabColor,
    pub collapsed: bool,
    pub pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabSnapshot {
    pub custom_title: Option<String>,
    pub root: PaneNodeSnapshot,
    pub default_directory_color: Option<AnsiColorIdentifier>,
    pub selected_color: SelectedTabColor,
    pub left_panel: Option<LeftPanelSnapshot>,
    pub right_panel: Option<RightPanelSnapshot>,
    /// Tab group this tab belongs to, if any.
    pub group_id: Option<TabGroupId>,
    /// True when this tab is pinned to the front of the tab list.
    pub pinned: bool,
}

impl TabSnapshot {
    pub(crate) fn color(&self) -> Option<AnsiColorIdentifier> {
        self.selected_color.resolve(self.default_directory_color)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(
    clippy::large_enum_variant,
    reason = "LeafSnapshot is significantly larger than BranchSnapshot due to nested snapshot types."
)]
pub enum PaneNodeSnapshot {
    Branch(BranchSnapshot),
    Leaf(LeafSnapshot),
}

impl PaneNodeSnapshot {
    pub fn has_horizontal_split(&self) -> bool {
        match self {
            PaneNodeSnapshot::Leaf(_) => false,
            PaneNodeSnapshot::Branch(BranchSnapshot {
                direction,
                children,
            }) => {
                let self_has_split = *direction == SplitDirection::Horizontal && children.len() > 1;
                self_has_split
                    || children
                        .iter()
                        .any(|(_, child)| child.has_horizontal_split())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchSnapshot {
    pub direction: SplitDirection,
    pub children: Vec<(PaneFlex, PaneNodeSnapshot)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeafSnapshot {
    pub is_focused: bool,
    pub custom_vertical_tabs_title: Option<String>,
    pub contents: LeafContents,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LeafContents {
    Terminal(TerminalPaneSnapshot),
    Notebook(NotebookPaneSnapshot),
    AIDocument(AIDocumentPaneSnapshot),
    Code(CodePaneSnapShot),
    EnvVarCollection(EnvVarCollectionPaneSnapshot),
    EnvironmentManagement(EnvironmentManagementPaneSnapshot),
    Workflow(WorkflowPaneSnapshot),
    Settings(SettingsPaneSnapshot),
    AIFact(AIFactPaneSnapshot),
    CustomRouterEditor,
    ExecutionProfileEditor,
    CodeReview(CodeReviewPaneSnapshot),
    AmbientAgent(AmbientAgentPaneSnapshot),
    /// The in-app network log pane. Not persisted across restarts because the
    /// backing log is an in-memory ring buffer that starts empty on launch.
    NetworkLog,
    /// A new first-time user experience which prioritizes choosing a coding repository.
    GetStarted,
}

#[cfg(feature = "local_fs")]
impl LeafContents {
    /// Whether this pane content should be written to (and later restored
    /// from) the SQLite app-state database.
    ///
    /// Non-persisted pane types are skipped entirely during the pane tree
    /// traversal in `save_app_state`, so no `pane_nodes` row is inserted for
    /// them. This is important: inserting a `pane_nodes` row with
    /// `is_leaf = true` but no matching `pane_leaves` row leaves an orphan
    /// that `read_node` cannot resolve, which causes the surrounding tab's
    /// restoration to fail and the whole tab to disappear on restart.
    pub(crate) fn is_persisted(&self) -> bool {
        match self {
            // Network log: the backing log is an in-memory ring buffer that
            // starts empty on launch; persisting would also regress back to
            // an on-disk log via the app-state database.
            LeafContents::NetworkLog
            // Environment management panes are opened on-demand via workspace
            // actions and have no persistable state.
            | LeafContents::EnvironmentManagement(_) => false,
            LeafContents::Terminal(_)
            | LeafContents::Notebook(_)
            | LeafContents::AIDocument(_)
            | LeafContents::Code(_)
            | LeafContents::EnvVarCollection(_)
            | LeafContents::Workflow(_)
            | LeafContents::Settings(_)
            | LeafContents::AIFact(_)
            | LeafContents::CustomRouterEditor
            | LeafContents::ExecutionProfileEditor
            | LeafContents::CodeReview(_)
            | LeafContents::AmbientAgent(_)
            | LeafContents::GetStarted => true,
        }
    }
}

/// Snapshot of an ambient agent pane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AmbientAgentPaneSnapshot {
    pub uuid: Vec<u8>,
    // `task_id` is purposefully optional,
    // as you can have a valid state (i.e. an empty cloud mode pane) where it is None.
    pub task_id: Option<AmbientAgentTaskId>,
}

/// What a pane's CLI agent was, and enough to pick it back up, for a pane whose original PTY
/// cannot be reattached -- a restored window, or a machine that has rebooted since.
///
/// Recorded for every pane that ran an agent, including ones that never reported a session ID,
/// because "Claude Code ran here and left no way back in" is itself the answer to why a session
/// did not come back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CliAgentResumeDescriptor {
    pub agent: CLIAgent,
    /// `None` when the agent was detected from its command line alone. Warp learns a session ID
    /// from the agent's Warp plugin, so an agent running without one is unresumable.
    #[serde(default)]
    pub session_id: Option<String>,
    pub cwd: PathBuf,
    /// The session's last user prompt, or its summary -- what distinguishes one saved session
    /// from the four others in the same directory.
    #[serde(default)]
    pub title: Option<String>,
    /// When this descriptor was captured, as a Unix timestamp in seconds.
    #[serde(default)]
    pub captured_unix_seconds: Option<i64>,
}

impl CliAgentResumeDescriptor {
    /// The shell command that reopens this session, if the agent's CLI can be told to resume a
    /// specific session by ID.
    ///
    /// Only agents whose syntax has actually been verified against the shipped CLI appear here.
    /// A guessed flag is worse than none: it runs, fails or starts a fresh session, and the
    /// session ID it was supposed to recover is the thing it just scrolled off the screen.
    /// Everything else is recorded rather than invoked -- [`Self::session_id`] and the agent are
    /// in the pane snapshot, the log, and the session registry, which is enough to recover by
    /// hand.
    pub(crate) fn resume_command(&self) -> Option<String> {
        let session_id = self.session_id.as_deref()?;
        match self.agent {
            CLIAgent::Claude => Some(format!("claude --resume {session_id}")),
            CLIAgent::Codex => Some(format!("codex resume {session_id}")),
            CLIAgent::Vibe => Some(format!("vibe --resume {session_id}")),
            CLIAgent::Pi => Some(format!("pi --session {session_id}")),
            // `gemini --resume` takes "latest" or a per-project index, never a session ID.
            CLIAgent::Gemini
            // `opencode session` can list and delete by ID but cannot open one.
            | CLIAgent::OpenCode
            | CLIAgent::Amp
            | CLIAgent::Droid
            | CLIAgent::Copilot
            | CLIAgent::OhMyPi
            | CLIAgent::Auggie
            | CLIAgent::CursorCli
            | CLIAgent::Goose
            | CLIAgent::Hermes
            | CLIAgent::Antigravity
            | CLIAgent::WarpTui
            | CLIAgent::Unknown => None,
        }
    }
}

/// Snapshot of the contents of a terminal pane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerminalPaneSnapshot {
    pub uuid: Vec<u8>,
    #[serde(default)]
    pub persistent_session_id: Option<SessionId>,
    #[serde(default)]
    pub cli_agent_resume_descriptor: Option<CliAgentResumeDescriptor>,
    pub cwd: Option<String>,
    pub shell_launch_data: Option<ShellLaunchData>,
    pub is_active: bool,
    pub is_read_only: bool,
    pub input_config: Option<InputConfig>,
    pub llm_model_override: Option<String>,
    pub active_profile_id: Option<SyncId>,
    pub conversation_ids_to_restore: Vec<AIConversationId>,
    /// The active conversation ID if the agent view was open in fullscreen mode.
    /// When `Some`, the agent view should be restored to fullscreen for this conversation.
    pub active_conversation_id: Option<AIConversationId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NotebookPaneSnapshot {
    CloudNotebook {
        /// The ID of the notebook that was open in this pane. There are 3 possibilities:
        /// 1. The pane contains a newly-created notebook that has not been edited yet. It might not
        ///    have an ID yet (client or server), so this will be `None`.
        /// 2. The pane contains a notebook that hasn't been synced to the server yet, so this will
        ///    contain a client ID that should exist in SQLite.
        /// 3. The pane contains a notebook that's known to the server, so this will contain the
        ///    server ID.
        notebook_id: Option<SyncId>,
        // Settings for the notebook pane when it's opened (such as a folder to focus upon opening)
        settings: OpenWarpDriveObjectSettings,
    },
    LocalFileNotebook {
        /// The path to the local file that was open in this pane. This may be `None` if
        /// the pane contained an unreadable file.
        path: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AIDocumentPaneSnapshot {
    Local {
        document_id: String,
        version: i32,
        content: Option<String>,
        title: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodePaneTabSnapshot {
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CodePaneSnapShot {
    Local {
        tabs: Vec<CodePaneTabSnapshot>,
        active_tab_index: usize,
        /// The full `CodeSource` for this pane, serialized as JSON in the DB.
        source: Option<CodeSource>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum WorkflowPaneSnapshot {
    CloudWorkflow {
        workflow_id: Option<SyncId>,
        // Settings for the workflow pane when it's opened (such as a folder to focus upon opening)
        settings: OpenWarpDriveObjectSettings,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EnvVarCollectionPaneSnapshot {
    // CloudEnvVarCollection snapshots operate under the same heuristics
    // as NotebookPaneSnapshot::CloudNotebook
    CloudEnvVarCollection {
        env_var_collection_id: Option<SyncId>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentManagementPaneSnapshot {
    pub mode: EnvironmentsPage,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SettingsPaneSnapshot {
    Local {
        current_page: SettingsSection,
        search_query: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AIFactPaneSnapshot {
    Personal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CodeReviewPaneSnapshot {
    Local {
        terminal_uuid: Vec<u8>,
        repo_path: PathBuf,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LeftPanelDisplayedTab {
    FileTree,
    GlobalSearch,
    WarpDrive,
    ConversationListView,
}

impl From<ToolPanelView> for LeftPanelDisplayedTab {
    fn from(view: ToolPanelView) -> Self {
        match view {
            ToolPanelView::ProjectExplorer => LeftPanelDisplayedTab::FileTree,
            ToolPanelView::GlobalSearch { .. } => LeftPanelDisplayedTab::GlobalSearch,
            ToolPanelView::WarpDrive => LeftPanelDisplayedTab::WarpDrive,
            ToolPanelView::ConversationListView => LeftPanelDisplayedTab::ConversationListView,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeftPanelSnapshot {
    pub left_panel_displayed_tab: LeftPanelDisplayedTab,
    pub pane_group_id: String,
    pub width: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RightPanelSnapshot {
    pub pane_group_id: String,
    pub width: usize,
    pub is_maximized: bool,
}

/// Copied from pane group model, which should be private to pane group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneFlex(pub f32);

mod optional_rect {
    use pathfinder_geometry::rect::RectF;
    use pathfinder_geometry::vector::Vector2F;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    }

    pub fn serialize<S>(rect: &Option<RectF>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        rect.map(|rect| Rect {
            x: rect.origin().x(),
            y: rect.origin().y(),
            width: rect.size().x(),
            height: rect.size().y(),
        })
        .serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<RectF>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<Rect>::deserialize(deserializer).map(|rect| {
            rect.map(|rect| {
                RectF::new(
                    Vector2F::new(rect.x, rect.y),
                    Vector2F::new(rect.width, rect.height),
                )
            })
        })
    }
}

pub fn get_app_state(app: &AppContext) -> AppState {
    let active_window_id = app.windows().active_window();
    let quake_mode_id = quake_mode_window_id();

    let mut active_window_index = None;

    let mut windows = vec![];

    for (index, window_id) in app.window_ids().enumerate() {
        // Determine index of active window
        if let Some(active_window_id) = active_window_id
            && active_window_id == window_id
        {
            active_window_index = Some(index);
        }

        if let Some(workspace) = WorkspaceRegistry::as_ref(app).get(window_id, app) {
            let ws = workspace.as_ref(app);
            // Transient drag-preview windows are not real user-visible
            // workspaces; skip them so they never end up in the persisted
            // session. (Persistence is also short-circuited entirely while a
            // cross-window drag is active; see `save_app` in
            // `workspace/global_actions.rs`.)
            if ws.is_tab_drag_preview() {
                continue;
            }
            let snapshot = ws.snapshot(
                window_id,
                quake_mode_id.map(|id| id == window_id).unwrap_or(false),
                app,
            );
            if !snapshot.tabs.is_empty() {
                windows.push(snapshot);
            }
        }
    }

    AppState {
        windows,
        active_window_index,
        block_lists: Default::default(),
        running_mcp_servers: Vec::new(),
    }
}

#[cfg(test)]
#[path = "app_state_tests.rs"]
mod tests;
