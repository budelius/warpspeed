use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcessId, SetEvent, WaitForSingleObject,
};

use super::*;

/// A manual-reset event, which is a cheap kernel object whose state can be
/// observed through any handle that refers to it.
fn event() -> HANDLE {
    unsafe { CreateEventW(None, true, false, None) }.expect("should be able to create an event")
}

/// The property that makes lending work at all: the value handed to the target
/// process refers to the *same kernel object*, not a copy of its state. Here the
/// target is this process, so the duplicate can be waited on directly.
#[test]
fn a_lent_handle_refers_to_the_same_kernel_object() {
    let original = event();

    let lent = lend_handle_to_process(original, unsafe { GetCurrentProcessId() })
        .expect("lending into our own process should succeed");
    let duplicate = HANDLE(lent as *mut std::ffi::c_void);

    // Signalling through the original must be visible through the duplicate.
    unsafe { SetEvent(original) }.expect("should be able to signal the event");
    let waited = unsafe { WaitForSingleObject(duplicate, 0) };

    assert_eq!(
        waited, WAIT_OBJECT_0,
        "the duplicated handle did not observe the original's signal, so it is \
         not referring to the same object"
    );

    let _ = unsafe { CloseHandle(original) };
    let _ = unsafe { CloseHandle(duplicate) };
}

/// Guards the test above against passing vacuously. If lending returned the
/// original value unchanged, the wait would still succeed while nothing had
/// actually been duplicated -- and nothing would reach a real target process.
#[test]
fn lending_produces_a_distinct_handle_value() {
    let original = event();

    let lent = lend_handle_to_process(original, unsafe { GetCurrentProcessId() })
        .expect("lending into our own process should succeed");

    assert_ne!(
        lent, original.0 as i64,
        "lending returned the original handle value, so no duplication happened"
    );
    assert_ne!(lent, 0, "a lent handle value must not be null");

    let _ = unsafe { CloseHandle(original) };
    let _ = unsafe { CloseHandle(HANDLE(lent as *mut std::ffi::c_void)) };
}

/// The supervisor lends to a client it was told about. If that client is gone,
/// the failure has to surface rather than producing a handle value that is
/// meaningless everywhere.
#[test]
fn lending_to_a_process_that_does_not_exist_fails() {
    let original = event();

    // A pid that is not a multiple of four is never valid on Windows, so this
    // cannot collide with a real process.
    let result = lend_handle_to_process(original, 0xFFFF_FFFD);

    assert!(
        result.is_err(),
        "lending to a nonexistent process must fail, got {result:?}"
    );

    let _ = unsafe { CloseHandle(original) };
}

/// An invalid handle is rejected before any process is opened, so a bug in the
/// caller shows up as a clear error rather than an obscure Win32 failure.
#[test]
fn lending_an_invalid_handle_is_refused() {
    let result = lend_handle_to_process(HANDLE::default(), unsafe { GetCurrentProcessId() });

    assert!(result.is_err(), "an invalid handle must be refused");
}
