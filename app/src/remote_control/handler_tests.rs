use tempfile::TempDir;
use warp_core::features::FeatureFlag;
use warpui::{App, EntityId, SingletonEntity, ViewHandle};

use super::*;
use crate::remote_control::protocol::MAX_CLI_AGENT_EVENT_BYTES;
use crate::terminal::agent_session_handoff::MoveStep;
use crate::terminal::cli_agent_sessions::{
    CLIAgentInputState, CLIAgentSession, CLIAgentSessionContext, CLIAgentSessionStatus,
    CLIAgentSessionsModel,
};
use crate::terminal::{CLIAgent, TerminalView};
use crate::workspace::Workspace;
use crate::workspace::view::tests::{initialize_app_for_move_session, mock_workspace};

const SESSION_ID: &str = "0b6c1234-ffff-4a4a-8b8b-0123456789ab";

fn focused_terminal_view(
    workspace: &ViewHandle<Workspace>,
    app: &mut App,
) -> ViewHandle<TerminalView> {
    workspace.update(app, |workspace, ctx| {
        workspace
            .focused_terminal_view(ctx)
            .expect("mock workspace has a focused terminal pane")
    })
}

/// Gives `terminal_view` a Claude session the hooks reported, and a local shell in `shell_cwd` when
/// there is one.
fn seed_claude_session(
    terminal_view: &ViewHandle<TerminalView>,
    shell_cwd: Option<&std::path::Path>,
    agent_cwd: &str,
    transcript_path: &str,
    app: &mut App,
) {
    if let Some(shell_cwd) = shell_cwd {
        terminal_view.update(app, |view, ctx| {
            view.set_active_local_session_for_test(shell_cwd, ctx)
        });
    }
    let terminal_view_id = terminal_view.id();
    app.update(|ctx| {
        CLIAgentSessionsModel::handle(ctx).update(ctx, |model, ctx| {
            model.set_session(
                terminal_view_id,
                CLIAgentSession {
                    agent: CLIAgent::Claude,
                    status: CLIAgentSessionStatus::InProgress,
                    session_context: CLIAgentSessionContext {
                        cwd: Some(agent_cwd.to_owned()),
                        session_id: Some(SESSION_ID.to_owned()),
                        transcript_path: Some(transcript_path.into()),
                        ..Default::default()
                    },
                    input_state: CLIAgentInputState::Closed,
                    should_auto_toggle_input: false,
                    listener: None,
                    plugin_version: None,
                    remote_host: None,
                    draft_text: None,
                    custom_command_prefix: None,
                    received_rich_notification: false,
                },
                ctx,
            );
        });
    });
}

fn move_status(terminal_view_id: EntityId, app: &mut App) -> String {
    move_state(terminal_view_id, app).0
}

/// The step and the pushed handoff branch the instance reports for `terminal_view_id`'s move.
fn move_state(terminal_view_id: EntityId, app: &mut App) -> (String, Option<String>) {
    match app.update(|ctx| {
        handle(
            Request::MoveStatus {
                terminal_view_id: terminal_view_id.as_u64(),
            },
            ctx,
        )
    }) {
        Response::MoveState {
            step,
            pushed_branch,
        } => (step, pushed_branch),
        other => panic!("expected a move state, got {other:?}"),
    }
}

fn listed_pane(terminal_view_id: EntityId, app: &mut App) -> PaneSummary {
    let Response::Panes { panes } = app.update(|ctx| handle(Request::ListPanes, ctx)) else {
        panic!("expected panes");
    };
    panes
        .iter()
        .find(|pane| pane.terminal_view_id == Some(terminal_view_id.as_u64()))
        .cloned()
        .unwrap_or_else(|| panic!("the pane is listed: {panes:?}"))
}

#[test]
fn list_windows_includes_the_workspace_window() {
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let workspace = mock_workspace(&mut app);
        let workspace_window = app.read(|ctx| workspace.window_id(ctx));

        let Response::Windows { windows } = app.update(|ctx| handle(Request::ListWindows, ctx))
        else {
            panic!("expected windows");
        };

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].window_id, workspace_window.to_string());
        assert!(windows[0].has_workspace);
    });
}

