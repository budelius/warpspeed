use std::fs;

use futures::StreamExt;
use warpui::App;

use super::{AgentSessionHistoryEvent, AgentSessionHistoryModel};
use crate::terminal::CLIAgent;
use crate::terminal::agent_session_history::{PastAgentSession, describe, pi_candidates};

#[test]
fn refresh_loads_sessions_in_the_background_and_announces_them() {
    let root = tempfile::tempdir().expect("temporary root should be created");
    let project = root.path().join("--project--");
    fs::create_dir(&project).expect("project directory should be created");
    fs::write(
        project.join("2026-09-28T09-53-21-590Z_filename-id.jsonl"),
        "{\"type\":\"session\",\"id\":\"actual-id\",\"cwd\":\"/work/repo\"}
",
    )
    .expect("transcript should be written");
    static PI_ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    PI_ROOT
        .set(root.path().to_path_buf())
        .expect("root is set once");

    App::test((), |mut app| async move {
        let model = app.add_singleton_model(|_| AgentSessionHistoryModel::new());
        let (sender, mut updates) = futures::channel::mpsc::unbounded();
        app.update(|ctx| {
            ctx.subscribe_to_model(&model, move |_, event, _| match event {
                AgentSessionHistoryEvent::Updated => {
                    let _ = sender.unbounded_send(());
                }
            });
        });
        assert!(!model.read(&app, |model, _| model.is_loaded()));

        model.update(&mut app, |model, ctx| {
            model.refresh_with(
                || {
                    let root = PI_ROOT.get().expect("root is set");
                    pi_candidates(root).into_iter().map(describe).collect()
                },
                ctx,
            );
        });
        updates
            .next()
            .await
            .expect("refresh should announce itself");

        model.read(&app, |model, _| {
            assert!(model.is_loaded());
            assert_eq!(model.sessions().len(), 1);
            assert_eq!(model.sessions()[0].agent, CLIAgent::Pi);
            assert_eq!(model.sessions()[0].session_id, "actual-id");
        });
    });
}

#[test]
fn refreshes_requested_during_a_load_trigger_exactly_one_more_load() {
    static LOADS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    fn load() -> Vec<PastAgentSession> {
        let nth = LOADS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        (0..nth)
            .map(|index| PastAgentSession {
                agent: CLIAgent::Pi,
                session_id: format!("session-{index}"),
                cwd: std::path::PathBuf::new(),
                canonical_cwd: None,
                title: None,
                modified: std::time::SystemTime::UNIX_EPOCH,
            })
            .collect()
    }

    App::test((), |mut app| async move {
        let model = app.add_singleton_model(|_| AgentSessionHistoryModel::new());
        let (sender, mut updates) = futures::channel::mpsc::unbounded();
        app.update(|ctx| {
            ctx.subscribe_to_model(&model, move |_, event, _| match event {
                AgentSessionHistoryEvent::Updated => {
                    let _ = sender.unbounded_send(());
                }
            });
        });

        model.update(&mut app, |model, ctx| {
            model.refresh_with(load, ctx);
            model.refresh_with(load, ctx);
            model.refresh_with(load, ctx);
        });
        updates.next().await.expect("first load announces itself");
        updates
            .next()
            .await
            .expect("one follow-up load announces itself");

        assert_eq!(LOADS.load(std::sync::atomic::Ordering::SeqCst), 2);
        model.read(&app, |model, _| assert_eq!(model.sessions().len(), 2));
        assert!(updates.try_next().is_err(), "no third load should run");
    });
}
