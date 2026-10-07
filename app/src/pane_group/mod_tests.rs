use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use ai::index::full_source_code_embedding::manager::CodebaseIndexManager;
use ai::project_context::model::ProjectContextModel;
use chrono::Utc;
use instant::Instant;
use pathfinder_geometry::rect::RectF;
use persistence::model::{
    AgentConversation, AgentConversationData, AgentConversationRecord, ConversationUsageMetadata,
};
#[cfg(feature = "local_fs")]
use repo_metadata::RepoMetadataModel;
use repo_metadata::repositories::DetectedRepositories;
use repo_metadata::watcher::DirectoryWatcher;
use session_sharing_protocol::common::SessionId;
use shared_session::permissions_manager::SessionPermissionsManager;
use uuid::Uuid;
use warp_core::features::FeatureFlag;
use warp_server_client::iap::IapManager;
use warpui::platform::{WindowBounds, WindowStyle};
use warpui::windowing::WindowManager;
use warpui::windowing::state::ApplicationStage;
use warpui::{App, ModelHandle};
use watcher::HomeDirectoryWatcher;

use super::child_agent::restoration::is_stale_ancestor_list_completion;
use super::child_agent::{
    HiddenChildAgentConversationRequest, HiddenChildAgentTaskContext,
    create_hidden_child_agent_conversation,
};
use super::*;
use crate::ai::AIRequestUsageModel;
use crate::ai::active_agent_views_model::ActiveAgentViewsModel;
use crate::ai::agent::api::ServerConversationToken;
use crate::ai::agent::conversation::{
    AIAgentHarness, AIConversation, AIConversationId, ServerAIConversationMetadata,
};
use crate::ai::agent_conversations_model::AgentConversationsModel;
use crate::ai::ambient_agents::github_auth_notifier::GitHubAuthNotifier;
use crate::ai::ambient_agents::task::TaskPrincipalInfo;
use crate::ai::ambient_agents::{
    AgentSource, AmbientAgentTask, AmbientAgentTaskId, AmbientAgentTaskState,
};
use crate::ai::blocklist::agent_view::AgentViewEntryOrigin;
use crate::ai::blocklist::history_model::CloudConversationData;
use crate::ai::blocklist::local_agent_task_sync_model::LocalAgentTaskSyncModel;
use crate::ai::blocklist::orchestration_event_streamer::OrchestrationEventStreamer;
use crate::ai::blocklist::orchestration_events::OrchestrationEventService;
use crate::ai::blocklist::orchestration_topology::descendant_conversation_ids_in_spawn_order;
use crate::ai::blocklist::{BlocklistAIHistoryModel, QueuedQueryModel};
use crate::ai::cloud_environments::CloudEnvironmentCatalog;
use crate::ai::document::ai_document_model::AIDocumentModel;
use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::ai::harness_availability::HarnessAvailabilityModel;
use crate::ai::llms::LLMPreferences;
use crate::ai::mcp::templatable_manager::TemplatableMCPServerManager;
use crate::ai::mcp::{FileBasedMCPManager, FileMCPWatcher};
use crate::ai::outline::RepoOutlines;
use crate::ai::persisted_workspace::PersistedWorkspace;
use crate::ai::restored_conversations::RestoredAgentConversations;
use crate::ai::skills::SkillManager;
use crate::app_state::{BranchSnapshot, PaneFlex, SessionHolder, SplitDirection};
use crate::auth::auth_manager::AuthManager;
use crate::auth::user::TEST_USER_UID;
use crate::changelog_model::ChangelogModel;
use crate::cloud_object::model::persistence::CloudModel;
use crate::cloud_object::{Owner, Revision, ServerMetadata, ServerPermissions};
use crate::context_chips::prompt::Prompt;
use crate::network::NetworkStatus;
use crate::notebooks::editor::keys::NotebookKeybindings;
use crate::notebooks::manager::NotebookManager;
use crate::notebooks::notebook::NotebookView;
use crate::pricing::PricingInfoModel;
use crate::resource_center::TipsCompleted;
use crate::search::files::model::FileSearchModel;
use crate::server::cloud_objects::listener::Listener;
use crate::server::cloud_objects::update_manager::UpdateManager;
use crate::server::ids::ServerId;
use crate::server::server_api::ServerApiProvider;
use crate::server::server_api::presigned_upload::HttpStatusError;
use crate::server::sync_queue::SyncQueue;
use crate::server::telemetry::context_provider::AppTelemetryContextProvider;
use crate::settings::PrivacySettings;
use crate::settings_view::keybindings::KeybindingChangedNotifier;
use crate::suggestions::ignored_suggestions_model::IgnoredSuggestionsModel;
use crate::system::SystemStats;
use crate::terminal::alt_screen_reporting::AltScreenReporting;
use crate::terminal::available_shells::AvailableShells;
use crate::terminal::cli_agent_sessions::CLIAgentSessionsModel;
use crate::terminal::history::History;
use crate::terminal::keys::TerminalKeybindings;
use crate::terminal::local_tty::TerminalManager;
use crate::terminal::local_tty::spawner::PtySpawner;
use crate::terminal::local_tty::terminal_manager::native_startup_directory;
use crate::terminal::model::terminal_model::{ConversationTranscriptViewerStatus, ExitReason};
use crate::terminal::resizable_data::ResizableData;
use crate::terminal::shared_session::{
    IsSharedSessionCreator, SharedSessionActionSource, SharedSessionScrollbackType,
    SharedSessionSource, SharedSessionStatus,
};
use crate::terminal::shell::ShellType;
use crate::terminal::shell_start_policy::ShellStartPolicy;
use crate::terminal::terminal_manager::{HeldVerdict, WithheldRestore};
use crate::terminal::view::Event as TerminalViewEvent;
use crate::terminal::{ShellLaunchData, TerminalSurface};
use crate::test_util::assert_eventually_within;
use crate::test_util::pane_snapshots::{single_terminal_leaf_snapshot, terminal_leaf_snapshot};
use crate::test_util::settings::initialize_settings_for_tests;
use crate::undo_close::UndoCloseStack;
use crate::warp_managed_paths_watcher::WarpManagedPathsWatcher;
use crate::workflows::local_workflows::LocalWorkflows;
use crate::workspace::sync_inputs::SyncedInputState;
use crate::workspace::{ActiveSession, OneTimeModalModel, WorkspaceRegistry};
use crate::workspaces::team_tester::TeamTesterStatus;
use crate::workspaces::update_manager::TeamUpdateManager;
use crate::workspaces::user_profiles::UserProfiles;
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::{
    AgentNotificationsModel, GlobalResourceHandles, GlobalResourceHandlesProvider, experiments,
};

fn initialize_app(app: &mut App) {
    initialize_app_with_history(app, Vec::new());
}

fn initialize_app_with_history(app: &mut App, conversations: Vec<AgentConversation>) {
    initialize_settings_for_tests(app);

    app.add_singleton_model(|_ctx| ServerApiProvider::new_for_test());
    // The shell-death path reads this for telemetry. Inert in tests: the real
    // constructor kicks off a background antivirus scan on Windows.
    app.add_singleton_model(|_ctx| crate::antivirus::AntivirusInfo::new_for_test());
    // Read by the same shell-death telemetry path.
    app.add_singleton_model(crate::system::SystemInfo::new);
    // Disabled (`None`) IapManager so shared-session viewer code that reads the
    // singleton doesn't panic in tests; it is an inert no-op.
    app.add_singleton_model(|ctx| {
        IapManager::new(
            None,
            Box::new(|_| futures::FutureExt::boxed(futures::future::ready(None::<String>))),
            None,
            ctx,
        )
    });
    app.add_singleton_model(|ctx| ChangelogModel::new(ServerApiProvider::as_ref(ctx).get()));
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(AppTelemetryContextProvider::new_context_provider);
    app.add_singleton_model(AuthManager::new_for_test);
    app.add_singleton_model(|_ctx| PtySpawner::new_for_test());
    app.add_singleton_model(|_| NetworkStatus::new());
    app.add_singleton_model(|_| SystemStats::new());
    app.add_singleton_model(SyncQueue::mock);
    app.add_singleton_model(CloudModel::mock);
    app.add_singleton_model(CloudEnvironmentCatalog::new);
    app.add_singleton_model(UserWorkspaces::default_mock);
    app.add_singleton_model(TeamTesterStatus::mock);
    app.add_singleton_model(TeamUpdateManager::mock);
    app.add_singleton_model(Listener::mock);
    app.add_singleton_model(UpdateManager::mock);

    // Initialize file-based MCP dependencies.
    app.add_singleton_model(|_| DetectedRepositories::default());
    app.add_singleton_model(HomeDirectoryWatcher::new_for_test);
    app.add_singleton_model(DirectoryWatcher::new);
    app.add_singleton_model(WarpManagedPathsWatcher::new_for_testing);
    app.add_singleton_model(FileMCPWatcher::new);
    app.add_singleton_model(|_| FileBasedMCPManager::default());

    app.add_singleton_model(|_| TemplatableMCPServerManager::default());
    app.add_singleton_model(|_ctx| UserProfiles::new(Vec::new()));
    app.add_singleton_model(|_| Appearance::mock());
    app.add_singleton_model(PrivacySettings::mock);
    app.add_singleton_model(|_ctx| SyncedInputState::mock());
    app.add_singleton_model(LocalWorkflows::new);
    app.add_singleton_model(|_| Prompt::mock());
    app.add_singleton_model(|_| ResizableData::default());
    app.add_singleton_model(NotebookManager::mock);
    app.add_singleton_model(shared_session::manager::Manager::new);
    app.add_singleton_model(|_| ActiveSession::default());
    let global_resources = GlobalResourceHandles::mock(app);
    app.add_singleton_model(|_| GlobalResourceHandlesProvider::new(global_resources.clone()));
    app.add_singleton_model(|_| KeybindingChangedNotifier::new());
    app.add_singleton_model(NotebookKeybindings::new);
    app.add_singleton_model(TerminalKeybindings::new);
    app.add_singleton_model(move |_| BlocklistAIHistoryModel::new(vec![], vec![], &conversations));
    // QueuedQueryModel subscribes to history events; register after the
    // history model is in place.
    app.add_singleton_model(QueuedQueryModel::new);
    // Pill bar model subscribes to history events; register after the
    // history model is in place.
    app.add_singleton_model(|ctx| {
        crate::ai::blocklist::agent_view::orchestration_pill_bar_model::OrchestrationPillBarModel::new(
            Default::default(),
            ctx,
        )
    });
    app.add_singleton_model(|_| CLIAgentSessionsModel::new());
    app.add_singleton_model(|_| {
        crate::terminal::agent_session_history_model::AgentSessionHistoryModel::new()
    });
    app.add_singleton_model(OrchestrationEventService::new);
    app.add_singleton_model(LocalAgentTaskSyncModel::new);
    app.add_singleton_model(OrchestrationEventStreamer::new);
    app.add_singleton_model(|_| ActiveAgentViewsModel::new());
    app.add_singleton_model(crate::ai::blocklist::BlocklistAIPermissions::new);
    app.add_singleton_model(AgentNotificationsModel::new);
    app.add_singleton_model(|ctx| {
        AIExecutionProfilesModel::new(&crate::LaunchMode::new_for_unit_test(), ctx)
    });
    app.add_singleton_model(|ctx| {
        AIRequestUsageModel::new_for_test(ServerApiProvider::as_ref(ctx).get_ai_client(), ctx)
    });
    app.add_singleton_model(SessionPermissionsManager::new);
    app.add_singleton_model(LLMPreferences::new);
    app.add_singleton_model(HarnessAvailabilityModel::new);
    #[cfg(feature = "voice_input")]
    app.add_singleton_model(voice_input::VoiceInput::new);
    #[cfg(feature = "local_fs")]
    app.add_singleton_model(RepoMetadataModel::new);
    app.add_singleton_model(SkillManager::new);
    app.add_singleton_model(FileSearchModel::new);
    app.add_singleton_model(|_| crate::code_review::git_repo_model::GitRepoModels::new());
    app.add_singleton_model(RepoOutlines::new_for_test);
    crate::terminal::available_shells::register(app);
    app.update(experiments::init);
    AltScreenReporting::register(app);
    app.add_singleton_model(|ctx| {
        CodebaseIndexManager::new_for_test(ServerApiProvider::as_ref(ctx).get(), ctx)
    });
    app.add_singleton_model(|ctx| PersistedWorkspace::new(vec![], HashMap::new(), None, ctx));
    app.add_singleton_model(|_| ProjectContextModel::default());
    app.add_singleton_model(|ctx| crate::ai::agent_tips::AITipModel::new_for_agent_tips(ctx));
    app.add_singleton_model(|_| RestoredAgentConversations::new_seeded(vec![]));
    app.add_singleton_model(OneTimeModalModel::new);
    app.add_singleton_model(|_| WorkspaceRegistry::new());
    app.add_singleton_model(UndoCloseStack::new);
    app.add_singleton_model(|_| IgnoredSuggestionsModel::new(vec![]));
    app.add_singleton_model(|_| PricingInfoModel::new());
    app.add_singleton_model(crate::ai::pricing_promotion::PricingPromotionState::new);
    app.add_singleton_model(AIDocumentModel::new);
    app.add_singleton_model(|_| History::new(vec![]));
    app.add_singleton_model(|_| GitHubAuthNotifier::new());
    app.add_singleton_model(AgentConversationsModel::new);
    app.add_singleton_model(remote_server::manager::RemoteServerManager::new);
}

struct MockOptions {
    layout: PanesLayout,
    window_bounds: WindowBounds,
}

impl Default for MockOptions {
    fn default() -> Self {
        Self {
            layout: Default::default(),
            window_bounds: WindowBounds::ExactPosition(RectF::new(
                Vector2F::zero(),
                Vector2F::new(1024., 768.),
            )),
        }
    }
}

fn mock_pane_group(app: &mut App, options: MockOptions) -> ViewHandle<PaneGroup> {
    let tips_model = app.add_model(|_| TipsCompleted::default());
    let (_, pane_group) =
        app.add_window_with_bounds(WindowStyle::NotStealFocus, options.window_bounds, |ctx| {
            let user_default_shell_changed_banner_dismissal_model_handle =
                ctx.add_model(|_| BannerState::default());
            let block_lists = Arc::new(HashMap::new());
            PaneGroup::new_with_panes_layout(
                tips_model,
                user_default_shell_changed_banner_dismissal_model_handle,
                ServerApiProvider::as_ref(ctx).get(),
                options.layout,
                block_lists,
                None,
                ctx,
            )
        });
    pane_group
}

fn get_newly_created_pane_id(panes: &PaneGroup, existing_ids: &[PaneId]) -> PaneId {
    panes
        .pane_ids()
        .find(|id| !existing_ids.contains(id))
        .unwrap()
}

fn split_pane_state(panes: &PaneGroup, pane_id: PaneId, ctx: &AppContext) -> SplitPaneState {
    panes
        .focus_state_handle()
        .as_ref(ctx)
        .split_pane_state_for(pane_id)
}

fn is_active_session(panes: &PaneGroup, pane_id: PaneId, ctx: &AppContext) -> bool {
    panes.active_session_id(ctx).map(Into::into) == Some(pane_id)
}

fn new_notebook(ctx: &mut ViewContext<PaneGroup>) -> ViewHandle<NotebookView> {
    ctx.add_typed_action_view(NotebookView::new)
}

fn new_ambient_agent_task_id() -> AmbientAgentTaskId {
    Uuid::new_v4().to_string().parse().unwrap()
}

fn ambient_agent_task_for_current_user(task_id: AmbientAgentTaskId) -> AmbientAgentTask {
    let now = Utc::now();
    AmbientAgentTask {
        task_id,
        parent_run_id: None,
        title: "Owned task".to_string(),
        state: AmbientAgentTaskState::Succeeded,
        prompt: "test".to_string(),
        created_at: now,
        started_at: Some(now),
        updated_at: now,
        run_time: Some("PT1S".parse().unwrap()),
        status_message: None,
        source: Some(AgentSource::CloudMode),
        execution_location: None,
        session_id: None,
        session_link: None,
        executor: None,
        creator: Some(TaskPrincipalInfo {
            creator_type: "USER".to_string(),
            uid: TEST_USER_UID.to_string(),
            display_name: None,
        }),
        conversation_id: None,
        request_usage: None,
        is_sandbox_running: false,
        agent_config_snapshot: None,
        artifacts: vec![],
        last_event_sequence: None,
        children: vec![],
    }
}

/// Builds an *attachable* ambient task (InProgress + running sandbox +
/// parseable session id) so the unified child-pane dispatch resolves to
/// `AttachLive`.
fn attachable_ambient_agent_task(task_id: AmbientAgentTaskId) -> AmbientAgentTask {
    let mut task = ambient_agent_task_for_current_user(task_id);
    task.state = AmbientAgentTaskState::InProgress;
    task.is_sandbox_running = true;
    task.session_id = Some("22222222-2222-2222-2222-222222222222".to_string());
    task
}

fn mock_server_metadata() -> ServerMetadata {
    ServerMetadata {
        uid: ServerId::default(),
        revision: Revision::now(),
        metadata_last_updated_ts: Utc::now().into(),
        trashed_ts: None,
        folder_id: None,
        is_welcome_object: false,
        creator_uid: None,
        last_editor_uid: None,
        current_editor_uid: None,
    }
}

fn mock_server_permissions() -> ServerPermissions {
    ServerPermissions {
        space: Owner::mock_current_user(),
        guests: Vec::new(),
        anyone_link_sharing: None,
        permissions_last_updated_ts: Utc::now().into(),
    }
}

fn test_server_conversation_metadata(
    task_id: Option<AmbientAgentTaskId>,
) -> ServerAIConversationMetadata {
    ServerAIConversationMetadata {
        title: "Restored cloud conversation".to_string(),
        working_directory: None,
        harness: AIAgentHarness::Oz,
        usage: ConversationUsageMetadata {
            was_summarized: false,
            context_window_usage: 0.0,
            credits_spent: 0.0,
            platform_credits_spent: 0.0,
            total_provider_cost_in_cents: None,
            credits_spent_for_last_block: None,
            charged_usage_for_last_block: None,
            total_charged_usage: None,
            token_usage: vec![],
            tool_usage_metadata: Default::default(),
            context_window_segments: Vec::new(),
        },
        metadata: mock_server_metadata(),
        creator: None,
        permissions: mock_server_permissions(),
        ambient_agent_task_id: task_id,
        server_conversation_token: ServerConversationToken::new("test-server-token".to_string()),
        artifacts: Vec::new(),
    }
}

fn cloud_conversation_with_ambient_task(task_id: AmbientAgentTaskId) -> CloudConversationData {
    let mut conversation = AIConversation::new(false, false);
    conversation.set_task_id(task_id);
    conversation.set_server_metadata(test_server_conversation_metadata(Some(task_id)));
    CloudConversationData::Oz(Box::new(conversation))
}

fn persisted_remote_child_conversation(
    conversation_id: AIConversationId,
    parent_conversation_id: Option<AIConversationId>,
    parent_agent_id: Option<String>,
    task_id: AmbientAgentTaskId,
) -> AgentConversation {
    AgentConversation {
        conversation: AgentConversationRecord {
            id: 0,
            conversation_id: conversation_id.to_string(),
            conversation_data: serde_json::to_string(&AgentConversationData {
                server_conversation_token: Some("restored-child-token".to_string()),
                conversation_usage_metadata: None,
                reverted_action_ids: None,
                forked_from_server_conversation_token: None,
                artifacts_json: None,
                parent_agent_id,
                agent_name: Some("Agent 1".to_string()),
                orchestration_harness_type: None,
                parent_conversation_id: parent_conversation_id.map(|id| id.to_string()),
                is_remote_child: true,
                root_task_is_optimistic: None,
                run_id: Some(task_id.to_string()),
                autoexecute_override: None,
                last_event_sequence: None,
                pinned: false,
            })
            .expect("conversation data should serialize"),
            last_modified_at: Utc::now().naive_utc(),
            summary: None,
        },
        tasks: vec![warp_multi_agent_api::Task {
            id: Uuid::new_v4().to_string(),
            messages: vec![],
            dependencies: None,
            description: String::new(),
            summary: String::new(),
            server_data: String::new(),
        }],
    }
}

fn start_parent_conversation(
    panes: &PaneGroup,
    parent_pane_id: PaneId,
    ctx: &mut ViewContext<PaneGroup>,
) -> AIConversationId {
    let parent_terminal_view_id = panes
        .terminal_view_from_pane_id(parent_pane_id, ctx)
        .expect("parent pane should have a terminal view")
        .id();
    start_parent_conversation_for_terminal_view(parent_terminal_view_id, ctx)
}

fn start_parent_conversation_for_terminal_view(
    terminal_view_id: EntityId,
    ctx: &mut ViewContext<PaneGroup>,
) -> AIConversationId {
    BlocklistAIHistoryModel::handle(ctx).update(ctx, |history_model, ctx| {
        history_model.start_new_conversation(terminal_view_id, false, false, false, ctx)
    })
}
fn restore_conversation_for_terminal_view(
    terminal_view_id: EntityId,
    conversation: AIConversation,
    ctx: &mut ViewContext<PaneGroup>,
) -> AIConversationId {
    let conversation_id = conversation.id();

    BlocklistAIHistoryModel::handle(ctx).update(ctx, |history_model, ctx| {
        history_model.restore_conversations(terminal_view_id, vec![conversation], ctx);
    });

    conversation_id
}

fn restore_child_conversation_for_terminal_view(
    terminal_view_id: EntityId,
    parent_conversation_id: AIConversationId,
    ctx: &mut ViewContext<PaneGroup>,
) -> AIConversationId {
    let mut child_conversation = AIConversation::new(false, false);
    child_conversation.set_parent_conversation_id(parent_conversation_id);
    restore_conversation_for_terminal_view(terminal_view_id, child_conversation, ctx)
}

fn restore_child_conversation_with_task_context_for_terminal_view(
    terminal_view_id: EntityId,
    parent_conversation_id: AIConversationId,
    task_id: AmbientAgentTaskId,
    ctx: &mut ViewContext<PaneGroup>,
) -> AIConversationId {
    let mut child_conversation = AIConversation::new(false, false);
    child_conversation.set_parent_conversation_id(parent_conversation_id);
    child_conversation.set_task_id(task_id);
    restore_conversation_for_terminal_view(terminal_view_id, child_conversation, ctx)
}

fn restore_remote_child_conversation_for_terminal_view(
    terminal_view_id: EntityId,
    parent_conversation_id: AIConversationId,
    task_id: AmbientAgentTaskId,
    ctx: &mut ViewContext<PaneGroup>,
) -> AIConversationId {
    let mut child_conversation = AIConversation::new(false, false);
    child_conversation.set_parent_conversation_id(parent_conversation_id);
    child_conversation.set_task_id(task_id);
    child_conversation.mark_as_remote_child();
    restore_conversation_for_terminal_view(terminal_view_id, child_conversation, ctx)
}
fn restore_child_conversation(
    panes: &PaneGroup,
    pane_id: PaneId,
    parent_conversation_id: AIConversationId,
    ctx: &mut ViewContext<PaneGroup>,
) -> AIConversationId {
    let terminal_view_id = panes
        .terminal_view_from_pane_id(pane_id, ctx)
        .expect("child pane should have a terminal view")
        .id();
    restore_child_conversation_for_terminal_view(terminal_view_id, parent_conversation_id, ctx)
}

