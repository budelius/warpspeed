//! Whether a lent PTY pipe can be read by a second process.
//!
//! The supervisor keeps a session's pipe and lends a *handle* to whichever
//! client holds the session, by duplicating it into that process. Duplication
//! makes a second handle to the same kernel file object, which is what lets the
//! client read the shell directly and keeps the supervisor out of the hot path.
//!
//! Attaching asks that to happen a second time, to a different process. This
//! pins down whether it can.

use std::os::windows::io::FromRawHandle;

use mio::windows::NamedPipe;
use mio::{Interest, Poll, Token};
use windows::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_INVALID_PARAMETER, HANDLE,
};
use windows::Win32::System::Threading::GetCurrentProcess;

use super::pipes::create_async_anonymous_pipe;

/// Duplicates a handle within this process, exactly as
/// [`super::handle_lending::lend_handle_to_process`] does across one.
///
/// Same process, because what matters is the kernel file object the duplicate
/// points at, and that is shared either way. A second process would only make
/// the test harder to run.
fn duplicate(handle: HANDLE) -> HANDLE {
    let mut duplicate = HANDLE::default();
    unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle,
            GetCurrentProcess(),
            &mut duplicate,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )
    }
    .expect("duplicating a handle in this process should succeed");
    duplicate
}

/// A lent pipe can be read once, by whoever is lent it first.
///
/// This is the spawn path, and it works: the supervisor creates the pipe, hands
/// a duplicate to the pane that asked for the shell, and that pane registers it
/// for reading.
#[test]
fn the_first_client_lent_a_pipe_can_read_it() {
    let pipe = create_async_anonymous_pipe().expect("a pipe should be creatable");

    let lent = duplicate(pipe.server);
    let mut client = unsafe { NamedPipe::from_raw_handle(lent.0 as *mut _) };
    let poll = Poll::new().expect("a poll should be creatable");

    let registered = poll
        .registry()
        .register(&mut client, Token(0), Interest::READABLE);

    assert!(
        registered.is_ok(),
        "the pane a session is spawned for must be able to read it: {registered:?}"
    );

    drop(client);
    let _ = unsafe { CloseHandle(pipe.server) };
    let _ = unsafe { CloseHandle(pipe.client) };
}

/// A second client lent the same pipe cannot read it, and this is why attaching
/// a session fails on Windows.
///
/// Registering a handle for asynchronous reads binds the underlying *file
/// object* to a completion port, and Windows keeps that binding for the life of
/// the object: "a handle can be associated with only one I/O completion port,
/// and after making the association, the handle remains associated with that
/// I/O completion port until it is closed". A duplicate is a second handle to
/// the same file object, so the binding follows it.
///
/// The first pane to hold a session therefore claims its pipe permanently. When
/// that pane goes and another attaches, the supervisor lends the same file
/// object again, the new pane registers it, and the kernel refuses with
/// `ERROR_INVALID_PARAMETER` -- which surfaced as a `PTY reader` thread panic
/// and a tab that closed itself.
///
/// Nothing in the client can work around this: the constraint is on the object,
/// not on the handle or the process. Attaching needs the supervisor either to
/// hand over a *different* pipe or to relay the bytes itself.
#[test]
fn a_second_client_lent_the_same_pipe_is_refused() {
    let pipe = create_async_anonymous_pipe().expect("a pipe should be creatable");

    let first_lent = duplicate(pipe.server);
    let mut first = unsafe { NamedPipe::from_raw_handle(first_lent.0 as *mut _) };
    let first_poll = Poll::new().expect("a poll should be creatable");
    first_poll
        .registry()
        .register(&mut first, Token(0), Interest::READABLE)
        .expect("the first client must be able to read the pipe");

    // The pane that held the session goes away, taking its handle and its
    // completion port with it. The supervisor still holds the pipe.
    drop(first);
    drop(first_poll);

    let second_lent = duplicate(pipe.server);
    let mut second = unsafe { NamedPipe::from_raw_handle(second_lent.0 as *mut _) };
    let second_poll = Poll::new().expect("a poll should be creatable");

    let registered = second_poll
        .registry()
        .register(&mut second, Token(0), Interest::READABLE);

    let error = registered.expect_err(
        "if a second client can now read the pipe, lending it twice is sound and the \
         attach failure has a different cause than this test records",
    );
    assert_eq!(
        error.raw_os_error(),
        Some(ERROR_INVALID_PARAMETER.0 as i32),
        "expected the completion-port refusal, got {error:?}"
    );

    drop(second);
    drop(second_poll);
    let _ = unsafe { CloseHandle(pipe.server) };
    let _ = unsafe { CloseHandle(pipe.client) };
}
