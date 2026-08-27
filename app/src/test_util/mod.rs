pub mod ai_agent_tasks;
pub mod pane_snapshots;
pub mod settings;
pub mod terminal;
mod virtual_fs;

pub use terminal::add_window_with_terminal;
pub use virtual_fs::{Stub, VirtualFS};
pub use warp_terminal::test_util::mock_blockgrid;

macro_rules! assert_eventually {
    ($cond:expr_2021, $($arg:tt)+) => {
        $crate::test_util::assert_eventually!(20 => $cond, $($arg)+);
    };
    // Run the condition up to ticks times, yielding to the executor in between.  If it does
    // not become true, this panics with the provided format string + args.
    ($ticks:literal => $cond:expr_2021, $($arg:tt)+) => {{
        let mut pass = false;
        for _ in 0..$ticks {
            if $cond {
                pass = true;
                break;
            }
            warpui::r#async::Timer::after(std::time::Duration::from_millis(5)).await;
        }
        if !pass {
            panic!("{}", format_args!($($arg)+));
        }
    }};
}
pub(crate) use assert_eventually;

/// Wall-clock-bounded sibling of [`assert_eventually!`].
///
/// Use this, not a tick or yield budget, whenever the condition waits on work
/// that leaves the cooperative executor entirely -- a tokio round-trip, a
/// forked shell, a joined thread. A fixed budget prices that work in scheduler
/// turns, and how much wall clock a turn buys is exactly what CI load changes;
/// when the budget runs short the test fails *false-red*, blaming the code for
/// the machine being busy. Seconds are the unit such a wait is actually
/// specified in, so wait in seconds.
///
/// Polls `$cond` until it holds or `$timeout` elapses, sleeping briefly between
/// polls so the executor can run the work being waited on. Panics with the
/// given message on timeout.
macro_rules! assert_eventually_within {
    ($timeout:expr_2021 => $cond:expr_2021, $($arg:tt)+) => {{
        let deadline = std::time::Instant::now() + $timeout;
        let mut pass = false;
        loop {
            if $cond {
                pass = true;
                break;
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            warpui::r#async::Timer::after(std::time::Duration::from_millis(5)).await;
        }
        if !pass {
            panic!("{}", format_args!($($arg)+));
        }
    }};
}
pub(crate) use assert_eventually_within;
