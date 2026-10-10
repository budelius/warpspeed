use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use itertools::Itertools;
use parking_lot::Mutex;
#[cfg(feature = "local_fs")]
use repo_metadata::DirectoryWatcher;
use settings::Setting as _;
use warp_completer::completer::{CommandExitStatus, CommandOutput};
use warp_core::command::ExitCode;
use warpui::{App, SingletonEntity};
use warpui_extras::user_preferences;

use super::{ActiveChipSurfaces, ChipUpdateStatus, CurrentPrompt, PromptContext};
use crate::CLIAgentSessionsModel;
use crate::ai::blocklist::agent_view::toolbar_item::AgentToolbarItemKind;
use crate::auth::AuthStateProvider;
use crate::auth::auth_manager::AuthManager;
#[cfg(feature = "local_fs")]
use crate::code_review::diff_state::DiffStats;
#[cfg(feature = "local_fs")]
use crate::code_review::git_repo_model::{GitRepoStatusModel, GitStatusMetadata};
#[cfg(feature = "local_fs")]
use crate::code_review::github_repo_model::GitHubRepoModel;
use crate::context_chips::context_chip::{
    ChipFingerprintInput, Environment, PromptGenerator, RefreshConfig,
};
#[cfg(feature = "local_fs")]
use crate::context_chips::display_chip::GitBranchTrackingStatus;
use crate::context_chips::prompt::Prompt;
use crate::context_chips::{ChipAvailability, ChipDisabledReason, ContextChipKind};
use crate::features::FeatureFlag;
use crate::menu::MenuItem;
use crate::server::server_api::ServerApiProvider;
use crate::server::telemetry::context_provider::AppTelemetryContextProvider;
use crate::settings::WarpPromptSeparator;
#[cfg(windows)]
use crate::system::SystemInfo;
use crate::terminal::cli_agent_sessions::{
    CLIAgentInputState, CLIAgentSession, CLIAgentSessionContext, CLIAgentSessionStatus,
};
use crate::terminal::model::block::BlockMetadata;
use crate::terminal::model::session::{
    CommandExecutor, ExecuteCommandOptions, SessionId, SessionInfo, Sessions,
};
use crate::terminal::session_settings::{
    AgentToolbarChipSelection, CLIAgentToolbarChipSelection, SessionSettings, ToolbarChipSelection,
};
use crate::terminal::shell::Shell;
use crate::terminal::view::PromptPosition;
use crate::terminal::{CLIAgent, History};
#[cfg(feature = "local_fs")]
use crate::util::git::PrInfo;

#[cfg(feature = "local_fs")]
fn git_status_metadata(branch: &str) -> GitStatusMetadata {
    GitStatusMetadata {
        current_branch_name: branch.to_string(),
        main_branch_name: "main".to_string(),
        stats_against_head: DiffStats::default(),
        branch_tracking_status: GitBranchTrackingStatus::new(branch.to_string(), None, 0, 0),
        branch_dropdown_lines: Vec::new(),
    }
}

#[test]
fn git_shell_fallbacks_refresh_only_on_demand() {
    let branch_chip = ContextChipKind::ShellGitBranch.to_chip().unwrap();
    assert!(matches!(
        branch_chip.refresh_config(),
        RefreshConfig::OnDemandOnly
    ));
    assert_eq!(
        branch_chip.runtime_policy().fingerprint_inputs(),
        &[
            ChipFingerprintInput::SessionId,
            ChipFingerprintInput::WorkingDirectory,
            ChipFingerprintInput::InvalidatingCommandCount,
        ]
    );
    assert_eq!(
        branch_chip.runtime_policy().invalidate_on_commands(),
        &["git"]
    );
    assert!(matches!(
        ContextChipKind::GitBranchStatus
            .to_chip()
            .unwrap()
            .refresh_config(),
        RefreshConfig::OnDemandOnly
    ));
    assert!(matches!(
        ContextChipKind::GitDiffStats
            .to_chip()
            .unwrap()
            .refresh_config(),
        RefreshConfig::OnDemandOnly
    ));
}

#[test]
fn test_context_menu_items() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| {
            Prompt::mock_with(
                [
                    ContextChipKind::WorkingDirectory,
                    ContextChipKind::VirtualEnvironment,
                ],
                false,
                WarpPromptSeparator::None,
            )
        });
        app.add_singleton_model(SessionSettings::new_with_defaults);
        app.add_singleton_model(|_ctx| {
            settings::PublicPreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| {
            settings::PrivatePreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });

        let sessions = app.add_model(|_| Sessions::new_for_test());
        let current_prompt = app.add_model(move |ctx| CurrentPrompt::new(sessions, ctx));

        // Set a value for the working directory, but not the virtual environment.
        current_prompt.update(&mut app, |current_prompt, ctx| {
            // Ensure there are state entries for the expected chips.
            current_prompt.update_states_with_new_context(ctx);
            current_prompt.update_chip_value(
                &ContextChipKind::WorkingDirectory,
                Some(crate::context_chips::ChipValue::Text(
                    "/path/to/dir".to_string(),
                )),
            );
        });

        app.read(|ctx| {
            let menu_items = current_prompt
                .as_ref(ctx)
                .copy_menu_items(PromptPosition::Input, ctx)
                .into_iter()
                .filter_map(|item| match item {
                    MenuItem::Item(fields) => Some(fields.label().to_string()),
                    _ => None,
                })
                .collect_vec();

            assert_eq!(menu_items, vec!["Copy Working Directory"]);
        })
    });
}

