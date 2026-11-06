use std::cell::Cell;
use std::rc::Rc;

use warp::integration_testing::pane_group::assert_focused_pane_index;
use warp::integration_testing::step::new_step_with_default_assertions;
use warp::integration_testing::terminal::{
    execute_long_running_command_for_pane, wait_until_bootstrapped_pane,
    wait_until_bootstrapped_single_pane_for_tab,
};
use warp::integration_testing::view_getters::{pane_group_view, terminal_view};
use warp::pane_group::{Direction, PaneGroupAction};
use warpui_core::async_assert;
use warpui_core::integration::TestStep;

use crate::Builder;

pub fn test_maximize_pane_updates_terminal_size_without_window_resize() -> Builder {
    let split_columns = Rc::new(Cell::new(0));
    let capture_columns = split_columns.clone();
    let expanded_columns = split_columns.clone();
    Builder::new()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(
            new_step_with_default_assertions("Split a second pane")
                .with_action(|app, window_id, _| {
                    pane_group_view(app, window_id, 0).update(app, |_, ctx| {
                        ctx.dispatch_typed_action_deferred(PaneGroupAction::Add(Direction::Right));
                    });
                })
                .add_assertion(assert_focused_pane_index(0, 1)),
        )
        .with_step(wait_until_bootstrapped_pane(0, 1))
        .with_step(execute_long_running_command_for_pane(
            0,
            1,
            "python3 -u -c 'import os,signal,time; report=lambda *args: print(\"SIZE=\"+str(os.get_terminal_size().columns),flush=True); signal.signal(signal.SIGWINCH,report); report(); time.sleep(120)'",
        ))
        .with_step(
            TestStep::new("Record split viewport width").add_named_assertion(
                "split pane has a nonzero viewport",
                move |app, window_id| {
                    let columns = terminal_view(app, window_id, 0, 1)
                        .read(app, |view, _| view.size_info().columns());
                    capture_columns.set(columns);
                    async_assert!(columns > 0, "expected nonzero split columns, got {columns}")
                },
            ),
        )
        .with_step(
            TestStep::new("Maximize without resizing the window")
                .with_action(|app, window_id, _| {
                    pane_group_view(app, window_id, 0).update(app, |_, ctx| {
                        ctx.dispatch_typed_action_deferred(PaneGroupAction::ToggleMaximizePane);
                    });
                })
                .add_named_assertion("maximized viewport grows", move |app, window_id| {
                    let columns = terminal_view(app, window_id, 0, 1)
                        .read(app, |view, _| view.size_info().columns());
                    let output = terminal_view(app, window_id, 0, 1).read(app, |view, _| {
                        view.model.lock().block_list().active_block().output_to_string()
                    });
                    async_assert!(
                        columns > expanded_columns.get()
                            && output.trim_end().ends_with(&format!("SIZE={columns}")),
                        "expected grown viewport and child SIGWINCH report; columns={columns}, split={}, output={output:?}",
                        expanded_columns.get()
                    )
                }),
        )
        .with_step(
            TestStep::new("Restore without resizing the window")
                .with_action(|app, window_id, _| {
                    pane_group_view(app, window_id, 0).update(app, |_, ctx| {
                        ctx.dispatch_typed_action_deferred(PaneGroupAction::ToggleMaximizePane);
                    });
                })
                .add_named_assertion(
                    "restored viewport returns to split width",
                    move |app, window_id| {
                        let columns = terminal_view(app, window_id, 0, 1)
                            .read(app, |view, _| view.size_info().columns());
                        let output = terminal_view(app, window_id, 0, 1).read(app, |view, _| {
                            view.model.lock().block_list().active_block().output_to_string()
                        });
                        async_assert!(
                            columns == split_columns.get()
                                && output.trim_end().ends_with(&format!("SIZE={columns}")),
                            "expected {} columns and child SIGWINCH report, got {columns}, output={output:?}",
                            split_columns.get()
                        )
                    },
                ),
        )
        .with_step(TestStep::new("Stop the resize probe").with_keystrokes(&["ctrl-c"]))
}
