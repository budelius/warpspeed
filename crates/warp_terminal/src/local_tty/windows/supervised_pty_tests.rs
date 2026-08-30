use super::*;

/// Reads until `needle` appears or the budget runs out.
///
/// A pseudoconsole emits a good deal of setup and cursor traffic before the
/// command's own output, so a single read is not enough to conclude anything.
fn read_until(pty: &mut SupervisedPty, needle: &str, attempts: usize) -> String {
    let mut collected = String::new();
    let mut buffer = [0u8; 4096];

    for _ in 0..attempts {
        match pty.read(&mut buffer, 200) {
            Ok(0) => {
                if !pty.is_running() && collected.contains(needle) {
                    break;
                }
            }
            Ok(read) => collected.push_str(&String::from_utf8_lossy(&buffer[..read])),
            Err(_) => break,
        }
        if collected.contains(needle) {
            break;
        }
    }

    collected
}

/// The core of the Windows backend: the supervisor owns a real pseudoconsole,
/// a real shell runs under it, and the supervisor reads its output without any
/// app, event loop, or `AppContext` involved.
#[test]
fn a_supervised_pty_runs_a_shell_and_produces_its_output() {
    let mut pty = spawn_supervised_pty("cmd.exe /c echo supervised-hello", 80, 24, None)
        .expect("spawning a supervised pty should succeed");

    assert!(pty.pid() > 0, "the shell must have a process id");

    let output = read_until(&mut pty, "supervised-hello", 40);

    assert!(
        output.contains("supervised-hello"),
        "expected the command's output from the pseudoconsole, got {output:?}"
    );
}

/// Guards the test above against passing on echoed input rather than real
/// output: nothing is written to this pty, so anything read came from the
/// shell.
#[test]
fn a_supervised_pty_produces_output_without_being_written_to() {
    let mut pty = spawn_supervised_pty("cmd.exe /c echo untouched-output", 80, 24, None)
        .expect("spawning a supervised pty should succeed");

    let output = read_until(&mut pty, "untouched-output", 40);

    assert!(
        output.contains("untouched-output"),
        "the pseudoconsole produced nothing, so reads are not working: {output:?}"
    );
}

/// The supervisor has to notice when a session's shell has ended, or it would
/// hold dead sessions forever and offer them in the drawer.
#[test]
fn a_supervised_pty_reports_when_its_shell_exits() {
    let pty = spawn_supervised_pty("cmd.exe /c exit", 80, 24, None)
        .expect("spawning a supervised pty should succeed");

    assert!(
        pty.wait(10_000),
        "a shell that exits immediately should be observed exiting"
    );
    assert!(
        !pty.is_running(),
        "the shell has exited, so it must not report as running"
    );
}

/// A long-running shell is the case the whole feature exists for, so the
/// opposite of the test above must hold too: a live shell reports as running.
#[test]
fn a_supervised_pty_reports_a_live_shell_as_running() {
    let mut pty = spawn_supervised_pty("cmd.exe /c ping -n 20 127.0.0.1", 80, 24, None)
        .expect("spawning a supervised pty should succeed");

    assert!(
        pty.is_running(),
        "a shell that should still be pinging reports as exited"
    );

    pty.kill().expect("should be able to end the session");
}

/// Reading with nothing to read must time out rather than block forever, or a
/// supervisor polling its sessions would wedge on the first idle one.
#[test]
fn reading_an_idle_pty_times_out_instead_of_blocking() {
    // Output is redirected away, so the shell is genuinely silent while it
    // runs. An earlier version of this test used a bare `ping`, which prints a
    // line per second -- it read four bytes where it expected none, and only
    // failed once the whole suite ran and the timing shifted.
    let mut pty = spawn_supervised_pty("cmd.exe /c ping -n 20 127.0.0.1 >nul", 80, 24, None)
        .expect("spawning a supervised pty should succeed");

    // Drain the pseudoconsole's startup traffic. Two consecutive empty reads,
    // not one, because that traffic arrives in bursts with gaps between them.
    let mut buffer = [0u8; 4096];
    let mut consecutive_empty = 0;
    for _ in 0..40 {
        if pty.read(&mut buffer, 100).expect("read should succeed") == 0 {
            consecutive_empty += 1;
            if consecutive_empty == 2 {
                break;
            }
        } else {
            consecutive_empty = 0;
        }
    }

    let read = pty
        .read(&mut buffer, 150)
        .expect("an idle read should time out, not fail");

    assert_eq!(read, 0, "an idle pty should read nothing");

    pty.kill().expect("should be able to end the session");
}

/// The handle the supervisor lends is the pipe, and it has to be a real one --
/// lending an invalid handle would fail at the point a pane tries to attach.
#[test]
fn a_supervised_pty_exposes_a_lendable_pipe_handle() {
    let pty = spawn_supervised_pty("cmd.exe /c exit", 80, 24, None)
        .expect("spawning a supervised pty should succeed");

    assert!(
        !pty.pipe_handle().is_invalid(),
        "the pipe handle must be valid so it can be duplicated into a client"
    );

    let lent = super::super::handle_lending::lend_handle_to_process(pty.pipe_handle(), unsafe {
        windows::Win32::System::Threading::GetCurrentProcessId()
    })
    .expect("the pty's pipe should be lendable");

    assert_ne!(lent, 0, "a lent pipe handle must not be null");
}

/// Resizing is what a pane does when its window changes, and a pseudoconsole
/// that rejects it would leave applications drawing at the wrong width.
#[test]
fn a_supervised_pty_can_be_resized() {
    let mut pty = spawn_supervised_pty("cmd.exe /c ping -n 20 127.0.0.1", 80, 24, None)
        .expect("spawning a supervised pty should succeed");

    pty.resize(120, 40).expect("resizing should succeed");

    pty.kill().expect("should be able to end the session");
}

/// The input half of the contract. A detached session that can be read but not
/// written to would come back as a pane that displays output and ignores the
/// keyboard.
#[test]
fn writing_to_a_supervised_pty_reaches_the_shell() {
    let mut pty = spawn_supervised_pty("cmd.exe", 80, 24, None)
        .expect("spawning a supervised pty should succeed");

    // Let the shell finish starting before typing at it.
    let mut buffer = [0u8; 4096];
    for _ in 0..20 {
        if pty.read(&mut buffer, 200).expect("read should succeed") == 0 {
            break;
        }
    }

    pty.write(b"echo written-through\r\n")
        .expect("writing should succeed");

    let output = read_until(&mut pty, "written-through", 60);

    assert!(
        output.contains("written-through"),
        "input never reached the shell, got {output:?}"
    );

    pty.kill().expect("should be able to end the session");
}

/// The supervisor reports whether its shell escaped Warp's job object, and that
/// report has to match what the job actually permits -- otherwise a caller is
/// told its sessions are more durable than they are.
#[test]
fn a_supervised_pty_reports_whether_it_escaped_the_job_object() {
    let pty = spawn_supervised_pty("cmd.exe /c exit", 80, 24, None)
        .expect("spawning a supervised pty should succeed");

    let (program, args) = (
        std::path::PathBuf::from("cmd.exe"),
        vec!["/c".to_owned(), "exit".to_owned()],
    );
    let mut probe = super::super::daemon_spawn::spawn_detached_supervisor(&program, &args)
        .expect("the probe spawn should succeed");
    let job_allows_breakaway = probe.escaped_job_object;
    let _ = probe.child.wait();

    assert_eq!(
        pty.escaped_job_object(),
        job_allows_breakaway,
        "the pty's job-escape report disagrees with what this job actually allows"
    );
}