#[test]
fn test_prompt_to_string() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| {
            Prompt::mock_with(
                [
                    ContextChipKind::Username,
                    ContextChipKind::VirtualEnvironment,
                    ContextChipKind::WorkingDirectory,
                    ContextChipKind::ShellGitBranch,
                ],
                false,
                WarpPromptSeparator::None,
            )
        });
        app.add_singleton_model(SessionSettings::new_with_defaults);
        app.add_singleton_model(|_ctx| {
            settings::PublicPreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| {
            settings::PrivatePreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });

        let sessions = app.add_model(|_| Sessions::new_for_test());
        let current_prompt = app.add_model(move |ctx| CurrentPrompt::new(sessions, ctx));

        // Set a value for the working directory, but not the virtual environment.
        current_prompt.update(&mut app, |current_prompt, ctx| {
            // Ensure there are state entries for the expected chips.
            current_prompt.update_states_with_new_context(ctx);
            current_prompt.update_chip_value(
                &ContextChipKind::Username,
                Some(crate::context_chips::ChipValue::Text("user".to_string())),
            );
            current_prompt.update_chip_value(
                &ContextChipKind::WorkingDirectory,
                Some(crate::context_chips::ChipValue::Text(
                    "/path/to/dir".to_string(),
                )),
            );
            current_prompt.update_chip_value(
                &ContextChipKind::ShellGitBranch,
                Some(crate::context_chips::ChipValue::Text(
                    "my-branch".to_string(),
                )),
            );
        });

        app.read(|ctx| {
            let prompt_string = current_prompt.as_ref(ctx).prompt_as_string(ctx);
            // Components should be in order, and missing components should be skipped.
            assert_eq!(prompt_string, "user /path/to/dir git:(my-branch)");
        })
    });
}

