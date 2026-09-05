use super::resize_is_worth_sending;

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