fn restore_child_conversation_with_task_context(
    panes: &PaneGroup,
    pane_id: PaneId,
    parent_conversation_id: AIConversationId,
    task_id: AmbientAgentTaskId,
    ctx: &mut ViewContext<PaneGroup>,
) -> AIConversationId {
    let terminal_view_id = panes
        .terminal_view_from_pane_id(pane_id, ctx)
        .expect("child pane should have a terminal view")
        .id();
    restore_child_conversation_with_task_context_for_terminal_view(
        terminal_view_id,
        parent_conversation_id,
        task_id,
        ctx,
    )
}

fn restore_remote_child_conversation(
    panes: &PaneGroup,
    pane_id: PaneId,
    parent_conversation_id: AIConversationId,
    task_id: AmbientAgentTaskId,
    ctx: &mut ViewContext<PaneGroup>,
) -> AIConversationId {
    let terminal_view_id = panes
        .terminal_view_from_pane_id(pane_id, ctx)
        .expect("child pane should have a terminal view")
        .id();
    restore_remote_child_conversation_for_terminal_view(
        terminal_view_id,
        parent_conversation_id,
        task_id,
        ctx,
    )
}

fn enter_agent_view_for_conversation(
    panes: &PaneGroup,
    pane_id: PaneId,
    conversation_id: AIConversationId,
    ctx: &mut ViewContext<PaneGroup>,
) {
    panes
        .terminal_view_from_pane_id(pane_id, ctx)
        .expect("pane should have a terminal view")
        .update(ctx, |terminal_view, ctx| {
            terminal_view.enter_agent_view_for_conversation(
                None,
                AgentViewEntryOrigin::RestoreExistingConversation,
                conversation_id,
                ctx,
            );
        });
}

fn create_already_fullscreen_parent_pane_data(
    panes: &PaneGroup,
    ctx: &mut ViewContext<PaneGroup>,
) -> (TerminalPane, PaneId, AIConversationId) {
    let (pane_data, terminal_view) = panes.create_terminal_pane_data(
        None,
        HashMap::new(),
        IsSharedSessionCreator::No,
        None,
        None,
        ctx,
    );
    let pane_id = pane_data.terminal_pane_id().into();
    let parent_conversation_id =
        start_parent_conversation_for_terminal_view(terminal_view.id(), ctx);
    let child_conversation_id = restore_child_conversation_for_terminal_view(
        terminal_view.id(),
        parent_conversation_id,
        ctx,
    );

    terminal_view.update(ctx, |terminal_view, ctx| {
        terminal_view.enter_agent_view_for_conversation(
            None,
            AgentViewEntryOrigin::RestoreExistingConversation,
            parent_conversation_id,
            ctx,
        );
    });

    (pane_data, pane_id, child_conversation_id)
}

fn request_ambient_agent_task_id_for_hidden_child(
    panes: &PaneGroup,
    child_pane_id: PaneId,
    ctx: &mut ViewContext<PaneGroup>,
) -> Option<AmbientAgentTaskId> {
    let terminal_view = panes
        .terminal_view_from_pane_id(child_pane_id, ctx)
        .expect("child pane should have a terminal view");
    let ai_controller = terminal_view.as_ref(ctx).ai_controller().clone();

    ai_controller.update(ctx, |controller, _| controller.get_ambient_agent_task_id())
}

fn ambient_child_session_state(
    panes: &PaneGroup,
    child_pane_id: PaneId,
    ctx: &mut ViewContext<PaneGroup>,
) -> (Option<AmbientAgentTaskId>, bool, Option<AIConversationId>) {
    let terminal_view = panes
        .terminal_view_from_pane_id(child_pane_id, ctx)
        .expect("child pane should have a terminal view");
    let terminal_view_ref = terminal_view.as_ref(ctx);
    let active_conversation_id = terminal_view_ref.active_conversation_id(ctx);
    let ambient_model = terminal_view_ref
        .ambient_agent_view_model()
        .expect("child pane should have an ambient agent model")
        .as_ref(ctx);

    (
        ambient_model.task_id(),
        ambient_model.is_agent_running(),
        active_conversation_id,
    )
}

struct PreAttachReturnsFalsePane {
    pane_id: PaneId,
    pane_configuration: ModelHandle<PaneConfiguration>,
}

impl PreAttachReturnsFalsePane {
    fn new(ctx: &mut ViewContext<PaneGroup>) -> Self {
        Self {
            pane_id: PaneId::dummy_pane_id(),
            pane_configuration: ctx.add_model(|_ctx| PaneConfiguration::new("")),
        }
    }
}

impl pane::PaneContent for PreAttachReturnsFalsePane {
    fn id(&self) -> PaneId {
        self.pane_id
    }

    fn pre_attach(&self, _group: &PaneGroup, _ctx: &mut ViewContext<PaneGroup>) -> bool {
        false
    }

    fn attach(
        &self,
        _group: &PaneGroup,
        _focus_handle: focus_state::PaneFocusHandle,
        _ctx: &mut ViewContext<PaneGroup>,
    ) {
    }

    fn detach(
        &self,
        _group: &PaneGroup,
        _detach_type: pane::DetachType,
        _ctx: &mut ViewContext<PaneGroup>,
    ) {
    }

    fn snapshot(&self, _app: &AppContext) -> LeafContents {
        LeafContents::GetStarted
    }

    fn has_application_focus(&self, _ctx: &mut ViewContext<PaneGroup>) -> bool {
        false
    }

    fn focus(&self, _ctx: &mut ViewContext<PaneGroup>) {}

    fn shareable_link(
        &self,
        _ctx: &mut ViewContext<PaneGroup>,
    ) -> Result<pane::ShareableLink, pane::ShareableLinkError> {
        Ok(pane::ShareableLink::Base)
    }

    fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    fn is_pane_being_dragged(&self, _ctx: &AppContext) -> bool {
        false
    }
}

// TODO: This test is commented out for now until we can fix it. It is flaky and sometimes hangs, causing the CI to cancel.
// #[test]
// #[allow(clippy::clone_on_copy)]
// fn test_pane_history() {
//     App::test((), |mut app| async move {
//         let pane_group = mock_pane_group(&mut app, platform);

//         pane_group.update(&mut app, |panes, ctx| {
//             let mut entity_ids: Vec<EntityId> =
//                 panes.view_id_to_session_data.keys().cloned().collect();

//             let first_entity_id = entity_ids.get(0).unwrap().clone();

//             // Add pane Left.
//             panes.add_pane(Direction::Left, ctx);
//             entity_ids = panes.view_id_to_session_data.keys().cloned().collect();
//             entity_ids.retain(|x| *x != first_entity_id);
//             let second_entity_id = entity_ids.get(0).unwrap().clone();
//             // Add pane Up.
//             panes.add_pane(Direction::Up, ctx);
//             entity_ids = panes.view_id_to_session_data.keys().cloned().collect();
//             entity_ids.retain(|x| *x != first_entity_id && *x != second_entity_id);
//             let third_entity_id = entity_ids.get(0).unwrap().clone();

//             assert!(panes.prev_session_id(third_entity_id).unwrap() == second_entity_id);
//         })
//     });
// }

#[test]
#[allow(clippy::clone_on_copy)]
fn test_pane_focus_on_close() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let first_pane_id = get_newly_created_pane_id(panes, &[]);

            // Add pane Left.
            panes.add_terminal_pane(Direction::Left, None, ctx);
            let second_pane_id = get_newly_created_pane_id(panes, &[first_pane_id]);

            assert!(panes.prev_pane_id(second_pane_id).unwrap() == first_pane_id);

            // Add pane Up.
            panes.add_terminal_pane(Direction::Up, None, ctx);
            let third_pane_id = get_newly_created_pane_id(panes, &[first_pane_id, second_pane_id]);

            // Close the third pane and check that the second pane opened is now focused.
            panes.close_pane(third_pane_id, ctx);
            assert_eq!(second_pane_id, panes.focused_pane_id(ctx));
        })
    });
}

#[test]
fn test_adding_terminal_pane_emits_app_state_changed() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());
        let app_state_changed = Rc::new(Cell::new(false));

        app.update({
            let app_state_changed = app_state_changed.clone();
            |ctx| {
                ctx.subscribe_to_view(&pane_group, move |_, event, _| {
                    if matches!(event, Event::AppStateChanged) {
                        app_state_changed.set(true);
                    }
                });
            }
        });

        pane_group.update(&mut app, |panes, ctx| {
            panes.add_terminal_pane(Direction::Right, None, ctx);
        });

        assert!(app_state_changed.get());
    });
}

#[test]
fn test_insert_hidden_child_agent_pane_keeps_focus_and_active_session() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let initial_tree_pane_count = panes.pane_count();
            let initial_content_pane_count = panes.pane_ids().count();
            let initial_visible_count = panes.visible_pane_count();
            let initial_active_session = panes.active_session_id(ctx);

            let child_pane_id = panes.insert_terminal_pane_hidden_for_child_agent(
                parent_pane_id,
                HashMap::new(),
                IsSharedSessionCreator::No,
                ctx,
            );

            assert_eq!(panes.pane_count(), initial_tree_pane_count);
            assert_eq!(panes.pane_ids().count(), initial_content_pane_count + 1);
            assert_eq!(panes.terminal_pane_ids().count(), 2);
            assert_eq!(panes.visible_pane_count(), initial_visible_count);
            assert!(panes.has_pane_id(child_pane_id.into()));
            assert!(!panes.panes.is_pane_in_tree(child_pane_id.into()));

            // The new child pane should remain off-tree and not affect visible ordering.
            assert_eq!(panes.pane_id_by_index(0), Some(parent_pane_id));
            assert_eq!(panes.pane_id_by_index(1), None);
            let visible_terminal_views = panes.visible_terminal_views(ctx);
            assert_eq!(visible_terminal_views.len(), 1);
            assert_eq!(
                visible_terminal_views[0].id(),
                panes
                    .terminal_view_from_pane_id(parent_pane_id, ctx)
                    .unwrap()
                    .id()
            );

            // Creating a hidden child pane should not steal focus or active session.
            assert_eq!(panes.focused_pane_id(ctx), parent_pane_id);
            assert_eq!(panes.active_session_id(ctx), initial_active_session);
        });
    });
}

#[test]
fn test_swapping_to_child_agent_from_maximized_pane_keeps_maximized_state() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            panes.add_terminal_pane(Direction::Right, None, ctx);
            panes.focus_pane(parent_pane_id, true, ctx);

            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let child = create_hidden_child_agent_conversation(
                panes,
                HiddenChildAgentConversationRequest {
                    parent_pane_id,
                    name: "Agent 1".to_string(),
                    parent_conversation_id,
                    orchestration_harness: None,
                    env_vars: HashMap::new(),
                    task_context: None,
                    is_shared_session_creator: IsSharedSessionCreator::No,
                },
                ctx,
            )
            .expect("fresh hidden child conversation should be created");
            let child_pane_id = panes
                .child_agent_panes
                .get(&child.conversation_id)
                .copied()
                .expect("fresh hidden child pane should be tracked");

            panes.toggle_maximize_pane(ctx);
            assert!(panes.is_focused_pane_maximized(ctx));

            panes.swap_active_pane_to_conversation(parent_pane_id, child.conversation_id, ctx);

            assert_eq!(panes.focused_pane_id(ctx), child_pane_id);
            assert!(panes.is_focused_pane_maximized(ctx));
            assert_eq!(
                split_pane_state(panes, child_pane_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Maximized),
            );
        });
    });
}
#[test]
fn test_insert_hidden_ambient_child_agent_pane_suppresses_details_auto_open() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let child_pane_id =
                panes.insert_ambient_agent_pane_hidden_for_child_agent(parent_pane_id, ctx);

            let terminal_view = panes
                .terminal_view_from_pane_id(child_pane_id, ctx)
                .expect("hidden ambient child pane should have a terminal view");
            assert!(
                terminal_view
                    .as_ref(ctx)
                    .is_initial_conversation_details_panel_auto_open_suppressed_for_test(),
                "hidden ambient child panes opened from the parent orchestration UI should not \
                 auto-open details during environment setup or session readiness"
            );
        });
    });
}
#[test]
fn test_hidden_child_creation_applies_ambient_task_id_to_controller() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let task_id = new_ambient_agent_task_id();

            let child = create_hidden_child_agent_conversation(
                panes,
                HiddenChildAgentConversationRequest {
                    parent_pane_id,
                    name: "Agent 1".to_string(),
                    parent_conversation_id,
                    orchestration_harness: None,
                    env_vars: HashMap::new(),
                    task_context: Some(HiddenChildAgentTaskContext {
                        task_id,
                        working_dir: None,
                    }),
                    is_shared_session_creator: IsSharedSessionCreator::No,
                },
                ctx,
            )
            .expect("fresh hidden child conversation should be created");

            let child_pane_id = panes
                .child_agent_panes
                .get(&child.conversation_id)
                .copied()
                .expect("fresh hidden child pane should be tracked");

            assert_eq!(
                request_ambient_agent_task_id_for_hidden_child(panes, child_pane_id, ctx,),
                Some(task_id)
            );
        });
    });
}

#[test]
fn test_restored_hidden_child_pane_reapplies_ambient_task_id_to_controller() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let task_id = new_ambient_agent_task_id();

            let mut child_conversation = AIConversation::new(false, false);
            child_conversation.set_parent_conversation_id(parent_conversation_id);
            child_conversation.set_task_id(task_id);
            let child_conversation_id = child_conversation.id();

            panes.create_hidden_child_agent_pane(child_conversation, parent_pane_id, ctx);

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("restored hidden child pane should be tracked");

            assert_eq!(
                request_ambient_agent_task_id_for_hidden_child(panes, child_pane_id, ctx,),
                Some(task_id)
            );
        });
    });
}

#[test]
fn test_restored_remote_hidden_child_pane_enters_existing_ambient_session() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let task_id = new_ambient_agent_task_id();

            // Inject an *attachable* task (InProgress + running sandbox +
            // parseable session id) so the unified dispatch resolves to
            // `AttachLive` and routes through
            // `attach_ambient_orchestration_child_session`, joining the live
            // ambient session in place.
            AgentConversationsModel::handle(ctx).update(ctx, |model, _| {
                model.insert_task_for_test(attachable_ambient_agent_task(task_id));
            });

            let mut child_conversation = AIConversation::new(false, false);
            child_conversation.set_parent_conversation_id(parent_conversation_id);
            child_conversation.set_task_id(task_id);
            child_conversation.mark_as_remote_child();
            let child_conversation_id = child_conversation.id();

            panes.create_hidden_child_agent_pane(child_conversation, parent_pane_id, ctx);

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("restored remote hidden child pane should be tracked");

            let (ambient_task_id, is_agent_running, active_conversation_id) =
                ambient_child_session_state(panes, child_pane_id, ctx);

            assert_eq!(ambient_task_id, Some(task_id));
            assert!(
                is_agent_running,
                "remote child restore should view the existing ambient session"
            );
            assert_eq!(active_conversation_id, Some(child_conversation_id));

            // Fix B: the placeholder's local AIConversationId must remain the
            // canonical key in `child_agent_panes`. Any in-place hydration
            // (live attach, transcript merge, or fallback) must preserve this
            // key so the orchestration pill bar and topology indexes can
            // still find the pane.
            assert!(
                panes.child_agent_panes.contains_key(&child_conversation_id),
                "placeholder AIConversationId must stay the child_agent_panes key after Fix B \
                 hydration",
            );

            let terminal_view = panes
                .terminal_view_from_pane_id(child_pane_id, ctx)
                .expect("remote child pane should have a terminal view");
            assert!(
                terminal_view
                    .as_ref(ctx)
                    .is_initial_conversation_details_panel_auto_open_suppressed_for_test(),
                "remote child panes opened from the parent orchestration UI should not auto-open \
                 details when the ambient session becomes ready"
            );
        });
    });
}

/// When task data for a restored remote child is NOT yet cached at
/// `create_hidden_child_agent_pane` time, the unified dispatch resolves to
/// `Pending`: the hidden pane is still created and registered in
/// `child_agent_panes` keyed by its local AIConversationId (so the pill can
/// reveal it), using a passive loading transcript vehicle with no live attach.
/// The tracker re-drives materialization on the next lifecycle /
/// session-linked event.
#[test]
fn test_restored_remote_hidden_child_pane_pending_when_task_data_unavailable() {
    let _unified_stack = FeatureFlag::OrchestrationUnifiedStack.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let task_id = new_ambient_agent_task_id();

            // Deliberately do NOT inject a task into AgentConversationsModel.
            // `get_or_async_fetch_task_data` returns `None`, so the unified
            // dispatch resolves to `Pending`: the hidden passive loading pane
            // is created and tracked so the pill can reveal it.
            // A later TasksUpdated re-drives the retained pending hydration.

            let mut child_conversation = AIConversation::new(false, false);
            child_conversation.set_parent_conversation_id(parent_conversation_id);
            child_conversation.set_task_id(task_id);
            child_conversation.mark_as_remote_child();
            let child_conversation_id = child_conversation.id();

            panes.create_hidden_child_agent_pane(child_conversation, parent_pane_id, ctx);

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("remote child pane must be registered even when task data unavailable");

            // The placeholder local AIConversationId remains the canonical key.
            assert!(
                panes.child_agent_panes.contains_key(&child_conversation_id),
                "placeholder AIConversationId must stay the child_agent_panes key in fallback path",
            );

            // Pending uses the passive loading presentation: no ambient
            // composer is exposed before task metadata can select live or
            // transcript materialization.
            let terminal_view = panes
                .terminal_view_from_pane_id(child_pane_id, ctx)
                .expect("pending child pane has a terminal view");
            let view = terminal_view.as_ref(ctx);
            assert!(view.ambient_agent_view_model().is_none());
            assert!(
                !view.has_agent_view_zero_state_for_test(),
                "pending child must not expose the cloud composition zero state",
            );
            assert_eq!(
                view.active_conversation_id(ctx),
                Some(child_conversation_id)
            );
            let model = view.model.lock();
            assert!(model.is_conversation_transcript_viewer());
            assert!(model.is_read_only());
            assert_eq!(
                model.conversation_transcript_viewer_status(),
                Some(&ConversationTranscriptViewerStatus::Loading),
            );
        });
    });
}

/// A terminal owner remote child (`Succeeded` run with a server
/// `conversation_id`, no live session) resolves to `LoadTranscript`: the
/// unified dispatch still materializes the hidden ambient pane keyed by the
/// placeholder's local id, into which the cloud transcript merges
/// asynchronously.
#[test]
fn test_restored_remote_hidden_child_pane_terminal_owner_loads_transcript() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let task_id = new_ambient_agent_task_id();

            // Terminal task + server conversation id -> LoadTranscript.
            let mut task = ambient_agent_task_for_current_user(task_id);
            task.state = AmbientAgentTaskState::Succeeded;
            task.is_sandbox_running = false;
            task.conversation_id = Some("owner-child-server-token".to_string());
            AgentConversationsModel::handle(ctx).update(ctx, |model, _| {
                model.insert_task_for_test(task);
            });

            let mut child_conversation = AIConversation::new(false, false);
            child_conversation.set_parent_conversation_id(parent_conversation_id);
            child_conversation.set_task_id(task_id);
            child_conversation.mark_as_remote_child();
            let child_conversation_id = child_conversation.id();

            panes.create_hidden_child_agent_pane(child_conversation, parent_pane_id, ctx);

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("terminal owner remote child must materialize an ambient transcript pane");
            // The transcript branch builds a cloud-mode ambient pane (so the
            // pill can reveal it) keyed by the placeholder's local id.
            let (_task_id, _running, active_conversation_id) =
                ambient_child_session_state(panes, child_pane_id, ctx);
            assert_eq!(active_conversation_id, Some(child_conversation_id));
        });
    });
}

/// A terminal *viewer* child (`is_viewing_shared_session`, `Succeeded` run
/// with a server `conversation_id`, no live session) resolves to
/// `LoadTranscript` in a passive transcript pane. It must not expose the
/// ambient cloud-composition model or its new-conversation zero state while
/// the transcript fetch is in flight.
#[test]
fn test_restored_viewer_hidden_child_pane_terminal_loads_transcript() {
    let _unified_stack = FeatureFlag::OrchestrationUnifiedStack.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let task_id = new_ambient_agent_task_id();

            let mut task = ambient_agent_task_for_current_user(task_id);
            task.state = AmbientAgentTaskState::Succeeded;
            task.is_sandbox_running = false;
            task.conversation_id = Some("viewer-child-server-token".to_string());
            AgentConversationsModel::handle(ctx).update(ctx, |model, _| {
                model.insert_task_for_test(task);
            });

            let mut child_conversation = AIConversation::new(false, false);
            child_conversation.set_parent_conversation_id(parent_conversation_id);
            child_conversation.set_task_id(task_id);
            child_conversation.set_is_viewing_shared_session(true);
            let child_conversation_id = child_conversation.id();

            panes.create_hidden_child_agent_pane(child_conversation, parent_pane_id, ctx);

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("terminal viewer child must materialize a transcript pane");
            let terminal_view = panes
                .terminal_view_from_pane_id(child_pane_id, ctx)
                .expect("terminal viewer child pane has a terminal view");
            let view = terminal_view.as_ref(ctx);
            assert_eq!(
                view.active_conversation_id(ctx),
                Some(child_conversation_id),
            );
            assert!(
                view.ambient_agent_view_model().is_none(),
                "passive viewer transcripts must not retain a configuring cloud-agent model",
            );
            assert!(
                !view.has_agent_view_zero_state_for_test(),
                "viewer child placeholders must not insert new-cloud composition zero state",
            );
            let model = view.model.lock();
            assert!(model.is_conversation_transcript_viewer());
            assert!(model.is_read_only());
            assert_eq!(
                model.conversation_transcript_viewer_status(),
                Some(&ConversationTranscriptViewerStatus::Loading),
            );
        });
    });
}

#[test]
fn completed_shared_session_child_with_edit_access_uses_continuation_pane() {
    let _unified_stack = FeatureFlag::OrchestrationUnifiedStack.override_enabled(true);
    let _handoff = FeatureFlag::HandoffCloudCloud.override_enabled(true);
    let _cloud_mode = FeatureFlag::CloudMode.override_enabled(true);
    let _setup_v2 = FeatureFlag::CloudModeSetupV2.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let task_id = new_ambient_agent_task_id();
            let mut task = ambient_agent_task_for_current_user(task_id);
            task.creator = Some(TaskPrincipalInfo {
                creator_type: "USER".to_string(),
                uid: "other-user".to_string(),
                display_name: None,
            });
            task.conversation_id = Some("test-server-token".to_string());
            AgentConversationsModel::handle(ctx).update(ctx, |model, _| {
                model.insert_task_for_test(task);
            });

            let mut child = AIConversation::new(true, false);
            child.set_task_id(task_id);
            let child_id = child.id();
            let mut merged = child.clone();
            merged.set_server_metadata(test_server_conversation_metadata(Some(task_id)));

            let loading_pane_id = panes
                .create_child_loading_placeholder(
                    child,
                    AgentViewEntryOrigin::SharedSessionSelection,
                    ctx,
                )
                .expect("viewer child loading pane");
            panes.replace_child_loading_with_continuation_pane(
                loading_pane_id,
                child_id,
                task_id,
                merged,
                ctx,
            );

            let pane_id = panes.child_agent_panes[&child_id];
            assert_ne!(pane_id, loading_pane_id);
            let view = panes
                .terminal_view_from_pane_id(pane_id, ctx)
                .expect("continuation pane");
            assert!(view.as_ref(ctx).ambient_agent_view_model().is_some());
            let model = view.as_ref(ctx).model.lock();
            assert!(!model.is_conversation_transcript_viewer());
            assert!(!model.is_read_only());
            assert!(matches!(
                model.shared_session_status(),
                SharedSessionStatus::NotShared
            ));
        });
    });
}