#[test]
fn test_fingerprint_skips_contextual_chip_recompute_when_context_is_unchanged() {
    App::test((), |mut app| async move {
        let session_id = SessionId::from(777);
        app.add_singleton_model(|_| {
            Prompt::mock_with(
                [ContextChipKind::WorkingDirectory],
                false,
                WarpPromptSeparator::None,
            )
        });
        app.add_singleton_model(SessionSettings::new_with_defaults);
        app.add_singleton_model(|_ctx| {
            settings::PublicPreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| {
            settings::PrivatePreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });

        let sessions = app.add_model(|_| Sessions::new_for_test());
        let current_prompt = app.add_model(move |ctx| CurrentPrompt::new(sessions, ctx));

        current_prompt.update(&mut app, |current_prompt, ctx| {
            current_prompt.latest_context = Some(PromptContext {
                active_block_metadata: BlockMetadata::new(
                    Some(session_id),
                    Some("/tmp/project".to_string()),
                ),
                environment: Environment::default(),
            });
            current_prompt.update_states_with_new_context(ctx);

            let state = current_prompt
                .states
                .get(&ContextChipKind::WorkingDirectory)
                .expect("expected working directory state");
            assert_eq!(state.update_status, ChipUpdateStatus::Ready);
            assert!(state.last_fingerprint.is_some());
        });

        current_prompt.update(&mut app, |current_prompt, ctx| {
            current_prompt.update_states_with_new_context(ctx);

            let state = current_prompt
                .states
                .get(&ContextChipKind::WorkingDirectory)
                .expect("expected working directory state");
            assert_eq!(state.update_status, ChipUpdateStatus::Cached);
            assert!(matches!(
                state.last_computed_value.as_ref().and_then(|v| v.as_text()),
                Some("/tmp/project")
            ));
        });
    });
}

#[test]
fn test_environment_updates_only_refresh_matching_session() {
    App::test((), |mut app| async move {
        let prompt_session_id = SessionId::from(777);
        let other_session_id = SessionId::from(778);
        app.add_singleton_model(|_| {
            Prompt::mock_with(
                [ContextChipKind::WorkingDirectory],
                false,
                WarpPromptSeparator::None,
            )
        });
        app.add_singleton_model(SessionSettings::new_with_defaults);
        app.add_singleton_model(|_ctx| {
            settings::PublicPreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| {
            settings::PrivatePreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });

        let sessions = app.add_model(|_| Sessions::new_for_test());
        let sessions_for_prompt = sessions.clone();
        let current_prompt = app.add_model(move |ctx| CurrentPrompt::new(sessions_for_prompt, ctx));

        current_prompt.update(&mut app, |current_prompt, ctx| {
            current_prompt.latest_context = Some(PromptContext {
                active_block_metadata: BlockMetadata::new(
                    Some(prompt_session_id),
                    Some("/tmp/project".to_string()),
                ),
                environment: Environment::default(),
            });
            current_prompt.update_states_with_new_context(ctx);
        });

        sessions.update(&mut app, |sessions, ctx| {
            sessions.set_env_vars_for_session(
                other_session_id,
                HashMap::from([("OTHER".to_string(), "changed".to_string())]),
                ctx,
            );
        });
        current_prompt.read(&app, |current_prompt, _| {
            assert_eq!(
                current_prompt.states[&ContextChipKind::WorkingDirectory].update_status,
                ChipUpdateStatus::Ready
            );
        });

        sessions.update(&mut app, |sessions, ctx| {
            sessions.set_env_vars_for_session(
                prompt_session_id,
                HashMap::from([("MATCHING".to_string(), "changed".to_string())]),
                ctx,
            );
        });
        current_prompt.read(&app, |current_prompt, _| {
            assert_eq!(
                current_prompt.states[&ContextChipKind::WorkingDirectory].update_status,
                ChipUpdateStatus::Cached
            );
        });
    });
}

#[test]
fn test_shell_chip_is_disabled_when_required_executable_is_missing() {
    App::test((), |mut app| async move {
        let session_id = SessionId::from(456);
        app.add_singleton_model(|_| {
            Prompt::mock_with(
                [ContextChipKind::ShellGitBranch],
                false,
                WarpPromptSeparator::None,
            )
        });
        app.add_singleton_model(SessionSettings::new_with_defaults);
        app.add_singleton_model(|_| History::new(vec![]));
        app.add_singleton_model(|_ctx| {
            settings::PublicPreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| {
            settings::PrivatePreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| ServerApiProvider::new_for_test());
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(AppTelemetryContextProvider::new_context_provider);
        app.add_singleton_model(AuthManager::new_for_test);
        app.add_singleton_model(|_| crate::settings::manager::SettingsManager::default());
        crate::settings::InputSettings::register(&mut app);
        app.update(crate::settings::AISettings::register_and_subscribe_to_events);
        app.add_singleton_model(crate::workspaces::user_workspaces::UserWorkspaces::default_mock);
        #[cfg(windows)]
        app.add_singleton_model(SystemInfo::new);

        let executor = Arc::new(RecordingCommandExecutor::default());
        let sessions = app.add_model(|ctx| {
            let mut sessions = Sessions::new_for_test().with_command_executor(executor.clone());
            sessions.initialize_bootstrapped_session(
                SessionInfo::new_for_test().with_id(session_id),
                "test command".to_string(),
                vec![],
                None,
                ctx,
            );
            sessions
        });
        let sessions_for_prompt = sessions.clone();
        let current_prompt =
            app.add_model(move |ctx| CurrentPrompt::new(sessions_for_prompt.clone(), ctx));

        let session = app
            .read(|ctx| sessions.as_ref(ctx).get(session_id))
            .expect("session should exist");
        session.load_external_commands().await;
        executor.clear();

        current_prompt.update(&mut app, |current_prompt, ctx| {
            current_prompt.latest_context = Some(PromptContext {
                active_block_metadata: BlockMetadata::new(
                    Some(session_id),
                    Some("/tmp/project".to_string()),
                ),
                environment: Environment::default(),
            });
            current_prompt.update_states_with_new_context(ctx);

            let state = current_prompt
                .states
                .get(&ContextChipKind::ShellGitBranch)
                .expect("expected git branch state");
            assert_eq!(
                state.availability,
                ChipAvailability::Disabled(ChipDisabledReason::RequiresExecutable {
                    command: "git".to_string(),
                })
            );
            assert_eq!(state.update_status, ChipUpdateStatus::Disabled);
            assert!(state.generator_handle.is_none());
            assert!(state.on_click_generator_handle.is_none());
        });

        assert!(executor.commands.lock().is_empty());
    });
}

#[test]
fn test_github_pr_chip_runtime_policy_configuration() {
    let _flag_guard = FeatureFlag::GithubPrPromptChip.override_enabled(true);
    let chip = ContextChipKind::GithubPullRequest
        .to_chip()
        .expect("github pr chip should exist");
    let policy = chip.runtime_policy();

    assert!(matches!(
        chip.generator(),
        PromptGenerator::Contextual { .. }
    ));
    assert!(policy.required_executables().is_empty());
    assert_eq!(policy.shell_command_timeout(), None);
    assert!(!policy.suppress_on_failure());
    assert!(policy.fingerprint_inputs().is_empty());
    assert!(policy.invalidate_on_commands().is_empty());
}

#[test]
fn test_invalidating_command_count_unaffected_for_chips_without_invalidate_on_commands() {
    App::test((), |mut app| async move {
        let session_id = SessionId::from(888);
        app.add_singleton_model(|_| {
            Prompt::mock_with(
                [ContextChipKind::WorkingDirectory],
                false,
                WarpPromptSeparator::None,
            )
        });
        app.add_singleton_model(SessionSettings::new_with_defaults);
        app.add_singleton_model(|_ctx| {
            settings::PublicPreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| {
            settings::PrivatePreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });

        let sessions = app.add_model(|_| Sessions::new_for_test());
        let current_prompt = app.add_model(move |ctx| CurrentPrompt::new(sessions, ctx));

        current_prompt.update(&mut app, |current_prompt, ctx| {
            current_prompt.latest_context = Some(PromptContext {
                active_block_metadata: BlockMetadata::new(
                    Some(session_id),
                    Some("/tmp/project".to_string()),
                ),
                environment: Environment::default(),
            });
            current_prompt.update_states_with_new_context(ctx);

            // WorkingDirectory has no invalidate_on_commands, so the counter should be 0.
            let state = current_prompt
                .states
                .get(&ContextChipKind::WorkingDirectory)
                .expect("expected working directory state");
            assert_eq!(state.invalidating_command_count, 0);
        });
    });
}

#[test]
fn test_disabling_chips() {
    App::test((), |mut app| async move {
        let session_id = SessionId::from(123);
        app.add_singleton_model(|_| {
            Prompt::mock_with(
                [ContextChipKind::ShellGitBranch],
                false,
                WarpPromptSeparator::None,
            )
        });
        app.add_singleton_model(SessionSettings::new_with_defaults);
        app.add_singleton_model(|_| History::new(vec![]));
        app.add_singleton_model(|_ctx| {
            settings::PublicPreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| {
            settings::PrivatePreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| ServerApiProvider::new_for_test());
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(AppTelemetryContextProvider::new_context_provider);
        app.add_singleton_model(AuthManager::new_for_test);

        // Register required singleton models to fix the singleton model error
        app.add_singleton_model(|_| crate::settings::manager::SettingsManager::default());
        crate::settings::InputSettings::register(&mut app);
        app.update(crate::settings::AISettings::register_and_subscribe_to_events);
        app.add_singleton_model(crate::workspaces::user_workspaces::UserWorkspaces::default_mock);
        #[cfg(windows)]
        app.add_singleton_model(SystemInfo::new);

        let executor = Arc::new(RecordingCommandExecutor::default());

        let sessions = app.add_model(|ctx| {
            let mut sessions = Sessions::new_for_test().with_command_executor(executor.clone());
            sessions.initialize_bootstrapped_session(
                SessionInfo::new_for_test().with_id(session_id),
                "test command".to_string(),
                vec![],
                None,
                ctx,
            );
            sessions
        });
        let current_prompt = app.add_model(move |ctx| CurrentPrompt::new(sessions, ctx));

        // Context chips can only be disabled in Classic mode.
        app.update(|ctx| {
            crate::settings::InputSettings::handle(ctx).update(ctx, |settings, ctx| {
                let _ = settings
                    .input_box_type
                    .set_value(crate::settings::InputBoxType::Classic, ctx);
            });
        });

        executor.clear();

        current_prompt
            .update(&mut app, |current_prompt, ctx| {
                current_prompt.latest_context = Some(PromptContext {
                    active_block_metadata: BlockMetadata::new(Some(session_id), None),
                    environment: Environment::default(),
                });
                // This is needed because we set latest_context directly.
                current_prompt.update_states_with_new_context(ctx);
                assert!(current_prompt.are_any_generators_running());
                current_prompt.await_generators(ctx)
            })
            .await;

        // By default, context chips are enabled, so the git branch dropdown command should run.
        assert!(!executor.commands.lock().is_empty());

        // If PS1 is enabled, the command should not run.
        app.update(|ctx| {
            SessionSettings::handle(ctx).update(ctx, |settings, ctx| {
                let _ = settings.honor_ps1.set_value(true, ctx);
            });
        });
        // Clear the command history right after changing the PS1 setting, to ensure that the
        // CurrentPrompt model has processed the change.
        executor.clear();

        current_prompt.update(&mut app, |current_prompt, ctx| {
            // Ensure that, if the model were going to run generators, it had a chance to.
            current_prompt.update_states_with_new_context(ctx);
            // There may be some shell generators still pending in the background, which won't be
            // directly cancelled. Instead of asserting that no commands run, assert that the
            // CurrentPrompt model is not still trying to run generators.
            assert!(!current_prompt.are_any_generators_running());
        });

        // If context chips are re-enabled, generator commands should start running again.
        app.update(|ctx| {
            SessionSettings::handle(ctx).update(ctx, |settings, ctx| {
                let _ = settings.honor_ps1.set_value(false, ctx);
            });
        });

        current_prompt
            .update(&mut app, |current_prompt, ctx| {
                assert!(current_prompt.are_any_generators_running());
                current_prompt.await_generators(ctx)
            })
            .await;

        assert!(!executor.commands.lock().is_empty());
    });
}

#[test]
fn test_chips_to_run_only_includes_active_surface_configurations() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| {
            Prompt::mock_with(
                [ContextChipKind::Username],
                false,
                WarpPromptSeparator::None,
            )
        });
        app.add_singleton_model(SessionSettings::new_with_defaults);
        app.add_singleton_model(|_ctx| {
            settings::PublicPreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| {
            settings::PrivatePreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.update(|ctx| {
            SessionSettings::handle(ctx).update(ctx, |settings, ctx| {
                settings
                    .agent_footer_chip_selection
                    .set_value(
                        AgentToolbarChipSelection::Custom {
                            left: vec![AgentToolbarItemKind::ContextChip(
                                ContextChipKind::WorkingDirectory,
                            )],
                            right: vec![],
                        },
                        ctx,
                    )
                    .unwrap();
                settings
                    .cli_agent_footer_chip_selection
                    .set_value(
                        CLIAgentToolbarChipSelection::Custom {
                            left: vec![AgentToolbarItemKind::ContextChip(
                                ContextChipKind::ShellGitBranch,
                            )],
                            right: vec![],
                        },
                        ctx,
                    )
                    .unwrap();
            });
        });

        let sessions = app.add_model(|_| Sessions::new_for_test());
        let current_prompt = app.add_model(move |ctx| CurrentPrompt::new(sessions, ctx));

        app.read(|ctx| {
            let current_prompt = current_prompt.as_ref(ctx);
            let chips_for = |surfaces| current_prompt.chips_to_run_for_surfaces(surfaces, ctx);

            assert!(chips_for(ActiveChipSurfaces::default()).is_empty());
            assert_eq!(
                chips_for(ActiveChipSurfaces {
                    prompt: true,
                    ..Default::default()
                }),
                vec![ContextChipKind::Username]
            );
            assert_eq!(
                chips_for(ActiveChipSurfaces {
                    agent_footer: true,
                    ..Default::default()
                }),
                vec![ContextChipKind::WorkingDirectory]
            );
            assert_eq!(
                chips_for(ActiveChipSurfaces {
                    cli_agent_footer: true,
                    ..Default::default()
                }),
                vec![ContextChipKind::ShellGitBranch]
            );
            assert_eq!(
                chips_for(ActiveChipSurfaces {
                    prompt: true,
                    agent_footer: true,
                    cli_agent_footer: true,
                }),
                vec![
                    ContextChipKind::Username,
                    ContextChipKind::WorkingDirectory,
                    ContextChipKind::ShellGitBranch,
                ]
            );
        });
    });
}

#[test]
fn test_cli_agent_footer_chips_require_a_visible_supported_footer() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Prompt::mock());
        app.add_singleton_model(SessionSettings::new_with_defaults);
        app.add_singleton_model(|_| History::new(vec![]));
        app.add_singleton_model(|_ctx| {
            settings::PublicPreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| {
            settings::PrivatePreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| ServerApiProvider::new_for_test());
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(AppTelemetryContextProvider::new_context_provider);
        app.add_singleton_model(AuthManager::new_for_test);
        app.add_singleton_model(|_| crate::settings::manager::SettingsManager::default());
        crate::settings::InputSettings::register(&mut app);
        app.update(crate::settings::AISettings::register_and_subscribe_to_events);
        app.add_singleton_model(crate::workspaces::user_workspaces::UserWorkspaces::default_mock);
        app.add_singleton_model(|_| CLIAgentSessionsModel::new());
        app.add_singleton_model(|_| {
            crate::terminal::agent_session_history_model::AgentSessionHistoryModel::new()
        });
        #[cfg(windows)]
        app.add_singleton_model(SystemInfo::new);

        let sessions = app.add_model(|_| Sessions::new_for_test());
        let current_prompt = app.add_model(move |ctx| CurrentPrompt::new(sessions, ctx));
        let terminal_view_id = current_prompt.id();
        current_prompt.update(&mut app, |current_prompt, _| {
            current_prompt.terminal_view_id = Some(terminal_view_id);
        });

        let cli_footer_chips =
            |app: &App| app.read(|ctx| current_prompt.as_ref(ctx).chips_to_run(ctx));
        assert!(cli_footer_chips(&app).is_empty());

        let session_for = |agent| CLIAgentSession {
            agent,
            status: CLIAgentSessionStatus::InProgress,
            session_context: CLIAgentSessionContext::default(),
            input_state: CLIAgentInputState::Closed,
            should_auto_toggle_input: false,
            listener: None,
            plugin_version: None,
            remote_host: None,
            draft_text: None,
            custom_command_prefix: None,
            received_rich_notification: false,
        };

        CLIAgentSessionsModel::handle(&app).update(&mut app, |sessions, ctx| {
            sessions.set_session(terminal_view_id, session_for(CLIAgent::Claude), ctx);
        });
        assert_eq!(
            cli_footer_chips(&app),
            app.read(|ctx| {
                SessionSettings::as_ref(ctx)
                    .cli_agent_footer_chip_selection
                    .all_chips()
            })
        );

        CLIAgentSessionsModel::handle(&app).update(&mut app, |sessions, ctx| {
            sessions.set_session(terminal_view_id, session_for(CLIAgent::WarpTui), ctx);
        });
        assert!(cli_footer_chips(&app).is_empty());

        CLIAgentSessionsModel::handle(&app).update(&mut app, |sessions, ctx| {
            sessions.set_session(terminal_view_id, session_for(CLIAgent::Claude), ctx);
        });
        crate::settings::AISettings::handle(&app).update(&mut app, |settings, ctx| {
            settings
                .should_render_cli_agent_footer
                .set_value(false, ctx)
                .unwrap();
        });
        assert!(cli_footer_chips(&app).is_empty());
    });
}

#[test]
fn test_ps1_without_active_agent_surface_runs_no_footer_generators() {
    let _flag_guard = FeatureFlag::AgentView.override_enabled(true);
    App::test((), |mut app| async move {
        let session_id = SessionId::from(321);
        app.add_singleton_model(|_| Prompt::mock());
        app.add_singleton_model(SessionSettings::new_with_defaults);
        app.add_singleton_model(|_| History::new(vec![]));
        app.add_singleton_model(|_ctx| {
            settings::PublicPreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| {
            settings::PrivatePreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.update(|ctx| {
            SessionSettings::handle(ctx).update(ctx, |settings, ctx| {
                settings.honor_ps1.set_value(true, ctx).unwrap();
            });
        });
        app.add_singleton_model(|_| ServerApiProvider::new_for_test());
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(AppTelemetryContextProvider::new_context_provider);
        app.add_singleton_model(AuthManager::new_for_test);
        app.add_singleton_model(|_| crate::settings::manager::SettingsManager::default());
        crate::settings::InputSettings::register(&mut app);
        app.update(crate::settings::AISettings::register_and_subscribe_to_events);
        app.add_singleton_model(crate::workspaces::user_workspaces::UserWorkspaces::default_mock);
        #[cfg(windows)]
        app.add_singleton_model(SystemInfo::new);

        let executor = Arc::new(RecordingCommandExecutor::default());
        let sessions = app.add_model(|ctx| {
            let mut sessions = Sessions::new_for_test().with_command_executor(executor.clone());
            sessions.initialize_bootstrapped_session(
                SessionInfo::new_for_test().with_id(session_id),
                "test command".to_string(),
                vec![],
                None,
                ctx,
            );
            sessions
        });
        let current_prompt = app.add_model(move |ctx| CurrentPrompt::new(sessions, ctx));

        current_prompt
            .update(&mut app, |current_prompt, ctx| {
                current_prompt.latest_context = Some(PromptContext {
                    active_block_metadata: BlockMetadata::new(Some(session_id), None),
                    environment: Environment::default(),
                });
                current_prompt.update_states_with_new_context(ctx);
                current_prompt.await_generators(ctx)
            })
            .await;

        assert!(executor.commands.lock().is_empty());
        app.read(|ctx| {
            assert!(current_prompt.as_ref(ctx).states.is_empty());
        });
    });
}
#[cfg(feature = "local_fs")]
#[test]
fn test_externally_driven_chip_skips_periodic_timer() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| {
            Prompt::mock_with(
                [ContextChipKind::ShellGitBranch],
                false,
                WarpPromptSeparator::None,
            )
        });
        app.add_singleton_model(SessionSettings::new_with_defaults);
        app.add_singleton_model(|_ctx| {
            settings::PublicPreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| {
            settings::PrivatePreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });

        let temp_dir = tempfile::TempDir::new().unwrap();
        let watcher_handle = app.add_singleton_model(DirectoryWatcher::new_for_testing);
        let repo_handle = watcher_handle.update(&mut app, |watcher, ctx| {
            watcher
                .add_directory(
                    warp_util::standardized_path::StandardizedPath::from_local_canonicalized(
                        temp_dir.path(),
                    )
                    .unwrap(),
                    ctx,
                )
                .unwrap()
        });
        let git_status = app
            .add_model(move |ctx| GitRepoStatusModel::new_local_for_test(repo_handle, None, ctx));

        let sessions = app.add_model(|_| Sessions::new_for_test());
        let current_prompt = app.add_model(move |ctx| CurrentPrompt::new(sessions, ctx));

        current_prompt.update(&mut app, |cp, ctx| {
            cp.set_git_repo_status(Some(git_status.downgrade()), ctx);
            cp.update_states_with_new_context(ctx);
        });

        app.read(|ctx| {
            let cp = current_prompt.as_ref(ctx);
            let state = cp
                .states
                .get(&ContextChipKind::ShellGitBranch)
                .expect("ShellGitBranch state should exist after set_git_repo_status");
            assert!(
                state.refresh_handle.is_none(),
                "Externally-driven chip should not have a periodic refresh handle"
            );
        });
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn test_git_status_change_updates_chip_value() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| {
            Prompt::mock_with(
                [ContextChipKind::ShellGitBranch],
                false,
                WarpPromptSeparator::None,
            )
        });
        app.add_singleton_model(SessionSettings::new_with_defaults);
        app.add_singleton_model(|_ctx| {
            settings::PublicPreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| {
            settings::PrivatePreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });

        let temp_dir = tempfile::TempDir::new().unwrap();
        let watcher_handle = app.add_singleton_model(DirectoryWatcher::new_for_testing);
        let repo_handle = watcher_handle.update(&mut app, |watcher, ctx| {
            watcher
                .add_directory(
                    warp_util::standardized_path::StandardizedPath::from_local_canonicalized(
                        temp_dir.path(),
                    )
                    .unwrap(),
                    ctx,
                )
                .unwrap()
        });

        let initial_metadata = git_status_metadata("main");
        let git_status = app.add_model(move |ctx| {
            GitRepoStatusModel::new_local_for_test(repo_handle, Some(initial_metadata), ctx)
        });

        let sessions = app.add_model(|_| Sessions::new_for_test());
        let current_prompt = app.add_model(move |ctx| CurrentPrompt::new(sessions, ctx));

        // Subscribe to the git status model and run chips.
        current_prompt.update(&mut app, |cp, ctx| {
            cp.set_git_repo_status(Some(git_status.downgrade()), ctx);
            cp.update_states_with_new_context(ctx);
        });

        // Simulate a branch change by updating the model's metadata.
        git_status.update(&mut app, |model, ctx| {
            model.set_metadata_for_test(Some(git_status_metadata("feature-branch")), ctx);
        });

        app.read(|ctx| {
            let value = current_prompt
                .as_ref(ctx)
                .latest_chip_value(&ContextChipKind::ShellGitBranch);
            assert_eq!(
                value,
                Some(&crate::context_chips::ChipValue::Text(
                    "feature-branch".to_string(),
                )),
                "Chip value should reflect the new branch name after metadata change"
            );
        });
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn test_git_status_change_updates_branch_status_chip_value() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| {
            Prompt::mock_with(
                [ContextChipKind::GitBranchStatus],
                false,
                WarpPromptSeparator::None,
            )
        });
        app.add_singleton_model(SessionSettings::new_with_defaults);
        app.add_singleton_model(|_ctx| {
            settings::PublicPreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| {
            settings::PrivatePreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });

        let temp_dir = tempfile::TempDir::new().unwrap();
        let watcher_handle = app.add_singleton_model(DirectoryWatcher::new_for_testing);
        let repo_handle = watcher_handle.update(&mut app, |watcher, ctx| {
            watcher
                .add_directory(
                    warp_util::standardized_path::StandardizedPath::from_local_canonicalized(
                        temp_dir.path(),
                    )
                    .unwrap(),
                    ctx,
                )
                .unwrap()
        });

        let git_status = app
            .add_model(move |ctx| GitRepoStatusModel::new_local_for_test(repo_handle, None, ctx));
        let sessions = app.add_model(|_| Sessions::new_for_test());
        let current_prompt = app.add_model(move |ctx| CurrentPrompt::new(sessions, ctx));

        current_prompt.update(&mut app, |cp, ctx| {
            cp.set_git_repo_status(Some(git_status.downgrade()), ctx);
            cp.update_states_with_new_context(ctx);
        });

        let branch_tracking_status = GitBranchTrackingStatus::new(
            "feature-branch".to_string(),
            Some("origin/feature-branch".to_string()),
            3,
            1,
        );
        git_status.update(&mut app, |model, ctx| {
            model.set_metadata_for_test(
                Some(GitStatusMetadata {
                    current_branch_name: "feature-branch".to_string(),
                    main_branch_name: "main".to_string(),
                    stats_against_head: DiffStats::default(),
                    branch_tracking_status: branch_tracking_status.clone(),
                    branch_dropdown_lines: Vec::new(),
                }),
                ctx,
            );
        });

        app.read(|ctx| {
            let value = current_prompt
                .as_ref(ctx)
                .latest_chip_value(&ContextChipKind::GitBranchStatus);
            assert_eq!(
                value,
                Some(&crate::context_chips::ChipValue::GitBranchStatus(
                    branch_tracking_status,
                )),
                "Branch status chip should reflect ahead and behind metadata"
            );
        });
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn test_git_status_pr_info_updates_github_pr_chip_value() {
    let _flag_guard = FeatureFlag::GithubPrPromptChip.override_enabled(true);
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| {
            Prompt::mock_with(
                [ContextChipKind::GithubPullRequest],
                false,
                WarpPromptSeparator::None,
            )
        });
        app.add_singleton_model(SessionSettings::new_with_defaults);
        app.add_singleton_model(|_ctx| {
            settings::PublicPreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });
        app.add_singleton_model(|_| {
            settings::PrivatePreferences::new(
                Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
            )
        });

        let temp_dir = tempfile::TempDir::new().unwrap();
        let watcher_handle = app.add_singleton_model(DirectoryWatcher::new_for_testing);
        let repo_handle = watcher_handle.update(&mut app, |watcher, ctx| {
            watcher
                .add_directory(
                    warp_util::standardized_path::StandardizedPath::from_local_canonicalized(
                        temp_dir.path(),
                    )
                    .unwrap(),
                    ctx,
                )
                .unwrap()
        });

        let git_status = app.add_model(move |ctx| {
            GitRepoStatusModel::new_local_for_test(
                repo_handle,
                Some(git_status_metadata("feature-a")),
                ctx,
            )
        });
        let github_repo_model = {
            let git_status = git_status.clone();
            app.add_model(move |ctx| GitHubRepoModel::new_local_for_test(git_status, ctx))
        };

        let sessions = app.add_model(|_| Sessions::new_for_test());
        let current_prompt = app.add_model(move |ctx| CurrentPrompt::new(sessions, ctx));

        current_prompt.update(&mut app, |cp, ctx| {
            cp.set_git_repo_status(Some(git_status.downgrade()), ctx);
            cp.set_github_repo_model(Some(github_repo_model.downgrade()), ctx);
            cp.update_states_with_new_context(ctx);
        });

        github_repo_model.update(&mut app, |model, ctx| {
            model.set_pr_info_for_test(
                Some(PrInfo {
                    number: 123,
                    url: "https://github.com/warp/warp/pull/123".to_string(),
                    state: "OPEN".to_string(),
                    draft: true,
                    base_branch: "main".to_string(),
                }),
                ctx,
            );
        });

        app.read(|ctx| {
            let value = current_prompt
                .as_ref(ctx)
                .latest_chip_value(&ContextChipKind::GithubPullRequest);
            assert_eq!(
                value,
                Some(&crate::context_chips::ChipValue::Text(
                    "https://github.com/warp/warp/pull/123".to_string(),
                )),
            );
        });

        // Clearing the model's PR info propagates to the chip.
        github_repo_model.update(&mut app, |model, ctx| {
            model.set_pr_info_for_test(None, ctx);
        });

        app.read(|ctx| {
            let value = current_prompt
                .as_ref(ctx)
                .latest_chip_value(&ContextChipKind::GithubPullRequest);
            assert_eq!(
                value, None,
                "PR chip should clear when the model's cached PR info is cleared"
            );
        });

        github_repo_model.update(&mut app, |model, ctx| {
            model.set_pr_info_for_test(
                Some(PrInfo {
                    number: 456,
                    url: "https://github.com/warp/warp/pull/456".to_string(),
                    state: "OPEN".to_string(),
                    draft: false,
                    base_branch: "main".to_string(),
                }),
                ctx,
            );
        });

        current_prompt.update(&mut app, |cp, ctx| {
            {
                let state = cp
                    .states
                    .get_mut(&ContextChipKind::GithubPullRequest)
                    .expect("expected github PR chip state");
                state.last_on_click_values = Some(vec!["stale".to_string()]);
                state.last_fingerprint = Some(123);
                state.last_failure_fingerprint = Some(456);
                state.update_status = ChipUpdateStatus::Cached;
                state.generator_handle = Some(ctx.spawn(
                    async {
                        async_io::Timer::after(std::time::Duration::from_secs(60)).await;
                    },
                    |_, _, _| {},
                ));
            }

            cp.set_github_repo_model(None, ctx);

            let state = cp
                .states
                .get(&ContextChipKind::GithubPullRequest)
                .expect("expected github PR chip state");
            assert!(state.last_computed_value.is_none());
            assert!(state.last_on_click_values.is_none());
            assert!(state.last_fingerprint.is_none());
            assert!(state.last_failure_fingerprint.is_none());
            assert_eq!(state.update_status, ChipUpdateStatus::Idle);
            assert!(state.generator_handle.is_none());
        });
    });
}

/// Builds a prompt whose only chips are the three git chips the repository
/// model supplies values for, wired to a live bootstrapped session so a shell
/// generator would run if anything asked for one.
#[cfg(feature = "local_fs")]
fn setup_git_chip_prompt(
    app: &mut App,
    session_id: SessionId,
) -> (
    Arc<RecordingCommandExecutor>,
    warpui::ModelHandle<CurrentPrompt>,
    warpui::ModelHandle<GitRepoStatusModel>,
) {
    app.add_singleton_model(|_| {
        Prompt::mock_with(
            [
                ContextChipKind::ShellGitBranch,
                ContextChipKind::GitBranchStatus,
                ContextChipKind::GitDiffStats,
            ],
            false,
            WarpPromptSeparator::None,
        )
    });
    app.add_singleton_model(SessionSettings::new_with_defaults);
    app.add_singleton_model(|_ctx| {
        settings::PublicPreferences::new(
            Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
        )
    });
    app.add_singleton_model(|_| {
        settings::PrivatePreferences::new(
            Box::<user_preferences::in_memory::InMemoryPreferences>::default(),
        )
    });
    app.add_singleton_model(|_| History::new(vec![]));
    app.add_singleton_model(|_| ServerApiProvider::new_for_test());
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(AppTelemetryContextProvider::new_context_provider);
    app.add_singleton_model(AuthManager::new_for_test);
    app.add_singleton_model(|_| crate::settings::manager::SettingsManager::default());
    crate::settings::InputSettings::register(app);
    app.update(crate::settings::AISettings::register_and_subscribe_to_events);
    app.add_singleton_model(crate::workspaces::user_workspaces::UserWorkspaces::default_mock);
    #[cfg(windows)]
    app.add_singleton_model(SystemInfo::new);

    let temp_dir = tempfile::TempDir::new().unwrap();
    let watcher_handle = app.add_singleton_model(DirectoryWatcher::new_for_testing);
    let repo_handle = watcher_handle.update(app, |watcher, ctx| {
        watcher
            .add_directory(
                warp_util::standardized_path::StandardizedPath::from_local_canonicalized(
                    temp_dir.path(),
                )
                .unwrap(),
                ctx,
            )
            .unwrap()
    });
    let initial_metadata = git_status_metadata("main");
    let git_status = app.add_model(move |ctx| {
        GitRepoStatusModel::new_local_for_test(repo_handle, Some(initial_metadata), ctx)
    });

    let executor = Arc::new(RecordingCommandExecutor::default());
    let executor_for_sessions = executor.clone();
    let sessions = app.add_model(move |ctx| {
        let mut sessions = Sessions::new_for_test().with_command_executor(executor_for_sessions);
        sessions.initialize_bootstrapped_session(
            SessionInfo::new_for_test().with_id(session_id),
            "test command".to_string(),
            vec![],
            None,
            ctx,
        );
        sessions
    });
    let current_prompt = app.add_model(move |ctx| CurrentPrompt::new(sessions, ctx));

    (executor, current_prompt, git_status)
}

/// The three git chips take their values from `GitRepoStatusModel`, so running
/// their shell generators is discarded work. With a repository model attached,
/// a context change must run no command at all.
#[cfg(feature = "local_fs")]
#[test]
fn test_git_chips_run_no_shell_command_when_repo_model_supplies_values() {
    App::test((), |mut app| async move {
        let session_id = SessionId::from(4242);
        let (executor, current_prompt, git_status) = setup_git_chip_prompt(&mut app, session_id);

        current_prompt.update(&mut app, |cp, ctx| {
            cp.set_git_repo_status(Some(git_status.downgrade()), ctx);
            cp.latest_context = Some(PromptContext {
                active_block_metadata: BlockMetadata::new(
                    Some(session_id),
                    Some("/tmp/project".to_string()),
                ),
                environment: Environment::default(),
            });
            cp.update_states_with_new_context(ctx);
        });
        current_prompt
            .update(&mut app, |cp, ctx| cp.await_generators(ctx))
            .await;

        let commands = executor.commands.lock().clone();
        assert!(
            commands.is_empty(),
            "chips backed by the repository model should run no shell command, but ran: {commands:?}"
        );
    });
}

/// The suppression is conditional on a repository model existing. A session
/// without one has no other source for these values, so the shell generators
/// must still run.
#[cfg(feature = "local_fs")]
#[test]
fn test_git_chips_still_run_shell_command_without_a_repo_model() {
    App::test((), |mut app| async move {
        let session_id = SessionId::from(4243);
        let (executor, current_prompt, _git_status) = setup_git_chip_prompt(&mut app, session_id);

        // Deliberately skip `set_git_repo_status`: nothing supplies these
        // values externally, so the shell path is the only one left.
        current_prompt.update(&mut app, |cp, ctx| {
            cp.latest_context = Some(PromptContext {
                active_block_metadata: BlockMetadata::new(
                    Some(session_id),
                    Some("/tmp/project".to_string()),
                ),
                environment: Environment::default(),
            });
            cp.update_states_with_new_context(ctx);
        });
        current_prompt
            .update(&mut app, |cp, ctx| cp.await_generators(ctx))
            .await;

        let commands = executor.commands.lock().clone();
        assert!(
            commands.iter().any(|command| command.contains("git")),
            "without a repository model the git chips must fall back to the shell, but ran: {commands:?}"
        );
    });
}

/// `is_updated_externally` gates the suppression, so its scope is the scope of
/// the change. It must name the three git chips and nothing else.
#[cfg(feature = "local_fs")]
#[test]
fn test_only_git_chips_are_updated_externally_by_the_repo_model() {
    App::test((), |mut app| async move {
        let session_id = SessionId::from(4244);
        let (_executor, current_prompt, git_status) = setup_git_chip_prompt(&mut app, session_id);

        current_prompt.update(&mut app, |cp, ctx| {
            cp.set_git_repo_status(Some(git_status.downgrade()), ctx);
        });

        app.read(|ctx| {
            let cp = current_prompt.as_ref(ctx);
            for chip_kind in [
                ContextChipKind::ShellGitBranch,
                ContextChipKind::GitBranchStatus,
                ContextChipKind::GitDiffStats,
            ] {
                assert!(
                    cp.is_updated_externally(&chip_kind),
                    "{chip_kind:?} takes its value from the repository model"
                );
            }
            assert!(
                !cp.is_updated_externally(&ContextChipKind::WorkingDirectory),
                "a chip the repository model does not supply must keep its generator"
            );
        });
    });
}

/// A remote session carries every chip value over the wire but leaves
/// `branch_dropdown_lines` empty, which is what makes `apply_git_branch_dropdown`
/// fall back to the shell. That fallback reaches `refresh_on_click_values`
/// directly rather than through `fetch_chip_value_once`, so the early return
/// must not reach it.
#[cfg(feature = "local_fs")]
#[test]
fn test_branch_dropdown_falls_back_to_the_shell_when_the_model_supplies_no_lines() {
    App::test((), |mut app| async move {
        let session_id = SessionId::from(4245);
        let (executor, current_prompt, git_status) = setup_git_chip_prompt(&mut app, session_id);

        current_prompt.update(&mut app, |cp, ctx| {
            cp.set_git_repo_status(Some(git_status.downgrade()), ctx);
            cp.latest_context = Some(PromptContext {
                active_block_metadata: BlockMetadata::new(
                    Some(session_id),
                    Some("/tmp/project".to_string()),
                ),
                environment: Environment::default(),
            });
            cp.update_states_with_new_context(ctx);
        });
        executor.clear();

        // `git_status_metadata` leaves `branch_dropdown_lines` empty, standing
        // in for a remote repository whose branch list is not carried over the
        // wire. Changing the branch is what triggers the dropdown refresh.
        git_status.update(&mut app, |model, ctx| {
            model.set_metadata_for_test(Some(git_status_metadata("feature-branch")), ctx);
        });
        current_prompt
            .update(&mut app, |cp, ctx| cp.await_generators(ctx))
            .await;

        let commands = executor.commands.lock().clone();
        assert!(
            commands.iter().any(|command| command.contains("branch")),
            "the dropdown must fall back to the shell when the model supplies no lines, but ran: {commands:?}"
        );
    });
}

/// A [`CommandExecutor`] implementation that records which commands were run, but does not
/// execute them.
#[derive(Debug, Default)]
struct RecordingCommandExecutor {
    commands: Mutex<Vec<String>>,
    response_queue: Mutex<VecDeque<CommandOutput>>,
}

impl RecordingCommandExecutor {
    pub fn with_success_responses(responses: impl IntoIterator<Item = &'static str>) -> Self {
        Self::with_outputs(
            responses
                .into_iter()
                .map(Self::success_output)
                .collect::<Vec<_>>(),
        )
    }

    pub fn with_outputs(outputs: impl IntoIterator<Item = CommandOutput>) -> Self {
        Self {
            commands: Mutex::default(),
            response_queue: Mutex::new(outputs.into_iter().collect()),
        }
    }

    pub fn success_output(stdout: impl AsRef<[u8]>) -> CommandOutput {
        CommandOutput {
            stdout: stdout.as_ref().to_vec(),
            stderr: vec![],
            status: CommandExitStatus::Success,
            exit_code: Some(ExitCode::from(0)),
        }
    }

    pub fn failure_output(stderr: impl AsRef<[u8]>, exit_code: ExitCode) -> CommandOutput {
        CommandOutput {
            stdout: vec![],
            stderr: stderr.as_ref().to_vec(),
            status: CommandExitStatus::Failure,
            exit_code: Some(exit_code),
        }
    }

    pub fn clear(&self) {
        self.commands.lock().clear();
    }
}

#[async_trait]
impl CommandExecutor for RecordingCommandExecutor {
    async fn execute_command(
        &self,
        command: &str,
        _shell: &Shell,
        _current_directory_path: Option<&str>,
        _environment_variables: Option<HashMap<String, String>>,
        _execute_command_options: ExecuteCommandOptions,
    ) -> anyhow::Result<CommandOutput> {
        self.commands.lock().push(command.to_string());
        let output = self
            .response_queue
            .lock()
            .pop_front()
            .unwrap_or_else(|| Self::success_output("test"));
        Ok(output)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn supports_parallel_command_execution(&self) -> bool {
        false
    }
}
