use futures_util::future::{AbortHandle, Abortable, Aborted};
use warpui::App;

use super::*;
use crate::terminal::model::session::ExecuteCommandOptions;

impl InBandCommandExecutor {
    /// Returns an `Option` containing the ID of the actively running command.
    pub fn running_command_id(&self) -> Option<String> {
        self.running_command
            .lock()
            .as_ref()
            .map(|command_info| command_info.id.clone())
    }

    /// Returns a `HashSet` containing IDs of all pending commands (i.e. commands that are queued
    /// for execution but not executing yet).
    pub fn pending_command_ids(&self) -> Vec<String> {
        Vec::from_iter(
            self.pending_commands
                .lock()
                .iter()
                .map(|command_info| &command_info.id)
                .cloned(),
        )
    }
}

/// Returns a closure that asserts the given `Result<CommandOutput>` is `Ok(..)` and contains
/// `CommandOutput` with the given `expected_output` and `expected_success` values.
fn assert_command_output_result_fn(
    expected_output: &'static str,
    expected_success: bool,
) -> impl FnOnce(Result<CommandOutput>) {
    move |result| {
        assert!(result.is_ok());

        let output = result.unwrap();
        assert_eq!(output.success(), expected_success);
        assert_eq!(output.output(), expected_output.as_bytes());
    }
}

async fn execute_test_command<F>(
    executor: Arc<InBandCommandExecutor>,
    command: &'static str,
    assert_result_fn: F,
) where
    F: FnOnce(Result<CommandOutput>) + Send + 'static,
{
    let shell = Shell::new(ShellType::Zsh, None, None, Default::default(), None);
    let test_command_result = executor
        .execute_command(
            command,
            &shell,
            /*current_directory_path=*/ None,
            /*environment_variables=*/ None,
            ExecuteCommandOptions::default(),
        )
        .await;

    assert_result_fn(test_command_result);
}

#[test]
fn test_emits_successful_command_output() {
    App::test((), |_app| async move {
        let (executor_command_tx, _) = async_channel::unbounded();
        let (in_band_command_cancelled_tx, _) = async_channel::unbounded();
        let executor = Arc::new(InBandCommandExecutor::new(
            executor_command_tx,
            in_band_command_cancelled_tx,
        ));

        // TODO(zachbai): Figure out how to make these tests work without
        // requiring a single-threaded executor (e.g.: running on
        // app.background_executor()).
        let task_executor = async_executor::LocalExecutor::new();

        let execute_command_future = task_executor.spawn(execute_test_command(
            executor.clone(),
            "echo foo",
            assert_command_output_result_fn("foo", true),
        ));
        let handle_command_output_future = task_executor.spawn(async move {
            let pending_command_ids = executor.pending_command_ids();
            assert!(pending_command_ids.is_empty());

            let test_command_id = executor
                .running_command_id()
                .expect("Executor should be running test command.");
            executor.handle_executed_command_event(ExecutedExecutorCommandEvent {
                command_id: test_command_id,
                exit_code: 0,
                output: "foo".as_bytes().to_vec(),
            });
        });

        task_executor
            .run(async move {
                execute_command_future.await;
                handle_command_output_future.await;
            })
            .await;
    });
}

#[test]
fn test_emits_error_command_output() {
    App::test((), |_app| async move {
        let (executor_command_tx, _) = async_channel::unbounded();
        let (in_band_command_cancelled_tx, _) = async_channel::unbounded();
        let executor = Arc::new(InBandCommandExecutor::new(
            executor_command_tx,
            in_band_command_cancelled_tx,
        ));

        // TODO(zachbai): Figure out how to make these tests work without
        // requiring a single-threaded executor (e.g.: running on
        // app.background_executor()).
        let task_executor = async_executor::LocalExecutor::new();

        let execute_command_future = task_executor.spawn(execute_test_command(
            executor.clone(),
            "echo bar",
            assert_command_output_result_fn("failed!", false),
        ));
        let handle_command_output_future = task_executor.spawn(async move {
            let pending_command_ids = executor.pending_command_ids();
            assert!(pending_command_ids.is_empty());

            let test_command_id = executor
                .running_command_id()
                .expect("Executor should be running test command.");
            executor.handle_executed_command_event(ExecutedExecutorCommandEvent {
                command_id: test_command_id,
                exit_code: 1,
                output: "failed!".as_bytes().to_vec(),
            });
        });

        task_executor
            .run(async move {
                execute_command_future.await;
                handle_command_output_future.await;
            })
            .await;
    });
}