#[test]
fn failed_viewer_child_session_stays_unavailable_without_retrying_same_session() {
    let _unified_stack = FeatureFlag::OrchestrationUnifiedStack.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());
        let task_id = new_ambient_agent_task_id();
        let failed_session_id = SessionId::new();

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);

            let mut pending_task = ambient_agent_task_for_current_user(task_id);
            pending_task.state = AmbientAgentTaskState::Pending;
            pending_task.is_sandbox_running = false;
            pending_task.session_id = None;
            AgentConversationsModel::handle(ctx).update(ctx, |model, _| {
                model.insert_task_for_test(pending_task);
            });

            let mut child_conversation = AIConversation::new(false, false);
            child_conversation.set_parent_conversation_id(parent_conversation_id);
            child_conversation.set_task_id(task_id);
            child_conversation.set_is_viewing_shared_session(true);
            let child_id = child_conversation.id();
            panes.create_hidden_child_agent_pane(child_conversation, parent_pane_id, ctx);
            let pane_id = panes.child_agent_panes[&child_id];

            panes.recover_viewer_child_join_failure(pane_id, child_id, failed_session_id, ctx);

            let mut running_task = ambient_agent_task_for_current_user(task_id);
            running_task.state = AmbientAgentTaskState::InProgress;
            running_task.is_sandbox_running = true;
            running_task.session_id = Some(failed_session_id.to_string());
            AgentConversationsModel::handle(ctx).update(ctx, |model, _| {
                model.insert_task_for_test(running_task);
            });
            panes.process_pending_child_hydrations(ctx);

            assert_eq!(panes.child_agent_panes[&child_id], pane_id);
            assert_eq!(
                panes.failed_viewer_child_sessions.get(&child_id),
                Some(&failed_session_id),
            );
            assert_eq!(
                panes.pending_child_hydrations.get(&task_id),
                Some(&child_id),
            );
            let view = panes
                .terminal_view_from_pane_id(pane_id, ctx)
                .expect("pending child pane remains available");
            assert!(
                view.as_ref(ctx)
                    .is_orchestration_child_live_unavailable_for_test(),
                "failed child join should leave bounded non-error unavailable UI",
            );
        });
    });
}

/// Phase 1 integration coverage: validates that after `BlocklistAIHistoryModel`
/// restoration (the same code path the disk-load Fix C unblocks), the
/// orchestration topology is fully wired BEFORE the parent's fullscreen
/// agent view is entered, AND that entering fullscreen lazily materializes
/// the hidden child pane keyed by the placeholder local AIConversationId.
///
/// This is the integration boundary the user-visible bug lives at:
///   * pill bar / transcript name resolution must succeed before the
///     parent fullscreen entry (Fix C eagerly hydrates `conversations_by_id`
///     so this works on disk-load; this test exercises the equivalent
///     restore-into-history-model + lazy pane materialization flow).
///   * the hidden child pane must materialize in `child_agent_panes` keyed
///     by the placeholder conversation id after parent fullscreen.
///
/// The disk-load construction path (`BlocklistAIHistoryModel::new(_, _, &conversations)`
/// invoking `initialize_historical_conversations`) is covered by
/// `test_initialize_historical_conversations_eagerly_hydrates_orchestration_children`
/// in `app/src/ai/blocklist/history_model_tests.rs`. `agent_display_name_from_id`
/// resolution for restored children is covered by
/// `participant_for_restored_child_run_id_resolves_to_agent_name` in
/// `app/src/ai/blocklist/block/view_impl/orchestration_tests.rs`. The pill
/// bar data-layer coverage is in
/// `pill_bar_data_layer_finds_restored_children_before_pane_creation` in
/// `app/src/ai/blocklist/agent_view/orchestration_pill_bar_tests.rs`. This
/// test ties those three boundaries together at the PaneGroup integration
/// layer.
#[test]
fn test_pane_group_restore_loop_keeps_orchestration_topology_and_materializes_child_pane() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        let (
            parent_pane_id,
            parent_conversation_id,
            parent_run_id,
            child_conversation_id,
            child_run_id,
            child_agent_name,
        ) = pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_terminal_view_id = panes
                .terminal_view_from_pane_id(parent_pane_id, ctx)
                .expect("parent pane should have a terminal view")
                .id();

            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let parent_run_id = new_ambient_agent_task_id().to_string();
            let child_run_id = new_ambient_agent_task_id().to_string();
            let child_agent_name = "Agent 1".to_string();

            // Restore a child conversation into the parent's terminal view. This
            // is the same code path `RestoredAgentConversations::take_conversations`
            // feeds into during pane restoration. Fix C ensures the equivalent
            // wiring happens earlier (at history-model construction) so the data
            // is also available before any terminal view materializes the parent.
            let mut child_conversation = AIConversation::new(false, false);
            child_conversation.set_parent_conversation_id(parent_conversation_id);
            child_conversation.set_agent_name(child_agent_name.clone());
            let child_conversation_id = child_conversation.id();
            BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                history.restore_conversations(
                    parent_terminal_view_id,
                    vec![child_conversation],
                    ctx,
                );
                // Stamp run_ids so orchestration agent_id lookups resolve.
                history.assign_run_id_for_conversation(
                    parent_conversation_id,
                    parent_run_id.clone(),
                    None,
                    parent_terminal_view_id,
                    ctx,
                );
                history.assign_run_id_for_conversation(
                    child_conversation_id,
                    child_run_id.clone(),
                    None,
                    parent_terminal_view_id,
                    ctx,
                );
            });

            (
                parent_pane_id,
                parent_conversation_id,
                parent_run_id,
                child_conversation_id,
                child_run_id,
                child_agent_name,
            )
        });

        // BEFORE the parent's fullscreen agent view is entered, the
        // orchestration data layer must already know:
        //   (a) the parent → child topology (pill bar source),
        //   (b) the child's local conversation (with agent name set), and
        //   (c) the child's run_id → conversation id (transcript name
        //       resolution source via `conversation_id_for_agent_id`).
        pane_group.read(&app, |panes, ctx| {
            let history = BlocklistAIHistoryModel::as_ref(ctx);

            // (a) Topology — direct children index and the transitive walker
            // used by `OrchestrationPillBar::pill_specs` must both find the
            // child immediately, even though the hidden child pane has not
            // been created yet.
            assert_eq!(
                history.child_conversation_ids_of(&parent_conversation_id),
                &[child_conversation_id],
                "orchestration topology must list the restored child under its parent before any pane materializes",
            );
            assert_eq!(
                descendant_conversation_ids_in_spawn_order(history, parent_conversation_id),
                vec![child_conversation_id],
                "pill bar pre-order walker must reach the restored child before any pane materializes",
            );

            // (b) The child must be hydrated into `conversations_by_id`
            // with its agent name preserved — this is the data Fix C
            // eagerly populates on disk-load so the transcript name
            // resolver finds the display name instead of falling back to
            // "Unknown agent".
            let child_conversation = history
                .conversation(&child_conversation_id)
                .expect("restored child must be in conversations_by_id before parent fullscreen");
            assert_eq!(
                child_conversation.agent_name(),
                Some(child_agent_name.as_str()),
                "restored child must retain its display name for transcript / pill bar rendering",
            );

            // (c) Run-id → conversation lookups (used by `participant_for_agent_id`).
            assert_eq!(
                history.conversation_id_for_agent_id(&child_run_id),
                Some(child_conversation_id),
                "child run_id must resolve to the restored child conversation",
            );
            assert_eq!(
                history.conversation_id_for_agent_id(&parent_run_id),
                Some(parent_conversation_id),
                "parent run_id must resolve to the parent conversation",
            );

            // Hidden child pane must NOT exist yet — restoration is lazy and
            // only materializes when the parent's agent view is entered.
            assert!(
                !panes.child_agent_panes.contains_key(&child_conversation_id),
                "hidden child pane must not exist before parent fullscreen entry",
            );
        });

        // Enter the parent's fullscreen agent view. This is the trigger for
        // `restore_missing_child_agent_panes_for_parent`, which is the
        // PaneGroup-side of the user-visible restart-loop bug.
        pane_group.update(&mut app, |panes, ctx| {
            enter_agent_view_for_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
        });

        // AFTER fullscreen entry, the hidden child pane must materialize in
        // `child_agent_panes` keyed by the placeholder local AIConversationId.
        pane_group.read(&app, |panes, _ctx| {
            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("parent fullscreen entry must materialize the hidden child pane");
            assert!(
                panes.has_pane_id(child_pane_id),
                "materialized child pane must be tracked by the pane group",
            );
            assert!(
                !panes.panes.is_pane_in_tree(child_pane_id),
                "materialized child pane must remain off-tree (hidden)",
            );
        });
    });
}

/// A concurrent seed call racing a re-drive must not dispatch a second
/// `?ancestor_run_id=` request for the same parent while the first is
/// still in flight.
#[test]
fn seed_child_conversations_from_task_coalesces_concurrent_ancestor_list_fetches() {
    let _unified_stack = FeatureFlag::OrchestrationUnifiedStack.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let parent_task_id = new_ambient_agent_task_id();

            // Simulate multiple entry points trying to seed the same parent
            // before its first ancestor-list fetch has resolved: a direct
            // re-entrant call, plus two `TasksUpdated` re-drives.
            panes.seed_child_conversations_from_task(parent_conversation_id, parent_task_id, ctx);
            panes.seed_child_conversations_from_task(parent_conversation_id, parent_task_id, ctx);
            panes.process_pending_parent_child_seeds(ctx);
            panes.process_pending_parent_child_seeds(ctx);

            assert_eq!(
                panes.parent_child_seed_fetch_dispatch_count, 1,
                "a parent with an ancestor-list fetch already in flight must not get a second \
                 request dispatched by a concurrent seed call or TasksUpdated re-drive",
            );
            assert!(
                panes
                    .pending_parent_child_seeds
                    .contains_key(&parent_task_id),
                "the parent should remain pending until the in-flight fetch resolves",
            );
        });
    });
}

/// Drives the real completion handler with a synthetic successful response
/// whose only child is already cached. Both the child link and the
/// pending-entry removal must happen with zero additional network dispatches.
#[test]
fn finish_seed_child_conversations_from_task_links_children_and_clears_pending_once_resolved() {
    let _unified_stack = FeatureFlag::OrchestrationUnifiedStack.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let parent_task_id = new_ambient_agent_task_id();
            let child_task_id = new_ambient_agent_task_id();

            // Seed the child's task data directly so `get_or_async_fetch_task_data`
            // resolves from cache instead of issuing a network call.
            AgentConversationsModel::handle(ctx).update(ctx, |model, _| {
                model.insert_task_for_test(ambient_agent_task_for_current_user(child_task_id));
            });

            // Mark pending the way `seed_child_conversations_from_task` does,
            // then drive the completion handler directly with a synthetic
            // response reporting one direct child.
            panes.seed_child_conversations_from_task(parent_conversation_id, parent_task_id, ctx);
            // The real completion callback clears `fetch_in_flight` before
            // calling `finish_seed_child_conversations_from_task`; mirror
            // that here since this test drives the completion handler
            // directly, bypassing the wrapper.
            panes
                .pending_parent_child_seeds
                .get_mut(&parent_task_id)
                .unwrap()
                .fetch_in_flight = false;
            let response = vec![ambient_agent_task_for_current_user(child_task_id)];
            panes.finish_seed_child_conversations_from_task(
                parent_conversation_id,
                parent_task_id,
                Ok(response),
                ctx,
            );

            assert!(
                !panes
                    .pending_parent_child_seeds
                    .contains_key(&parent_task_id),
                "the parent must be cleared once its only known child has resolved locally",
            );

            let history = BlocklistAIHistoryModel::as_ref(ctx);
            assert_eq!(
                history
                    .child_conversation_ids_of(&parent_conversation_id)
                    .len(),
                1,
                "the known child must be linked under the parent",
            );
        });
    });
}

/// While any reported child hasn't resolved from the local task cache yet,
/// the parent must stay pending (not be dropped) so a subsequent re-drive
/// still re-lists and can pick up a child spawned in the interim; clearing
/// early would stop discovering such children. Only once every currently
/// reported child resolves does the parent clear.
#[test]
fn finish_seed_child_conversations_from_task_stays_pending_while_a_child_is_unresolved() {
    let _unified_stack = FeatureFlag::OrchestrationUnifiedStack.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let parent_task_id = new_ambient_agent_task_id();
            let unresolved_child_task_id = new_ambient_agent_task_id();

            // Deliberately do NOT insert the child's task data, so
            // `get_or_async_fetch_task_data` returns `None` for it.
            panes.seed_child_conversations_from_task(parent_conversation_id, parent_task_id, ctx);
            let response = vec![ambient_agent_task_for_current_user(
                unresolved_child_task_id,
            )];
            panes.finish_seed_child_conversations_from_task(
                parent_conversation_id,
                parent_task_id,
                Ok(response),
                ctx,
            );

            assert!(
                panes
                    .pending_parent_child_seeds
                    .contains_key(&parent_task_id),
                "the parent must remain pending while a reported child hasn't resolved yet, so \
                 the next TasksUpdated re-drive still re-lists",
            );

            // The real completion callback (in `spawn_ancestor_list_fetch_if_needed`)
            // clears `fetch_in_flight` before calling
            // `finish_seed_child_conversations_from_task`; mirror that here
            // since this test drives the completion handler directly.
            panes
                .pending_parent_child_seeds
                .get_mut(&parent_task_id)
                .unwrap()
                .fetch_in_flight = false;

            // A subsequent TasksUpdated re-drive must actually re-list (not
            // silently no-op) now that the previous fetch has completed.
            let dispatch_count_before = panes.parent_child_seed_fetch_dispatch_count;
            panes.process_pending_parent_child_seeds(ctx);
            assert_eq!(
                panes.parent_child_seed_fetch_dispatch_count,
                dispatch_count_before + 1,
                "an unresolved parent must be re-listed on the next TasksUpdated re-drive",
            );
        });
    });
}

/// A transient ancestor-list failure (e.g. a network blip) must not leave
/// the parent stranded waiting on an incidental external event that may
/// never come (e.g. an idle completed conversation) — a one-shot retry
/// must be scheduled so the fetch is retried on its own.
#[test]
fn finish_seed_child_conversations_from_task_schedules_retry_on_transient_failure() {
    let _unified_stack = FeatureFlag::OrchestrationUnifiedStack.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let parent_task_id = new_ambient_agent_task_id();

            panes.seed_child_conversations_from_task(parent_conversation_id, parent_task_id, ctx);
            // Mirror the real completion callback's `fetch_in_flight` reset,
            // since this test drives the completion handler directly.
            panes
                .pending_parent_child_seeds
                .get_mut(&parent_task_id)
                .unwrap()
                .fetch_in_flight = false;
            // No `HttpStatusError` in the chain => classified as transient by
            // `is_transient_http_error` (network-level failure).
            panes.finish_seed_child_conversations_from_task(
                parent_conversation_id,
                parent_task_id,
                Err(anyhow::anyhow!("connection reset")),
                ctx,
            );

            let seed = panes
                .pending_parent_child_seeds
                .get(&parent_task_id)
                .expect("a transient failure must leave the parent pending for a retry");
            assert!(
                !seed.fetch_in_flight,
                "the completion path must not leave fetch_in_flight stuck true after handling \
                 a transient failure",
            );
            assert!(
                seed.retry_handle.is_some(),
                "a transient failure must schedule a guaranteed one-shot retry instead of \
                 relying on an incidental TasksUpdated, so no subsequent external event is \
                 needed for the parent to eventually link its children",
            );
        });
    });
}

/// If a pending seed is removed (e.g. its pane closes) and a new one
/// created for the same `parent_task_id` while the old fetch is still in
/// flight (e.g. the same parent conversation is reopened), the old
/// completion must be recognized as stale so it can't clobber the new
/// seed's in-flight state or feed it stale results.
#[test]
fn stale_ancestor_list_completion_is_detected_when_seed_removed_or_recreated() {
    let dispatched_at = Instant::now();
    let live_seed = PendingParentChildSeed {
        parent_conversation_id: AIConversationId::new(),
        fetch_in_flight: true,
        in_flight_fetch_started_at: Some(dispatched_at),
        retry_handle: None,
    };
    assert!(
        !is_stale_ancestor_list_completion(Some(&live_seed), dispatched_at),
        "a completion matching the seed's own in-flight dispatch marker must not be stale",
    );

    assert!(
        is_stale_ancestor_list_completion(None, dispatched_at),
        "a completion for a seed that was removed entirely (e.g. pane closed) must be stale",
    );

    // A later dispatch on a recreated seed (e.g. the same parent conversation
    // reopened while the old fetch was still in flight) has a distinct
    // dispatch marker.
    let recreated_seed = PendingParentChildSeed {
        parent_conversation_id: AIConversationId::new(),
        fetch_in_flight: true,
        in_flight_fetch_started_at: Some(dispatched_at + Duration::from_secs(1)),
        retry_handle: None,
    };
    assert!(
        is_stale_ancestor_list_completion(Some(&recreated_seed), dispatched_at),
        "a completion whose dispatch marker doesn't match the current seed's must be stale, \
         since a newer fetch has since been dispatched for the same parent_task_id",
    );
}

/// A permanent (non-transient) ancestor-list failure such as a 404/403
/// can't succeed by retrying blindly, so the parent must be dropped
/// instead of staying pending forever.
#[test]
fn finish_seed_child_conversations_from_task_gives_up_on_permanent_failure() {
    let _unified_stack = FeatureFlag::OrchestrationUnifiedStack.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let parent_task_id = new_ambient_agent_task_id();

            panes.seed_child_conversations_from_task(parent_conversation_id, parent_task_id, ctx);
            let err = anyhow::Error::new(HttpStatusError {
                status: 404,
                body: String::new(),
            });
            panes.finish_seed_child_conversations_from_task(
                parent_conversation_id,
                parent_task_id,
                Err(err),
                ctx,
            );

            assert!(
                !panes
                    .pending_parent_child_seeds
                    .contains_key(&parent_task_id),
                "a permanent failure can't succeed by retrying blindly, so the parent must be \
                 dropped instead of staying pending forever",
            );
        });
    });
}

/// A parent with no terminal surface to seed into (e.g. a background
/// ancestor several levels above the conversation the user actually
/// opened) must not stay pending forever, since that would re-list it on
/// every future re-drive indefinitely.
#[test]
fn finish_seed_child_conversations_from_task_gives_up_when_parent_has_no_terminal_surface() {
    let _unified_stack = FeatureFlag::OrchestrationUnifiedStack.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            // Never attached via `start_new_conversation` / `restore_conversations`,
            // so it has no terminal surface.
            let orphan_parent_conversation_id = AIConversationId::new();
            let parent_task_id = new_ambient_agent_task_id();
            let child_task_id = new_ambient_agent_task_id();

            // Mark pending directly (bypassing `seed_child_conversations_from_task`,
            // which would spawn a real network fetch) then drive the real
            // completion handler, so the test exercises the actual
            // no-terminal-surface early return instead of asserting against
            // fabricated state.
            panes.pending_parent_child_seeds.insert(
                parent_task_id,
                PendingParentChildSeed {
                    parent_conversation_id: orphan_parent_conversation_id,
                    fetch_in_flight: true,
                    in_flight_fetch_started_at: None,
                    retry_handle: None,
                },
            );

            let response = vec![ambient_agent_task_for_current_user(child_task_id)];
            panes.finish_seed_child_conversations_from_task(
                orphan_parent_conversation_id,
                parent_task_id,
                Ok(response),
                ctx,
            );

            assert!(
                !panes
                    .pending_parent_child_seeds
                    .contains_key(&parent_task_id),
                "a parent with no terminal surface to seed into must not stay pending forever \
                 and be re-listed on every future re-drive",
            );
        });
    });
}

#[test]
fn test_create_missing_child_agent_panes_restores_remote_child_from_history_model() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let child_conversation_id = AIConversationId::new();
            let task_id = new_ambient_agent_task_id();

            assert!(
                !panes.child_agent_panes.contains_key(&child_conversation_id),
                "child pane should not exist before startup restoration runs",
            );

            BlocklistAIHistoryModel::handle(ctx).update(ctx, |history_model, _| {
                history_model
                    .set_parent_for_conversation(child_conversation_id, parent_conversation_id);
            });
            RestoredAgentConversations::handle(ctx).update(ctx, |store, _| {
                *store = RestoredAgentConversations::new_seeded(vec![
                    persisted_remote_child_conversation(
                        child_conversation_id,
                        Some(parent_conversation_id),
                        None,
                        task_id,
                    ),
                ]);
            });
            // Attachable task so restoration live-attaches (was the old
            // task-data-unavailable fallback; now an explicit AttachLive).
            AgentConversationsModel::handle(ctx).update(ctx, |model, _| {
                model.insert_task_for_test(attachable_ambient_agent_task(task_id));
            });

            panes.restore_missing_child_agent_panes_for_parent(
                parent_conversation_id,
                parent_pane_id,
                true,
                ctx,
            );

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("startup restoration should recreate the remote child pane");
            let (ambient_task_id, is_agent_running, active_conversation_id) =
                ambient_child_session_state(panes, child_pane_id, ctx);

            assert_eq!(ambient_task_id, Some(task_id));
            assert!(
                is_agent_running,
                "restored remote child pane should reconnect to the ambient session",
            );
            assert_eq!(active_conversation_id, Some(child_conversation_id));
            assert_eq!(panes.focused_pane_id(ctx), parent_pane_id);
        });
    });
}

#[test]
fn test_ambient_transcript_restore_creates_cloud_mode_pane_when_handoff_enabled() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);
    let _cloud_mode = FeatureFlag::CloudMode.override_enabled(true);
    let _setup_v2 = FeatureFlag::CloudModeSetupV2.override_enabled(true);
    let _handoff = FeatureFlag::HandoffCloudCloud.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());
        let task_id = new_ambient_agent_task_id();

        pane_group.update(&mut app, |panes, ctx| {
            AgentConversationsModel::handle(ctx).update(ctx, |model, _| {
                model.insert_task_for_test(ambient_agent_task_for_current_user(task_id));
            });
            panes.load_data_into_conversation_transcript_viewer(
                cloud_conversation_with_ambient_task(task_id),
                Some(task_id),
                ctx,
            );
        });

        pane_group.read(&app, |panes, ctx| {
            let terminal_view = panes
                .active_session_view(ctx)
                .expect("restored pane should have an active terminal view");
            let view = terminal_view.as_ref(ctx);
            let ambient_model = view
                .ambient_agent_view_model()
                .expect("ambient restore should create a Cloud Mode view")
                .as_ref(ctx);

            assert_eq!(ambient_model.task_id(), Some(task_id));
            assert!(ambient_model.is_agent_running());
            assert_eq!(
                view.ambient_agent_task_id_for_details_panel(ctx),
                Some(task_id)
            );
            assert!(view.active_conversation_id(ctx).is_some());

            let model = view.model.lock();
            assert!(!model.is_conversation_transcript_viewer());
            assert!(!model.is_read_only());
            assert!(matches!(
                model.shared_session_status(),
                SharedSessionStatus::NotShared
            ));
        });
    });
}