#[test]
fn list_panes_reports_the_agent_session_of_each_pane() {
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let shell_cwd = TempDir::new().unwrap();
        let workspace = mock_workspace(&mut app);
        let terminal_view = focused_terminal_view(&workspace, &mut app);
        seed_claude_session(
            &terminal_view,
            Some(shell_cwd.path()),
            "/srv/app/sub",
            "/home/ben/.claude/projects/-srv-app/session.jsonl",
            &mut app,
        );

        let pane = listed_pane(terminal_view.id(), &mut app);
        assert_eq!(pane.agent.as_deref(), Some("claude"));
        assert_eq!(pane.agent_session_id.as_deref(), Some(SESSION_ID));
        assert_eq!(pane.agent_cwd.as_deref(), Some("/srv/app/sub"));
        assert_eq!(
            pane.transcript_path.as_deref(),
            Some("/home/ben/.claude/projects/-srv-app/session.jsonl")
        );
    });
}

#[test]
fn list_panes_leaves_agent_details_empty_without_an_agent() {
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let workspace = mock_workspace(&mut app);
        let terminal_view = focused_terminal_view(&workspace, &mut app);

        let pane = listed_pane(terminal_view.id(), &mut app);
        assert_eq!(pane.agent, None);
        assert_eq!(pane.agent_session_id, None);
        assert_eq!(pane.agent_cwd, None);
        assert_eq!(pane.transcript_path, None);
    });
}

#[test]
fn move_session_starts_a_move_for_a_qualifying_pane() {
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let shell_cwd = TempDir::new().unwrap();
        let workspace = mock_workspace(&mut app);
        let terminal_view = focused_terminal_view(&workspace, &mut app);
        let terminal_view_id = terminal_view.id();
        seed_claude_session(
            &terminal_view,
            Some(shell_cwd.path()),
            "/srv/app",
            "/tmp/session.jsonl",
            &mut app,
        );

        assert_eq!(move_status(terminal_view_id, &mut app), "none");

        // Asked in the same update as the request: the preparation it spawns has not been
        // polled yet, so the move is still at its first step.
        let (response, step, status) = app.update(|ctx| {
            let response = handle(
                Request::MoveSessionToAgentCtrl {
                    terminal_view_id: terminal_view_id.as_u64(),
                    cwd: None,
                },
                ctx,
            );
            let step = workspace.as_ref(ctx).agent_ctrl_move_step(terminal_view_id);
            let status = handle(
                Request::MoveStatus {
                    terminal_view_id: terminal_view_id.as_u64(),
                },
                ctx,
            );
            (response, step, status)
        });
        assert!(matches!(response, Response::MoveStarted), "{response:?}");
        assert_eq!(step, Some(MoveStep::Preparing));
        assert!(
            matches!(
                &status,
                Response::MoveState { step, pushed_branch: None } if step == "preparing"
            ),
            "{status:?}"
        );

        let again = app.update(|ctx| {
            handle(
                Request::MoveSessionToAgentCtrl {
                    terminal_view_id: terminal_view_id.as_u64(),
                    cwd: None,
                },
                ctx,
            )
        });
        assert!(
            matches!(&again, Response::Failed { error } if error.contains("already")),
            "a second move of the same pane must be refused: {again:?}"
        );
    });
}

/// A pane whose shell reports no directory -- a restored pane, or one whose directory is gone --
/// still moves using the agent's hook-reported directory.
#[test]
fn move_session_uses_the_agents_directory_when_the_pane_has_none() {
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let agent_cwd = TempDir::new().unwrap();
        let agent_cwd_text = agent_cwd.path().display().to_string();
        let workspace = mock_workspace(&mut app);
        let terminal_view = focused_terminal_view(&workspace, &mut app);
        let terminal_view_id = terminal_view.id();
        seed_claude_session(
            &terminal_view,
            None,
            &agent_cwd_text,
            "/tmp/session.jsonl",
            &mut app,
        );
        assert_eq!(
            listed_pane(terminal_view_id, &mut app).working_directory,
            ""
        );

        let (response, step) = app.update(|ctx| {
            let response = handle(
                Request::MoveSessionToAgentCtrl {
                    terminal_view_id: terminal_view_id.as_u64(),
                    cwd: None,
                },
                ctx,
            );
            (
                response,
                workspace.as_ref(ctx).agent_ctrl_move_step(terminal_view_id),
            )
        });
        assert!(matches!(response, Response::MoveStarted), "{response:?}");
        assert_eq!(step, Some(MoveStep::Preparing));
    });
}