#[test]
fn test_runs_commands_serially() {
    App::test((), |_app| async move {
        let (executor_command_tx, _) = async_channel::unbounded();
        let (in_band_command_cancelled_tx, _) = async_channel::unbounded();
        let executor = Arc::new(InBandCommandExecutor::new(
            executor_command_tx,
            in_band_command_cancelled_tx,
        ));

        // TODO(zachbai): Figure out how to make these tests work without
        // requiring a single-threaded executor (e.g.: running on
        // app.background_executor()).
        let task_executor = async_executor::LocalExecutor::new();

        let first_execute_command_future = task_executor.spawn(execute_test_command(
            executor.clone(),
            "echo foo",
            assert_command_output_result_fn("foo", true),
        ));
        let second_execute_command_future = task_executor.spawn(execute_test_command(
            executor.clone(),
            "echo bar",
            assert_command_output_result_fn("bar", true),
        ));

        let handle_command_output_future = task_executor.spawn(async move {
            // "echo bar" is pending.
            let pending_command_ids = executor.pending_command_ids();
            assert_eq!(pending_command_ids.len(), 1);

            // "echo foo" is running.
            let test_command_id = executor
                .running_command_id()
                .expect("Executor should be running test command.");
            executor.handle_executed_command_event(ExecutedExecutorCommandEvent {
                command_id: test_command_id,
                exit_code: 0,
                output: "foo".as_bytes().to_vec(),
            });

            // "echo bar" is now running.
            assert_eq!(
                pending_command_ids[0],
                executor.running_command_id().unwrap()
            );
            executor.handle_executed_command_event(ExecutedExecutorCommandEvent {
                command_id: pending_command_ids[0].clone(),
                exit_code: 0,
                output: "bar".as_bytes().to_vec(),
            });
        });

        task_executor
            .run(async move {
                first_execute_command_future.await;
                second_execute_command_future.await;
                handle_command_output_future.await;
            })
            .await;
    });
}

#[test]
fn test_runs_commands_serially_after_failed_command() {
    App::test((), |_app| async move {
        let (executor_command_tx, _) = async_channel::unbounded();
        let (in_band_command_cancelled_tx, _) = async_channel::unbounded();
        let executor = Arc::new(InBandCommandExecutor::new(
            executor_command_tx,
            in_band_command_cancelled_tx,
        ));

        // TODO(zachbai): Figure out how to make these tests work without
        // requiring a single-threaded executor (e.g.: running on
        // app.background_executor()).
        let task_executor = async_executor::LocalExecutor::new();

        let first_execute_command_future = task_executor.spawn(execute_test_command(
            executor.clone(),
            "echo foo",
            assert_command_output_result_fn("foo", true),
        ));
        let second_execute_command_future = task_executor.spawn(execute_test_command(
            executor.clone(),
            "echo bar",
            assert_command_output_result_fn("bar", false),
        ));
        let third_execute_command_future = task_executor.spawn(execute_test_command(
            executor.clone(),
            "echo baz",
            assert_command_output_result_fn("baz", true),
        ));

        let handle_command_output_future = task_executor.spawn(async move {
            // "echo bar" is pending.
            let pending_command_ids = executor.pending_command_ids();
            assert_eq!(pending_command_ids.len(), 2);

            // "echo foo" is running.
            let test_command_id = executor
                .running_command_id()
                .expect("Executor should be running test command.");
            executor.handle_executed_command_event(ExecutedExecutorCommandEvent {
                command_id: test_command_id,
                exit_code: 0,
                output: "foo".as_bytes().to_vec(),
            });

            // "echo bar" is now running.
            assert_eq!(
                pending_command_ids[0],
                executor.running_command_id().unwrap()
            );
            executor.handle_executed_command_event(ExecutedExecutorCommandEvent {
                command_id: pending_command_ids[0].clone(),
                exit_code: 1,
                output: "bar".as_bytes().to_vec(),
            });

            // "echo baz" is now running.
            assert_eq!(
                pending_command_ids[1],
                executor.running_command_id().unwrap()
            );
            executor.handle_executed_command_event(ExecutedExecutorCommandEvent {
                command_id: pending_command_ids[1].clone(),
                exit_code: 0,
                output: "baz".as_bytes().to_vec(),
            });
        });

        task_executor
            .run(async move {
                first_execute_command_future.await;
                second_execute_command_future.await;
                third_execute_command_future.await;
                handle_command_output_future.await;
            })
            .await;
    });
}

