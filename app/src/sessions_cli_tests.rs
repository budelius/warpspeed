use super::*;

fn pane(session_id: Option<u64>, survives_quit: bool) -> PaneSummary {
    PaneSummary {
        session_id,
        window: "1".to_owned(),
        tab_index: 0,
        title: String::new(),
        shell: "bash".to_owned(),
        working_directory: "/home/user".to_owned(),
        has_running_agent: false,
        is_busy: false,
        survives_quit,
        terminal_view_id: None,
        agent: None,
        agent_session_id: None,
        agent_cwd: None,
        transcript_path: None,
        agent_not_resumed: false,
    }
}

/// The two ways a pane does not come back are different problems, and the
/// column has to say which. An idle pane is working as designed; a pane whose
/// shell the supervisor never owned is the bug that makes a quit lose work, and
/// reporting both as a bare "no" hides it exactly where someone is looking for
/// it.
#[test]
fn a_pane_that_will_not_survive_says_why() {
    assert_eq!(survives_label(&pane(Some(7), true)), "yes");
    assert_eq!(survives_label(&pane(None, true)), "no (pty)");
    assert_eq!(survives_label(&pane(Some(7), false)), "no (idle)");
}

/// A pane with work in it that the supervisor does not own is the case worth
/// naming: the exit path would try to keep it and have nothing to keep.
#[test]
fn a_busy_unsupervised_pane_is_not_reported_as_surviving() {
    let mut busy = pane(None, true);
    busy.is_busy = true;
    busy.has_running_agent = true;

    assert_eq!(survives_label(&busy), "no (pty)");
}

#[test]
fn a_short_value_is_left_alone() {
    assert_eq!(truncate("bash", 10), "bash");
    assert_eq!(truncate("exactlyten", 10), "exactlyten");
}

#[test]
fn a_long_value_is_cut_to_width() {
    let cut = truncate("powershell-preview", 10);

    assert_eq!(cut.chars().count(), 10, "got {cut:?}");
    assert!(cut.ends_with('…'), "got {cut:?}");
}

/// Truncation counts characters, not bytes: cutting a multi-byte character in
/// half would panic on a string slice and print a replacement character at
/// best.
#[test]
fn a_multibyte_value_is_cut_without_splitting_a_character() {
    let cut = truncate("ünïcödë-shell-name", 6);

    assert_eq!(cut.chars().count(), 6, "got {cut:?}");
}
