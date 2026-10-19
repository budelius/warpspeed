use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use warpui::EntityId;

use super::*;
use crate::pane_group::PaneId;
use crate::terminal::agent_session_history::{AgentSessionPreviewMessage, AgentSessionPreviewRole};
use crate::terminal::cli_agent::CLIAgent;

fn claude(id: &str, cwd: &str) -> CliAgentResumeDescriptor {
    CliAgentResumeDescriptor {
        agent: CLIAgent::Claude,
        session_id: Some(id.into()),
        cwd: cwd.into(),
        title: None,
        captured_unix_seconds: None,
    }
}

fn claude_without_id(cwd: &str) -> CliAgentResumeDescriptor {
    CliAgentResumeDescriptor {
        session_id: None,
        ..claude("", cwd)
    }
}

fn pane_at(index: usize, cwd: &str) -> PaneAgentInput {
    PaneAgentInput {
        locator: Some(PaneViewLocator {
            pane_group_id: EntityId::from_usize(index + 1),
            pane_id: PaneId::dummy_pane_id(),
        }),
        remote_instance_pid: None,
        tab_index: index,
        tab_title: format!("tab {index}"),
        pane_number: 1,
        cwd: Some(PathBuf::from(cwd)),
        session_uuid_hex: None,
        latest_user_prompt: None,
        running: None,
        pending: None,
    }
}

fn past_session_at(agent: CLIAgent, id: &str, cwd: &str, age_secs: u64) -> PastAgentSession {
    PastAgentSession {
        agent,
        session_id: id.into(),
        cwd: cwd.into(),
        canonical_cwd: None,
        title: None,
        last_prompt: None,
        modified: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 - age_secs),
        preview: Vec::new(),
    }
}

fn past_session(agent: CLIAgent, id: &str, cwd: &str) -> PastAgentSession {
    past_session_at(agent, id, cwd, 0)
}

fn detached(id: u64, title: &str, cwd: &str, seconds: u64) -> DetachedInput {
    DetachedInput {
        session_id: warp_core::SessionId::from(id),
        remote_instance_pid: None,
        title: title.into(),
        cwd: cwd.into(),
        detached_for_seconds: seconds,
        session_uuid_hex: String::new(),
    }
}

#[test]
fn a_running_conversation_appears_once_as_running() {
    let pane = PaneAgentInput {
        running: Some(claude("a", "/p")),
        ..pane_at(0, "/p")
    };
    let past = vec![past_session(CLIAgent::Claude, "a", "/p")];
    let rows = build_rows(&[pane], &[], &past);
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].state, AgentSessionState::Running { .. }));
}

#[test]
fn a_running_conversation_uses_its_history_title_when_live_context_has_none() {
    let pane = PaneAgentInput {
        running: Some(claude("a", "/p")),
        ..pane_at(0, "/p")
    };
    let mut past = past_session(CLIAgent::Claude, "a", "/p");
    past.title = Some("fix resume button".to_owned());

    let rows = build_rows(&[pane], &[], &[past]);

    assert_eq!(rows[0].title.as_deref(), Some("fix resume button"));
}

#[test]
fn a_live_conversations_latest_prompt_outranks_stale_history() {
    let pane = PaneAgentInput {
        latest_user_prompt: Some("Fix the current bug".into()),
        running: Some(claude("a", "/p")),
        ..pane_at(0, "/p")
    };
    let mut past = past_session(CLIAgent::Claude, "a", "/p");
    past.last_prompt = Some("Fix the old bug".into());

    let rows = build_rows(&[pane], &[], &[past]);

    assert_eq!(rows[0].last_prompt.as_deref(), Some("Fix the current bug"));
}

#[test]
fn a_live_conversation_keeps_its_history_preview() {
    let pane = PaneAgentInput {
        running: Some(claude("a", "/p")),
        ..pane_at(0, "/p")
    };
    let mut past = past_session(CLIAgent::Claude, "a", "/p");
    past.preview = vec![AgentSessionPreviewMessage {
        role: AgentSessionPreviewRole::User,
        text: "Fix resume".to_owned(),
    }];

    let rows = build_rows(&[pane], &[], &[past]);

    assert_eq!(rows[0].preview[0].text, "Fix resume");
}