#[test]
fn test_clears_running_and_pending_commands_on_cancellation() {
    App::test((), |_app| async move {
        fn assert_failed(result: Result<CommandOutput>) {
            assert!(result.is_err());
        }

        let (executor_command_tx, _) = async_channel::unbounded();
        let (in_band_command_cancelled_tx, _) = async_channel::unbounded();
        let executor = Arc::new(InBandCommandExecutor::new(
            executor_command_tx,
            in_band_command_cancelled_tx,
        ));

        // TODO(zachbai): Figure out how to make these tests work without
        // requiring a single-threaded executor (e.g.: running on
        // app.background_executor()).
        let task_executor = async_executor::LocalExecutor::new();

        let first_execute_command_future = task_executor.spawn(execute_test_command(
            executor.clone(),
            "echo foo",
            assert_failed,
        ));
        let second_execute_command_future = task_executor.spawn(execute_test_command(
            executor.clone(),
            "echo bar",
            assert_failed,
        ));
        let third_execute_command_future = task_executor.spawn(execute_test_command(
            executor.clone(),
            "echo baz",
            assert_failed,
        ));

        let handle_command_output_future = task_executor.spawn(async move {
            executor.cancel_active_commands();

            assert_eq!(executor.running_command_id(), None);
            assert_eq!(executor.pending_command_ids().len(), 0);
        });

        task_executor
            .run(async move {
                first_execute_command_future.await;
                second_execute_command_future.await;
                third_execute_command_future.await;
                handle_command_output_future.await;
            })
            .await;
    });
}

#[test]
fn test_commands_are_cleared_if_execute_command_future_is_aborted() {
    App::test((), |_app| async move {
        fn assert_failed(result: Result<CommandOutput>) {
            assert!(result.is_err());
        }

        let (executor_command_tx, _) = async_channel::unbounded();
        let (in_band_command_cancelled_tx, _) = async_channel::unbounded();
        let executor = Arc::new(InBandCommandExecutor::new(
            executor_command_tx,
            in_band_command_cancelled_tx,
        ));

        // TODO(zachbai): Figure out how to make these tests work without
        // requiring a single-threaded executor (e.g.: running on
        // app.background_executor()).
        let task_executor = async_executor::LocalExecutor::new();

        let executor_clone = executor.clone();
        let execute_commands_future = async move {
            execute_test_command(executor_clone.clone(), "echo foo", assert_failed).await;
            execute_test_command(executor_clone.clone(), "echo bar", assert_failed).await;
            execute_test_command(executor_clone.clone(), "echo baz", assert_failed).await;
        };

        let (handle, registration) = AbortHandle::new_pair();
        let execute_command_future =
            task_executor.spawn(Abortable::new(execute_commands_future, registration));
        // Abort the future where the commands are executing.
        handle.abort();

        let handle_command_output_future = task_executor.spawn(async move {
            assert_eq!(executor.running_command_id(), None);
            assert_eq!(executor.pending_command_ids().len(), 0);
        });

        task_executor
            .run(async move {
                assert_eq!(execute_command_future.await, Err(Aborted));
                handle_command_output_future.await;
            })
            .await;
    });
}

