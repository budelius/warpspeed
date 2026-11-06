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
fn an_agent_row_json_exposes_process_identity_and_provenance() {
    let row = AgentRow {
        agent: "Pi".to_owned(),
        session_id: "session-1".to_owned(),
        pid: 42,
        started_at_unix_seconds: 100,
        last_active_unix_seconds: Some(200),
        state: "held_by_other_build".to_owned(),
        build: Some("old-build".to_owned()),
        saved_session: Some("Saved session".to_owned()),
        saved_tab: Some("Tab 1".to_owned()),
        working_directory: Some("/work/repo".to_owned()),
        last_prompt: Some("Fix the bug".to_owned()),
        process_count: 2,
    };

    assert_eq!(
        serde_json::to_value(row).unwrap(),
        serde_json::json!({
            "agent": "Pi",
            "session_id": "session-1",
            "pid": 42,
            "started_at_unix_seconds": 100,
            "last_active_unix_seconds": 200,
            "state": "held_by_other_build",
            "build": "old-build",
            "saved_session": "Saved session",
            "saved_tab": "Tab 1",
            "working_directory": "/work/repo",
            "last_prompt": "Fix the bug",
            "process_count": 2,
        })
    );
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
