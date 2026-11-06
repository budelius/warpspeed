//! Answering a `sessions` request with the app's own state.
//!
//! Everything in here runs on the main thread with a real `AppContext`, which
//! is why the answers are worth trusting: a detach reports success only once
//! the supervisor has confirmed it owns the session, and a pane listing is the
//! same traversal the exit path uses to decide what survives.

use std::path::PathBuf;

use warpui::AppContext;

use super::protocol::{
    DetachedSessionSummary, PaneSummary, Request, Response, StoredSessionInfo, WindowSummary,
};

/// Carries out one request.
pub(super) fn handle(request: Request, ctx: &mut AppContext) -> Response {
    match request {
        Request::Ping => Response::Pong {
            pid: std::process::id(),
            version: super::build_identity(),
        },
        Request::ListPanes => list_panes(ctx),
        Request::ListWindows => list_windows(ctx),
        Request::ListDetachedSessions => list_detached_sessions(ctx),
        Request::DetachSession { session_id } => detach(session_id.into(), ctx),
        Request::AttachSession { session_id } => attach(session_id.into(), ctx),
        Request::ListStoredSessions => list_stored_sessions(),
        Request::OpenStoredSession { name } => open_stored_session(&name, ctx),
        Request::Quit { close_windows } => quit(close_windows, ctx),
        Request::MoveSessionToAgentCtrl {
            terminal_view_id,
            cwd,
        } => move_session_to_agent_ctrl(terminal_view_id, cwd.map(PathBuf::from), ctx),
        Request::MoveStatus { terminal_view_id } => move_status(terminal_view_id, ctx),
        #[cfg(feature = "local_tty")]
        Request::CliAgentEvent {
            token,
            notification,
        } => deliver_cli_agent_event(token.as_str(), notification, ctx),
        #[cfg(not(feature = "local_tty"))]
        Request::CliAgentEvent { .. } => Response::Failed {
            error: "This build has no local terminals".to_owned(),
        },
    }
}

fn list_stored_sessions() -> Response {
    let sessions = crate::launch_configs::session_store::list_summaries()
        .iter()
        .map(|summary| StoredSessionInfo {
            name: summary.name().to_owned(),
            autosaved: summary.is_current(),
            saved_at_unix_seconds: summary.saved_at_unix_seconds(),
            tabs: summary.tabs().count(),
            panes: summary.pane_count(),
            agents: summary.agent_count(),
        })
        .collect();
    Response::StoredSessions { sessions }
}

/// Opens the newest autosave of the session called `name`, or its newest stored snapshot when
/// it has no autosave.
fn open_stored_session(name: &str, ctx: &mut AppContext) -> Response {
    let summaries = crate::launch_configs::session_store::list_summaries();
    let named = || {
        summaries
            .iter()
            .filter(|summary| summary.name().eq_ignore_ascii_case(name))
    };
    let Some(summary) = named()
        .find(|summary| summary.is_current())
        .or_else(|| named().next())
    else {
        return Response::Failed {
            error: format!("No saved session is called {name:?}"),
        };
    };
    ctx.dispatch_global_action(
        "root_view:open_stored_session",
        &crate::root_view::OpenStoredSessionArg {
            summary: std::sync::Arc::new(summary.clone()),
            ui_location: crate::server::telemetry::LaunchConfigUiLocation::Uri,
        },
    );
    Response::Done
}

#[cfg(feature = "local_tty")]
fn list_panes(ctx: &mut AppContext) -> Response {
    use warpui::SingletonEntity;

    let workspaces = crate::workspace::WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx);
    let mut panes = Vec::new();
    for (window_id, workspace) in workspaces {
        panes.extend(
            workspace
                .as_ref(ctx)
                .terminal_panes(ctx)
                .into_iter()
                .map(|pane| PaneSummary {
                    session_id: pane.session_id.map(|session_id| session_id.as_u64()),
                    window: window_id.to_string(),
                    tab_index: pane.tab_index,
                    title: pane.title,
                    shell: pane.shell,
                    working_directory: pane.working_directory,
                    has_running_agent: pane.has_running_agent,
                    is_busy: pane.is_busy,
                    survives_quit: pane.survives_quit,
                    terminal_view_id: Some(pane.terminal_view_id.as_u64()),
                    agent: pane.agent,
                    agent_session_id: pane.agent_session_id,
                    agent_cwd: pane.agent_cwd,
                    transcript_path: pane.transcript_path,
                    agent_not_resumed: pane.agent_not_resumed,
                }),
        );
    }
    Response::Panes { panes }
}