/// A directory the agent never recorded is refused, so a caller on the pipe cannot choose which
/// repository gets pushed.
#[test]
fn move_session_refuses_a_directory_the_agent_did_not_record() {
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let agent_cwd = TempDir::new().unwrap();
        let other_dir = TempDir::new().unwrap();
        let workspace = mock_workspace(&mut app);
        let terminal_view = focused_terminal_view(&workspace, &mut app);
        let terminal_view_id = terminal_view.id();
        seed_claude_session(
            &terminal_view,
            None,
            &agent_cwd.path().display().to_string(),
            "/tmp/session.jsonl",
            &mut app,
        );

        let (response, step) = app.update(|ctx| {
            let response = handle(
                Request::MoveSessionToAgentCtrl {
                    terminal_view_id: terminal_view_id.as_u64(),
                    cwd: Some(other_dir.path().display().to_string()),
                },
                ctx,
            );
            (
                response,
                workspace.as_ref(ctx).agent_ctrl_move_step(terminal_view_id),
            )
        });
        assert!(matches!(response, Response::Failed { .. }), "{response:?}");
        assert_eq!(step, None);
    });
}

#[test]
fn move_session_fails_for_a_pane_without_an_agent_session() {
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let workspace = mock_workspace(&mut app);
        let terminal_view_id = focused_terminal_view(&workspace, &mut app).id();

        for requested in [terminal_view_id, EntityId::new()] {
            let response = app.update(|ctx| {
                handle(
                    Request::MoveSessionToAgentCtrl {
                        terminal_view_id: requested.as_u64(),
                        cwd: Some("/srv/app".to_owned()),
                    },
                    ctx,
                )
            });
            assert!(matches!(response, Response::Failed { .. }), "{response:?}");
            assert_eq!(move_status(requested, &mut app), "none");
        }
    });
}

#[test]
fn move_steps_have_stable_labels() {
    let job_id = "job-1".to_owned();
    let cases = [
        (Some(MoveStep::Preparing), "preparing"),
        (Some(MoveStep::Importing), "importing"),
        (
            Some(MoveStep::WaitingForResume {
                job_id: job_id.clone(),
            }),
            "waiting_for_resume",
        ),
        (
            Some(MoveStep::CuttingOver {
                job_id: job_id.clone(),
                pushed_branch: "gitea/handoff/x".to_owned(),
            }),
            "cutting_over",
        ),
        (
            Some(MoveStep::Done {
                job_id,
                pushed_branch: "gitea/handoff/x".to_owned(),
            }),
            "done:job-1",
        ),
        (
            Some(MoveStep::Failed {
                reason: "push rejected".to_owned(),
                pushed_branch: None,
            }),
            "failed:push rejected",
        ),
        (None, "none"),
    ];
    for (step, label) in cases {
        assert_eq!(move_step_label(step.as_ref()), label);
    }
}

#[test]
fn move_status_reports_the_move_of_a_pane_closed_mid_move() {
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let workspace = mock_workspace(&mut app);
        // A terminal view in no tab of any window, as a pane closed during its move is.
        let closed_view_id = EntityId::new();

        workspace.update(&mut app, |workspace, _| {
            workspace.set_agent_ctrl_move_step_for_test(
                closed_view_id,
                MoveStep::CuttingOver {
                    job_id: "job-1".to_owned(),
                    pushed_branch: "gitea/handoff/x".to_owned(),
                },
            )
        });
        assert_eq!(move_status(closed_view_id, &mut app), "cutting_over");

        workspace.update(&mut app, |workspace, ctx| {
            workspace.finish_agent_ctrl_move_for_test(
                closed_view_id,
                MoveStep::Done {
                    job_id: "job-1".to_owned(),
                    pushed_branch: "gitea/handoff/x".to_owned(),
                },
                ctx,
            )
        });
        assert_eq!(
            move_state(closed_view_id, &mut app),
            ("done:job-1".to_owned(), Some("gitea/handoff/x".to_owned()))
        );
    });
}

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const KIMI_SESSION_ID: &str = "session_0b6c1234-ffff-4a4a-8b8b-0123456789ab";

fn deliver(token: &str, notification: &str, app: &mut App) -> Response {
    app.update(|ctx| {
        handle(
            Request::CliAgentEvent {
                token: token.to_owned().into(),
                notification: notification.to_owned(),
            },
            ctx,
        )
    })
}