#[test]
fn a_running_conversation_uses_its_history_timestamp_when_live_context_has_none() {
    let pane = PaneAgentInput {
        running: Some(claude("a", "/p")),
        ..pane_at(0, "/p")
    };
    let past = past_session_at(CLIAgent::Claude, "a", "/p", 30);
    let expected = past.modified;

    let rows = build_rows(&[pane], &[], &[past]);

    assert_eq!(rows[0].last_used, Some(expected));
}

#[test]
fn a_pending_restore_outranks_its_past_entry() {
    let pane = PaneAgentInput {
        pending: Some(claude("b", "/p")),
        ..pane_at(0, "/p")
    };
    let past = vec![past_session(CLIAgent::Claude, "b", "/p")];
    let rows = build_rows(&[pane], &[], &past);
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].state, AgentSessionState::Pending { .. }));
}

#[test]
fn past_sessions_follow_live_ones_newest_first() {
    let pane = PaneAgentInput {
        running: Some(claude("live", "/p")),
        ..pane_at(0, "/p")
    };
    let past = vec![
        past_session_at(CLIAgent::Claude, "old", "/p", 500),
        past_session_at(CLIAgent::Claude, "new", "/p", 10),
    ];
    let rows = build_rows(&[pane], &[], &past);
    let ids: Vec<_> = rows.iter().map(|row| row.session_id.as_deref()).collect();
    assert_eq!(ids, [Some("live"), Some("new"), Some("old")]);
    assert_eq!(rows[1].state, AgentSessionState::Past);
}

#[test]
fn an_agent_without_a_session_id_is_kept_per_pane() {
    let panes = [
        PaneAgentInput {
            running: Some(claude_without_id("/p")),
            ..pane_at(0, "/p")
        },
        PaneAgentInput {
            running: Some(claude_without_id("/p")),
            ..pane_at(1, "/p")
        },
    ];
    let rows = build_rows(&panes, &[], &[]);
    assert_eq!(rows.len(), 2);
}

#[test]
fn live_rows_keep_tab_order_with_running_and_pending_interleaved() {
    let panes = [
        PaneAgentInput {
            pending: Some(claude("p", "/p")),
            ..pane_at(0, "/p")
        },
        PaneAgentInput {
            running: Some(claude("r", "/p")),
            ..pane_at(1, "/p")
        },
    ];
    let rows = build_rows(&panes, &[], &[]);
    let ids: Vec<_> = rows.iter().map(|row| row.session_id.as_deref()).collect();
    assert_eq!(ids, [Some("p"), Some("r")]);
}

#[test]
fn a_running_pane_outranks_its_own_pending_restore() {
    let pane = PaneAgentInput {
        running: Some(claude("a", "/p")),
        pending: Some(claude("a", "/p")),
        ..pane_at(0, "/p")
    };
    let rows = build_rows(&[pane], &[], &[]);
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].state, AgentSessionState::Running { .. }));
}

#[test]
fn detached_sessions_come_after_live_ones_most_recently_detached_first() {
    let pane = PaneAgentInput {
        running: Some(claude("a", "/p")),
        ..pane_at(0, "/p")
    };
    let detached = [detached(1, "one", "/x", 900), detached(2, "two", "/y", 30)];
    let rows = build_rows(&[pane], &detached, &[]);
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[1].state,
        AgentSessionState::Detached {
            session_id: warp_core::SessionId::from(2),
            instance_pid: None,
        }
    );
    assert_eq!(rows[1].agent, None);
    assert_eq!(rows[1].title.as_deref(), Some("two"));
    assert!(matches!(rows[2].state, AgentSessionState::Detached { .. }));
}