#[cfg(not(feature = "local_tty"))]
fn list_panes(_ctx: &mut AppContext) -> Response {
    Response::Failed {
        error: "This build has no local terminals".to_owned(),
    }
}

fn list_windows(ctx: &mut AppContext) -> Response {
    use std::collections::HashSet;

    use warpui::SingletonEntity;

    let active_window = ctx.windows().active_window();
    let workspace_windows = crate::workspace::WorkspaceRegistry::as_ref(ctx)
        .all_workspaces(ctx)
        .into_iter()
        .map(|(window_id, _)| window_id)
        .collect::<HashSet<_>>();
    let windows = ctx
        .window_ids()
        .enumerate()
        .map(|(index, window_id)| WindowSummary {
            window_id: window_id.to_string(),
            index: index as u32,
            is_active: active_window == Some(window_id),
            has_workspace: workspace_windows.contains(&window_id),
        })
        .collect();
    Response::Windows { windows }
}

fn list_detached_sessions(ctx: &mut AppContext) -> Response {
    match crate::terminal::detached_sessions::list_checked(ctx) {
        Ok(sessions) => Response::DetachedSessions {
            sessions: sessions
                .into_iter()
                .map(|session| DetachedSessionSummary {
                    session_id: session.session_id.as_u64(),
                    title: session.title,
                    working_directory: session.cwd.to_string_lossy().into_owned(),
                    detached_for_seconds: session.detached_for_seconds,
                    shell_pid: session.shell_pid,
                    session_uuid_hex: session.session_uuid_hex,
                })
                .collect(),
        },
        Err(error) => Response::Failed {
            error: format!("{error:?}"),
        },
    }
}

/// Detaches a session, whichever window is holding it.
///
/// Searching every window rather than requiring the caller to name one is
/// deliberate: a session id already identifies exactly one pane on the machine,
/// so making the caller also work out which window it is in would be asking for
/// something it can only get wrong.
#[cfg(feature = "local_tty")]
fn detach(session_id: warp_core::SessionId, ctx: &mut AppContext) -> Response {
    use warpui::SingletonEntity;

    let workspaces = crate::workspace::WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx);
    for (_, workspace) in workspaces {
        let holds_it = workspace
            .as_ref(ctx)
            .terminal_panes(ctx)
            .iter()
            .any(|pane| pane.session_id == Some(session_id));
        if !holds_it {
            continue;
        }
        return match workspace.update(ctx, |workspace, ctx| {
            workspace.detach_session_by_id(session_id, ctx)
        }) {
            Ok(()) => Response::Done,
            Err(error) => Response::Failed {
                error: format!("{error:#}"),
            },
        };
    }
    Response::Failed {
        error: format!(
            "No pane in this instance holds session {}",
            session_id.as_u64()
        ),
    }
}

#[cfg(not(feature = "local_tty"))]
fn detach(_session_id: warp_core::SessionId, _ctx: &mut AppContext) -> Response {
    Response::Failed {
        error: "This build has no local terminals".to_owned(),
    }
}

/// Attaches a session into a new tab of whichever window is active.
#[cfg(feature = "local_tty")]
fn attach(session_id: warp_core::SessionId, ctx: &mut AppContext) -> Response {
    use warpui::SingletonEntity;

    let active = ctx.windows().active_window();
    let workspaces = crate::workspace::WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx);
    let Some((_, workspace)) = workspaces
        .iter()
        .find(|(window_id, _)| Some(*window_id) == active)
        .or_else(|| workspaces.first())
        .cloned()
    else {
        return Response::Failed {
            error: "This Warp instance has no window to attach into".to_owned(),
        };
    };

    match workspace.update(ctx, |workspace, ctx| {
        workspace.attach_session_by_id(session_id, ctx)
    }) {
        Ok(()) => Response::Done,
        Err(error) => Response::Failed {
            error: format!("{error:#}"),
        },
    }
}

