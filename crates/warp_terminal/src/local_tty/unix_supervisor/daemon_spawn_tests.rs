use std::path::PathBuf;
use std::time::Duration;

use instant::Instant;

use super::*;

/// Runs `script` through the detached spawn and returns the pid it reported.
fn spawn_reporting_pid(script: &str, pid_file: &PathBuf) -> u32 {
    spawn_detached_supervisor(
        Path::new("/bin/sh"),
        &[
            "-c".to_owned(),
            format!("echo $$ > {}; {script}", pid_file.display()),
        ],
    )
    .expect("the supervisor launcher must start");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(contents) = std::fs::read_to_string(pid_file)
            && let Ok(pid) = contents.trim().parse()
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "the detached process never ran");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn kill(pid: u32) {
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
}

/// The point of the double fork: Warp is left with nothing to wait on, so the
/// supervisor is not reaped along with it and does not linger as a zombie
/// child either.
#[test]
fn a_spawned_supervisor_is_not_a_child_of_this_process() {
    let directory = tempfile::tempdir().expect("a temp dir must be creatable");
    let pid_file = directory.path().join("pid");
    let pid = spawn_reporting_pid("sleep 30", &pid_file);

    let mut status = 0;
    let waited = unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) };
    let error = std::io::Error::last_os_error().raw_os_error();

    kill(pid);
    assert_eq!(waited, -1, "the supervisor must not be waitable here");
    assert_eq!(
        error,
        Some(libc::ECHILD),
        "the supervisor must not be a child of this process"
    );
}

/// `setsid` is what keeps the `SIGHUP` that reaches Warp's session from
/// reaching the supervisor.
#[test]
fn a_spawned_supervisor_leads_its_own_session() {
    let directory = tempfile::tempdir().expect("a temp dir must be creatable");
    let pid_file = directory.path().join("pid");
    let pid = spawn_reporting_pid("sleep 30", &pid_file);

    let supervisor_session = unsafe { libc::getsid(pid as i32) };
    let our_session = unsafe { libc::getsid(0) };

    kill(pid);
    assert_ne!(
        supervisor_session, -1,
        "the supervisor's session must be readable"
    );
    assert_ne!(
        supervisor_session, our_session,
        "the supervisor must not share Warp's session"
    );
}

/// Inherited standard streams would keep Warp's pipes open after it exits.
#[cfg(target_os = "linux")]
#[test]
fn a_spawned_supervisor_inherits_no_standard_streams() {
    let directory = tempfile::tempdir().expect("a temp dir must be creatable");
    let pid_file = directory.path().join("pid");
    let streams_file = directory.path().join("streams");
    let pid = spawn_reporting_pid(
        // The standard streams are copied aside before anything is redirected,
        // because `readlink /proc/self/fd/1 >&3` would otherwise report fd 3 --
        // the redirection the command itself just made.
        &format!(
            "exec 3>{0}; exec 4<&0 5>&1 6>&2; readlink /proc/self/fd/4 >&3; \
             readlink /proc/self/fd/5 >&3; readlink /proc/self/fd/6 >&3; sleep 30",
            streams_file.display()
        ),
        &pid_file,
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut streams = String::new();
    while Instant::now() < deadline {
        streams = std::fs::read_to_string(&streams_file).unwrap_or_default();
        if streams.lines().count() == 3 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    kill(pid);
    assert_eq!(
        streams.lines().collect::<Vec<_>>(),
        vec!["/dev/null", "/dev/null", "/dev/null"],
        "standard streams were {streams:?}"
    );
}
