use super::*;

#[test]
fn this_process_is_running() {
    let me = ClientProcess::new(std::process::id());

    assert!(me.is_running());
}

/// The recycled-PID case, as far as it can be reached without actually waiting
/// for the kernel to recycle one: a start time that does not match the process
/// currently holding the PID must not read as still attached.
#[cfg(target_os = "linux")]
#[test]
fn a_pid_whose_start_time_moved_is_not_the_process_that_attached() {
    let mut stale = ClientProcess::new(std::process::id());
    stale.start_time = stale.start_time.map(|time| time + 1);

    assert!(
        !stale.is_running(),
        "a different start time means a different process behind the same PID"
    );
}

/// A child that has exited and been reaped leaves a PID nothing holds.
///
/// `std::process::Command` is disallowed for flashing a console on Windows;
/// this module is Unix-only.
#[allow(clippy::disallowed_types)]
#[test]
fn a_process_that_exited_is_not_running() {
    let mut child = std::process::Command::new("true")
        .spawn()
        .expect("`true` must be runnable");
    let client = ClientProcess::new(child.id());
    child.wait().expect("the child must be reapable");

    assert!(!client.is_running());
}