#[cfg(not(feature = "local_tty"))]
fn attach(_session_id: warp_core::SessionId, _ctx: &mut AppContext) -> Response {
    Response::Failed {
        error: "This build has no local terminals".to_owned(),
    }
}

/// The workspace holding the terminal view `terminal_view_id`, in whichever window it is.
#[cfg(all(feature = "local_fs", feature = "local_tty"))]
fn workspace_holding_terminal_view(
    terminal_view_id: warpui::EntityId,
    ctx: &AppContext,
) -> Option<warpui::ViewHandle<crate::workspace::Workspace>> {
    use warpui::SingletonEntity;

    crate::workspace::WorkspaceRegistry::as_ref(ctx)
        .all_workspaces(ctx)
        .into_iter()
        .map(|(_, workspace)| workspace)
        .find(|workspace| {
            workspace
                .as_ref(ctx)
                .workspace_contains_terminal_view(terminal_view_id, ctx)
        })
}

#[cfg(all(feature = "local_fs", feature = "local_tty"))]
fn terminal_view_entity_id(terminal_view_id: u64) -> Option<warpui::EntityId> {
    usize::try_from(terminal_view_id)
        .ok()
        .map(warpui::EntityId::from_usize)
}

/// Starts moving a pane's agent session to AgentCtrl, whichever window holds the pane.
#[cfg(all(feature = "local_fs", feature = "local_tty"))]
fn move_session_to_agent_ctrl(
    terminal_view_id: u64,
    fallback_cwd: Option<PathBuf>,
    ctx: &mut AppContext,
) -> Response {
    let held = terminal_view_entity_id(terminal_view_id).and_then(|view_id| {
        workspace_holding_terminal_view(view_id, ctx).map(|workspace| (view_id, workspace))
    });
    let Some((view_id, workspace)) = held else {
        return Response::Failed {
            error: format!("No pane in this instance has terminal view {terminal_view_id}"),
        };
    };
    match workspace.update(ctx, |workspace, ctx| {
        workspace.move_session_to_agent_ctrl_unattended(view_id, fallback_cwd, ctx)
    }) {
        Ok(()) => Response::MoveStarted,
        Err(error) => Response::Failed { error },
    }
}

#[cfg(not(all(feature = "local_fs", feature = "local_tty")))]
fn move_session_to_agent_ctrl(_: u64, _: Option<PathBuf>, _: &mut AppContext) -> Response {
    Response::Failed {
        error: "This build cannot move sessions to AgentCtrl".to_owned(),
    }
}

#[cfg(all(feature = "local_fs", feature = "local_tty"))]
fn move_status(terminal_view_id: u64, ctx: &mut AppContext) -> Response {
    use warpui::SingletonEntity;

    // Looked up in every workspace's moves rather than through the workspace holding the pane:
    // a pane closed mid-move is in no tab, yet its move carries on to an outcome.
    let step = terminal_view_entity_id(terminal_view_id).and_then(|view_id| {
        crate::workspace::WorkspaceRegistry::as_ref(ctx)
            .all_workspaces(ctx)
            .into_iter()
            .find_map(|(_, workspace)| workspace.as_ref(ctx).agent_ctrl_move_step(view_id))
    });
    Response::MoveState {
        step: move_step_label(step.as_ref()),
        pushed_branch: step.and_then(move_step_pushed_branch),
    }
}

#[cfg(not(all(feature = "local_fs", feature = "local_tty")))]
fn move_status(_terminal_view_id: u64, _ctx: &mut AppContext) -> Response {
    Response::Failed {
        error: "This build cannot move sessions to AgentCtrl".to_owned(),
    }
}