#[test]
fn test_ambient_transcript_restore_uses_generic_viewer_when_handoff_disabled() {
    let _handoff = FeatureFlag::HandoffCloudCloud.override_enabled(false);
    let _setup_v2 = FeatureFlag::CloudModeSetupV2.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());
        let task_id = new_ambient_agent_task_id();

        pane_group.update(&mut app, |panes, ctx| {
            panes.load_data_into_conversation_transcript_viewer(
                cloud_conversation_with_ambient_task(task_id),
                Some(task_id),
                ctx,
            );
        });

        pane_group.read(&app, |panes, ctx| {
            let terminal_view = panes
                .active_session_view(ctx)
                .expect("fallback viewer should have an active terminal view");
            let view = terminal_view.as_ref(ctx);
            assert!(view.ambient_agent_view_model().is_none());

            let model = view.model.lock();
            assert!(model.is_conversation_transcript_viewer());
            assert!(model.is_read_only());
            assert_eq!(
                model.conversation_transcript_viewer_status(),
                Some(&ConversationTranscriptViewerStatus::ViewingAmbientConversation(task_id))
            );
        });
    });
}

/// REMOTE-2208: attaching a live execution session to a read-only conversation transcript
/// viewer is impossible (it is backed by a mock manager with no network), so the attach must
/// report failure. Reporting success left the caller focused on a transcript with no input box
/// — the "session opens but the terminal is not interactive" symptom — instead of falling back
/// to opening a fresh, writable shared-session tab.
#[test]
fn attach_execution_session_refuses_read_only_transcript_viewer_pane() {
    let _handoff = FeatureFlag::HandoffCloudCloud.override_enabled(false);
    let _setup_v2 = FeatureFlag::CloudModeSetupV2.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());
        let task_id = new_ambient_agent_task_id();

        pane_group.update(&mut app, |panes, ctx| {
            panes.load_data_into_conversation_transcript_viewer(
                cloud_conversation_with_ambient_task(task_id),
                Some(task_id),
                ctx,
            );
        });

        pane_group.update(&mut app, |panes, ctx| {
            let terminal_view = panes
                .active_session_view(ctx)
                .expect("transcript viewer should have an active terminal view");
            let pane_id = panes
                .find_pane_id_for_terminal_view(terminal_view.id(), ctx)
                .expect("transcript viewer pane should be found");
            assert!(
                terminal_view.as_ref(ctx).model.lock().is_read_only(),
                "precondition: the transcript viewer pane is read-only",
            );

            assert!(
                !panes.attach_execution_session_to_ambient_pane(pane_id, SessionId::new(), ctx),
                "a read-only transcript viewer must not report a successful live-session attach",
            );
        });
    });
}

/// REMOTE-2208: the read-only state is cleared as part of reattaching, so it must only be
/// cleared when a join actually starts. A caller that gets `false` opens a fresh pane instead,
/// and clearing eagerly would leave this pane looking writable while attached to nothing.
#[test]
fn attach_execution_session_keeps_read_only_state_when_the_attach_fails() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let terminal_view = panes
                .active_session_view(ctx)
                .expect("mock pane group should have an active terminal view");
            let pane_id = panes
                .find_pane_id_for_terminal_view(terminal_view.id(), ctx)
                .expect("active terminal view should have a pane");

            // A plain terminal pane's manager is not a shared-session viewer, so the attach below
            // fails at the downcast — the same shape as a manager that is already connecting.
            terminal_view.update(ctx, |view, _| {
                view.model
                    .lock()
                    .set_shared_session_status(SharedSessionStatus::FinishedViewer);
            });
            assert!(
                terminal_view.as_ref(ctx).model.lock().is_read_only(),
                "precondition: the pane is in a finished, read-only state",
            );

            assert!(
                !panes.attach_execution_session_to_ambient_pane(pane_id, SessionId::new(), ctx),
                "precondition: this attach cannot succeed",
            );
            assert!(
                terminal_view.as_ref(ctx).model.lock().is_read_only(),
                "a failed attach must leave the pane read-only so the caller's fresh-tab fallback \
                 is not shadowed by a pane that looks writable but joined nothing",
            );
        });
    });
}

/// Pins the contract that cloud-mode shared-session viewers (the local pane
/// of a remote orchestration parent) get an `ambient_agent_view_model` so
/// the snapshot path in `TerminalPane::snapshot` can emit
/// `LeafContents::AmbientAgent` with the task id preserved. Without this,
/// the snapshot falls through to an empty `LeafContents::Terminal` and the
/// pane restores as a stray local terminal on the next launch.
#[test]
fn create_shared_session_viewer_with_cloud_mode_populates_ambient_agent_view_model() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let resources = TerminalViewResources {
                tips_completed: panes.tips_completed.clone(),
                server_api: panes.server_api.clone(),
                model_event_sender: panes.model_event_sender.clone(),
            };
            let (terminal_view, _terminal_manager) = PaneGroup::create_shared_session_viewer(
                SessionId::new(),
                resources,
                Vector2F::new(800., 600.),
                false, // enable_orchestration_polling
                true,  // is_cloud_mode
                ctx,
            );
            assert!(
                terminal_view.as_ref(ctx).ambient_agent_view_model().is_some(),
                "cloud-mode shared-session viewer must construct an ambient_agent_view_model so the snapshot path emits LeafContents::AmbientAgent on restart",
            );
        });
    });
}

/// Pins the existing behavior of the non-cloud-mode branch so callers that
/// rely on it (e.g. `new_for_shared_session_viewer`, the per-child viewer
/// path) keep getting a `TerminalView` without an `ambient_agent_view_model`.
/// Future changes that would flip this default are loud.
#[test]
fn create_shared_session_viewer_without_cloud_mode_does_not_populate_ambient_agent_view_model() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let resources = TerminalViewResources {
                tips_completed: panes.tips_completed.clone(),
                server_api: panes.server_api.clone(),
                model_event_sender: panes.model_event_sender.clone(),
            };
            let (terminal_view, _terminal_manager) = PaneGroup::create_shared_session_viewer(
                SessionId::new(),
                resources,
                Vector2F::new(800., 600.),
                false, // enable_orchestration_polling
                false, // is_cloud_mode
                ctx,
            );
            assert!(
                terminal_view.as_ref(ctx).ambient_agent_view_model().is_none(),
                "non-cloud-mode shared-session viewer must not construct an ambient_agent_view_model; existing callers depend on this",
            );
        });
    });
}

#[test]
fn test_entering_parent_agent_view_lazily_restores_hidden_child_pane() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());
        let (child_conversation_id, initial_pane_count, initial_visible_pane_count) = pane_group
            .update(&mut app, |panes, ctx| {
                let parent_pane_id = get_newly_created_pane_id(panes, &[]);
                let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
                let child_conversation_id =
                    restore_child_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
                let initial_pane_count = panes.pane_count();
                let initial_visible_pane_count = panes.visible_pane_count();

                assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));

                enter_agent_view_for_conversation(
                    panes,
                    parent_pane_id,
                    parent_conversation_id,
                    ctx,
                );
                (
                    child_conversation_id,
                    initial_pane_count,
                    initial_visible_pane_count,
                )
            });

        pane_group.update(&mut app, |panes, _ctx| {
            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("parent fullscreen restore should materialize the missing child pane");

            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert_eq!(panes.visible_pane_count(), initial_visible_pane_count);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
        });
    });
}

#[test]
fn test_entering_remote_parent_agent_view_lazily_restores_local_hidden_child_pane() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());
        let (
            parent_pane_id,
            local_child_conversation_id,
            local_child_task_id,
            initial_pane_count,
            initial_visible_pane_count,
        ) = pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let root_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let remote_parent_task_id = new_ambient_agent_task_id();
            let remote_parent_conversation_id = restore_remote_child_conversation(
                panes,
                parent_pane_id,
                root_conversation_id,
                remote_parent_task_id,
                ctx,
            );
            let local_child_task_id = new_ambient_agent_task_id();
            let local_child_conversation_id = restore_child_conversation_with_task_context(
                panes,
                parent_pane_id,
                remote_parent_conversation_id,
                local_child_task_id,
                ctx,
            );
            let initial_pane_count = panes.pane_count();
            let initial_visible_pane_count = panes.visible_pane_count();

            assert!(
                !panes
                    .child_agent_panes
                    .contains_key(&local_child_conversation_id)
            );

            enter_agent_view_for_conversation(
                panes,
                parent_pane_id,
                remote_parent_conversation_id,
                ctx,
            );
            (
                parent_pane_id,
                local_child_conversation_id,
                local_child_task_id,
                initial_pane_count,
                initial_visible_pane_count,
            )
        });

        pane_group.update(&mut app, |panes, ctx| {
            let child_pane_id = panes
                .child_agent_panes
                .get(&local_child_conversation_id)
                .copied()
                .expect(
                    "remote parent fullscreen restore should materialize the missing local child pane",
                );

            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert_eq!(panes.visible_pane_count(), initial_visible_pane_count);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
            assert_eq!(panes.focused_pane_id(ctx), parent_pane_id);
            assert_eq!(
                request_ambient_agent_task_id_for_hidden_child(
                    panes,
                    child_pane_id,
                    ctx,
                ),
                Some(local_child_task_id)
            );
        });
    });
}

#[test]
fn test_entering_remote_parent_agent_view_lazily_restores_remote_hidden_child_pane() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());
        let (
            parent_pane_id,
            remote_child_conversation_id,
            remote_child_task_id,
            initial_pane_count,
            initial_visible_pane_count,
        ) = pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let root_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let remote_parent_task_id = new_ambient_agent_task_id();
            let remote_parent_conversation_id = restore_remote_child_conversation(
                panes,
                parent_pane_id,
                root_conversation_id,
                remote_parent_task_id,
                ctx,
            );
            let remote_child_task_id = new_ambient_agent_task_id();
            let remote_child_conversation_id = restore_remote_child_conversation(
                panes,
                parent_pane_id,
                remote_parent_conversation_id,
                remote_child_task_id,
                ctx,
            );
            let initial_pane_count = panes.pane_count();
            let initial_visible_pane_count = panes.visible_pane_count();

            assert!(
                !panes
                    .child_agent_panes
                    .contains_key(&remote_child_conversation_id)
            );

            // Attachable task so the lazily-restored remote child live-attaches.
            AgentConversationsModel::handle(ctx).update(ctx, |model, _| {
                model.insert_task_for_test(attachable_ambient_agent_task(remote_child_task_id));
            });

            enter_agent_view_for_conversation(
                panes,
                parent_pane_id,
                remote_parent_conversation_id,
                ctx,
            );
            (
                parent_pane_id,
                remote_child_conversation_id,
                remote_child_task_id,
                initial_pane_count,
                initial_visible_pane_count,
            )
        });

        pane_group.update(&mut app, |panes, ctx| {
            let child_pane_id = panes
                .child_agent_panes
                .get(&remote_child_conversation_id)
                .copied()
                .expect(
                    "remote parent fullscreen restore should materialize the missing remote child pane",
                );
            let (ambient_task_id, is_agent_running, active_conversation_id) =
                ambient_child_session_state(panes, child_pane_id, ctx);

            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert_eq!(panes.visible_pane_count(), initial_visible_pane_count);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
            assert_eq!(panes.focused_pane_id(ctx), parent_pane_id);
            assert_eq!(ambient_task_id, Some(remote_child_task_id));
            assert!(
                is_agent_running,
                "remote child restore should reconnect to the existing ambient session",
            );
            assert_eq!(active_conversation_id, Some(remote_child_conversation_id));
        });
    });
}

#[test]
fn test_add_pane_restores_hidden_child_when_parent_is_already_fullscreen() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let initial_pane_count = panes.pane_count();
            let initial_visible_pane_count = panes.visible_pane_count();
            let (pane_data, parent_pane_id, child_conversation_id) =
                create_already_fullscreen_parent_pane_data(panes, ctx);

            panes.add_pane_with_direction(Direction::Right, pane_data, true, ctx);

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("adding an already-fullscreen parent should materialize the child pane");

            assert!(panes.has_pane_id(parent_pane_id));
            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(panes.pane_count(), initial_pane_count + 1);
            assert_eq!(panes.visible_pane_count(), initial_visible_pane_count + 1);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
            assert_eq!(panes.focused_pane_id(ctx), parent_pane_id);
            assert_eq!(
                panes.pane_id_for_owned_conversation(child_conversation_id, ctx),
                Some(child_pane_id)
            );
        });
    });
}

#[test]
fn test_reattach_panes_restores_hidden_child_when_parent_is_already_fullscreen() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let child_conversation_id =
                restore_child_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
            let initial_pane_count = panes.pane_count();
            let initial_visible_pane_count = panes.visible_pane_count();

            panes.detach_panes(ctx);
            enter_agent_view_for_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
            assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));

            panes.reattach_panes(ctx);

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect(
                    "reattaching an already-fullscreen parent should materialize the child pane",
                );

            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert_eq!(panes.visible_pane_count(), initial_visible_pane_count);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
            assert_eq!(
                panes.pane_id_for_owned_conversation(child_conversation_id, ctx),
                Some(child_pane_id)
            );
        });
    });
}

#[test]
fn test_restore_closed_pane_restores_hidden_child_when_parent_is_already_fullscreen() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);
    let _undo_closed_panes = FeatureFlag::UndoClosedPanes.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            panes.add_pane_with_direction(
                Direction::Right,
                NotebookPane::new(new_notebook(ctx), ctx),
                false,
                ctx,
            );

            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let child_conversation_id =
                restore_child_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
            let initial_pane_count = panes.pane_count();
            let initial_visible_pane_count = panes.visible_pane_count();

            panes.close_pane(parent_pane_id, ctx);
            assert!(panes.is_pane_hidden_for_close(parent_pane_id));

            enter_agent_view_for_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
            assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));

            assert!(panes.restore_closed_pane(parent_pane_id, ctx));

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect(
                    "restoring an already-fullscreen closed parent should materialize the child pane",
                );

            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert_eq!(panes.visible_pane_count(), initial_visible_pane_count);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
            assert_eq!(panes.focused_pane_id(ctx), parent_pane_id);
            assert_eq!(
                panes.pane_id_for_owned_conversation(child_conversation_id, ctx),
                Some(child_pane_id)
            );
        });
    });
}

#[test]
fn test_replace_pane_restores_hidden_child_when_replacement_is_already_fullscreen() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let original_pane_id = get_newly_created_pane_id(panes, &[]);
            let initial_pane_count = panes.pane_count();
            let initial_visible_pane_count = panes.visible_pane_count();
            let (replacement_pane, replacement_pane_id, child_conversation_id) =
                create_already_fullscreen_parent_pane_data(panes, ctx);

            assert!(panes.replace_pane(original_pane_id, replacement_pane, false, ctx));

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect(
                    "replacing with an already-fullscreen parent should materialize the child pane",
                );

            assert!(!panes.has_pane_id(original_pane_id));
            assert!(panes.has_pane_id(replacement_pane_id));
            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert_eq!(panes.visible_pane_count(), initial_visible_pane_count);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
            assert_eq!(panes.focused_pane_id(ctx), replacement_pane_id);
            assert_eq!(
                panes.pane_id_for_owned_conversation(child_conversation_id, ctx),
                Some(child_pane_id)
            );
        });
    });
}

#[test]
fn test_ensure_hidden_child_agent_pane_materializes_missing_child_pane() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let child_conversation_id =
                restore_child_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
            let initial_pane_count = panes.pane_count();

            assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));
            assert!(
                panes.ensure_hidden_child_agent_pane_for_conversation(child_conversation_id, ctx),
                "navigation fallback should materialize the missing child pane on demand"
            );

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("on-demand ensure should track the restored child pane");
            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
        });
    });
}

#[test]
fn test_ensure_hidden_child_agent_pane_materializes_restored_remote_child_linked_by_parent_agent_id()
 {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_terminal_view_id = panes
                .terminal_view_from_pane_id(parent_pane_id, ctx)
                .expect("parent pane should have a terminal view")
                .id();
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let child_conversation_id = AIConversationId::new();
            let parent_run_id = new_ambient_agent_task_id().to_string();
            let task_id = new_ambient_agent_task_id();
            let initial_pane_count = panes.pane_count();

            BlocklistAIHistoryModel::handle(ctx).update(ctx, |history_model, ctx| {
                history_model.assign_run_id_for_conversation(
                    parent_conversation_id,
                    parent_run_id.clone(),
                    None,
                    parent_terminal_view_id,
                    ctx,
                );
                history_model
                    .set_parent_for_conversation(child_conversation_id, parent_conversation_id);
            });
            RestoredAgentConversations::handle(ctx).update(ctx, |store, _| {
                *store = RestoredAgentConversations::new_seeded(vec![
                    persisted_remote_child_conversation(
                        child_conversation_id,
                        None,
                        Some(parent_run_id),
                        task_id,
                    ),
                ]);
            });
            // Attachable task so the on-demand restore live-attaches.
            AgentConversationsModel::handle(ctx).update(ctx, |model, _| {
                model.insert_task_for_test(attachable_ambient_agent_task(task_id));
            });

            assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));
            assert!(
                panes.ensure_hidden_child_agent_pane_for_conversation(child_conversation_id, ctx),
                "navigation fallback should restore a parent_agent_id-linked remote child pane",
            );

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("parent_agent_id-linked child pane should be tracked after restoration");
            let (ambient_task_id, is_agent_running, active_conversation_id) =
                ambient_child_session_state(panes, child_pane_id, ctx);

            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
            assert_eq!(ambient_task_id, Some(task_id));
            assert!(
                is_agent_running,
                "restored remote child pane should reconnect to the ambient session",
            );
            assert_eq!(active_conversation_id, Some(child_conversation_id));
        });
    });
}

#[test]
fn test_entering_parent_agent_view_skips_child_owned_by_another_pane() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());
        let (child_conversation_id, initial_pane_count) =
            pane_group.update(&mut app, |panes, ctx| {
                let parent_pane_id = get_newly_created_pane_id(panes, &[]);
                let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);

                panes.add_terminal_pane(Direction::Right, None, ctx);
                let sibling_pane_id = get_newly_created_pane_id(panes, &[parent_pane_id]);
                let child_conversation_id =
                    restore_child_conversation(panes, sibling_pane_id, parent_conversation_id, ctx);
                let initial_pane_count = panes.pane_count();

                enter_agent_view_for_conversation(
                    panes,
                    sibling_pane_id,
                    child_conversation_id,
                    ctx,
                );
                assert_eq!(
                    panes.pane_id_for_owned_conversation(child_conversation_id, ctx),
                    Some(sibling_pane_id)
                );

                enter_agent_view_for_conversation(
                    panes,
                    parent_pane_id,
                    parent_conversation_id,
                    ctx,
                );
                (child_conversation_id, initial_pane_count)
            });

        pane_group.update(&mut app, |panes, _ctx| {
            assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
        });
    });
}

#[test]
fn test_ensure_hidden_child_agent_pane_skips_child_owned_by_another_pane_group() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let parent_pane_group = mock_pane_group(&mut app, Default::default());
        let other_pane_group = mock_pane_group(&mut app, Default::default());

        let parent_conversation_id = parent_pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            start_parent_conversation(panes, parent_pane_id, ctx)
        });
        let (child_conversation_id, child_owner_terminal_view_id) =
            other_pane_group.update(&mut app, |panes, ctx| {
                let child_pane_id = get_newly_created_pane_id(panes, &[]);
                let child_conversation_id =
                    restore_child_conversation(panes, child_pane_id, parent_conversation_id, ctx);
                let initial_owner_terminal_view_id = panes
                    .terminal_view_from_pane_id(child_pane_id, ctx)
                    .expect("child pane should have a terminal view")
                    .id();

                enter_agent_view_for_conversation(panes, child_pane_id, child_conversation_id, ctx);
                (child_conversation_id, initial_owner_terminal_view_id)
            });

        parent_pane_group.update(&mut app, |panes, ctx| {
            let initial_pane_count = panes.pane_count();

            assert!(
                panes.ensure_hidden_child_agent_pane_for_conversation(child_conversation_id, ctx),
                "cross-tab child ownership should be treated as already reachable"
            );
            assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert_eq!(
                BlocklistAIHistoryModel::as_ref(ctx)
                    .terminal_surface_id_for_conversation(&child_conversation_id),
                Some(child_owner_terminal_view_id)
            );
        });
    });
}

#[test]
fn test_entering_parent_agent_view_skips_child_owned_by_another_pane_group() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let parent_pane_group = mock_pane_group(&mut app, Default::default());
        let other_pane_group = mock_pane_group(&mut app, Default::default());

        let (parent_conversation_id, parent_pane_id) =
            parent_pane_group.update(&mut app, |panes, ctx| {
                let parent_pane_id = get_newly_created_pane_id(panes, &[]);
                let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
                (parent_conversation_id, parent_pane_id)
            });
        let (child_conversation_id, child_owner_terminal_view_id) =
            other_pane_group.update(&mut app, |panes, ctx| {
                let child_pane_id = get_newly_created_pane_id(panes, &[]);
                let child_conversation_id =
                    restore_child_conversation(panes, child_pane_id, parent_conversation_id, ctx);
                let initial_owner_terminal_view_id = panes
                    .terminal_view_from_pane_id(child_pane_id, ctx)
                    .expect("child pane should have a terminal view")
                    .id();

                enter_agent_view_for_conversation(panes, child_pane_id, child_conversation_id, ctx);
                (child_conversation_id, initial_owner_terminal_view_id)
            });
        let initial_pane_count = parent_pane_group.update(&mut app, |panes, ctx| {
            let initial_pane_count = panes.pane_count();
            enter_agent_view_for_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
            initial_pane_count
        });

        parent_pane_group.update(&mut app, |panes, ctx| {
            assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert_eq!(
                BlocklistAIHistoryModel::as_ref(ctx)
                    .terminal_surface_id_for_conversation(&child_conversation_id),
                Some(child_owner_terminal_view_id)
            );
        });
    });
}

#[test]
fn test_active_session_id_reset_on_last_pane_close() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let terminal_id = get_newly_created_pane_id(panes, &[]);
            assert_eq!(
                panes.active_session_id(ctx),
                terminal_id.as_terminal_pane_id()
            );

            // Add a non-terminal pane (Notebook) so the pane group remains alive when terminal is closed.
            panes.add_pane_with_direction(
                Direction::Right,
                NotebookPane::new(new_notebook(ctx), ctx),
                false, /* focus_new_pane */
                ctx,
            );

            // Close the terminal.
            panes.close_pane(terminal_id, ctx);

            // active_session_id should be None after closing the last pane.
            assert_eq!(
                panes.active_session_id(ctx),
                None,
                "active_session_id should be None after closing the last pane"
            );
        });
    });
}

#[test]
fn test_add_pane_aborts_cleanly_when_pre_attach_returns_false() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let before_snapshot = panes.snapshot(ctx);
            let before_count = panes.pane_count();

            panes.add_pane_with_direction(
                Direction::Right,
                PreAttachReturnsFalsePane::new(ctx),
                true, /* focus_new_pane */
                ctx,
            );

            assert_eq!(panes.pane_count(), before_count);
            assert_eq!(panes.snapshot(ctx), before_snapshot);
        });
    });
}