#[test]
fn test_commands_are_cleared_if_controller_cancels_command() {
    App::test((), |_app| async move {
        let (executor_command_tx, _) = async_channel::unbounded();
        let (in_band_command_cancelled_tx, _) = async_channel::unbounded();
        let executor = Arc::new(InBandCommandExecutor::new(
            executor_command_tx,
            in_band_command_cancelled_tx,
        ));

        let task_executor = async_executor::LocalExecutor::new();

        let execute_command_future = task_executor.spawn(execute_test_command(
            executor.clone(),
            "echo foo",
            |result| {
                let output = result.expect("Expect result to be ok");
                assert_eq!(output.status, CommandExitStatus::Failure)
            },
        ));

        let cancel_command_future = task_executor.spawn(async move {
            let current_running_id = executor
                .running_command_id()
                .expect("There should be a running command");
            executor.handle_cancelled_in_band_command_event(InBandCommandCancelledEvent {
                command_id: current_running_id,
            });
            assert_eq!(executor.running_command_id(), None);
        });

        task_executor
            .run(async move {
                execute_command_future.await;
                cancel_command_future.await;
            })
            .await;
    });
}

#[test]
fn test_commands_are_not_cleared_if_controller_cancels_different_command() {
    App::test((), |_app| async move {
        let (executor_command_tx, _) = async_channel::unbounded();
        let (in_band_command_cancelled_tx, _) = async_channel::unbounded();
        let executor = Arc::new(InBandCommandExecutor::new(
            executor_command_tx,
            in_band_command_cancelled_tx,
        ));

        let task_executor = async_executor::LocalExecutor::new();

        let execute_command_future = task_executor.spawn(execute_test_command(
            executor.clone(),
            "echo foo",
            assert_command_output_result_fn("foo", true),
        ));

        let cancel_command_future = task_executor.spawn(async move {
            let current_running_id = executor
                .running_command_id()
                .expect("There should be a running command");

            let a_different_id = format!("{current_running_id}_but_not_really");
            executor.handle_cancelled_in_band_command_event(InBandCommandCancelledEvent {
                command_id: a_different_id,
            });
            assert!(
                executor
                    .running_command_id()
                    .is_some_and(|id| id == current_running_id)
            );

            // Resolving the command to unblock the test
            executor.handle_executed_command_event(ExecutedExecutorCommandEvent {
                command_id: current_running_id,
                exit_code: 0,
                output: "foo".as_bytes().to_vec(),
            })
        });

        task_executor
            .run(async move {
                execute_command_future.await;
                cancel_command_future.await;
            })
            .await;
    });
}

#[test]
fn test_cancelling_command_queues_up_next_command_command() {
    App::test((), |_app| async move {
        let (executor_command_tx, _) = async_channel::unbounded();
        let (in_band_command_cancelled_tx, _) = async_channel::unbounded();
        let executor = Arc::new(InBandCommandExecutor::new(
            executor_command_tx,
            in_band_command_cancelled_tx,
        ));

        let task_executor = async_executor::LocalExecutor::new();

        let execute_first_command_future = task_executor.spawn(execute_test_command(
            executor.clone(),
            "echo foo",
            |result| {
                let output = result.expect("Expect result to be ok");
                assert_eq!(output.status, CommandExitStatus::Failure)
            },
        ));
        let execute_second_command_future = task_executor.spawn(execute_test_command(
            executor.clone(),
            "echo bar",
            assert_command_output_result_fn("bar", true),
        ));

        let cancel_command_future = task_executor.spawn(async move {
            let current_running_id = executor
                .running_command_id()
                .expect("There should be a running command");

            let pending_command_id = executor
                .pending_command_ids()
                .first()
                .expect("There should be at least one pending command")
                .to_string();
            executor.handle_cancelled_in_band_command_event(InBandCommandCancelledEvent {
                command_id: current_running_id,
            });
            assert!(
                executor
                    .running_command_id()
                    .is_some_and(|id| id == pending_command_id)
            );

            // Resolving the command to unblock the test
            executor.handle_executed_command_event(ExecutedExecutorCommandEvent {
                command_id: pending_command_id,
                exit_code: 0,
                output: "bar".as_bytes().to_vec(),
            })
        });

        task_executor
            .run(async move {
                execute_first_command_future.await;
                execute_second_command_future.await;
                cancel_command_future.await;
            })
            .await;
    });
}

