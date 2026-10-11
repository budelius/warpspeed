use anyhow::Result;

use super::super::ChildEvent;
use super::super::spawner::PtyHandle;
use super::{next_polled_child_event, resize_is_worth_sending};

struct PollingHandle {
    terminated: bool,
    polling: bool,
}

impl PtyHandle for PollingHandle {
    fn pid(&self) -> u32 {
        42
    }

    fn has_process_terminated(&mut self) -> Result<bool> {
        Ok(self.terminated)
    }

    fn requires_exit_polling(&self) -> bool {
        self.polling
    }

    fn kill(&mut self) -> Result<()> {
        Ok(())
    }
}

#[test]
fn supervised_child_exit_is_reported_by_polling() {
    let mut handle = PollingHandle {
        terminated: true,
        polling: true,
    };

    assert_eq!(
        next_polled_child_event(&mut handle),
        Some(ChildEvent::Exited)
    );
}

#[test]
fn non_polling_child_is_left_to_its_event_watcher() {
    let mut handle = PollingHandle {
        terminated: true,
        polling: false,
    };

    assert_eq!(next_polled_child_event(&mut handle), None);
}

#[test]
fn first_resize_is_always_sent() {
    assert!(resize_is_worth_sending(None, (120, 30), false));
}

#[test]
fn a_new_column_or_row_count_is_sent() {
    assert!(resize_is_worth_sending(Some((120, 30)), (121, 30), false));
    assert!(resize_is_worth_sending(Some((120, 30)), (120, 31), false));
}

/// A window drag changes the pane's pixel size on every frame while its cell
/// counts sit still. ConPTY is told nothing but the cell counts, so those
/// frames used to cost every open session a console-host reflow apiece.
#[test]
fn an_unchanged_cell_count_is_dropped() {
    assert!(!resize_is_worth_sending(Some((120, 30)), (120, 30), false));
}

/// `TerminalView::resize_alt_screen_redundantly` re-sends the size an
/// alt-screen program may have missed, so a forced resize has to survive the
/// check that drops the rest.
#[test]
fn a_forced_resize_is_sent_even_when_unchanged() {
    assert!(resize_is_worth_sending(Some((120, 30)), (120, 30), true));
}

#[test]
fn shell_start_directory_keeps_an_existing_directory() {
    let dir = std::env::temp_dir();
    assert_eq!(super::shell_start_directory(Some(&dir)), Some(dir));
}

/// A shell given no usable directory must not inherit the starting process's own, which for the
/// app and the supervisor is the install directory.
#[test]
fn shell_start_directory_falls_back_to_home() {
    let home = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from);
    let missing = std::env::temp_dir().join("warp-shell-start-directory-that-does-not-exist");
    assert_eq!(super::shell_start_directory(None), home);
    assert_eq!(super::shell_start_directory(Some(&missing)), home);
}