#[test]
fn exact_pane_identity_merges_history_into_its_detached_shell() {
    let mut past = past_session(CLIAgent::Claude, "a", "/work/repo/sub");
    past.title = Some("fix the bug".into());
    let mut detached = detached(1, "fix the bug", "/work/repo", 5);
    detached.session_uuid_hex = "pane-uuid".into();
    let scanned = ScannedAgent {
        agent: CLIAgent::Claude,
        session_id: "a".into(),
        cwd: Some("/work/repo/sub".into()),
        pid: 42,
        started_at_unix_seconds: 0,
        warp_session_uuid: Some("pane-uuid".into()),
        placement: Placement::Known,
        process_count: 1,
    };
    let rows = build_rows_with_scanned(&[], &[detached], &[past], &[scanned]);
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].state, AgentSessionState::Detached { .. }));
    assert_eq!(rows[0].agent, Some(CLIAgent::Claude));
    assert_eq!(rows[0].session_id.as_deref(), Some("a"));
}

#[test]
fn sessions_reported_by_another_warp_instance_are_listed_without_duplicates() {
    let remote = PaneAgentInput {
        locator: None,
        remote_instance_pid: Some(4242),
        running: Some(claude("remote", "/work/remote")),
        ..pane_at(0, "/work/remote")
    };
    let rows = build_rows(std::slice::from_ref(&remote), &[], &[]);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].state,
        AgentSessionState::RunningInOtherInstance { instance_pid: 4242 }
    );
    let scanned = ScannedAgent {
        agent: CLIAgent::Claude,
        session_id: "remote".into(),
        cwd: Some("/work/remote".into()),
        pid: 52,
        started_at_unix_seconds: 0,
        warp_session_uuid: Some("remote-pane".into()),
        placement: Placement::HeldByOtherBuild {
            build: "other".into(),
            supervisor_pid: 51,
            saved_in: None,
        },
        process_count: 1,
    };
    let rows = build_rows_with_scanned(std::slice::from_ref(&remote), &[], &[], &[scanned]);
    assert_eq!(rows.len(), 1);

    let local = PaneAgentInput {
        running: Some(claude("remote", "/work/remote")),
        ..pane_at(1, "/work/remote")
    };
    let rows = build_rows(&[local, remote], &[], &[]);
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].state, AgentSessionState::Running { .. }));
}

#[test]
fn detached_sessions_reported_by_another_supervisor_keep_their_provenance() {
    let mut remote = detached(7, "remote shell", "/work/remote", 5);
    remote.remote_instance_pid = Some(4242);

    let rows = build_rows(&[], &[remote], &[]);

    assert_eq!(
        rows[0].state,
        AgentSessionState::Detached {
            session_id: SessionId::from(7),
            instance_pid: Some(4242),
        }
    );
}

#[test]
fn a_known_process_outside_this_windows_rows_is_still_listed() {
    let scanned = ScannedAgent {
        agent: CLIAgent::Claude,
        session_id: "other-window".into(),
        cwd: Some("/work/repo".into()),
        pid: 42,
        started_at_unix_seconds: 0,
        warp_session_uuid: Some("other-pane".into()),
        placement: Placement::Known,
        process_count: 1,
    };

    let rows = build_rows_with_scanned(&[], &[], &[], &[scanned]);

    assert_eq!(rows.len(), 1);
    assert!(matches!(
        rows[0].state,
        AgentSessionState::KnownProcess { pid: 42 }
    ));
}

#[test]
fn matching_titles_and_folders_do_not_guess_a_detached_shells_agent() {
    let mut past = past_session(CLIAgent::Claude, "a", "/work/repo");
    past.title = Some("fix the bug".into());
    let rows = build_rows(&[], &[detached(1, "fix the bug", "/work/repo", 5)], &[past]);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].agent, None);
    assert_eq!(rows[1].state, AgentSessionState::Past);
}

#[test]
fn a_conversation_restored_into_two_panes_is_listed_once_under_the_first() {
    let first = PaneAgentInput {
        pending: Some(claude("a", "/p")),
        ..pane_at(0, "/p")
    };
    let second = PaneAgentInput {
        pending: Some(claude("a", "/p")),
        ..pane_at(1, "/p")
    };
    let rows = build_rows(&[first.clone(), second], &[], &[]);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].state,
        AgentSessionState::Pending {
            locator: first.locator.unwrap()
        }
    );
}