/// A receiver sized like a real pane, starting with the cursor at `start`.
fn receiver_at(start: Point) -> InBandCommandOutputReceiver {
    let size = SizeInfo::new_without_font_metrics(/*rows=*/ 24, /*cols=*/ 80);
    InBandCommandOutputReceiver::new(start, &size)
}

fn feed(receiver: &mut InBandCommandOutputReceiver, text: &str) {
    for c in text.chars() {
        receiver.input(c);
    }
}

/// A cursor move to a point the capture has not reached must not underflow.
///
/// This is the pane-freezing case: the capture starts wherever the cursor happens to be, and a
/// generator (ConPTY in particular) routinely emits an absolute cursor move before it has printed
/// anything. Subtracting a cell distance from an empty capture used to underflow, panicking the
/// PTY reader thread and leaving the pane permanently dead.
#[test]
fn goto_before_anything_is_captured_does_not_underflow() {
    let mut receiver = receiver_at(Point::new(30, 12));

    receiver.goto(0, 0);

    assert_eq!(receiver.to_output_string(), "");
    assert!(!receiver.has_overflowed());
}

/// The same rewind, but with a partial capture: only what was captured can be dropped.
#[test]
fn goto_rewinding_past_the_capture_clears_it_rather_than_underflowing() {
    let mut receiver = receiver_at(Point::new(5, 0));
    feed(&mut receiver, "abc");

    receiver.goto(0, 0);

    assert_eq!(receiver.to_output_string(), "");
}

/// Cursor distances count grid cells, so the rewind must drop characters, not bytes. Truncating a
/// `String` by a cell count lands mid-character on any non-ASCII output and panics.
#[test]
fn goto_rewinds_by_characters_not_bytes() {
    let mut receiver = receiver_at(Point::new(0, 0));
    feed(&mut receiver, "제제제");

    receiver.goto(0, 2);

    assert_eq!(receiver.to_output_string(), "제제");
}

#[test]
fn goto_rewinds_mixed_width_output_by_characters() {
    let mut receiver = receiver_at(Point::new(0, 0));
    feed(&mut receiver, "héllo");

    receiver.goto(0, 3);

    assert_eq!(receiver.to_output_string(), "hél");
}

/// A forward cursor move is unsupported and must leave the capture untouched.
#[test]
fn goto_forward_leaves_the_capture_alone() {
    let mut receiver = receiver_at(Point::new(0, 0));
    feed(&mut receiver, "abc");

    receiver.goto(4, 0);

    assert_eq!(receiver.to_output_string(), "abc");
}

/// A capture that never sees its terminating marker has to give up: while it is running, every
/// printable byte is diverted away from the grid, so an unbounded capture renders the pane blind.
#[test]
fn capture_reports_overflow_past_the_size_limit() {
    let mut receiver = receiver_at(Point::new(0, 0));

    for _ in 0..MAX_IN_BAND_OUTPUT_CHARS {
        receiver.input('x');
    }
    assert!(
        !receiver.has_overflowed(),
        "a capture at the limit is still within budget"
    );

    receiver.input('x');
    assert!(receiver.has_overflowed());
}

/// A grid with no rows must not underflow the wrap bookkeeping.
#[test]
fn wrapping_in_a_zero_row_grid_does_not_underflow() {
    let size = SizeInfo::new_without_font_metrics(/*rows=*/ 0, /*cols=*/ 1);
    let mut receiver = InBandCommandOutputReceiver::new(Point::new(0, 0), &size);

    feed(&mut receiver, "ab");
    receiver.carriage_return();

    assert_eq!(receiver.to_output_string(), "ab");
}
