//! Detaching a terminal session and attaching it back.
//!
//! Every unit test around this feature stops at a seam. The supervisor's own
//! tests drive it in-process against a fake host; the app's tests never start
//! one at all. What none of them covers is the part a user actually performs:
//! a pane handed to a real supervisor *process*, the pane closed, and the same
//! session picked back up in a new one.
//!
//! The identity asserted throughout is the persistent session id -- what the
//! supervisor keys its table on, what the picker offers, and what an attach
//! names. If that id comes back attached to a shell that never stopped running,
//! the round trip did what it claims.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use warp::cmd_or_ctrl_shift;
use warp::features::FeatureFlag;
use warp::integration_testing::step::new_step_with_default_assertions;
use warp::integration_testing::terminal::{
    execute_long_running_command_for_pane, wait_until_bootstrapped_pane,
    wait_until_bootstrapped_single_pane_for_tab,
};
use warp::integration_testing::view_getters::{pane_group_view, workspace_view};
use warp::integration_testing::window::{add_and_save_window, close_window, save_active_window_id};
use warp::integration_testing::workspace::assert_tab_count;
use warp::workspace::WorkspaceAction;
use warpui_core::integration::TestStep;
use warpui_core::{App, WindowId, async_assert};

use super::{Builder, new_builder};

/// The supervisor session behind the pane at `(tab_index, pane_index)`.
///
/// `None` means the pane's shell is this process's own child: the supervisor was
/// unreachable and the spawn fell back. There is nothing to detach in that case,
/// which is why the first step asserts on it rather than letting a later step
/// fail for a confusing reason.
fn pane_session(
    app: &App,
    window_id: WindowId,
    tab_index: usize,
    pane_index: usize,
) -> Option<u64> {
    pane_group_view(app, window_id, tab_index).read(app, |pane_group, ctx| {
        pane_group
            .terminal_manager(pane_index, ctx)
            .and_then(|manager| manager.as_ref(ctx).persistent_session_id())
            .map(|session_id| session_id.as_u64())
    })
}

/// Detach a session, then attach it back, and get the same shell.
///
/// The pane detached is a split rather than the tab's only one, so closing it
/// leaves a window standing to attach into.
pub fn test_detach_and_attach_a_session() -> Builder {
    FeatureFlag::PersistentTerminalSessions.set_enabled(true);

    // Shared across steps because the id is only knowable at run time, and the
    // attach several steps later has to name the very session that was detached.
    let session = Rc::new(RefCell::new(None::<u64>));
    let for_capture = session.clone();
    let for_supervisor_check = session.clone();
    let for_attach = session.clone();
    let for_final_check = session.clone();

    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(
            new_step_with_default_assertions("Split off a pane to detach")
                .with_keystrokes(&[cmd_or_ctrl_shift("d")]),
        )
        .with_step(wait_until_bootstrapped_pane(0, 1))
        // Something has to still be running, or "the session came back" is
        // indistinguishable from a fresh shell that happens to look the same.
        .with_step(execute_long_running_command_for_pane(0, 1, "sleep 300"))
        .with_step(
            TestStep::new("Record the session the supervisor owns").add_named_assertion(
                "the pane's shell is supervisor-owned",
                move |app, window_id| {
                    let session_id = pane_session(app, window_id, 0, 1);
                    *for_capture.borrow_mut() = session_id;
                    async_assert!(
                        session_id.is_some(),
                        "the pane's shell must belong to the supervisor before it can be \
                         detached; no session id means the supervisor was unreachable and the \
                         spawn fell back to a child of this process"
                    )
                },
            ),
        )
        .with_step(
            TestStep::new("Detach the focused pane's session")
                .with_action(|app, window_id, _| {
                    let workspace = workspace_view(app, window_id);
                    app.update(|ctx| {
                        ctx.dispatch_typed_action_for_view(
                            window_id,
                            workspace.id(),
                            &WorkspaceAction::DetachSession,
                        );
                    });
                })
                .set_timeout(Duration::from_secs(30))
                .set_retries(2)
                .add_named_assertion("the detached pane is gone", |app, window_id| {
                    let panes = pane_group_view(app, window_id, 0)
                        .read(app, |pane_group, _| pane_group.visible_pane_ids().len());
                    async_assert!(
                        panes == 1,
                        "detaching must close the pane it handed over; {panes} panes remain"
                    )
                })
                .add_named_assertion("the supervisor is holding the session", {
                    move |app: &mut App, window_id: WindowId| {
                        let expected = for_supervisor_check
                            .borrow()
                            .expect("the session id was recorded in the previous step");
                        let held = workspace_view(app, window_id).read(app, |_, ctx| {
                            warp::terminal::detached_sessions::list(ctx)
                                .into_iter()
                                .any(|session| session.session_id.as_u64() == expected)
                        });
                        async_assert!(
                            held,
                            "the supervisor must list session {expected} as detached and \
                             attachable; a missing entry means the handover never reached it"
                        )
                    }
                }),
        )
        .with_step(
            TestStep::new("Attach the session into a new tab")
                .with_action(move |app, window_id, _| {
                    let session_id = for_attach
                        .borrow()
                        .expect("the session id was recorded before the detach");
                    let workspace = workspace_view(app, window_id);
                    app.update(|ctx| {
                        ctx.dispatch_typed_action_for_view(
                            window_id,
                            workspace.id(),
                            &WorkspaceAction::AttachDetachedSession {
                                session_id: session_id.into(),
                            },
                        );
                    });
                })
                .set_timeout(Duration::from_secs(30))
                .add_assertion(assert_tab_count(2)),
        )
        .with_step(
            // Deliberately not waiting for a bootstrap here, and that is a
            // known defect rather than a property of the design.
            //
            // An attached pane takes over a shell that bootstrapped long ago,
            // in a different Warp process. Bootstrap completion is signalled by
            // the shell's `InitShell` hook, which fires once, on a prompt that
            // has already been printed -- so the pane waits for a handshake
            // that is never coming and stays in `TerminalInputState::
            // NotBootstrapped` for good. The shell really is reattached and
            // really is running; the pane just refuses input, logging
            // `Tried to execute command but can_execute_command was false:
            // NotBootstrapped` for every keystroke.
            //
            // Until an attach can restore that state from what the supervisor
            // knows, the session id arriving is all this can assert.
            TestStep::new("The same session came back")
                .set_timeout(Duration::from_secs(30))
                .set_retries(2)
                .set_pause_on_failure(Duration::from_secs(30))
                .add_named_assertion(
                    "the new tab holds the session that was detached",
                    move |app, window_id| {
                        let expected = for_final_check
                            .borrow()
                            .expect("the session id was recorded before the detach");
                        let actual = pane_session(app, window_id, 1, 0);
                        async_assert!(
                            actual == Some(expected),
                            "the attached pane must hold the detached session {expected}, not \
                             {actual:?}; a different id means a fresh shell was started instead \
                             of the session being taken back"
                        )
                    },
                ),
        )
}