#[test]
fn test_focus_notebook() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let first_terminal_id = get_newly_created_pane_id(panes, &[]);

            // Add a notebook to the left.
            panes.add_pane_with_direction(
                Direction::Left,
                NotebookPane::new(new_notebook(ctx), ctx),
                true, /* focus_new_pane */
                ctx,
            );
            let notebook_id = get_newly_created_pane_id(panes, &[first_terminal_id]);

            // The new pane should be focused, but the terminal is still the active session.
            assert_eq!(panes.focused_pane_id(ctx), notebook_id);
            assert_eq!(
                panes.active_session_id(ctx).map(Into::into),
                Some(first_terminal_id)
            );
            assert_eq!(
                split_pane_state(panes, first_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
            assert!(is_active_session(panes, first_terminal_id, ctx));
            assert_eq!(
                split_pane_state(panes, notebook_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Focused)
            );

            // Add a terminal below.
            panes.add_terminal_pane(Direction::Down, None, ctx);
            let second_terminal_id =
                get_newly_created_pane_id(panes, &[first_terminal_id, notebook_id]);

            // The new terminal should be both focused and the active session.
            assert_eq!(panes.focused_pane_id(ctx), second_terminal_id);
            assert_eq!(
                panes.active_session_id(ctx).map(Into::into),
                Some(second_terminal_id)
            );
            assert_eq!(
                split_pane_state(panes, first_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
            assert!(!is_active_session(panes, first_terminal_id, ctx));
            assert_eq!(
                split_pane_state(panes, second_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Focused)
            );
            assert!(is_active_session(panes, second_terminal_id, ctx));
            assert_eq!(
                split_pane_state(panes, notebook_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );

            // Close the new terminal. Focus should switch to the notebook, and the first terminal
            // session will activate.
            panes.close_pane(second_terminal_id, ctx);
            assert_eq!(panes.focused_pane_id(ctx), notebook_id);
            assert_eq!(
                panes.active_session_id(ctx).map(Into::into),
                Some(first_terminal_id)
            );
            assert_eq!(
                split_pane_state(panes, first_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
            assert_eq!(
                split_pane_state(panes, notebook_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Focused)
            );
            assert!(is_active_session(panes, first_terminal_id, ctx));
        })
    });
}

#[test]
fn test_group_without_terminals() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let terminal_id = get_newly_created_pane_id(panes, &[]);

            // Add a notebook to the left.
            panes.add_pane_with_direction(
                Direction::Left,
                NotebookPane::new(new_notebook(ctx), ctx),
                true, /* focus_new_pane */
                ctx,
            );
            let notebook_id = get_newly_created_pane_id(panes, &[terminal_id]);

            // Close the terminal, which should leave the group without an active session.
            panes.close_pane(terminal_id, ctx);
            assert_eq!(panes.focused_pane_id(ctx), notebook_id);
            assert_eq!(panes.active_session_id(ctx), None);
            assert_eq!(
                split_pane_state(panes, notebook_id, ctx),
                SplitPaneState::NotInSplitPane
            );
        });
    });
}

#[test]
fn test_close_active_session() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            // Add two terminal sessions.
            let first_terminal_id = get_newly_created_pane_id(panes, &[]);
            panes.add_terminal_pane(Direction::Up, None, ctx);
            let second_terminal_id = get_newly_created_pane_id(panes, &[first_terminal_id]);

            // Add a notebook to the left.
            panes.add_pane_with_direction(
                Direction::Left,
                NotebookPane::new(new_notebook(ctx), ctx),
                true, /* focus_new_pane */
                ctx,
            );
            let notebook_id =
                get_newly_created_pane_id(panes, &[first_terminal_id, second_terminal_id]);
            assert_eq!(panes.focused_pane_id(ctx), notebook_id);
            assert_eq!(
                panes.active_session_id(ctx).map(Into::into),
                Some(second_terminal_id)
            );

            // Close the active session, which should leave the notebook focused and activate the
            // remaining session.
            panes.close_pane(second_terminal_id, ctx);
            assert_eq!(panes.focused_pane_id(ctx), notebook_id);
            assert_eq!(
                panes.active_session_id(ctx).map(Into::into),
                Some(first_terminal_id)
            );
            assert_eq!(
                split_pane_state(panes, first_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
            assert!(is_active_session(panes, first_terminal_id, ctx));

            // Now, focus the remaining session, which should keep it activated.
            panes.focus_pane_by_id(first_terminal_id, ctx);
            assert_eq!(panes.focused_pane_id(ctx), first_terminal_id);
            assert_eq!(
                panes.active_session_id(ctx).map(Into::into),
                Some(first_terminal_id)
            );
            assert_eq!(
                split_pane_state(panes, first_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Focused)
            );
            assert_eq!(
                split_pane_state(panes, notebook_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
            assert!(is_active_session(panes, first_terminal_id, ctx));
        });
    });
}

#[test]
fn test_update_session_visibility() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(&mut app, Default::default());
        pane_group.update(&mut app, |panes, ctx| {
            // Assert that there is no active window.
            WindowManager::handle(ctx).read(ctx, |state, _| {
                assert_eq!(state.stage(), ApplicationStage::Starting);
                assert!(state.active_window().is_none());
            });

            fn visibility_matches(panes: &PaneGroup, expected: bool, ctx: &ViewContext<PaneGroup>) {
                for data in panes.panes_of::<TerminalPane>() {
                    let view = data.terminal_view(ctx).as_ref(ctx);
                    assert_eq!(
                        view.was_ever_visible(),
                        expected,
                        "View {} visibility was {}, expected {}",
                        data.terminal_view(ctx).id(),
                        view.was_ever_visible(),
                        expected
                    );
                }
            }

            // Add pane Left.
            panes.add_terminal_pane(Direction::Left, None, ctx);

            // Assert that neither of the panes are marked as visible (due
            // to the fact that the window is not active).
            visibility_matches(panes, false, ctx);

            let window_id = ctx.window_id();
            WindowManager::handle(ctx).update(ctx, |state, ctx| {
                state.overwrite_for_test(ApplicationStage::Active, Some(window_id));
                ctx.notify();
            });

            // Assert that both of the panes are still not marked as
            // visible, given the fact that the pane group is not focused.
            visibility_matches(panes, false, ctx);

            panes.focus(ctx);

            // Assert that both of the panes are now visible.
            visibility_matches(panes, true, ctx);
        })
    });
}

#[test]
fn test_initial_widths_are_computed_correctly() {
    use launch_config::PaneTemplateType::*;

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        // Define a simple macro to help us create new leaf panes.
        macro_rules! leaf_pane {
            () => {
                PaneTemplate {
                    is_focused: None,
                    cwd: "".into(),
                    commands: vec![],
                    pane_mode: PaneMode::Terminal,
                    shell: None,
                }
            };
        }

        // Pick an arbitrary initial window that isn't the same as the
        // fallback value.
        let window_width = 864.;
        let window_height = 636.;
        assert_ne!(window_width, FALLBACK_INITIAL_WINDOW_SIZE.x());
        assert_ne!(window_height, FALLBACK_INITIAL_WINDOW_SIZE.y());

        // Create a template that looks like the following, with each pane
        // numbered by its index in the pane group:
        //
        //  ---------------------
        //  |         0         |
        //  | __________________|
        //  |     1   |____2____|
        //  | ________|____3____|
        //  |   4  |   5  |  6  |
        //  |      |      |     |
        //  ---------------------
        let template = PaneBranchTemplate {
            split_direction: launch_config::SplitDirection::Vertical,
            panes: vec![
                leaf_pane!(),
                PaneBranchTemplate {
                    split_direction: launch_config::SplitDirection::Horizontal,
                    panes: vec![
                        leaf_pane!(),
                        PaneBranchTemplate {
                            split_direction: launch_config::SplitDirection::Vertical,
                            panes: vec![leaf_pane!(), leaf_pane!()],
                        },
                    ],
                },
                PaneBranchTemplate {
                    split_direction: launch_config::SplitDirection::Horizontal,
                    panes: vec![leaf_pane!(), leaf_pane!(), leaf_pane!()],
                },
            ],
        };

        let window_size = Vector2F::new(window_width, window_height);
        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template,
                    shell_start: ShellStartPolicy::Eager,
                },
                window_bounds: WindowBounds::ExactPosition(RectF::new(
                    Vector2F::zero(),
                    window_size,
                )),
            },
        );

        // Assert that the window created by the call to `mock_pane_group`
        // has the expected bounds.
        let window_id = app.read(|ctx| pane_group.window_id(ctx));
        app.update(|ctx| {
            assert_eq!(
                Some(window_size),
                ctx.window_bounds(&window_id).map(|rect| rect.size())
            );
        });

        let pane_group_width = window_width - 2.0 * workspace::WORKSPACE_PADDING;
        let pane_group_height =
            window_height - workspace::TOTAL_TAB_BAR_HEIGHT - 2.0 * workspace::WORKSPACE_PADDING;

        pane_group.read(&app, |pane_group, ctx| {
            // Make assertions about the expected widths of the various
            // panes.
            assert_eq!(
                pane_group
                    .terminal_view_at_pane_index(0, ctx)
                    .unwrap()
                    .as_ref(ctx)
                    .size_info()
                    .pane_width_px()
                    .as_f32(),
                pane_group_width,
                "Pane with index 0 had unexpected width!"
            );
            let half_width = (pane_group_width - tree::get_divider_thickness()) / 2.;
            for i in 1..=3 {
                assert_eq!(
                    pane_group
                        .terminal_view_at_pane_index(i, ctx)
                        .unwrap()
                        .as_ref(ctx)
                        .size_info()
                        .pane_width_px()
                        .as_f32(),
                    half_width,
                    "Pane with index {i} had unexpected width!"
                );
            }
            let one_third_width = (pane_group_width - (2. * tree::get_divider_thickness())) / 3.;
            for i in 4..=6 {
                assert_eq!(
                    pane_group
                        .terminal_view_at_pane_index(i, ctx)
                        .unwrap()
                        .as_ref(ctx)
                        .size_info()
                        .pane_width_px()
                        .as_f32(),
                    one_third_width,
                    "Pane with index {i} had unexpected width!"
                );
            }

            // Make assertions about the expected heights of the various
            // panes.
            let one_third_height = (pane_group_height - (2. * tree::get_divider_thickness())) / 3.;
            for i in (0..=1).chain(4..=6) {
                assert_eq!(
                    pane_group
                        .terminal_view_at_pane_index(i, ctx)
                        .unwrap()
                        .as_ref(ctx)
                        .size_info()
                        .pane_height_px()
                        .as_f32(),
                    one_third_height,
                    "Pane with index {i} had unexpected height!"
                );
            }
            let one_sixth_height = (pane_group_height - (5. * tree::get_divider_thickness())) / 6.;
            for i in 2..=3 {
                assert_eq!(
                    pane_group
                        .terminal_view_at_pane_index(i, ctx)
                        .unwrap()
                        .as_ref(ctx)
                        .size_info()
                        .pane_height_px()
                        .as_f32(),
                    one_sixth_height,
                    "Pane with index {i} had unexpected height!"
                );
            }
        });
    });
}

#[test]
fn test_is_terminal_pane_being_shared() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(&mut app, Default::default());
        pane_group.update(&mut app, |panes, ctx| {
            assert!(!panes.is_terminal_pane_being_shared(ctx));

            // Add another pane; the pane group should still be "unshared".
            panes.add_terminal_pane(Direction::Left, None, ctx);
            assert!(!panes.is_terminal_pane_being_shared(ctx));

            // Make one of the terminal panes shared. There is now at least one terminal pane being shared.
            panes
                .terminal_session_by_pane_index(0)
                .expect("terminal pane exists")
                .terminal_manager(ctx)
                .as_ref(ctx)
                .model()
                .lock()
                .set_shared_session_status(SharedSessionStatus::ActiveSharer);
            assert!(panes.is_terminal_pane_being_shared(ctx));
        });
    });
}

#[test]
fn test_number_of_shared_panes() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            // We have two terminal sessions. Neither is shared
            let first_pane_id = get_newly_created_pane_id(panes, &[]);
            panes.add_terminal_pane(Direction::Up, None, ctx);
            assert_eq!(panes.number_of_shared_sessions(ctx), 0);

            // Make one pane shared
            panes
                .terminal_manager(0, ctx)
                .unwrap()
                .as_ref(ctx)
                .model()
                .lock()
                .set_shared_session_status(SharedSessionStatus::ActiveSharer);
            assert_eq!(panes.number_of_shared_sessions(ctx), 1);

            // Make both panes shared
            panes
                .terminal_manager(1, ctx)
                .unwrap()
                .as_ref(ctx)
                .model()
                .lock()
                .set_shared_session_status(SharedSessionStatus::ActiveSharer);
            assert_eq!(panes.number_of_shared_sessions(ctx), 2);

            // Close a pane
            panes.close_pane(first_pane_id, ctx);
            assert_eq!(panes.number_of_shared_sessions(ctx), 1);
        });
    });
}

#[test]
fn test_start_shared_session_from_modal() {
    let _guard = FeatureFlag::CreatingSharedSessions.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |pane_group, ctx| {
            let terminal_pane = pane_group.terminal_session_by_pane_index(0).unwrap();
            let terminal_pane_id = terminal_pane.terminal_pane_id();
            let terminal_model = terminal_pane.terminal_manager(ctx).as_ref(ctx).model();

            assert!(matches!(
                terminal_model.lock().shared_session_status(),
                SharedSessionStatus::NotShared
            ));

            pane_group.open_share_session_modal(
                terminal_pane_id,
                SharedSessionActionSource::PaneHeader,
                ctx,
            );
            assert!(pane_group.terminal_with_open_share_session_modal.is_some());
            assert_eq!(
                pane_group
                    .share_session_modal
                    .as_ref(ctx)
                    .terminal_pane_id(),
                Some(terminal_pane_id)
            );

            pane_group.handle_share_session_modal_event(
                &ShareSessionModalEvent::StartSharing {
                    terminal_pane_id,
                    scrollback_type: SharedSessionScrollbackType::None,
                    source: SharedSessionActionSource::PaneHeader,
                },
                ctx,
            );
            assert!(pane_group.terminal_with_open_share_session_modal.is_none());
            assert!(matches!(
                terminal_model.lock().shared_session_status(),
                SharedSessionStatus::SharePending
            ));
        });

        // Wait for one tick of the event loop for the share to be started.
        pane_group.read(&app, |pane_group, ctx| {
            let terminal_view = pane_group
                .terminal_view_at_pane_index(0, ctx)
                .unwrap()
                .to_owned();
            let model = terminal_view.as_ref(ctx).model.lock();
            assert!(matches!(
                model.shared_session_status(),
                SharedSessionStatus::ActiveSharer
            ));

            let manager = shared_session::manager::Manager::as_ref(ctx);
            let shared_views = manager.shared_views(ctx).collect_vec();
            assert_eq!(shared_views.len(), 1);
            assert_eq!(shared_views[0].id(), terminal_view.id());

            let terminal_pane = pane_group.terminal_session_by_pane_index(0).unwrap();
            assert!(
                terminal_pane
                    .pane_view()
                    .as_ref(ctx)
                    .header()
                    .as_ref(ctx)
                    .has_shareable_object(ctx)
            );
        });
    });
}

/// TODO: look into moving this test somewhere more suitable.
/// Currently, the pane group is responsible for creating and owning
/// the terminal manager, which in turn owns the Network model for the share.
#[test]
fn test_stop_shared_session() {
    let _guard = FeatureFlag::CreatingSharedSessions.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        // Start the shared session.
        pane_group.update(&mut app, |pane_group, ctx| {
            let terminal_pane = pane_group.terminal_session_by_pane_index(0).unwrap();
            let terminal_view = terminal_pane.terminal_view(ctx);
            terminal_view.update(ctx, |terminal_view, ctx| {
                terminal_view.attempt_to_share_session(
                    SharedSessionScrollbackType::None,
                    None,
                    SharedSessionSource::user(None),
                    false,
                    ctx,
                );
            });
        });

        // Wait for one tick of the event loop for the share to be started.
        pane_group.read(&app, |pane_group, ctx| {
            let terminal_model = pane_group
                .terminal_session_by_pane_index(0)
                .unwrap()
                .to_owned()
                .terminal_manager(ctx)
                .as_ref(ctx)
                .model();
            assert!(matches!(
                terminal_model.lock().shared_session_status(),
                SharedSessionStatus::ActiveSharer
            ));
        });

        // Stop the shared session.
        pane_group.update(&mut app, |pane_group, ctx| {
            let terminal_pane = pane_group.terminal_session_by_pane_index(0).unwrap();
            let terminal_view = terminal_pane.terminal_view(ctx);
            terminal_view.update(ctx, |terminal_view, ctx| {
                terminal_view.stop_sharing_session(SharedSessionActionSource::PaneHeader, ctx);
            });
        });

        // Ensure the state is correct after stopping.
        pane_group.update(&mut app, |pane_group, ctx| {
            let terminal_pane = pane_group.terminal_session_by_pane_index(0).unwrap();
            let terminal_manager = terminal_pane
                .terminal_manager(ctx)
                .as_ref(ctx)
                .as_any()
                .downcast_ref::<TerminalManager<TerminalView>>()
                .unwrap();
            let terminal_model = terminal_pane.terminal_manager(ctx).as_ref(ctx).model();

            assert!(terminal_manager.session_sharer().borrow().is_none());
            assert!(matches!(
                terminal_model.lock().shared_session_status(),
                SharedSessionStatus::NotShared
            ));

            let manager = shared_session::manager::Manager::as_ref(ctx);
            let shared_views = manager.shared_views(ctx).collect_vec();
            assert!(shared_views.is_empty());

            assert!(
                !terminal_pane
                    .pane_view()
                    .as_ref(ctx)
                    .header()
                    .as_ref(ctx)
                    .has_shareable_object(ctx)
            );
        });
    });
}

#[test]
fn test_navigation_skips_hidden_closed_panes() {
    let _guard = FeatureFlag::UndoClosedPanes.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            // Add second terminal to the right to create a horizontal pair
            panes.add_terminal_pane(Direction::Right, None, ctx);

            // Add third terminal; place it to the right of current focus
            panes.add_terminal_pane(Direction::Right, None, ctx);

            // Determine ordered visible panes by index 0..2
            let a = panes.pane_id_by_index(0).expect("pane 0 exists");
            let b = panes.pane_id_by_index(1).expect("pane 1 exists");
            let c = panes.pane_id_by_index(2).expect("pane 2 exists");

            // Focus C and confirm prev would be B when all are visible
            panes.focus_pane_by_id(c, ctx);
            assert_eq!(panes.prev_pane_id_navigation(c), Some(b));

            // Close B (it will be hidden for undo and excluded from visible navigation)
            panes.close_pane(b, ctx);

            // Now prev from C should skip B and go to A
            assert_eq!(panes.prev_pane_id_navigation(c), Some(a));

            // And next from A should skip B and go to C
            assert_eq!(panes.next_pane_id(a), Some(c));
        })
    });
}

/// Regression test: closing a host pane on the non-undo `close_pane` branch
/// must clear its entry from `transitively_shared_child_panes`. The undo
/// branch relies on `cleanup_closed_pane` to call
/// `forget_transitively_shared_pane`, but the non-undo branch destroys the
/// pane directly and previously skipped that cleanup, leaking stale entries.
#[test]
fn test_close_pane_clears_transitively_shared_child_entry_on_non_undo_branch() {
    let _undo_closed_panes = FeatureFlag::UndoClosedPanes.override_enabled(false);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let host_pane_id = get_newly_created_pane_id(panes, &[]);

            // Add a sibling terminal so the host close does not trip the
            // `pane_count() == 1` early return in `close_pane`'s non-undo
            // branch.
            panes.add_terminal_pane(Direction::Right, None, ctx);

            // Cascade an off-tree transitively-shared child onto the host
            // pane id; this populates `transitively_shared_child_panes`.
            let child_pane_id = panes.insert_terminal_pane_hidden_for_child_agent(
                host_pane_id,
                HashMap::new(),
                IsSharedSessionCreator::Yes {
                    source: SharedSessionSource::user(Some("host-task".to_string())),
                },
                ctx,
            );

            assert!(
                panes
                    .transitively_shared_child_panes
                    .get(&host_pane_id)
                    .is_some_and(|children| children.contains(&child_pane_id.into())),
                "setup precondition: host should track its transitively-shared child"
            );

            // Close the host via the non-undo branch.
            panes.close_pane(host_pane_id, ctx);

            assert!(
                !panes
                    .transitively_shared_child_panes
                    .contains_key(&host_pane_id),
                "host entry must be cleared after close_pane on the non-undo branch"
            );
        })
    });
}

// Ensures that we always show the pane header for terminal panes, regardless of split state.
#[test]
fn test_terminal_pane_headers() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        // There should be a single terminal pane to start and the pane header should not be shown.
        pane_group.read(&app, |pane_group, ctx| {
            assert_eq!(pane_group.pane_contents.len(), 1);

            let terminal_panes = pane_group.panes_of::<TerminalPane>().collect_vec();
            assert_eq!(terminal_panes.len(), 1);

            let pane_view = terminal_panes[0].pane_view();
            let header_visible = pane_view
                .as_ref(ctx)
                .header()
                .as_ref(ctx)
                .is_visible_in_pane_group();
            assert!(header_visible);
        });

        // Create a terminal split pane.
        pane_group.update(&mut app, |pane_group, ctx| {
            pane_group.add_terminal_pane(Direction::Left, None, ctx);
        });

        // There should be two terminal panes and they should both have the pane header.
        pane_group.read(&app, |pane_group, ctx| {
            assert_eq!(pane_group.pane_contents.len(), 2);

            let terminal_panes = pane_group.panes_of::<TerminalPane>().collect_vec();
            assert_eq!(terminal_panes.len(), 2);

            for terminal_pane in terminal_panes {
                let pane_view = terminal_pane.pane_view();
                assert!(
                    pane_view
                        .as_ref(ctx)
                        .header()
                        .as_ref(ctx)
                        .is_visible_in_pane_group()
                );
            }
        });

        // Close one of the panes; the remaining pane should still have a header.
        pane_group.update(&mut app, |pane_group, ctx| {
            pane_group.close_pane(pane_group.focused_pane_id(ctx), ctx);
        });

        pane_group.read(&app, |pane_group, ctx| {
            assert_eq!(pane_group.pane_contents.len(), 1);

            let terminal_panes = pane_group.panes_of::<TerminalPane>().collect_vec();
            assert_eq!(terminal_panes.len(), 1);

            let pane_view = terminal_panes[0].pane_view();
            assert!(
                pane_view
                    .as_ref(ctx)
                    .header()
                    .as_ref(ctx)
                    .is_visible_in_pane_group()
            );
        });

        // Create a non-terminal split pane. Terminal pane header remains visible.
        pane_group.update(&mut app, |pane_group, ctx| {
            pane_group.add_pane_with_direction(
                Direction::Left,
                NotebookPane::new(new_notebook(ctx), ctx),
                true, /* focus_new_pane */
                ctx,
            );
        });

        pane_group.read(&app, |pane_group, ctx| {
            assert_eq!(pane_group.pane_contents.len(), 2);

            let terminal_panes = pane_group.panes_of::<TerminalPane>().collect_vec();
            assert_eq!(terminal_panes.len(), 1);

            let pane_view = terminal_panes[0].pane_view();
            assert!(
                pane_view
                    .as_ref(ctx)
                    .header()
                    .as_ref(ctx)
                    .is_visible_in_pane_group()
            );
        });
    });
}