#[test]
fn cli_agent_event_reaches_the_pane_that_owns_the_token() {
    let _flag = FeatureFlag::PluggableNotifications.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let workspace = mock_workspace(&mut app);
        let terminal_view = focused_terminal_view(&workspace, &mut app);
        terminal_view.update(&mut app, |view, _| {
            view.set_agent_event_token(TOKEN.to_owned())
        });

        let response = deliver(
            TOKEN,
            &format!(
                r#"{{"v":1,"agent":"kimi","event":"session_start","session_id":"{KIMI_SESSION_ID}","cwd":"/srv/app","transcript_path":"/home/ben/.kimi-code/sessions/wd_app_0123456789ab/{KIMI_SESSION_ID}"}}"#
            ),
            &mut app,
        );
        assert!(matches!(response, Response::Done), "{response:?}");

        let response = deliver(
            TOKEN,
            r#"{"v":1,"agent":"kimi","event":"prompt_submit","query":"fix the build"}"#,
            &mut app,
        );
        assert!(matches!(response, Response::Done), "{response:?}");

        app.read(|ctx| {
            let session = CLIAgentSessionsModel::as_ref(ctx)
                .session(terminal_view.id())
                .expect("the event created an agent session");
            assert_eq!(session.agent, CLIAgent::Kimi);
            assert_eq!(
                session.session_context.session_id.as_deref(),
                Some(KIMI_SESSION_ID)
            );
            assert_eq!(session.session_context.cwd.as_deref(), Some("/srv/app"));
            assert_eq!(
                session.session_context.query.as_deref(),
                Some("fix the build")
            );
            assert_eq!(session.status, CLIAgentSessionStatus::InProgress);
            assert!(session.received_rich_notification);
        });
    });
}

#[test]
fn cli_agent_event_with_an_unknown_token_is_refused() {
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let workspace = mock_workspace(&mut app);
        let terminal_view = focused_terminal_view(&workspace, &mut app);
        terminal_view.update(&mut app, |view, _| {
            view.set_agent_event_token(TOKEN.to_owned())
        });

        let response = deliver(
            "ffffffffffffffffffffffffffffffff",
            r#"{"v":1,"agent":"kimi","event":"stop"}"#,
            &mut app,
        );
        assert!(matches!(response, Response::Failed { .. }), "{response:?}");
        app.read(|ctx| {
            assert!(
                CLIAgentSessionsModel::as_ref(ctx)
                    .session(terminal_view.id())
                    .is_none()
            );
        });
    });
}

#[test]
fn cli_agent_event_without_any_tokened_pane_is_refused() {
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let _workspace = mock_workspace(&mut app);
        let response = deliver("", r#"{"v":1,"agent":"kimi","event":"stop"}"#, &mut app);
        assert!(matches!(response, Response::Failed { .. }), "{response:?}");
    });
}

#[test]
fn cli_agent_event_is_refused_while_pluggable_notifications_are_off() {
    let _flag = FeatureFlag::PluggableNotifications.override_enabled(false);
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let workspace = mock_workspace(&mut app);
        let terminal_view = focused_terminal_view(&workspace, &mut app);
        terminal_view.update(&mut app, |view, _| {
            view.set_agent_event_token(TOKEN.to_owned())
        });

        let response = deliver(TOKEN, r#"{"v":1,"agent":"kimi","event":"stop"}"#, &mut app);

        assert!(matches!(response, Response::Failed { .. }), "{response:?}");
        app.read(|ctx| {
            assert!(
                CLIAgentSessionsModel::as_ref(ctx)
                    .session(terminal_view.id())
                    .is_none()
            );
        });
    });
}

#[test]
fn oversized_cli_agent_event_is_refused() {
    let _flag = FeatureFlag::PluggableNotifications.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let workspace = mock_workspace(&mut app);
        let terminal_view = focused_terminal_view(&workspace, &mut app);
        terminal_view.update(&mut app, |view, _| {
            view.set_agent_event_token(TOKEN.to_owned())
        });
        let query = "x".repeat(MAX_CLI_AGENT_EVENT_BYTES);
        let notification =
            format!(r#"{{"v":1,"agent":"kimi","event":"prompt_submit","query":"{query}"}}"#);

        let response = deliver(TOKEN, &notification, &mut app);

        assert!(matches!(response, Response::Failed { .. }), "{response:?}");
        app.read(|ctx| {
            assert!(
                CLIAgentSessionsModel::as_ref(ctx)
                    .session(terminal_view.id())
                    .is_none()
            );
        });
    });
}
