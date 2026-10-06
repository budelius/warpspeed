use std::fs;

use futures::StreamExt;
use warpui::App;

use super::{AgentSessionHistoryEvent, AgentSessionHistoryModel};
use crate::terminal::CLIAgent;
use crate::terminal::agent_session_history::{describe, pi_candidates};

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
    let pi_root = root.path().to_path_buf();

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
                move || pi_candidates(&pi_root).into_iter().map(describe).collect(),
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