/// Tests that focusing two different panes in quick succession does not cause
/// an infinite loop of focus changes, as outlined in this PR's description:
/// https://github.com/warpdotdev/warp-internal/pull/8990
#[cfg_attr(windows, ignore = "TODO(CORE-3626)")]
#[test]
fn test_pane_focus_does_not_have_an_infinite_event_loop() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        // Create a pane group with two terminal panes that will fight for
        // focus.
        let mock_options = MockOptions {
            layout: PanesLayout::Template {
                template: PaneTemplateType::PaneBranchTemplate {
                    split_direction:
                        crate::launch_configs::launch_config::SplitDirection::Horizontal,
                    panes: vec![
                        PaneTemplateType::PaneTemplate {
                            is_focused: Some(true),
                            cwd: "/".into(),
                            commands: vec![],
                            pane_mode: PaneMode::Terminal,
                            shell: None,
                        },
                        PaneTemplateType::PaneTemplate {
                            is_focused: None,
                            cwd: "/".into(),
                            commands: vec![],
                            pane_mode: PaneMode::Terminal,
                            shell: None,
                        },
                    ],
                },
                shell_start: ShellStartPolicy::Eager,
            },
            ..Default::default()
        };
        let pane_group = mock_pane_group(&mut app, mock_options);

        // The cycle requires that we are constantly trying to focus the input.
        // An active and long-running block causes focus to move to the
        // terminal instead of the input, so we need to wait until we've
        // finished bootstrapping to ensure no such block will exist.
        loop {
            let mut all_terminals_bootstrapped = true;
            pane_group.update(&mut app, |pane_group, ctx| {
                pane_group.for_all_terminal_panes(|terminal_view, _ctx| {
                    let model = terminal_view.model.lock();
                    let active_block = model.block_list().active_block();
                    if active_block.bootstrap_stage() != crate::terminal::model::bootstrap::BootstrapStage::PostBootstrapPrecmd ||
                        active_block.is_active_and_long_running() {
                        all_terminals_bootstrapped = false;
                    }
                }, ctx);
            });
            if all_terminals_bootstrapped {
                break;
            }
            // Return control back to the executor briefly so we can make
            // progress.
            futures_lite::future::yield_now().await;
        }

        pane_group.update(&mut app, |pane_group, ctx| {
            // Switch panes twice in quick succession.  We want to make
            // sure the test terminates and doesn't get into an infinite
            // loop.
            pane_group.navigate_next_pane(ctx);
            pane_group.navigate_next_pane(ctx);
        });
    });
}

/// A view to help us react to focus changes and know that they were processed
/// synchronously, not asynchronously (via an Effect::Event).
struct FocusDetectionView {
    pane_group: ViewHandle<PaneGroup>,
    new_focused_pane_id: Option<PaneId>,
}

impl FocusDetectionView {
    fn new(pane_group: ViewHandle<PaneGroup>, ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_view(&pane_group, |me, pane_group, event, ctx| {
            let Event::OpenPromptEditor = event else {
                return;
            };
            // This event is enqueued by us after the `Focus` effect, and so
            // by the time we receive it, application focus will have been
            // moved to the second pane, and (crucially) the pane group should
            // have updated its internal state accordingly (which is what we're
            // asserting here).

            let new_focused_pane_id = me
                .new_focused_pane_id
                .expect("should have set this already");
            pane_group.read(ctx, |pane_group, ctx| {
                assert_eq!(pane_group.focused_pane_id(ctx), new_focused_pane_id);
                assert_eq!(
                    pane_group.active_session_id(ctx),
                    new_focused_pane_id.as_terminal_pane_id()
                );
            });
        });
        Self {
            pane_group,
            new_focused_pane_id: None,
        }
    }
}

impl Entity for FocusDetectionView {
    type Event = ();
}

impl View for FocusDetectionView {
    fn ui_name() -> &'static str {
        "FocusDetectionView"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        ChildView::new(&self.pane_group).finish()
    }
}

impl TypedActionView for FocusDetectionView {
    type Action = ();
}

/// This test ensures that a change in application focus causes the pane group
/// focused pane to update synchronously, without needing to wait for effect
/// flushing to occur.
///
/// The goal is to avoid situations where a delayed response to application
/// focus changes leads to an infinite loop of focusing and re-focusing two
/// different panes.
#[test]
fn test_focused_pane_is_synchronized_with_application_focus() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        // Create a pane group with two terminal panes, so that we can move
        // focus and observe the effects.
        let panes_layout = PanesLayout::Template {
            template: PaneTemplateType::PaneBranchTemplate {
                split_direction: crate::launch_configs::launch_config::SplitDirection::Horizontal,
                panes: vec![
                    PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: "/".into(),
                        commands: vec![],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    PaneTemplateType::PaneTemplate {
                        is_focused: None,
                        cwd: "/".into(),
                        commands: vec![],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                ],
            },
            shell_start: ShellStartPolicy::Eager,
        };

        let tips_model = app.add_model(|_| TipsCompleted::default());
        let (_, root_view) =
            app.add_window_with_bounds(WindowStyle::NotStealFocus, WindowBounds::Default, |ctx| {
                let user_default_shell_changed_banner_dismissal_model_handle =
                    ctx.add_model(|_| BannerState::default());
                let block_lists = Arc::new(HashMap::new());
                let pane_group = ctx.add_typed_action_view(|ctx| {
                    PaneGroup::new_with_panes_layout(
                        tips_model,
                        user_default_shell_changed_banner_dismissal_model_handle,
                        ServerApiProvider::as_ref(ctx).get(),
                        panes_layout,
                        block_lists,
                        None,
                        ctx,
                    )
                });

                FocusDetectionView::new(pane_group, ctx)
            });
        let pane_group = root_view.read(&app, |root_view, _ctx| root_view.pane_group.clone());
        let app_state_changed = Rc::new(Cell::new(false));
        app.update({
            let app_state_changed = app_state_changed.clone();
            |ctx| {
                ctx.subscribe_to_view(&pane_group, move |_, event, _| {
                    if matches!(event, Event::AppStateChanged) {
                        app_state_changed.set(true);
                    }
                });
            }
        });

        let (focused_pane_id, active_session_id) = pane_group.read(&app, |pane_group, ctx| {
            (
                pane_group.focused_pane_id(ctx),
                pane_group.active_session_id(ctx),
            )
        });

        let second_pane_id = pane_group.read(&app, |pane_group, _ctx| {
            pane_group
                .pane_ids()
                .find(|pane_id| *pane_id != focused_pane_id)
                .expect("should have more than one pane")
        });

        // Verify that the "second" pane is not focused or active.
        assert_ne!(focused_pane_id, second_pane_id);
        assert_ne!(active_session_id, second_pane_id.as_terminal_pane_id());

        root_view.update(&mut app, |root_view, _ctx| {
            root_view.new_focused_pane_id = Some(second_pane_id);
        });

        pane_group.update(&mut app, |pane_group, ctx| {
            // First, request a change of application focus to the second
            // pane's terminal view.
            pane_group
                .terminal_view_from_pane_id(second_pane_id, ctx)
                .expect("second pane is a terminal pane")
                .update(ctx, |_terminal_view, ctx| {
                    ctx.focus_self();
                });

            // Second, emit an event on the pane group to trigger assertion
            // logic in the FocusDetectionView.  This event effect is enqueued after
            // the focus effect but before the focus effect is processed, meaning
            // it will observe any changes that occurred synchronously as part
            // of the focus effect but will _not_ observe any changes that result
            // from events dispatched during focus handling.
            //
            // We use `OpenPromptEditor` because we can be confident that
            // nothing else above may have emitted this event.
            //
            // IMPORTANT: This MUST be emitted in the same pane group update
            // during which we focus the terminal view, to ensure that the
            // effect queue doesn't get processed or further modified before we
            // enqueue this event on the effect queue.
            ctx.emit(Event::OpenPromptEditor);
        });

        assert!(app_state_changed.get());
    });
}

/// APP-5243: closing a file pane only hides it while undo-close is available, and the same view is
/// reattached without reopening its file. Releasing the file on close would therefore leave a
/// restored pane rendering content that can never update again. The file is released only once the
/// pane is permanently discarded.
#[cfg(feature = "local_fs")]
#[test]
fn test_undo_close_keeps_a_file_pane_watching_its_file() {
    use warp_files::FileModel;

    let _undo_closed_panes = FeatureFlag::UndoClosedPanes.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        app.add_singleton_model(FileModel::new);
        let pane_group = mock_pane_group(&mut app, Default::default());

        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("notes.md");
        std::fs::write(&path, "# before").expect("write file");

        pane_group.update(&mut app, |panes, ctx| {
            let pane = FilePane::new(
                Some(LocalOrRemotePath::Local(path.clone())),
                None,
                None,
                ctx,
            );
            panes.add_pane_with_direction(Direction::Right, pane, true, ctx);
        });

        let (file_pane_id, file_view) = pane_group.read(&app, |panes, ctx| {
            panes
                .file_notebook_panes(ctx)
                .next()
                .expect("the file pane should exist")
        });

        // Let the read settle so the pane is fully loaded and watching.
        let loaded = file_view.update(&mut app, |view, ctx| {
            let file_id = view.file_id_for_test().expect("the file should be open");
            let future_handle = FileModel::as_ref(ctx)
                .get_future_handle(file_id)
                .expect("Loading future should be present");
            ctx.await_spawned_future(future_handle.future_id())
        });
        loaded.await;

        // Close the way the pane header's close button does, which is the path that reaches
        // `BackingView::close` before the pane group hides the pane.
        file_view.update(&mut app, BackingView::close);
        pane_group.update(&mut app, |panes, ctx| {
            assert!(
                panes.is_pane_hidden_for_close(file_pane_id),
                "closing should hide the pane for undo rather than discard it"
            );
            assert!(
                panes.restore_closed_pane(file_pane_id, ctx),
                "the closed pane should be restorable"
            );
        });

        app.read(|ctx| {
            let file_id = file_view
                .as_ref(ctx)
                .file_id_for_test()
                .expect("a restored pane should still hold its file open");
            assert!(
                FileModel::as_ref(ctx).file_path(file_id).is_some(),
                "a restored pane should still be tracked by the file model"
            );
        });

        // Permanently discarding the pane does release it.
        pane_group.update(&mut app, |panes, ctx| {
            panes.close_pane(file_pane_id, ctx);
            panes.cleanup_closed_pane(file_pane_id, ctx);
        });

        app.read(|ctx| {
            assert!(
                file_view.as_ref(ctx).file_id_for_test().is_none(),
                "a permanently discarded pane should release its file"
            );
        });
    });
}

/// A background-tab template parks its shell instead of forking it, and the
/// first `ensure_shell_started` consumes the parked start while the second is a
/// no-op. Asserted against the real `local_tty` `TerminalManager<TerminalView>`:
/// `local_tty` is unconditionally active on this platform (see `app/build.rs`),
/// so this reaches `deferred_shell`/`spawn_shell_start` for real, not a mock.
#[test]
fn test_deferred_template_pane_parks_shell_until_started() {
    // LazyShellStartup does not gate this layer: `shell_start` is injected
    // directly by the caller (this is the mechanism Task 4 wires a real flag
    // check into). No override needed to exercise it here.
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: "/".into(),
                        commands: vec![],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");

        assert!(
            app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "a deferred pane parks its shell start"
        );

        app.update(|ctx| {
            manager.update(ctx, |m, ctx| m.ensure_shell_started(ctx));
        });
        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "the first activation consumes the parked start"
        );

        // Idempotency: Option::take left None, so this must do nothing rather
        // than fork a second shell.
        app.update(|ctx| {
            manager.update(ctx, |m, ctx| m.ensure_shell_started(ctx));
        });
        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "a second activation starts nothing"
        );
    });
}

#[test]
fn test_deferred_template_pane_exposes_its_working_directory() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let cwd = std::env::current_dir().expect("the test process has a current directory");
        let displayed_cwd = cwd.to_string_lossy().into_owned();
        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: cwd.clone(),
                        commands: vec![],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let working_directory = pane_group.read(&app, |group, ctx| {
            let pane_id = group
                .terminal_pane_ids()
                .next()
                .expect("the template has one terminal pane");
            group.terminal_working_directory(pane_id, ctx)
        });

        assert_eq!(working_directory, Some(displayed_cwd));
    });
}

/// A pane carrying startup commands now honors `Deferred` like any other
/// pane; the commands stay queued and run once the pane is activated. This
/// pins the behaviour Task 1 introduced in place of the old force-eager
/// guard (see `test_deferred_pane_with_commands_parks_its_shell` for the
/// full activation flow).
#[test]
fn test_deferred_template_pane_snapshots_its_parked_working_directory() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let cwd = std::env::current_dir().expect("the test process has a current directory");
        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: cwd.clone(),
                        commands: vec![],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let snapshot = pane_group.read(&app, |group, ctx| group.snapshot(ctx));
        let PaneNodeSnapshot::Leaf(LeafSnapshot {
            contents: LeafContents::Terminal(terminal),
            ..
        }) = snapshot
        else {
            panic!("expected a terminal pane snapshot");
        };

        assert_eq!(terminal.cwd, Some(cwd.to_string_lossy().into_owned()));
    });
}

#[test]
fn test_template_pane_with_commands_honors_deferred() {
    // LazyShellStartup does not gate this layer: `shell_start` is injected
    // directly by the caller (this is the mechanism Task 4 wires a real flag
    // check into). No override needed to exercise it here.
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: "/".into(),
                        commands: vec![crate::launch_configs::launch_config::CommandTemplate {
                            exec: "echo hello".to_string(),
                        }],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");

        assert!(
            app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "a pane with pending commands now defers"
        );
    });
}

/// With the flag off, a background tab's panes still fork eagerly. Drives the
/// real template pipeline rather than re-asserting `for_launch_config_tab`,
/// which Task 1 already covers: the requirement is about what the pane does,
/// not about what the constructor returns.
#[test]
fn test_flag_off_starts_every_template_pane_eagerly() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(false);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: "/".into(),
                        commands: vec![],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    // The policy a background tab would receive. With the flag
                    // off this resolves to `Eager`.
                    shell_start: ShellStartPolicy::for_launch_config_tab(false),
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");

        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "with the flag off no pane may park its shell"
        );
    });
}

/// With the flag on, a background tab's panes defer -- the mirror of
/// `test_flag_off_starts_every_template_pane_eagerly`, and the only place in
/// this file where `LazyShellStartup` actually reaches `PanesLayout::Template`
/// through `ShellStartPolicy::for_launch_config_tab` rather than a
/// hand-injected `Deferred`. Closes the flag-to-pipeline coverage gap before
/// Task 4 wires a real background tab into this constructor: without the
/// `is_enabled()` check inside `for_launch_config_tab`, this test and
/// `test_flag_off_starts_every_template_pane_eagerly` cannot both pass.
#[test]
fn test_flag_on_defers_background_tab_pane() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: "/".into(),
                        commands: vec![],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    // The policy a background tab would receive with the flag on.
                    shell_start: ShellStartPolicy::for_launch_config_tab(false),
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");

        assert!(
            app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "with the flag on, a background tab's pane parks its shell start"
        );
    });
}

/// `start_deferred_shells` starts every pane in the group, not just the focused
/// one: a split tab renders all of its panes simultaneously, so starting one
/// would leave the others blank.
#[test]
fn test_start_deferred_shells_starts_every_pane_in_the_group() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let leaf = |focused: bool| PaneTemplateType::PaneTemplate {
            is_focused: Some(focused),
            cwd: "/".into(),
            commands: vec![],
            pane_mode: PaneMode::Terminal,
            shell: None,
        };

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneBranchTemplate {
                        split_direction:
                            crate::launch_configs::launch_config::SplitDirection::Horizontal,
                        panes: vec![leaf(true), leaf(false)],
                    },
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let deferred_count = |app: &mut App| -> usize {
            app.read(|ctx| {
                pane_group.read(ctx, |group, ctx| {
                    (0..2)
                        .filter_map(|index| group.terminal_manager(index, ctx))
                        .filter(|manager| manager.read(ctx, |m, _| m.has_deferred_shell()))
                        .count()
                })
            })
        };

        assert_eq!(deferred_count(&mut app), 2, "both panes start parked");

        app.update(|ctx| {
            pane_group.update(ctx, |group, ctx| group.start_deferred_shells(ctx));
        });

        assert_eq!(deferred_count(&mut app), 0, "both panes started");
    });
}

/// `focus_pane_on_mouse_event` -- reached here via `PaneGroupAction::Activate`,
/// the same entry point `PaneGroup::handle_action` routes mouse clicks
/// through -- starts the clicked pane's deferred shell without touching its
/// sibling. This is the only recovery path for a pane made visible outside
/// the tab-activation hook (see `adopt_transferred_pane_group`'s doc
/// comment), so pinning the asymmetry here catches the hook being silently
/// dropped, which would otherwise leave a pane permanently blank.
#[test]
fn test_activating_one_pane_via_mouse_starts_only_that_pane() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let leaf = |focused: bool| PaneTemplateType::PaneTemplate {
            is_focused: Some(focused),
            cwd: "/".into(),
            commands: vec![],
            pane_mode: PaneMode::Terminal,
            shell: None,
        };

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneBranchTemplate {
                        split_direction:
                            crate::launch_configs::launch_config::SplitDirection::Horizontal,
                        panes: vec![leaf(true), leaf(false)],
                    },
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let is_deferred = |app: &mut App, index: usize| -> bool {
            app.read(|ctx| {
                pane_group.read(ctx, |group, ctx| {
                    group
                        .terminal_manager(index, ctx)
                        .map(|manager| manager.read(ctx, |m, _| m.has_deferred_shell()))
                        .unwrap_or(false)
                })
            })
        };

        assert!(is_deferred(&mut app, 0), "pane 0 starts parked");
        assert!(is_deferred(&mut app, 1), "pane 1 starts parked");

        let pane_id_1 =
            app.read(|ctx| pane_group.read(ctx, |group, _ctx| group.pane_id_by_index(1).unwrap()));

        app.update(|ctx| {
            pane_group.update(ctx, |group, ctx| {
                group.handle_action(
                    &PaneGroupAction::Activate(pane_id_1, ActivationReason::Click),
                    ctx,
                );
            });
        });

        assert!(!is_deferred(&mut app, 1), "activated pane started");
        assert!(is_deferred(&mut app, 0), "sibling pane stayed parked");
    });
}

/// A pane carrying startup commands now defers like any other. Its commands
/// stay queued and run when the pane is activated, which is the answer the
/// original design lacked when it forced such panes eager.
#[test]
fn test_deferred_pane_with_commands_parks_its_shell() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: "/".into(),
                        commands: vec![crate::launch_configs::launch_config::CommandTemplate {
                            exec: "echo hello".to_string(),
                        }],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");

        assert!(
            app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "a pane with commands must now defer instead of forcing eager"
        );

        let view = app
            .read(|ctx| {
                pane_group.read(ctx, |group, ctx| group.terminal_view_at_pane_index(0, ctx))
            })
            .expect("pane 0 has a terminal view");
        assert!(
            app.read(|ctx| view
                .as_ref(ctx)
                .has_pending_command_or_awaiting_completion(ctx)),
            "parking the shell must not discard the pane's queued commands"
        );

        // Activating starts the parked shell, and the queued commands are still
        // there waiting for it. They only *run* once the shell bootstraps:
        // `Input::execute_pending_command` refuses to submit anything until
        // `block_list().is_bootstrapped()`, which needs a real PTY handshake
        // this harness never performs. So the drain itself is not asserted
        // here -- only that activation neither drops the queue nor consumes it
        // early.
        app.update(|ctx| {
            pane_group.update(ctx, |group, ctx| group.start_deferred_shells(ctx));
        });
        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "activation starts the parked shell"
        );
        assert!(
            app.read(|ctx| view
                .as_ref(ctx)
                .has_pending_command_or_awaiting_completion(ctx)),
            "the queued commands must still be pending for the freshly started shell"
        );
    });
}

/// A pane with commands under `Eager` is unchanged — this is the case that
/// must not regress, since it is what every existing config does today.
#[test]
fn test_eager_pane_with_commands_still_starts_immediately() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: "/".into(),
                        commands: vec![crate::launch_configs::launch_config::CommandTemplate {
                            exec: "echo hello".to_string(),
                        }],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    shell_start: ShellStartPolicy::Eager,
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");

        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "an eager pane with commands starts immediately"
        );
    });
}

/// A restored pane defers when its policy says to. Restored panes carry
/// scrollback that renders without a PTY, so a parked restored pane shows its
/// history rather than being blank.
#[test]
fn test_restored_pane_defers_when_policy_is_deferred() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Snapshot {
                    snapshot: Box::new(single_terminal_leaf_snapshot()),
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("restored pane 0 has a terminal manager");

        assert!(
            app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "a restored pane must defer when the policy says Deferred"
        );

        app.update(|ctx| {
            pane_group.update(ctx, |group, ctx| group.start_deferred_shells(ctx));
        });
        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "activation starts the restored pane's shell"
        );
    });
}

/// Session restore under `Eager` is unchanged. This is the regression guard for
/// every existing restore path.
#[test]
fn test_restored_pane_starts_eagerly_under_eager_policy() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Snapshot {
                    snapshot: Box::new(single_terminal_leaf_snapshot()),
                    shell_start: ShellStartPolicy::Eager,
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("restored pane 0 has a terminal manager");

        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "an eagerly-restored pane starts its shell immediately"
        );
    });
}

/// A restored pane that is never activated must still re-save the `cwd` and
/// `shell_launch_data` it was restored with.
///
/// A parked pane has no PTY, so `TerminalView::active_block_metadata` -- which
/// both `pwd_if_local` and `shell_launch_data_if_local` bottom out in -- is
/// permanently `None`. Without a fallback the next save writes NULL for both,
/// and the tab silently comes back in the home directory under the default
/// shell on the following launch.
#[test]
fn test_parked_restored_pane_round_trips_cwd_and_shell_launch_data() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        // Restore has to accept the directory (it drops paths that no longer
        // exist), so use one that is guaranteed to be there.
        let cwd = std::env::current_dir()
            .expect("the test process has a current directory")
            .to_string_lossy()
            .into_owned();
        let shell_launch_data = ShellLaunchData::Executable {
            executable_path: PathBuf::from("/usr/bin/fish"),
            shell_type: ShellType::Fish,
        };

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Snapshot {
                    snapshot: Box::new(terminal_leaf_snapshot(
                        Some(cwd.clone()),
                        Some(shell_launch_data.clone()),
                    )),
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("restored pane 0 has a terminal manager");
        assert!(
            app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "the restored pane must be parked for this test to mean anything"
        );

        let saved = pane_group.update(&mut app, |panes, ctx| panes.snapshot(ctx));
        let PaneNodeSnapshot::Leaf(leaf) = saved else {
            panic!("expected the restored tab to snapshot as a single leaf");
        };
        let LeafContents::Terminal(terminal) = leaf.contents else {
            panic!("expected the restored leaf to snapshot as a terminal pane");
        };

        assert_eq!(
            terminal.cwd,
            Some(cwd),
            "a parked pane must re-save the cwd it was restored with"
        );
        assert_eq!(
            terminal.shell_launch_data,
            Some(shell_launch_data),
            "a parked pane must re-save the shell it was restored with"
        );
    });
}

/// A restored pane whose shell has started but not bootstrapped -- or died before it did -- has
/// reported no directory yet. It must re-save the one it was restored with: saving none brings
/// the pane back in the app's own working directory, which then gets saved for good.
#[test]
fn test_started_unbootstrapped_restored_pane_round_trips_cwd() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let cwd = std::env::current_dir()
            .expect("the test process has a current directory")
            .to_string_lossy()
            .into_owned();
        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Snapshot {
                    snapshot: Box::new(terminal_leaf_snapshot(Some(cwd.clone()), None)),
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );
        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("restored pane 0 has a terminal manager");
        app.update(|ctx| {
            pane_group.update(ctx, |group, ctx| group.start_deferred_shells(ctx));
        });
        assert!(
            !app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "the restored pane's shell must have started for this test to mean anything"
        );

        let saved = pane_group.update(&mut app, |panes, ctx| panes.snapshot(ctx));
        let PaneNodeSnapshot::Leaf(leaf) = saved else {
            panic!("expected the restored tab to snapshot as a single leaf");
        };
        let LeafContents::Terminal(terminal) = leaf.contents else {
            panic!("expected the restored leaf to snapshot as a terminal pane");
        };
        assert_eq!(
            terminal.cwd,
            Some(cwd),
            "a pane whose shell has not reported a directory must re-save the one it was              restored with"
        );
    });
}