/// The wire form of a move's step, which `sessions` callers match on.
#[cfg(all(feature = "local_fs", feature = "local_tty"))]
fn move_step_label(step: Option<&crate::terminal::agent_session_handoff::MoveStep>) -> String {
    use crate::terminal::agent_session_handoff::MoveStep;

    match step {
        None => "none".to_owned(),
        Some(MoveStep::Preparing) => "preparing".to_owned(),
        Some(MoveStep::Importing) => "importing".to_owned(),
        Some(MoveStep::WaitingForResume { .. }) => "waiting_for_resume".to_owned(),
        Some(MoveStep::CuttingOver { .. }) => "cutting_over".to_owned(),
        Some(MoveStep::Done { job_id, .. }) => format!("done:{job_id}"),
        Some(MoveStep::Failed { reason, .. }) => format!("failed:{reason}"),
    }
}

#[cfg(all(feature = "local_fs", feature = "local_tty"))]
fn move_step_pushed_branch(
    step: crate::terminal::agent_session_handoff::MoveStep,
) -> Option<String> {
    use crate::terminal::agent_session_handoff::MoveStep;

    match step {
        MoveStep::Preparing | MoveStep::Importing | MoveStep::WaitingForResume { .. } => None,
        MoveStep::CuttingOver { pushed_branch, .. } | MoveStep::Done { pushed_branch, .. } => {
            pushed_branch
        }
        MoveStep::Failed { pushed_branch, .. } => pushed_branch,
    }
}

/// Hands an out-of-band agent event to the pane whose shell carries `token`.
#[cfg(feature = "local_tty")]
fn deliver_cli_agent_event(token: &str, notification: String, ctx: &mut AppContext) -> Response {
    use warp_core::features::FeatureFlag;
    use warpui::SingletonEntity;

    use super::protocol::MAX_CLI_AGENT_EVENT_BYTES;

    // The same events written as OSC 777 are dropped with this flag off.
    if !FeatureFlag::PluggableNotifications.is_enabled() {
        return Response::Failed {
            error: "This build does not accept CLI agent notifications".to_owned(),
        };
    }
    if notification.len() > MAX_CLI_AGENT_EVENT_BYTES {
        return Response::Failed {
            error: format!("The notification is larger than {MAX_CLI_AGENT_EVENT_BYTES} bytes"),
        };
    }
    let view = (!token.is_empty())
        .then(|| {
            crate::workspace::WorkspaceRegistry::as_ref(ctx)
                .all_workspaces(ctx)
                .into_iter()
                .find_map(|(_, workspace)| {
                    workspace
                        .as_ref(ctx)
                        .terminal_view_with_agent_event_token(token, ctx)
                })
        })
        .flatten();
    let Some(view) = view else {
        return Response::Failed {
            error: "No pane in this instance owns that agent-event token".to_owned(),
        };
    };
    view.update(ctx, |view, ctx| {
        view.receive_out_of_band_cli_agent_event(notification, ctx)
    });
    Response::Done
}

/// Quits, the way a user quitting would.
///
/// Closing the windows takes `ForceTerminate`, which skips only the "are you
/// sure" prompt -- everything past that point, the per-window teardown this
/// exists to exercise included, runs exactly as it does for a click on the
/// close button. A prompt would leave a caller waiting on a dialog nobody is
/// there to answer.
fn quit(close_windows: bool, ctx: &mut AppContext) -> Response {
    use warpui::platform::TerminationMode;

    if !close_windows {
        ctx.terminate_app(TerminationMode::Cancellable, None);
        return Response::Done;
    }

    use warpui::SingletonEntity;
    use warpui::windowing::WindowManager;

    let window_ids: Vec<_> = ctx.window_ids().collect();
    if window_ids.is_empty() {
        return Response::Failed {
            error: "This Warp instance has no windows to close".to_owned(),
        };
    }
    for window_id in window_ids {
        WindowManager::as_ref(ctx).close_window(window_id, TerminationMode::ForceTerminate);
    }
    Response::Done
}

#[cfg(all(test, feature = "local_fs", feature = "local_tty"))]
#[path = "handler_tests.rs"]
mod tests;
