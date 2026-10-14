use std::collections::HashSet;

use warpui::App;

use super::*;

fn remote(job_id: &str, state: &str) -> RunSummary {
    serde_json::from_value(serde_json::json!({
        "job_id": job_id,
        "state": state,
        "agent_name": "staging-pi-interactive",
        "repo_url": "https://git.example/other-project.git",
        "node_name": "remote-node"
    }))
    .unwrap()
}

fn query(text: &str) -> Query {
    Query {
        text: text.to_owned(),
        filters: HashSet::new(),
    }
}

#[test]
fn palette_lists_remote_sessions_without_local_panes() {
    App::test((), |app| async move {
        let mut source = DataSource::new();
        source.set_snapshot(Vec::new(), Vec::new(), None, &[]);
        source.set_managed_snapshot(
            vec![
                remote("job-running", "running"),
                remote("job-paused", "paused"),
                remote("job-finished", "completed"),
            ],
            None,
            false,
        );
        app.read(|ctx| {
            let results = source.run_query(&query(""), ctx).unwrap();
            let ids = results
                .iter()
                .filter_map(|result| match result.accept_result() {
                    CommandPaletteItemAction::AgentSession {
                        action: AgentSessionAction::Managed { job_id, .. },
                    } => Some(job_id),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(ids, ["job-finished", "job-paused", "job-running"]);
            assert_eq!(
                results.last().unwrap().accessibility_label(),
                "Section: AgentCtrl sessions"
            );
        });
    });
}

#[test]
fn remote_search_keeps_the_originating_pane_in_its_action() {
    App::test((), |app| async move {
        let target = PaneViewLocator {
            pane_group_id: warpui::EntityId::new(),
            pane_id: crate::pane_group::PaneId::dummy_pane_id(),
        };
        let mut source = DataSource::new();
        source.set_snapshot(Vec::new(), Vec::new(), Some(target), &[]);
        source.set_managed_snapshot(vec![remote("job-paused", "paused")], None, false);
        app.read(|ctx| {
            let results = source.run_query(&query("remote-node"), ctx).unwrap();
            assert_eq!(results.len(), 1);
            let CommandPaletteItemAction::AgentSession { action } = results[0].accept_result()
            else {
                panic!("expected a managed session action");
            };
            assert_eq!(
                action,
                AgentSessionAction::Managed {
                    job_id: "job-paused".to_owned(),
                    target: Some(target)
                }
            );
        });
    });
}

#[test]
fn local_history_refresh_preserves_remote_sessions() {
    App::test((), |app| async move {
        let mut source = DataSource::new();
        source.set_snapshot(Vec::new(), Vec::new(), None, &[]);
        source.set_managed_snapshot(vec![remote("job-paused", "paused")], None, false);
        source.refresh_past(&[]);
        app.read(|ctx| {
            assert_eq!(source.run_query(&query("AgentCtrl"), ctx).unwrap().len(), 1);
        });
        source.clear();
        app.read(|ctx| {
            assert!(
                source
                    .run_query(&query("AgentCtrl"), ctx)
                    .unwrap()
                    .is_empty()
            );
        });
    });
}

#[test]
fn remote_failure_does_not_remove_local_history() {
    App::test((), |app| async move {
        let mut source = DataSource::new();
        source.set_snapshot(
            Vec::new(),
            Vec::new(),
            None,
            &[PastAgentSession {
                agent: crate::terminal::cli_agent::CLIAgent::Claude,
                session_id: "local-session".to_owned(),
                cwd: "/work/local".into(),
                canonical_cwd: None,
                title: Some("local conversation".to_owned()),
                modified: std::time::SystemTime::UNIX_EPOCH,
                preview: Vec::new(),
            }],
        );
        source.set_managed_snapshot(Vec::new(), Some("offline".to_owned()), false);
        app.read(|ctx| {
            let results = source.run_query(&query("local conversation"), ctx).unwrap();
            assert!(results.iter().any(|result| matches!(
                result.accept_result(),
                CommandPaletteItemAction::AgentSession {
                    action: AgentSessionAction::Resume { .. }
                }
            )));
        });
    });
}

#[test]
fn remote_failure_is_visible_and_not_actionable() {
    App::test((), |app| async move {
        let mut source = DataSource::new();
        source.set_managed_snapshot(Vec::new(), Some("sign in required".to_owned()), false);
        app.read(|ctx| {
            let results = source.run_query(&query(""), ctx).unwrap();
            assert!(
                results[0]
                    .accessibility_label()
                    .contains("sign in required")
            );
            assert!(matches!(
                results[0].accept_result(),
                CommandPaletteItemAction::NoOp
            ));
        });
    });
}