/// A restored agent pane whose shell is still parked has not started its agent, so no live agent
/// session exists for it yet. It must re-save the agent it was restored with, or the next save
/// drops the agent and the session stops resuming it.
#[test]
fn test_parked_restored_pane_round_trips_its_agent() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let cwd = std::env::current_dir()
            .expect("the test process has a current directory")
            .to_string_lossy()
            .into_owned();
        let descriptor = crate::app_state::CliAgentResumeDescriptor {
            agent: crate::terminal::CLIAgent::Claude,
            session_id: Some("restored-session".to_owned()),
            cwd: PathBuf::from(&cwd),
            title: None,
            captured_unix_seconds: Some(1),
        };
        let mut snapshot = terminal_leaf_snapshot(Some(cwd), None);
        if let PaneNodeSnapshot::Leaf(leaf) = &mut snapshot
            && let LeafContents::Terminal(terminal) = &mut leaf.contents
        {
            terminal.cli_agent_resume_descriptor = Some(descriptor.clone());
        }
        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Snapshot {
                    snapshot: Box::new(snapshot),
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );
        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("restored pane 0 has a terminal manager");
        assert!(
            app.read(|ctx| manager.read(ctx, |m, _| m.has_deferred_shell())),
            "the restored agent pane must be parked for this test to mean anything"
        );

        let saved = pane_group.update(&mut app, |panes, ctx| panes.snapshot(ctx));
        let PaneNodeSnapshot::Leaf(leaf) = saved else {
            panic!("expected the restored tab to snapshot as a single leaf");
        };
        let LeafContents::Terminal(terminal) = leaf.contents else {
            panic!("expected the restored leaf to snapshot as a terminal pane");
        };
        assert_eq!(
            terminal.cli_agent_resume_descriptor,
            Some(descriptor),
            "a parked pane must re-save the agent it was restored with"
        );
    });
}

/// The resume command a pane is restored with must reach its terminal manager, which runs it once
/// the shell starts. A manager that drops it restores a plain shell and the agent never resumes.
#[test]
fn test_restored_pane_hands_its_resume_command_to_the_terminal_manager() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let cwd = std::env::current_dir()
            .expect("the test process has a current directory")
            .to_string_lossy()
            .into_owned();
        let mut snapshot = terminal_leaf_snapshot(Some(cwd.clone()), None);
        if let PaneNodeSnapshot::Leaf(leaf) = &mut snapshot
            && let LeafContents::Terminal(terminal) = &mut leaf.contents
        {
            terminal.cli_agent_resume_descriptor =
                Some(crate::app_state::CliAgentResumeDescriptor {
                    agent: crate::terminal::CLIAgent::Claude,
                    session_id: Some("restored-session".to_owned()),
                    cwd: PathBuf::from(&cwd),
                    title: None,
                    captured_unix_seconds: Some(1),
                });
        }
        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Snapshot {
                    snapshot: Box::new(snapshot),
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );
        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("restored pane 0 has a terminal manager");

        let resume_command = app.read(|ctx| {
            manager.read(ctx, |manager, _| {
                manager
                    .as_any()
                    .downcast_ref::<TerminalManager<crate::terminal::TerminalView>>()
                    .expect("a restored pane runs on the GUI terminal manager")
                    .cli_agent_resume_command()
                    .map(str::to_owned)
            })
        });
        assert_eq!(
            resume_command.as_deref(),
            Some("claude --resume restored-session"),
            "the restored pane's resume command must wait in its terminal manager"
        );
    });
}

/// A restored agent pane whose tab nobody has opened has no live agent session, so anything
/// listing agents must ask for the restored one or report the pane as having none.
#[test]
fn test_parked_restored_pane_reports_the_agent_it_has_not_resumed() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let cwd = std::env::current_dir()
            .expect("the test process has a current directory")
            .to_string_lossy()
            .into_owned();
        let descriptor = crate::app_state::CliAgentResumeDescriptor {
            agent: crate::terminal::CLIAgent::Claude,
            session_id: Some("restored-session".to_owned()),
            cwd: PathBuf::from(&cwd),
            title: None,
            captured_unix_seconds: Some(1),
        };
        let mut snapshot = terminal_leaf_snapshot(Some(cwd), None);
        if let PaneNodeSnapshot::Leaf(leaf) = &mut snapshot
            && let LeafContents::Terminal(terminal) = &mut leaf.contents
        {
            terminal.cli_agent_resume_descriptor = Some(descriptor.clone());
        }
        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Snapshot {
                    snapshot: Box::new(snapshot),
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let restored = app.read(|ctx| {
            pane_group.read(ctx, |group, ctx| {
                let pane_id = group
                    .pane_id_from_index(0)
                    .expect("the restored tab has a pane");
                group.pending_agent_resume(pane_id, ctx)
            })
        });
        assert_eq!(restored, Some(descriptor));
    });
}

#[test]
fn test_parked_restored_mixed_shells_round_trip_per_pane() {
    let _lazy_shell_startup = FeatureFlag::LazyShellStartup.override_enabled(true);
    let _persistent_sessions = FeatureFlag::PersistentLaunchConfigSessions.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let cwd = std::env::current_dir()
            .expect("the test process has a current directory")
            .to_string_lossy()
            .into_owned();
        let git_bash = ShellLaunchData::Executable {
            executable_path: PathBuf::from("C:/Program Files/Git/bin/bash.exe"),
            shell_type: ShellType::Bash,
        };
        let powershell = ShellLaunchData::Executable {
            executable_path: PathBuf::from("C:/Program Files/PowerShell/7/pwsh.exe"),
            shell_type: ShellType::PowerShell,
        };
        let snapshot = PaneNodeSnapshot::Branch(BranchSnapshot {
            direction: SplitDirection::Horizontal,
            children: vec![
                (
                    PaneFlex(1.0),
                    terminal_leaf_snapshot(Some(cwd.clone()), Some(git_bash.clone())),
                ),
                (
                    PaneFlex(1.0),
                    terminal_leaf_snapshot(Some(cwd), Some(powershell.clone())),
                ),
            ],
        });
        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Snapshot {
                    snapshot: Box::new(snapshot),
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let saved = pane_group.update(&mut app, |panes, ctx| panes.snapshot(ctx));
        let PaneNodeSnapshot::Branch(branch) = saved else {
            panic!("expected the restored tab to remain split");
        };
        let [(_, git_bash_leaf), (_, powershell_leaf)] = branch.children.as_slice() else {
            panic!("expected exactly two restored panes");
        };
        let PaneNodeSnapshot::Leaf(git_bash_leaf) = git_bash_leaf else {
            panic!("expected the first pane to be a leaf");
        };
        let LeafContents::Terminal(git_bash_terminal) = &git_bash_leaf.contents else {
            panic!("expected the first pane to be a terminal");
        };
        let PaneNodeSnapshot::Leaf(powershell_leaf) = powershell_leaf else {
            panic!("expected the second pane to be a leaf");
        };
        let LeafContents::Terminal(powershell_terminal) = &powershell_leaf.contents else {
            panic!("expected the second pane to be a terminal");
        };

        assert_eq!(git_bash_terminal.shell_launch_data, Some(git_bash));
        assert_eq!(powershell_terminal.shell_launch_data, Some(powershell));
    });
}

#[test]
fn test_parked_restored_pane_exposes_its_working_directory() {
    let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let cwd = std::env::current_dir()
            .expect("the test process has a current directory")
            .to_string_lossy()
            .into_owned();
        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Snapshot {
                    snapshot: Box::new(terminal_leaf_snapshot(Some(cwd.clone()), None)),
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let displayed_cwd = pane_group.read(&app, |group, ctx| {
            let pane_id = group
                .terminal_pane_ids()
                .next()
                .expect("the restored layout has one terminal pane");
            group.terminal_working_directory(pane_id, ctx)
        });

        assert_eq!(displayed_cwd, Some(cwd));
    });
}

/// Restarting a pane's session keeps the pane and everything the user can see
/// in it -- and leaves it able to report a *real* shell death afterwards.
///
/// The hazard is an ordering one. `ModelEvent::Exit` reaches the view
/// asynchronously, so the shell being replaced can still have its own exit
/// queued when the restart runs. That is why this test queues one explicitly
/// rather than trusting a teardown to produce one at a convenient moment. An
/// exit that lands unsuppressed is treated as a shell that died: the "process
/// terminated" banner, an initialization block for a session that never
/// bootstrapped, and `Event::Exited` -- which `TerminalPane` turns into
/// `close_pane`.
///
/// The pane is marked bootstrapped first, because that is the only state in
/// which an unsuppressed exit actually reaches `close_pane`, and it is the
/// state any pane a user would think to restart is in.
///
/// Both halves of the guarantee are asserted, because each catches a different
/// way of getting this wrong:
///
/// - *during* the restart, the queued exit must not reach `Event::Exited`. A
///   restart that never marks the pane fails here.
/// - *after* it, a genuine exit must reach `Event::Exited` again. A restart
///   whose barrier never comes back round -- so the mark is set forever --
///   passes the first half and fails this one. Without it, deleting the
///   `finish_restart` call from the exit handler leaves every assertion
///   holding, while in production the pane silently swallows every later shell
///   death and can never be closed.
#[test]
fn test_restart_session_keeps_the_pane_and_rearms_its_exit_handling() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: "/".into(),
                        commands: vec![],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");
        let view = app
            .read(|ctx| {
                pane_group.read(ctx, |group, ctx| group.terminal_view_at_pane_index(0, ctx))
            })
            .expect("pane 0 has a terminal view");

        // `Event::Exited` is the only thing that closes a pane on shell death,
        // and it is emitted by exactly one place in the crate. Counting it is
        // the most direct statement of what a restart must and must not do.
        let exited_count = Rc::new(RefCell::new(0usize));
        let exited_count_handle = exited_count.clone();
        app.update(|ctx| {
            ctx.subscribe_to_view(&view, move |_, event, _| {
                if let TerminalViewEvent::Exited = event {
                    *exited_count_handle.borrow_mut() += 1;
                }
            });
        });

        // Give the pane a conversation, so "the restart preserved the
        // conversation" is a claim about something rather than `None == None`.
        let conversation_id = app.update(|ctx| {
            let terminal_surface_id = view.id();
            BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                history.start_new_conversation(terminal_surface_id, false, false, false, ctx)
            })
        });

        // The replaced shell had come up. This is the case that matters: an
        // unsuppressed exit only reaches `close_pane` for a pane that
        // bootstrapped (`terminal_pane.rs` keeps an unbootstrapped one open so
        // the user can read what went wrong), and it is the state any pane the
        // user would think to restart is actually in.
        app.update(|ctx| {
            view.update(ctx, |view, _| view.set_login_shell_bootstrapped_for_test());
        });

        let panes_before =
            app.read(|ctx| pane_group.read(ctx, |group, _| group.pane_ids().count()));
        let block_ids_before = app.read(|ctx| {
            view.as_ref(ctx)
                .model
                .lock()
                .block_list()
                .blocks()
                .iter()
                .map(|block| block.id().clone())
                .collect::<Vec<_>>()
        });
        let active_conversation_before =
            app.read(|ctx| view.as_ref(ctx).active_conversation_id(ctx));

        // The shell being replaced reports its own death, exactly as the PTY
        // event loop does on the way out. This queues an `Exit` on the async
        // channel that the restart has to order itself against.
        app.update(|ctx| {
            manager
                .read(ctx, |manager, _| manager.model())
                .lock()
                .exit(ExitReason::ShellProcessExited);
        });

        app.update(|ctx| {
            manager.update(ctx, |manager, ctx| {
                assert!(
                    manager.restart_session(None, None, ctx),
                    "a local-tty manager must report that it restarted"
                );
            });
        });

        // `App::test`'s executor is cooperative: the dispatcher that turns a
        // queued terminal event into a `ModelEvent` for this view is a spawned
        // task, and it makes no progress at all unless the test yields. Without
        // this wait the queued exit above would simply never be delivered and
        // the assertions below would hold vacuously.
        //
        // The condition is the restart barrier coming back round. That is the
        // strongest available proof the exit was already delivered *and*
        // suppressed: the barrier travels the same FIFO channel behind the
        // exit, so observing it means the exit is behind us. Waiting on it,
        // rather than on a yield budget, is what keeps the assertions below
        // from passing vacuously when a loaded machine hands this test fewer
        // scheduler turns than usual.
        assert_eventually_within!(
            Duration::from_secs(10) => app.read(|ctx| {
                !view.as_ref(ctx).is_restart_in_progress()
                    && manager.read(ctx, |manager, _| {
                        manager
                            .as_any()
                            .downcast_ref::<LocalTtyTerminalManager<TerminalView>>()
                            .expect("panes in this harness use the local-tty manager")
                            .has_running_event_loop()
                    })
            }),
            "the restart barrier never came back round with a live replacement event loop \
             within 10s; the restart did not complete"
        );

        assert_eq!(
            *exited_count.borrow(),
            0,
            "the replaced shell's exit must not reach `Event::Exited`, which closes the pane"
        );
        assert_eq!(
            app.read(|ctx| pane_group.read(ctx, |group, _| group.pane_ids().count())),
            panes_before,
            "a restart must not close the pane it is restarting"
        );
        let block_ids_after = app.read(|ctx| {
            view.as_ref(ctx)
                .model
                .lock()
                .block_list()
                .blocks()
                .iter()
                .map(|block| block.id().clone())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            &block_ids_after[..block_ids_before.len()],
            block_ids_before.as_slice(),
            "a restart must preserve every existing scrollback block"
        );
        assert_eq!(
            app.read(|ctx| view.as_ref(ctx).active_conversation_id(ctx)),
            active_conversation_before,
            "a restart must not change which conversation the pane is on"
        );
        assert!(
            app.read(|ctx| {
                BlocklistAIHistoryModel::as_ref(ctx)
                    .conversation(&conversation_id)
                    .is_some_and(|conversation| conversation.status().is_in_progress())
            }),
            "a restart must not finalize the pane's in-flight conversation as failed"
        );
        assert!(
            !app.read(|ctx| view.as_ref(ctx).is_restart_in_progress()),
            "the restart barrier must have come back round and cleared the mark"
        );
        assert!(
            app.read(|ctx| manager.read(ctx, |manager, _| {
                manager
                    .as_any()
                    .downcast_ref::<LocalTtyTerminalManager<TerminalView>>()
                    .expect("panes in this harness use the local-tty manager")
                    .has_running_event_loop()
            })),
            "the restart must have forked a replacement session, not just torn the old one down"
        );

        // Now the replacement comes up and the user exits it for real. The pane
        // must behave exactly like one that was never restarted.
        app.update(|ctx| {
            view.update(ctx, |view, _| view.set_login_shell_bootstrapped_for_test());
            manager
                .read(ctx, |manager, _| manager.model())
                .lock()
                .exit(ExitReason::ShellProcessExited);
        });
        // Waiting for the exit to be *delivered*, not for a yield budget to run
        // out. On timeout this fails with the same complaint the assertion
        // below makes, so a swallowed exit is still reported as a swallowed
        // exit rather than as a timeout of unclear cause.
        assert_eventually_within!(
            Duration::from_secs(10) => *exited_count.borrow() == 1,
            "a genuine shell death after the restart must still reach `Event::Exited`; a mark \
             that is never cleared silently swallows every later exit (waited 10s, saw {} of \
             them)",
            *exited_count.borrow()
        );

        assert_eq!(
            *exited_count.borrow(),
            1,
            "a genuine shell death after the restart must still reach `Event::Exited`; a mark \
             that is never cleared silently swallows every later exit"
        );
    });
}
/// Two restarts in a row establish exactly one session, not two.
///
/// `restart_session_inner` is a single synchronous main-thread body with no
/// `await`, and a spawned start's callback is dispatched only through the app's
/// task callbacks, which run when the test yields. So two calls inside one
/// `app.update` closure reproduce the superseded-start interleaving exactly,
/// with no timing dependence: both starts are outstanding, and neither callback
/// has run.
///
/// What that interleaving used to do: the first start's `on_shell_determined`
/// installs its event loop handle, and the second *overwrites* it. The first
/// thread is then dropped un-joined -- a detached PTY reader and an orphaned
/// shell process for the app's lifetime, with two event loops writing into one
/// blocklist. None of that surfaces in production: no log line, no error, just
/// stray shells. `established_session_count` exists so it surfaces here.
///
/// The blocking sleep is load-bearing rather than a timing hack. It lets the
/// first start's *background* future resolve while the foreground executor
/// stays parked, so its callback is guaranteed to be delivered rather than
/// aborted -- which is the case the generation counter has to handle, and the
/// reason a bare "is a start pending?" flag with an `abort()` would not do.
/// `ModelContext::spawn` honours an abort only at the future's next poll, so
/// once that future has resolved the abort is a no-op and the guard is the only
/// thing left. Under-sleeping can only make the abort win, which would make
/// this test pass for a weaker reason, never fail spuriously.
#[test]
fn test_two_restarts_in_flight_establish_only_one_session() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: "/".into(),
                        commands: vec![],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");
        let view = app
            .read(|ctx| {
                pane_group.read(ctx, |group, ctx| group.terminal_view_at_pane_index(0, ctx))
            })
            .expect("pane 0 has a terminal view");

        let exited_count = Rc::new(RefCell::new(0usize));
        let exited_count_handle = exited_count.clone();
        app.update(|ctx| {
            ctx.subscribe_to_view(&view, move |_, event, _| {
                if let TerminalViewEvent::Exited = event {
                    *exited_count_handle.borrow_mut() += 1;
                }
            });
        });

        let panes_before =
            app.read(|ctx| pane_group.read(ctx, |group, _| group.pane_ids().count()));

        app.update(|ctx| {
            manager.update(ctx, |manager, ctx| {
                assert!(
                    manager.restart_session(None, None, ctx),
                    "a local-tty manager must report that it restarted"
                );
            });
        });

        // Let the first start's background future resolve while the foreground
        // executor stays parked: `sleep` blocks this thread, and spawn
        // callbacks only run when the test awaits. The abort the second restart
        // issues is therefore a no-op, and the generation guard is on its own.
        std::thread::sleep(Duration::from_millis(250));

        app.update(|ctx| {
            manager.update(ctx, |manager, ctx| {
                assert!(
                    manager.restart_session(None, None, ctx),
                    "a local-tty manager must report that it restarted"
                );
            });
        });

        // The surviving restart's barrier clearing the mark, with a live event
        // loop behind it, is what says both restarts have run all the way
        // through the view. A yield budget only says "some scheduler turns
        // happened".
        assert_eventually_within!(
            Duration::from_secs(10) => app.read(|ctx| {
                !view.as_ref(ctx).is_restart_in_progress()
                    && manager.read(ctx, |manager, _| {
                        manager
                            .as_any()
                            .downcast_ref::<LocalTtyTerminalManager<TerminalView>>()
                            .expect("panes in this harness use the local-tty manager")
                            .has_running_event_loop()
                    })
            }),
            "neither restart's barrier came back round with a live event loop within 10s"
        );

        assert_eq!(
            app.read(|ctx| manager.read(ctx, |manager, _| {
                manager
                    .as_any()
                    .downcast_ref::<LocalTtyTerminalManager<TerminalView>>()
                    .expect("panes in this harness use the local-tty manager")
                    .established_session_count()
            })),
            1,
            "a superseded shell start must be discarded, not allowed to install a second event \
             loop over the live one -- that detaches the first thread and orphans its shell"
        );
        assert!(
            app.read(|ctx| manager.read(ctx, |manager, _| {
                manager
                    .as_any()
                    .downcast_ref::<LocalTtyTerminalManager<TerminalView>>()
                    .expect("panes in this harness use the local-tty manager")
                    .has_running_event_loop()
            })),
            "the surviving session must be the last restart's, not nothing at all"
        );

        // Two restarts also put two barriers in the channel at once. The second
        // arrives after the first has already cleared the mark, and must still
        // not be mistaken for a shell that died.
        assert_eq!(
            *exited_count.borrow(),
            0,
            "neither restart's barrier may reach `Event::Exited`, which closes the pane"
        );
        assert_eq!(
            app.read(|ctx| pane_group.read(ctx, |group, _| group.pane_ids().count())),
            panes_before,
            "back-to-back restarts must not close the pane"
        );
        assert!(
            !app.read(|ctx| view.as_ref(ctx).is_restart_in_progress()),
            "the last restart's barrier must have come back round and cleared the mark"
        );
    });
}

/// `native_startup_directory` translates a host-form directory into the
/// syntax the target shell expects: `Executable` is the identity, `WSL`
/// yields `/mnt/<drive>/...`, and `MSYS2` yields the MSYS2 form rooted at
/// the shell's own installation.
///
/// Table-driven across the three shell kinds so a regression in any one
/// conversion shows up as a named failure rather than a single opaque
/// assertion.
#[test]
fn test_native_startup_directory_converts_per_shell_kind() {
    struct Case {
        name: &'static str,
        shell: ShellLaunchData,
        expected: Option<PathBuf>,
    }

    let cwd = Path::new(r"C:\projects\warp");

    let cases = [
        Case {
            name: "Executable is the identity",
            shell: ShellLaunchData::Executable {
                executable_path: PathBuf::from("/usr/bin/fish"),
                shell_type: ShellType::Fish,
            },
            expected: Some(PathBuf::from(r"C:\projects\warp")),
        },
        Case {
            name: "WSL yields /mnt/<drive>/...",
            shell: ShellLaunchData::WSL {
                distro: "Ubuntu".to_owned(),
            },
            expected: Some(PathBuf::from("/mnt/c/projects/warp")),
        },
        Case {
            name: "MSYS2 yields the MSYS2 form rooted at the shell's install",
            shell: ShellLaunchData::MSYS2 {
                executable_path: PathBuf::from(r"C:\msys64\usr\bin\bash.exe"),
                shell_type: ShellType::Bash,
            },
            expected: Some(PathBuf::from("/c/projects/warp")),
        },
    ];

    for case in cases {
        assert_eq!(
            native_startup_directory(cwd, &case.shell),
            case.expected,
            "case: {}",
            case.name
        );
    }
}

