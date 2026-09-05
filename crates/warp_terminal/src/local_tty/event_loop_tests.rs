use super::fold_pending_resize;
use crate::SizeInfo;

fn size(columns: usize) -> SizeInfo {
    SizeInfo::new_without_font_metrics(30, columns)
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
