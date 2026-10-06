use tempfile::TempDir;
use warpui::{App, EntityId, SingletonEntity, ViewHandle};

use super::*;
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

/// Gives `terminal_view` a local shell in `shell_cwd` and a Claude session the hooks reported.
fn seed_claude_session(
    terminal_view: &ViewHandle<TerminalView>,
    shell_cwd: &std::path::Path,
    agent_cwd: &str,
    transcript_path: &str,
    app: &mut App,
) {
    terminal_view.update(app, |view, ctx| {
        view.set_active_local_session_for_test(shell_cwd, ctx)
    });
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
    match app.update(|ctx| {
        handle(
            Request::MoveStatus {
                terminal_view_id: terminal_view_id.as_u64(),
            },
            ctx,
        )
    }) {
        Response::MoveState { step } => step,
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
fn list_panes_reports_the_agent_session_of_each_pane() {
    App::test((), |mut app| async move {
        initialize_app_for_move_session(&mut app);
        let shell_cwd = TempDir::new().unwrap();
        let workspace = mock_workspace(&mut app);
        let terminal_view = focused_terminal_view(&workspace, &mut app);
        seed_claude_session(
            &terminal_view,
            shell_cwd.path(),
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
            shell_cwd.path(),
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
            matches!(&status, Response::MoveState { step } if step == "preparing"),
            "{status:?}"
        );

        let again = app.update(|ctx| {
            handle(
                Request::MoveSessionToAgentCtrl {
                    terminal_view_id: terminal_view_id.as_u64(),
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
            }),
            "cutting_over",
        ),
        (Some(MoveStep::Done { job_id }), "done:job-1"),
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
