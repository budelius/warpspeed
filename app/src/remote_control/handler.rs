//! Answering a `sessions` request with the app's own state.
//!
//! Everything in here runs on the main thread with a real `AppContext`, which
//! is why the answers are worth trusting: a detach reports success only once
//! the supervisor has confirmed it owns the session, and a pane listing is the
//! same traversal the exit path uses to decide what survives.

use warpui::AppContext;

use super::protocol::{PaneSummary, Request, Response, StoredSessionInfo};

/// Carries out one request.
pub(super) fn handle(request: Request, ctx: &mut AppContext) -> Response {
    match request {
        Request::Ping => Response::Pong {
            pid: std::process::id(),
            version: super::build_identity(),
        },
        Request::ListPanes => list_panes(ctx),
        Request::DetachSession { session_id } => detach(session_id.into(), ctx),
        Request::AttachSession { session_id } => attach(session_id.into(), ctx),
        Request::ListStoredSessions => list_stored_sessions(),
        Request::OpenStoredSession { name } => open_stored_session(&name, ctx),
        Request::Quit { close_windows } => quit(close_windows, ctx),
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