/// A restart's requested directory must survive even when the user has not
/// customized their shell setting -- `Config::SystemDefault`, the
/// out-of-the-box configuration for any user who has never touched "Startup
/// Shell" (`AvailableShell::default()`, and what
/// `get_user_preferred_shell_setting_fallback` returns for `StartupShell::
/// Default`).
///
/// This is a regression test for a bug where the directory conversion was
/// gated on `AvailableShell::get_valid_shell_path_and_type()`, which is
/// `None` for exactly `Config::SystemDefault` -- so for the default
/// configuration the requested directory collapsed to `None` *before*
/// `ShellStarter::init` ever resolved which shell was actually about to
/// launch, regardless of what that shell turned out to be. The fix moved
/// the conversion to `on_shell_determined`, where the real, resolved
/// `ShellLaunchData` is available.
#[test]
fn test_restart_with_system_default_shell_still_lands_in_the_requested_directory() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template {
                    template: PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: "/".into(),
                        commands: vec![],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    shell_start: ShellStartPolicy::Deferred,
                },
                ..Default::default()
            },
        );

        let manager = app
            .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
            .expect("pane 0 has a terminal manager");

        // No shell has run yet (the pane's shell start is deferred), and
        // nothing in this harness overrides the shell setting, so
        // `AvailableShells::get_user_preferred_shell` resolves to
        // `Config::SystemDefault` when the restart asks it to pick a shell.
        let requested_dir =
            std::env::current_dir().expect("the test process has a current directory");

        app.update(|ctx| {
            manager.update(ctx, |manager, ctx| {
                assert!(
                    manager.restart_session(None, Some(requested_dir.clone()), ctx),
                    "a local-tty manager must report that it restarted"
                );
            });
        });

        // The shell starter resolves through a spawned task, which only makes
        // progress when this test yields -- and it forks a real shell, so how
        // long that takes is a property of the machine, not of a yield count.
        // Wait for the directory to actually be resolved.
        assert_eventually_within!(
            Duration::from_secs(10) => app.read(|ctx| {
                manager.read(ctx, |manager, _| {
                    manager
                        .as_any()
                        .downcast_ref::<LocalTtyTerminalManager<TerminalView>>()
                        .expect("panes in this harness use the local-tty manager")
                        .last_resolved_startup_directory_for_test()
                        .is_some()
                })
            }),
            "the restarted shell never reached `create_pty` within 10s, so there is no \
             resolved startup directory to assert on"
        );

        let resolved_dir = app.read(|ctx| {
            manager.read(ctx, |manager, _| {
                manager
                    .as_any()
                    .downcast_ref::<LocalTtyTerminalManager<TerminalView>>()
                    .expect("panes in this harness use the local-tty manager")
                    .last_resolved_startup_directory_for_test()
            })
        });

        assert_eq!(
            resolved_dir,
            Some(requested_dir),
            "a restart under the default (unresolved-until-launch) shell setting must still \
             hand the actually-launched shell the requested directory, not silently drop it"
        );
    });
}

/// Builds a deferred-shell single-pane group and returns its manager and view.
///
/// Every restart test below wants exactly this and nothing else; spelling it
/// out four more times is four more places for the harness to drift.
fn restartable_pane(
    app: &mut App,
) -> (
    warpui::ViewHandle<PaneGroup>,
    ModelHandle<Box<dyn crate::terminal::TerminalManager>>,
    warpui::ViewHandle<TerminalView>,
) {
    let pane_group = mock_pane_group(
        app,
        MockOptions {
            layout: PanesLayout::Template {
                template: PaneTemplateType::PaneTemplate {
                    is_focused: Some(true),
                    cwd: "/".into(),
                    commands: vec![],
                    pane_mode: PaneMode::Terminal,
                    shell: None,
                },
                shell_start: ShellStartPolicy::Deferred,
            },
            ..Default::default()
        },
    );
    let manager = app
        .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(0, ctx)))
        .expect("pane 0 has a terminal manager");
    let view = app
        .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_view_at_pane_index(0, ctx)))
        .expect("pane 0 has a terminal view");
    (pane_group, manager, view)
}

/// The replaced shell's post-mortem must not outlive it.
///
/// Four pieces of state are written after a bootstrapped shell dies and never cleared by
/// anything else: the pinned "shell exited" banner, the forced-on initialization block,
/// `pty_spawn_failed`, and `is_login_shell_bootstrapped`. A user whose shell config broke fixes it,
/// restarts, and must get a clean pane, not a healthy prompt sitting under a banner describing a
/// process that no longer exists. The unbootstrapped rich-content banner path is covered by the
/// terminal-view unit tests.
///
/// Asserted on the state directly, one assertion per assignment, so deleting
/// any single reset in `on_session_restart_started` fails exactly one
/// assertion and names which. The two behavioural consequences are covered
/// separately by the two tests below, because a state assertion alone cannot
/// show that the stale value is what actually breaks the pane.
#[test]
fn test_restart_clears_the_replaced_shells_post_mortem_state() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let (_pane_group, manager, view) = restartable_pane(&mut app);

        // The banner is only pinned for a shell that had come up; an unbootstrapped death uses
        // the separately tested rich-content banner path.
        app.update(|ctx| {
            view.update(ctx, |view, ctx| {
                view.set_login_shell_bootstrapped_for_test();
                view.on_pty_spawn_failed(anyhow::Error::msg("mock spawn failure"), ctx);
            });
            view.update(ctx, |view, _| {
                view.model
                    .lock()
                    .block_list_mut()
                    .set_show_bootstrap_block(true);
            });
        });

        // Preconditions: without these the test could pass against a pane that
        // never had a post-mortem to clear.
        app.read(|ctx| {
            let view = view.as_ref(ctx);
            assert!(
                view.has_shell_process_terminated_banner_for_test(),
                "precondition: the replaced shell must have pinned its terminated banner"
            );
            assert_eq!(
                view.model.lock().block_list().inline_banner_count(),
                1,
                "precondition: the pinned banner must also hold a blocklist entry, which is \n                 what reserves its height"
            );
            assert!(
                view.pty_spawn_failed_for_test(),
                "precondition: the replaced shell must have recorded a spawn failure"
            );
            assert!(
                view.is_login_shell_bootstrapped_for_test(),
                "precondition: the replaced shell must be marked bootstrapped"
            );
            assert!(
                view.model.lock().block_list().show_bootstrap_block(),
                "precondition: the initialization block must be showing"
            );
        });

        app.update(|ctx| {
            manager.update(ctx, |manager, ctx| {
                assert!(
                    manager.restart_session(None, None, ctx),
                    "a local-tty manager must report that it restarted"
                );
            });
        });

        app.read(|ctx| {
            let view = view.as_ref(ctx);
            assert!(
                !view.has_shell_process_terminated_banner_for_test(),
                "the replaced shell's terminated banner must not stay pinned above the \
                 replacement's prompt"
            );
            // The half that is easy to forget. Only the state drives
            // rendering, so dropping it alone leaves the blocklist entry
            // behind as a visible-height item with nothing to draw: a blank
            // 2.5-line band above the new prompt, and "Missing banner element
            // for ID" logged on every layout pass.
            assert_eq!(
                view.model.lock().block_list().inline_banner_count(),
                0,
                "clearing the banner's state without removing its blocklist entry leaves a \n                 blank band where the banner used to be"
            );
            assert!(
                !view.model.lock().block_list().show_bootstrap_block(),
                "the initialization block forced on by the replaced shell's failure must not \
                 stay forced on across a restart"
            );
            assert!(
                !view.pty_spawn_failed_for_test(),
                "a restart is the recovery from a failed spawn; leaving the flag set makes the \
                 recovered pane unable to ever report a later shell death"
            );
            assert!(
                !view.is_login_shell_bootstrapped_for_test(),
                "the replacement has not bootstrapped yet, so the replaced shell's verdict \
                 about itself must not carry over"
            );
        });
    });
}

/// The guard runs when the attach fails and a holder is recorded. With no holder answering, the
/// verdict is `Unknown` and the pane restores as it always has: the resume command is queued.
#[test]
fn test_an_unreachable_holder_does_not_withhold_the_resume_command() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let (_pane_group, manager, view) = restartable_pane(&mut app);
        app.update(|ctx| {
            manager.update(ctx, |manager, ctx| {
                manager.restore_persistent_session(warp_core::SessionId::from(4242u64), ctx);
                manager.restore_cli_agent_resume_command("claude --resume abc".to_owned());
                manager.restore_session_holder(
                    SessionHolder {
                        channel: format!(
                            r"\\.\pipe\warp-terminal-supervisor-test-missing-{}",
                            std::process::id()
                        ),
                        version: "test".to_owned(),
                        pid: 1,
                    },
                    None,
                );
                manager.ensure_shell_started(ctx);
            });
        });
        // The guard runs where the shell start lands, after its starter resolves off the main
        // thread.
        assert_eventually_within!(
            Duration::from_secs(10) => app.read(|ctx| manager.read(ctx, |manager, _| {
                manager
                    .as_any()
                    .downcast_ref::<LocalTtyTerminalManager<TerminalView>>()
                    .expect("panes in this harness use the local-tty manager")
                    .established_session_count()
                    == 1
            })),
            "the restored pane's shell never started"
        );
        app.read(|ctx| {
            let view = view.as_ref(ctx);
            assert!(!view.has_session_holder_banner_for_test());
            assert_eq!(
                view.pending_command_for_test(ctx).as_deref(),
                Some("claude --resume abc")
            );
        });
    });
}

#[test]
fn test_run_withheld_restore_queues_the_command_and_clears_the_banner() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let (_pane_group, manager, view) = restartable_pane(&mut app);
        let restore = WithheldRestore {
            command: "claude --resume abc".to_owned(),
            holder: SessionHolder {
                channel: "x".into(),
                version: "test".into(),
                pid: 1,
            },
            session_id: warp_core::SessionId::from(4242u64),
            agent: None,
            verdict: HeldVerdict::Detached,
        };
        app.update(|ctx| {
            view.update(ctx, |view, ctx| {
                view.set_login_shell_bootstrapped_for_test();
                view.withhold_restore_command(restore.clone(), ctx);
            });
            manager.update(ctx, |manager, _| {
                manager.set_withheld_restore_for_test(restore)
            });
        });
        app.read(|ctx| assert!(view.as_ref(ctx).has_session_holder_banner_for_test()));
        app.update(|ctx| {
            manager.update(ctx, |manager, ctx| {
                assert!(manager.run_withheld_restore(ctx))
            });
        });
        app.read(|ctx| {
            let view = view.as_ref(ctx);
            assert!(!view.has_session_holder_banner_for_test());
            assert_eq!(
                view.pending_command_for_test(ctx).as_deref(),
                Some("claude --resume abc")
            );
        });
    });
}

/// The behavioural half of `is_login_shell_bootstrapped = false`.
///
/// Restarting *into a misconfigured shell* is the whole point of the
/// shell-switch path. `TerminalPane` keeps an unbootstrapped pane open on
/// `Event::Exited` precisely so the user can read what went wrong -- but only
/// if the view still knows the replacement never bootstrapped. A stale `true`
/// closes the pane instead, destroying the scrollback and conversation the
/// restart existed to preserve.
///
/// Deliberately does *not* re-mark the pane bootstrapped after the restart:
/// `test_restart_session_keeps_the_pane_and_rearms_its_exit_handling` does,
/// which is exactly why it cannot notice this reset going missing.
#[test]
fn test_a_restart_that_dies_before_bootstrapping_keeps_the_pane_open() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let (pane_group, manager, view) = restartable_pane(&mut app);

        let exited_count = Rc::new(RefCell::new(0usize));
        let exited_count_handle = exited_count.clone();
        app.update(|ctx| {
            ctx.subscribe_to_view(&view, move |_, event, _| {
                if let TerminalViewEvent::Exited = event {
                    *exited_count_handle.borrow_mut() += 1;
                }
            });
        });

        // The shell being replaced had come up: this is the state any pane a
        // user would think to restart is actually in, and the state that makes
        // an unsuppressed exit reach `close_pane`.
        app.update(|ctx| {
            view.update(ctx, |view, _| view.set_login_shell_bootstrapped_for_test());
        });

        let panes_before =
            app.read(|ctx| pane_group.read(ctx, |group, _| group.pane_ids().count()));

        app.update(|ctx| {
            manager.update(ctx, |manager, ctx| {
                assert!(
                    manager.restart_session(None, None, ctx),
                    "a local-tty manager must report that it restarted"
                );
            });
        });

        assert_eventually_within!(
            Duration::from_secs(10) => app.read(|ctx| !view.as_ref(ctx).is_restart_in_progress()),
            "the restart barrier never came back round within 10s"
        );

        // The replacement dies without ever bootstrapping -- the broken-config
        // case. This exit is *not* suppressed: the barrier has already cleared
        // the mark.
        app.update(|ctx| {
            manager
                .read(ctx, |manager, _| manager.model())
                .lock()
                .exit(ExitReason::ShellProcessExited);
        });
        assert_eventually_within!(
            Duration::from_secs(10) => *exited_count.borrow() > 0,
            "the replacement's death must still be delivered as `Event::Exited`; nothing was \
             delivered within 10s"
        );

        assert_eq!(
            app.read(|ctx| pane_group.read(ctx, |group, _| group.pane_ids().count())),
            panes_before,
            "a replacement shell that dies before bootstrapping must leave the pane open so \
             the user can read what went wrong -- exactly as a freshly forked pane does"
        );
        // Which banner the death produces forks on the same flag, and unlike
        // the pane count it is observable in this harness: an unbootstrapped
        // death inserts a rich-content explanation into the blocklist, while a
        // bootstrapped one pins the terse inline "shell exited" banner. A stale
        // `true` therefore shows the user the wrong post-mortem for a shell
        // that never came up.
        assert!(
            !app.read(|ctx| view
                .as_ref(ctx)
                .has_shell_process_terminated_banner_for_test()),
            "a replacement that never bootstrapped must not report its death with the \n             bootstrapped shell's inline banner"
        );
    });
}

/// The behavioural half of `pty_spawn_failed = false`.
///
/// `pty_spawn_failed` suppresses both the terminated banner and the
/// `Event::Exited` emission. Restarting is the obvious recovery from a failed
/// spawn, so a stale `true` leaves the recovered pane permanently unable to
/// report a later clean shell death: no banner, no close, a pane sitting open
/// around a dead shell.
#[test]
fn test_a_pane_restarted_after_a_spawn_failure_can_still_report_a_shell_death() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let (_pane_group, manager, view) = restartable_pane(&mut app);

        let exited_count = Rc::new(RefCell::new(0usize));
        let exited_count_handle = exited_count.clone();
        app.update(|ctx| {
            ctx.subscribe_to_view(&view, move |_, event, _| {
                if let TerminalViewEvent::Exited = event {
                    *exited_count_handle.borrow_mut() += 1;
                }
            });
        });

        app.update(|ctx| {
            view.update(ctx, |view, ctx| {
                view.set_login_shell_bootstrapped_for_test();
                view.on_pty_spawn_failed(anyhow::Error::msg("mock spawn failure"), ctx);
            });
        });

        app.update(|ctx| {
            manager.update(ctx, |manager, ctx| {
                assert!(
                    manager.restart_session(None, None, ctx),
                    "a local-tty manager must report that it restarted"
                );
            });
        });
        assert_eventually_within!(
            Duration::from_secs(10) => app.read(|ctx| !view.as_ref(ctx).is_restart_in_progress()),
            "the restart barrier never came back round within 10s"
        );

        // The replacement comes up and the user exits it for real.
        let exits_before = *exited_count.borrow();
        app.update(|ctx| {
            view.update(ctx, |view, _| view.set_login_shell_bootstrapped_for_test());
            manager
                .read(ctx, |manager, _| manager.model())
                .lock()
                .exit(ExitReason::ShellProcessExited);
        });

        assert_eventually_within!(
            Duration::from_secs(10) => *exited_count.borrow() > exits_before,
            "a pane recovered by restarting out of a spawn failure must still report a later \
             clean shell death; `pty_spawn_failed` left set swallows it forever (waited 10s)"
        );
    });
}

/// A restart with a `target` is a shell *switch*, and step 7 of
/// `restart_session_inner` must prefer that target over the shell the pane is
/// already running -- then relabel the pane for the shell it is launching, not
/// the one it just replaced.
///
/// Asserted synchronously, before any yield: step 7 has no `await` in it, so
/// the label and the resolved shell are already settled when
/// `restart_session` returns. That keeps this test independent of whether the
/// target shell can actually be forked on the machine running it.
///
/// Discriminates on the resolved `AvailableShell`, not on a display string: an
/// implementation that dropped `target` would fall through to
/// `get_user_preferred_shell`, which in this harness is
/// `Config::SystemDefault` -- whose `get_valid_shell_path_and_type()` is
/// `None`, so the comparison below fails loudly rather than coincidentally
/// matching.
#[test]
fn test_restart_with_a_target_shell_switches_to_it_and_relabels_the_pane() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let (_pane_group, manager, view) = restartable_pane(&mut app);

        // Any launchable shell the machine actually has. `AvailableShells`
        // always lists at least the user's preferred shell, and everything
        // except the `SystemDefault` placeholder resolves to launch data.
        let Some(target) = app.read(|ctx| {
            AvailableShells::handle(ctx).read(ctx, |shells, _| {
                shells
                    .get_available_shells()
                    .find_map(|shell| shell.get_valid_shell_path_and_type())
            })
        }) else {
            panic!(
                "this machine lists no launchable shell at all, so there is no shell to switch \
                 to; the harness, not the code under test, is broken"
            );
        };

        let shell_before = app.read(|ctx| {
            view.as_ref(ctx)
                .model
                .lock()
                .shell_launch_state()
                .available_shell()
        });

        app.update(|ctx| {
            manager.update(ctx, |manager, ctx| {
                assert!(
                    manager.restart_session(Some(target.clone()), None, ctx),
                    "a local-tty manager must report that it restarted"
                );
            });
        });

        let shell_after = app.read(|ctx| {
            view.as_ref(ctx)
                .model
                .lock()
                .shell_launch_state()
                .available_shell()
        });

        assert_eq!(
            shell_after
                .as_ref()
                .and_then(|shell| shell.get_valid_shell_path_and_type()),
            Some(target),
            "a restart with a target must launch that target, not the shell the pane was \
             already running and not the user's preferred shell"
        );
        assert_ne!(
            shell_after, shell_before,
            "`set_determining_shell` must relabel the pane for the shell it is switching to; \
             carrying the replaced shell's label forward leaves the pane header lying"
        );
    });
}

/// A restart's directory is converted against the shell that *actually*
/// launches, in `on_shell_determined` -- which is what puts
/// `native_startup_directory`'s WSL arm on the restart path rather than
/// leaving it a pure function nothing calls.
///
/// `test_native_startup_directory_converts_per_shell_kind` proves the
/// conversion is correct; `test_restart_with_system_default_shell_still_lands_
/// in_the_requested_directory` proves the *identity* (`Executable`) arm is
/// reached. Neither can show that a non-identity arm is reached, because for
/// `Executable` a call site that forgot to convert is indistinguishable from
/// one that did.
///
/// Environment-conditional by necessity: reaching the WSL arm end-to-end means
/// a WSL distro really resolving and really forking. Where none is installed
/// there is nothing to switch to and the test reports that rather than failing
/// -- a false red here would be exactly the flake this branch is removing.
#[test]
#[cfg_attr(not(windows), ignore = "WSL shells only exist on Windows")]
fn test_restart_switching_to_wsl_converts_the_directory_to_the_wsl_form() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let (_pane_group, manager, view) = restartable_pane(&mut app);

        let wsl_target = app.read(|ctx| {
            AvailableShells::handle(ctx).read(ctx, |shells, _| {
                shells.get_available_shells().find_map(|shell| {
                    match shell.get_valid_shell_path_and_type() {
                        Some(data @ ShellLaunchData::WSL { .. }) => Some(data),
                        _ => None,
                    }
                })
            })
        });
        let Some(wsl_target) = wsl_target else {
            log::info!(
                "no WSL distro is available on this machine; skipping the WSL directory \
                 conversion test"
            );
            return;
        };

        let requested_dir =
            std::env::current_dir().expect("the test process has a current directory");
        let expected = native_startup_directory(&requested_dir, &wsl_target).expect(
            "the WSL arm must be able to convert the test process's own directory; if it \
             cannot, the conversion itself is broken and the table-driven test says so",
        );
        assert_ne!(
            expected, requested_dir,
            "precondition: the WSL form must differ from the host form, or this test cannot \
             tell a converted directory from an unconverted one"
        );

        app.update(|ctx| {
            manager.update(ctx, |manager, ctx| {
                assert!(
                    manager.restart_session(
                        Some(wsl_target.clone()),
                        Some(requested_dir.clone()),
                        ctx
                    ),
                    "a local-tty manager must report that it restarted"
                );
            });
        });

        // Forking a WSL shell is a cold start; give it room.
        assert_eventually_within!(
            Duration::from_secs(30) => app.read(|ctx| {
                manager.read(ctx, |manager, _| {
                    manager
                        .as_any()
                        .downcast_ref::<LocalTtyTerminalManager<TerminalView>>()
                        .expect("panes in this harness use the local-tty manager")
                        .last_resolved_startup_directory_for_test()
                        .is_some()
                })
            }),
            "the WSL shell never reached `create_pty` within 30s"
        );
        let _ = &view;

        let resolved_dir = app.read(|ctx| {
            manager.read(ctx, |manager, _| {
                manager
                    .as_any()
                    .downcast_ref::<LocalTtyTerminalManager<TerminalView>>()
                    .expect("panes in this harness use the local-tty manager")
                    .last_resolved_startup_directory_for_test()
            })
        });

        assert_eq!(
            resolved_dir,
            Some(expected),
            "a restart that switches to WSL must hand the WSL shell the `/mnt/<drive>/...` \
             form of the requested directory; handing it the Windows host path starts the \
             shell somewhere else entirely"
        );
    });
}

fn two_pane_group(app: &mut App) -> warpui::ViewHandle<PaneGroup> {
    let leaf = |is_focused| PaneTemplateType::PaneTemplate {
        is_focused,
        cwd: "/".into(),
        commands: vec![],
        pane_mode: PaneMode::Terminal,
        shell: None,
    };
    mock_pane_group(
        app,
        MockOptions {
            layout: PanesLayout::Template {
                template: PaneTemplateType::PaneBranchTemplate {
                    split_direction:
                        crate::launch_configs::launch_config::SplitDirection::Horizontal,
                    panes: vec![leaf(Some(true)), leaf(None)],
                },
                shell_start: ShellStartPolicy::Deferred,
            },
            ..Default::default()
        },
    )
}

/// Ends pane 1's bootstrapped shell, after `before_exit` has run against its view, and waits for
/// the exit to reach the view.
async fn end_second_pane_shell(
    app: &mut App,
    pane_group: &warpui::ViewHandle<PaneGroup>,
    before_exit: impl FnOnce(&mut TerminalView),
) {
    let view = app
        .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_view_at_pane_index(1, ctx)))
        .expect("pane 1 has a terminal view");
    let manager = app
        .read(|ctx| pane_group.read(ctx, |group, ctx| group.terminal_manager(1, ctx)))
        .expect("pane 1 has a terminal manager");
    let exited = Rc::new(RefCell::new(false));
    app.update(|ctx| {
        let exited = exited.clone();
        ctx.subscribe_to_view(&view, move |_, event, _| {
            if let TerminalViewEvent::Exited = event {
                *exited.borrow_mut() = true;
            }
        });
        view.update(ctx, |view, _| {
            view.set_login_shell_bootstrapped_for_test();
            before_exit(view);
        });
        manager
            .read(ctx, |manager, _| manager.model())
            .lock()
            .exit(ExitReason::ShellProcessExited);
    });
    assert_eventually_within!(
        Duration::from_secs(10) => *exited.borrow(),
        "the shell's exit never reached the view (waited 10s)"
    );
}

/// A shell that ends without being asked to keeps its pane, so the pane stays in its saved
/// session and can be restarted.
#[test]
fn test_crashed_shell_keeps_its_pane() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = two_pane_group(&mut app);

        end_second_pane_shell(&mut app, &pane_group, |_| {}).await;

        assert_eq!(
            app.read(|ctx| pane_group.as_ref(ctx).visible_pane_ids().len()),
            2,
            "a crashed shell must not close its pane"
        );
    });
}

/// A shell the user ended with EOF closes its pane, as it always has.
#[test]
fn test_shell_ended_by_eof_closes_its_pane() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = two_pane_group(&mut app);

        end_second_pane_shell(&mut app, &pane_group, TerminalView::set_eof_sent_for_test).await;

        assert_eventually_within!(
            Duration::from_secs(10) => app.read(|ctx| pane_group.as_ref(ctx).visible_pane_ids().len()) == 1,
            "a shell the user ended must close its pane (waited 10s)"
        );
    });
}
