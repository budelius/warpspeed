use super::{fold_pending_resize, should_detach_on_shutdown, should_kill_on_shutdown};
use crate::SizeInfo;

fn size(columns: usize) -> SizeInfo {
    SizeInfo::new_without_font_metrics(30, columns)
}

#[test]
fn a_dead_supervised_child_is_still_removed_from_the_supervisor() {
    assert!(should_kill_on_shutdown(true, true));
}

#[test]
fn a_dead_direct_child_does_not_need_a_second_kill() {
    assert!(!should_kill_on_shutdown(true, false));
}

#[test]
fn a_disconnected_living_child_is_killed_for_both_ownership_modes() {
    assert!(should_kill_on_shutdown(false, false));
    assert!(should_kill_on_shutdown(false, true));
}

#[test]
fn the_newest_size_wins() {
    let (folded, _) = fold_pending_resize(Some((size(100), false)), (size(300), false));
    assert_eq!(folded, size(300));
}

#[test]
fn an_ordinary_fold_stays_unforced() {
    let (_, forced) = fold_pending_resize(Some((size(100), false)), (size(300), false));
    assert!(!forced);
}

/// A refresh re-announces a size the child may have missed. Being overtaken by
/// a later resize inside the same batch must not swallow that.
#[test]
fn an_overtaken_forced_resize_keeps_forcing() {
    let (folded, forced) = fold_pending_resize(Some((size(100), true)), (size(300), false));
    assert_eq!(folded, size(300));
    assert!(forced);
}

#[test]
fn a_forced_resize_forces_the_batch() {
    let (_, forced) = fold_pending_resize(Some((size(100), false)), (size(300), true));
    assert!(forced);
}

#[test]
fn a_lost_connection_to_a_running_supervised_shell_detaches_it() {
    assert!(should_detach_on_shutdown(true, true, true));
}

#[test]
fn a_requested_shutdown_still_ends_a_running_supervised_shell() {
    assert!(!should_detach_on_shutdown(false, true, true));
}

#[test]
fn a_lost_connection_to_a_direct_shell_cannot_detach_it() {
    assert!(!should_detach_on_shutdown(true, false, true));
}

#[test]
fn a_lost_connection_to_a_dead_supervised_shell_is_not_detached() {
    assert!(!should_detach_on_shutdown(true, true, false));
}
