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
        shell_ready: true,
        command_parked: false,
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

const NO_TIME: std::time::Duration = std::time::Duration::ZERO;

const NO_WAITING: ResumeTimeouts = ResumeTimeouts {
    shell: NO_TIME,
    command: NO_TIME,
    agent: NO_TIME,
};

/// A pane whose resumed agent has got as far as the given flags say.
fn resuming_pane(shell_ready: bool, command_parked: bool, has_running_agent: bool) -> PaneSummary {
    let mut pane = pane(Some(7), true);
    pane.shell_ready = shell_ready;
    pane.command_parked = command_parked;
    pane.has_running_agent = has_running_agent;
    pane
}

#[test]
fn a_bulk_restart_covers_agents_in_panes_that_have_not_attached() {
    let mut running = pane(Some(7), true);
    running.agent = Some("pi".to_owned());
    running.has_running_agent = true;
    assert!(is_agent_pane(&running));

    let mut unattached = pane(Some(8), true);
    unattached.agent = Some("pi".to_owned());
    unattached.shell_ready = false;
    assert!(is_agent_pane(&unattached));
}

/// Restarting a shell with no agent in it ends work the user never asked to lose.
#[test]
fn a_bulk_restart_leaves_panes_without_an_agent_alone() {
    let mut unattached = pane(Some(8), true);
    unattached.shell_ready = false;

    assert!(!is_agent_pane(&pane(Some(7), true)));
    assert!(!is_agent_pane(&unattached));
}

/// A shell that is slow to bootstrap has not lost its agent; each stage has to be waited out
/// before the next, and the report has to say which one stopped it.
#[test]
fn a_resume_reports_the_stage_it_stopped_at() {
    let stopped_at = |pane: PaneSummary| wait_for_resume(|| Some(pane.clone()), NO_WAITING);

    assert_eq!(
        stopped_at(resuming_pane(false, true, false)),
        ResumeProgress::ShellNotReady
    );
    assert_eq!(
        stopped_at(resuming_pane(true, true, false)),
        ResumeProgress::CommandNotRun
    );
    assert_eq!(
        stopped_at(resuming_pane(true, false, false)),
        ResumeProgress::AgentSilent
    );
    assert_eq!(
        stopped_at(resuming_pane(true, false, true)),
        ResumeProgress::AgentRunning
    );
}

/// A pane still parked at its shell must not read as having run its command: the agent has not
/// had the chance to report in yet.
#[test]
fn a_resume_waits_for_the_shell_before_judging_the_agent() {
    let mut states = vec![
        resuming_pane(true, false, true),
        resuming_pane(true, true, false),
        resuming_pane(false, true, false),
    ];
    let progress = wait_for_resume(
        || {
            let next = states.pop();
            next.or_else(|| Some(resuming_pane(true, false, true)))
        },
        ResumeTimeouts {
            shell: std::time::Duration::from_secs(5),
            command: std::time::Duration::from_secs(5),
            agent: NO_TIME,
        },
    );

    assert_eq!(progress, ResumeProgress::AgentRunning);
}

#[test]
fn a_pane_that_disappears_stops_the_wait() {
    assert_eq!(
        wait_for_resume(|| None, NO_WAITING),
        ResumeProgress::ShellNotReady
    );
}

#[test]
fn a_failed_resume_says_where_the_command_is() {
    let report = |resume_progress| RestartReport {
        terminal_view_id: 5465,
        agent: Some("pi".to_owned()),
        agent_session_id: Some("01a14edb".to_owned()),
        resume_command: Some("pi --session 01a14edb".to_owned()),
        resume_progress,
    };

    assert_eq!(
        describe_restart(&report(Some(ResumeProgress::AgentRunning))),
        "Restarted terminal view 5465; resumed `pi --session 01a14edb`"
    );
    assert!(
        describe_restart(&report(Some(ResumeProgress::ShellNotReady)))
            .contains("parked until it is")
    );
    assert!(
        describe_restart(&report(Some(ResumeProgress::CommandNotRun)))
            .contains("still in the pane's input")
    );
    assert!(
        describe_restart(&report(Some(ResumeProgress::AgentSilent)))
            .contains("did not report back")
    );
}
