use std::path::PathBuf;

use super::*;

/// A trivial program that exits immediately, for spawn tests that care about
/// the spawn rather than what runs.
fn exits_immediately() -> (PathBuf, Vec<String>) {
    (
        PathBuf::from("cmd.exe"),
        vec!["/c".to_owned(), "exit".to_owned()],
    )
}

/// The point of the fallback. Warp is often inside a job object that forbids
/// breakaway -- under a terminal, a test runner, or CI -- and this test process
/// is one such place. Spawning must still succeed there, because a supervisor
/// inside the job is worth far more than no supervisor.
#[test]
fn a_supervisor_starts_even_when_the_job_forbids_breakaway() {
    let (program, args) = exits_immediately();

    let mut daemon = spawn_detached_supervisor(&program, &args)
        .expect("spawning the supervisor must succeed whether or not breakaway is permitted");

    assert!(daemon.id() > 0, "a spawned process must have a pid");

    let status = daemon.child.wait().expect("should be able to wait");
    assert!(status.success(), "the child should have exited cleanly");
}

/// The discriminating test for the fallback, and the guard against it being
/// dead code. If breakaway was reported as unavailable, then a spawn that
/// *requires* it must genuinely fail -- otherwise the fallback is reporting a
/// restriction that is not there, and the caller is told its supervisor is less
/// durable than it really is.
///
/// The reverse holds too: if breakaway was reported as achieved, requiring it
/// must succeed.
#[test]
fn the_reported_breakaway_result_matches_what_the_job_actually_allows() {
    let (program, args) = exits_immediately();

    let mut daemon =
        spawn_detached_supervisor(&program, &args).expect("spawning should succeed either way");
    let reported_escape = daemon.escaped_job_object;
    let _ = daemon.child.wait();

    let breakaway_only = spawn_with_flags(
        &program,
        &args,
        DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB,
    );

    match breakaway_only {
        Ok(mut child) => {
            let _ = child.wait();
            assert!(
                reported_escape,
                "breakaway succeeds here, so the supervisor should have reported escaping the job"
            );
        }
        Err(error) => {
            assert!(
                is_access_denied(&error),
                "expected breakaway to fail with access denied, got {error:?}"
            );
            assert!(
                !reported_escape,
                "breakaway is denied here, so the supervisor must not claim it escaped the job"
            );
        }
    }
}

/// A supervisor must not hold Warp's standard streams open: if it did, a parent
/// reading them would never see them close, and pipelines around Warp would
/// hang after it exits.
#[test]
fn a_spawned_supervisor_does_not_inherit_standard_streams() {
    let (program, args) = exits_immediately();

    let daemon = spawn_detached_supervisor(&program, &args).expect("spawning should succeed");

    assert!(
        daemon.child.stdin.is_none(),
        "the supervisor must not be handed a stdin pipe"
    );
    assert!(
        daemon.child.stdout.is_none(),
        "the supervisor must not be handed a stdout pipe"
    );
    assert!(
        daemon.child.stderr.is_none(),
        "the supervisor must not be handed a stderr pipe"
    );
}

/// A program that does not exist is a real error, not something to degrade
/// around: the fallback exists for a job restriction, and must not swallow
/// every other failure.
#[test]
fn spawning_a_program_that_does_not_exist_fails() {
    let program = PathBuf::from("warp-supervisor-that-does-not-exist.exe");

    let result = spawn_detached_supervisor(&program, &["--help"]);

    assert!(
        result.is_err(),
        "a missing program must surface as an error"
    );
}