/// The key the window under test is saved under, so the close step can name it
/// after another window has taken over as active.
const CLOSING_WINDOW: &str = "the window that closes";

/// Closing a window with work in it keeps the work running.
///
/// This is the exit path, tested at the only place a test can reach it. On
/// Windows and Linux, quitting Warp *is* closing its windows: the event loop
/// ends once the last one has gone, and every window removes itself from the
/// workspace registry on the way out. The app's termination hook therefore runs
/// with nothing left to ask, which is why handing sessions over has to happen
/// as each window closes.
///
/// A second window is opened first so that closing the first one is a window
/// close and not the end of the process -- which would take the test with it.
/// The mechanism is the same either way: `Workspace::on_window_closed` runs
/// once per window, and the last one to run is the one that quits the app.
pub fn test_a_closing_window_hands_its_session_to_the_supervisor() -> Builder {
    FeatureFlag::PersistentTerminalSessions.set_enabled(true);

    let session = Rc::new(RefCell::new(None::<u64>));
    let for_capture = session.clone();
    let for_check = session.clone();

    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        // The rule is that a pane with work in it survives and an idle one does
        // not, so the pane has to be busy for this to be testing anything.
        .with_step(execute_long_running_command_for_pane(0, 0, "sleep 300"))
        .with_step(
            TestStep::new("Record the session that must survive the close")
                .add_named_assertion(
                    "the pane's shell is supervisor-owned",
                    move |app, window_id| {
                        let session_id = pane_session(app, window_id, 0, 0);
                        *for_capture.borrow_mut() = session_id;
                        async_assert!(
                            session_id.is_some(),
                            "the pane's shell must belong to the supervisor for closing the window                              to be able to keep it; no session id means the supervisor was                              unreachable and the spawn fell back to a child of this process"
                        )
                    },
                )
                .add_assertion(save_active_window_id(CLOSING_WINDOW)),
        )
        // Somewhere for the app to keep living once the busy window has gone.
        .with_step(add_and_save_window("the window that stays"))
        .with_step(close_window(CLOSING_WINDOW, 1))
        .with_step(
            TestStep::new("The supervisor kept the session")
                .set_timeout(Duration::from_secs(30))
                .set_retries(2)
                .add_named_assertion("the closed window's session is still held", {
                    move |app: &mut App, window_id: WindowId| {
                        let expected = for_check
                            .borrow()
                            .expect("the session id was recorded before the close");
                        let held = workspace_view(app, window_id).read(app, |_, ctx| {
                            warp::terminal::detached_sessions::list(ctx)
                                .into_iter()
                                .any(|session| session.session_id.as_u64() == expected)
                        });
                        async_assert!(
                            held,
                            "closing a window must hand its running sessions to the supervisor                              rather than killing them; session {expected} is gone, so quitting                              Warp would lose whatever was running in it"
                        )
                    }
                }),
        )
}