#[test]
fn an_unplaced_process_is_named_from_its_exact_history() {
    let mut past = past_session(CLIAgent::Pi, "process-session", "/work/repo");
    past.last_prompt = Some("Fix the reboot path".into());
    let scanned = ScannedAgent {
        agent: CLIAgent::Pi,
        session_id: "process-session".into(),
        cwd: Some("/work/repo".into()),
        pid: 42,
        started_at_unix_seconds: 0,
        warp_session_uuid: None,
        placement: Placement::NoKnownPane { saved_in: None },
        process_count: 1,
    };

    let rows = build_rows_with_scanned(&[], &[], &[past], &[scanned]);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].last_prompt.as_deref(), Some("Fix the reboot path"));
    assert_eq!(rows[0].process_count, 1);
    assert!(matches!(
        rows[0].state,
        AgentSessionState::NoKnownPane { pid: 42, .. }
    ));
}

#[test]
fn a_process_start_time_is_the_activity_fallback_without_history() {
    let scanned = ScannedAgent {
        agent: CLIAgent::Pi,
        session_id: "process-session".into(),
        cwd: Some("/work/repo".into()),
        pid: 42,
        started_at_unix_seconds: 123,
        warp_session_uuid: None,
        placement: Placement::NoKnownPane { saved_in: None },
        process_count: 1,
    };

    let rows = build_rows_with_scanned(&[], &[], &[], &[scanned]);

    assert_eq!(
        rows[0].last_used,
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(123))
    );
}

#[test]
fn duplicate_live_processes_each_get_a_row_and_a_warning_count() {
    let process = |pid| ScannedAgent {
        agent: CLIAgent::Pi,
        session_id: "duplicate".into(),
        cwd: Some("/work/repo".into()),
        pid,
        started_at_unix_seconds: 0,
        warp_session_uuid: None,
        placement: Placement::NoKnownPane { saved_in: None },
        process_count: 2,
    };

    let rows = build_rows_with_scanned(&[], &[], &[], &[process(42), process(43)]);

    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.process_count == 2));
}

#[test]
fn pane_uuid_accounts_for_only_its_process_when_a_session_id_is_duplicated() {
    let pane = PaneAgentInput {
        session_uuid_hex: Some("pane-one".into()),
        running: Some(claude("duplicate", "/work/repo")),
        ..pane_at(0, "/work/repo")
    };
    let process = |pid, uuid: &str| ScannedAgent {
        agent: CLIAgent::Claude,
        session_id: "duplicate".into(),
        cwd: Some("/work/repo".into()),
        pid,
        started_at_unix_seconds: 0,
        warp_session_uuid: Some(uuid.into()),
        placement: Placement::NoKnownPane { saved_in: None },
        process_count: 2,
    };

    let rows = build_rows_with_scanned(
        &[pane],
        &[],
        &[],
        &[process(42, "pane-one"), process(43, "pane-two")],
    );

    assert_eq!(rows.len(), 2);
    assert!(matches!(rows[0].state, AgentSessionState::Running { .. }));
    assert!(matches!(
        rows[1].state,
        AgentSessionState::NoKnownPane { pid: 43, .. }
    ));
}

#[test]
fn each_state_has_its_label() {
    let locator = pane_at(0, "/p").locator.unwrap();
    assert_eq!(AgentSessionState::Running { locator }.label(), "● running");
    assert_eq!(
        AgentSessionState::RunningInOtherInstance { instance_pid: 42 }.label(),
        "● another Warp instance"
    );
    assert_eq!(AgentSessionState::Pending { locator }.label(), "○ pending");
    assert_eq!(
        AgentSessionState::Detached {
            session_id: SessionId::from(1),
            instance_pid: None,
        }
        .label(),
        "detached"
    );
    assert_eq!(
        AgentSessionState::KnownProcess { pid: 1 }.label(),
        "this build"
    );
    assert_eq!(
        AgentSessionState::HeldByOtherBuild {
            pid: 1,
            build: "old".into(),
            saved_in: None,
        }
        .label(),
        "held by another build"
    );
    assert_eq!(
        AgentSessionState::NoKnownPane {
            pid: 1,
            saved_in: None,
        }
        .label(),
        "no pane"
    );
    assert_eq!(AgentSessionState::Past.label(), "past");
}
