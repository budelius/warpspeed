use futures::StreamExt;
use managed_agent_sessions::{RepositoryInfo, RunSummary};
use warpui::App;

use super::*;
use crate::test_util::assert_eventually_within;

fn run(job_id: &str, state: &str, repo_url: Option<&str>) -> RunSummary {
    RunSummary {
        job_id: job_id.to_owned(),
        state: state.to_owned(),
        agent_name: "staging-claude-interactive-rw".to_owned(),
        repo_url: repo_url.map(str::to_owned),
        branch: Some(format!("agent/{job_id}")),
        interactive: true,
        last_activity_at: Some("2026-10-06T12:00:00Z".to_owned()),
        node_name: Some("agent-ctrl-node-0".to_owned()),
        paused_at: None,
        pause_allowed: Some(true),
    }
}

fn repository(name: &str, url: &str) -> RepositoryInfo {
    RepositoryInfo {
        name: name.to_owned(),
        url: url.to_owned(),
        branch: "dev".to_owned(),
        reachable: true,
        error: None,
    }
}

const WARP: &str = "https://git-dev.obachan.dev/obachan/warp";
const DASH: &str = "https://gitlab.com/stozn/dash";

fn job_ids(group: &FleetGroup) -> Vec<&str> {
    group.rows.iter().map(|row| row.job_id.as_str()).collect()
}

#[test]
fn runs_on_the_same_repository_form_one_group() {
    let groups = group_runs(
        &[
            run("job-a", "running", Some(WARP)),
            run("job-b", "running", Some(&format!("{WARP}.git"))),
        ],
        &[repository("warp", WARP)],
    );

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].repo_name, "warp");
    assert_eq!(groups[0].repo_url.as_deref(), Some(WARP));
    assert_eq!(job_ids(&groups[0]), ["job-a", "job-b"]);
}

#[test]
fn runs_without_a_repository_are_grouped_last() {
    let groups = group_runs(
        &[
            run("job-none", "running", None),
            run("job-warp", "running", Some(WARP)),
            run("job-dash", "running", Some(DASH)),
        ],
        &[repository("warp", WARP), repository("dash", DASH)],
    );

    let names: Vec<_> = groups
        .iter()
        .map(|group| group.repo_name.as_str())
        .collect();
    assert_eq!(names, ["dash", "warp", NO_REPOSITORY]);
    assert_eq!(groups[2].repo_url, None);
    assert_eq!(job_ids(&groups[2]), ["job-none"]);
}

#[test]
fn finished_runs_follow_the_live_ones() {
    let groups = group_runs(
        &[
            run("job-done", "completed", Some(WARP)),
            run("job-failed", "failed", Some(WARP)),
            run("job-live", "running", Some(WARP)),
            run("job-waiting", "waiting_for_input", Some(WARP)),
        ],
        &[repository("warp", WARP)],
    );

    assert_eq!(
        job_ids(&groups[0]),
        ["job-live", "job-waiting", "job-done", "job-failed"]
    );
}

#[test]
fn a_repository_agent_ctrl_does_not_list_is_named_after_its_url() {
    let groups = group_runs(&[run("job-a", "running", Some(DASH))], &[]);

    assert_eq!(groups[0].repo_name, "stozn/dash");
}

#[test]
fn rows_carry_the_branch_link() {
    let groups = group_runs(&[run("job-a", "running", Some(WARP))], &[]);

    assert_eq!(
        groups[0].rows[0].branch_url.as_deref(),
        Some("https://git-dev.obachan.dev/obachan/warp/src/branch/agent/job-a")
    );
}

#[test]
fn branch_links_point_at_gitea_and_gitlab_branch_pages() {
    assert_eq!(
        branch_web_url("https://git-dev.obachan.dev/obachan/warp.git", "feat/fleet").as_deref(),
        Some("https://git-dev.obachan.dev/obachan/warp/src/branch/feat/fleet")
    );
    assert_eq!(
        branch_web_url("git@git-dev.obachan.dev:obachan/warp.git", "dev").as_deref(),
        Some("https://git-dev.obachan.dev/obachan/warp/src/branch/dev")
    );
    assert_eq!(
        branch_web_url("https://gitlab.com/stozn/group/dash", "feat/fleet").as_deref(),
        Some("https://gitlab.com/stozn/group/dash/-/tree/feat/fleet")
    );
    assert_eq!(
        branch_web_url("https://user:token@GitLab.com/stozn/dash.git/", "dev").as_deref(),
        Some("https://gitlab.com/stozn/dash/-/tree/dev")
    );
    assert_eq!(
        branch_web_url("https://gitlab.com/stozn/dash", "a b#c").as_deref(),
        Some("https://gitlab.com/stozn/dash/-/tree/a%20b%23c")
    );
}

#[test]
fn branch_links_are_not_offered_for_other_hosts() {
    assert_eq!(
        branch_web_url("https://github.com/warpdotdev/warp", "dev"),
        None
    );
    assert_eq!(branch_web_url("/srv/git/warp", "dev"), None);
    assert_eq!(branch_web_url("https://gitlab.com/", "dev"), None);
    assert_eq!(branch_web_url("https://gitlab.com/stozn/dash", ""), None);
}

#[test]
fn polling_stops_without_an_attached_view() {
    assert!(!should_poll(0));
    assert!(should_poll(1));
}

fn list_two_runs() -> anyhow::Result<FleetSnapshot> {
    Ok(FleetSnapshot {
        runs: vec![
            run("job-a", "running", Some(WARP)),
            run("job-b", "completed", None),
        ],
        repos: vec![repository("warp", WARP)],
    })
}

fn list_fails() -> anyhow::Result<FleetSnapshot> {
    Err(anyhow::anyhow!("dashboard unreachable"))
}

fn no_start(_: &str, _: &str) -> anyhow::Result<String> {
    panic!("not started in these tests")
}

fn no_stop(_: &str) -> anyhow::Result<()> {
    panic!("not stopped in these tests")
}

fn no_pause(_: &str) -> anyhow::Result<()> {
    panic!("not paused in these tests")
}

fn no_resume(_: &str) -> anyhow::Result<()> {
    panic!("not resumed in these tests")
}

fn no_preview(_: &str) -> anyhow::Result<managed_agent_sessions::RunPreview> {
    panic!("preview not fetched in these tests")
}

fn preview_ok(job_id: &str) -> anyhow::Result<managed_agent_sessions::RunPreview> {
    Ok(managed_agent_sessions::RunPreview {
        events: vec![managed_agent_sessions::RunPreviewEvent {
            kind: managed_agent_sessions::RunPreviewEventKind::Assistant,
            content: format!("preview for {job_id}"),
            timestamp: String::new(),
            redacted: false,
            truncated: false,
        }],
        terminal: false,
        history_gap: false,
        truncated: false,
    })
}

fn backend(list: fn() -> anyhow::Result<FleetSnapshot>) -> FleetBackend {
    FleetBackend {
        list,
        start: no_start,
        stop: no_stop,
        pause: no_pause,
        resume: no_resume,
        resume_for_attach: no_resume,
        preview: no_preview,
        stream_preview: None,
    }
}

#[test]
fn preview_is_loaded_in_the_background_for_the_requested_job() {
    App::test((), |mut app| async move {
        let model = app.add_model(|_| {
            FleetModel::with_backend(FleetBackend {
                preview: preview_ok,
                ..backend(list_two_runs)
            })
        });

        model.update(&mut app, |model, ctx| {
            model.request_preview("job-b".to_owned(), false, ctx)
        });
        assert!(model.read(&app, |model, _| model.is_previewing("job-b")));

        assert_eventually_within!(
            std::time::Duration::from_secs(5) => model.read(&app, |model, _| {
                model.preview("job-b").is_some()
            }),
            "the preview never finished"
        );
        let content = model.read(&app, |model, _| {
            model
                .preview("job-b")
                .and_then(|preview| preview.as_ref().ok())
                .and_then(|preview| preview.events.first())
                .map(|event| event.content.clone())
        });
        assert_eq!(content.as_deref(), Some("preview for job-b"));
    });
}

#[test]
fn resume_for_attach_announces_success_only_after_agent_ctrl_accepts_it() {
    App::test((), |mut app| async move {
        let accept: fn(&str) -> anyhow::Result<()> = |_| Ok(());
        let model = app.add_model(|_| {
            FleetModel::with_backend(FleetBackend {
                resume_for_attach: accept,
                ..backend(list_two_runs)
            })
        });
        let (sender, mut events) = futures::channel::mpsc::unbounded();
        app.update(|ctx| {
            ctx.subscribe_to_model(&model, move |_, event, _| {
                if let FleetModelEvent::ResumeForAttachSucceeded { job_id, .. } = event {
                    let _ = sender.unbounded_send(job_id.clone());
                }
            });
        });

        model.update(&mut app, |model, ctx| {
            model.resume_for_attach("job-paused".to_owned(), warpui::EntityId::new(), ctx)
        });

        assert_eq!(events.next().await.as_deref(), Some("job-paused"));
    });
}

#[test]
fn refresh_applies_the_fetched_runs_and_clears_the_last_error() {
    App::test((), |mut app| async move {
        let model = app.add_model(|_| {
            let mut model = FleetModel::with_backend(backend(list_two_runs));
            model.last_error = Some("old failure".to_owned());
            model
        });

        model.update(&mut app, |model, ctx| model.refresh(ctx));
        assert!(model.read(&app, |model, _| model.is_refreshing()));

        assert_eventually_within!(
            std::time::Duration::from_secs(5) => !model.read(&app, |model, _| model.is_refreshing()),
            "the refresh never finished"
        );
        model.read(&app, |model, _| {
            assert_eq!(model.runs.len(), 2);
            assert_eq!(model.repos.len(), 1);
            assert_eq!(model.last_error(), None);
        });
    });
}

#[test]
fn a_failed_refresh_keeps_the_old_runs_and_records_the_error() {
    App::test((), |mut app| async move {
        let model = app.add_model(|_| FleetModel::with_backend(backend(list_two_runs)));
        model.update(&mut app, |model, ctx| model.refresh(ctx));
        assert_eventually_within!(
            std::time::Duration::from_secs(5) => !model.read(&app, |model, _| model.is_refreshing()),
            "the first refresh never finished"
        );

        model.update(&mut app, |model, ctx| {
            model.backend = backend(list_fails);
            model.refresh(ctx);
        });
        assert_eventually_within!(
            std::time::Duration::from_secs(5) => !model.read(&app, |model, _| model.is_refreshing()),
            "the failing refresh never finished"
        );

        model.read(&app, |model, _| {
            assert_eq!(model.runs.len(), 2);
            let error = model.last_error().expect("the failure is recorded");
            assert!(error.contains("dashboard unreachable"), "{error}");
        });
    });
}

#[test]
fn deselecting_a_live_preview_clears_its_loading_state() {
    App::test((), |mut app| async move {
        let model = app.add_model(|_| {
            FleetModel::with_backend(FleetBackend {
                stream_preview: Some(|_| Err(anyhow::anyhow!("stream unavailable"))),
                ..backend(list_two_runs)
            })
        });
        let view = EntityId::new();
        model.update(&mut app, |model, ctx| {
            model.attach_view(view, ctx);
            model.select_preview(view, Some("job-a".to_owned()), false, ctx);
            assert!(model.is_previewing("job-a"));
            model.select_preview(view, None, false, ctx);
            assert!(!model.is_previewing("job-a"));
            assert!(model.preview("job-a").is_none());
        });
    });
}

#[test]
fn closing_the_last_preview_view_stops_loading_and_hidden_views_cannot_restart_it() {
    App::test((), |mut app| async move {
        let model = app.add_model(|_| {
            FleetModel::with_backend(FleetBackend {
                stream_preview: Some(|_| Err(anyhow::anyhow!("stream unavailable"))),
                ..backend(list_two_runs)
            })
        });
        let first = EntityId::new();
        let second = EntityId::new();
        model.update(&mut app, |model, ctx| {
            model.attach_view(first, ctx);
            model.attach_view(second, ctx);
            model.select_preview(first, Some("job-a".to_owned()), false, ctx);
            model.select_preview(second, Some("job-a".to_owned()), false, ctx);
            model.detach_view(first);
            assert!(model.is_previewing("job-a"));
            model.detach_view(second);
            assert!(!model.is_previewing("job-a"));
            model.select_preview(first, Some("job-a".to_owned()), false, ctx);
            assert!(!model.is_previewing("job-a"));
        });
    });
}

#[test]
fn live_preview_failure_is_visible_without_staying_in_the_loading_state() {
    App::test((), |mut app| async move {
        let model = app.add_model(|_| {
            FleetModel::with_backend(FleetBackend {
                stream_preview: Some(|_| Err(anyhow::anyhow!("stream unavailable"))),
                ..backend(list_two_runs)
            })
        });
        let view = EntityId::new();
        model.update(&mut app, |model, ctx| {
            model.attach_view(view, ctx);
            model.select_preview(view, Some("job-a".to_owned()), false, ctx);
        });
        assert_eventually_within!(
            std::time::Duration::from_secs(5) => model.read(&app, |model, _| {
                model.preview("job-a").is_some_and(Result::is_err)
            }),
            "the stream failure never reached the preview"
        );
        assert!(!model.read(&app, |model, _| model.is_previewing("job-a")));
    });
}

#[test]
fn a_view_detached_twice_is_uncounted_once() {
    App::test((), |mut app| async move {
        let model = app.add_model(|_| FleetModel::with_backend(backend(list_two_runs)));
        let closed = warpui::EntityId::new();
        let reopened = warpui::EntityId::new();

        model.update(&mut app, |model, ctx| {
            model.attach_view(closed, ctx);
            // Hidden for undo, then closed after the grace period.
            model.detach_view(closed);
            model.attach_view(reopened, ctx);
            model.detach_view(closed);
        });

        assert_eq!(model.read(&app, |model, _| model.attached_view_count()), 1);
    });
}

fn start_fails(_: &str, _: &str) -> anyhow::Result<String> {
    Err(anyhow::anyhow!("node refused the run"))
}

#[test]
fn a_failed_start_outlives_the_refresh_that_follows_it() {
    App::test((), |mut app| async move {
        let model = app.add_model(|_| {
            FleetModel::with_backend(FleetBackend {
                start: start_fails,
                ..backend(list_two_runs)
            })
        });

        model.update(&mut app, |model, ctx| {
            model.start(WARP.to_owned(), crate::terminal::CLIAgent::Claude, ctx)
        });
        assert_eventually_within!(
            std::time::Duration::from_secs(5) => model.read(&app, |model, _| {
                model.runs.len() == 2 && !model.is_refreshing()
            }),
            "the refresh after the start never landed"
        );

        model.read(&app, |model, _| {
            assert_eq!(model.last_error(), None);
            let error = model.action_error().expect("the start failure is kept");
            assert!(error.contains("node refused the run"), "{error}");
        });

        model.update(&mut app, |model, ctx| model.dismiss_action_error(ctx));
        assert_eq!(
            model.read(&app, |model, _| model.action_error().map(str::to_owned)),
            None
        );
    });
}

static LISTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn list_counting() -> anyhow::Result<FleetSnapshot> {
    LISTED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    list_two_runs()
}

#[test]
fn a_refresh_asked_for_during_one_runs_after_it() {
    App::test((), |mut app| async move {
        let model = app.add_model(|_| FleetModel::with_backend(backend(list_counting)));

        model.update(&mut app, |model, ctx| {
            model.refresh(ctx);
            model.refresh(ctx);
        });

        assert_eventually_within!(
            std::time::Duration::from_secs(5) =>
                LISTED.load(std::sync::atomic::Ordering::SeqCst) == 2
                    && !model.read(&app, |model, _| model.is_refreshing()),
            "the second refresh never ran"
        );
    });
}

#[test]
fn paused_runs_follow_the_running_ones_and_precede_the_finished_ones() {
    let groups = group_runs(
        &[
            run("job-done", "completed", Some(WARP)),
            run("job-paused", "paused", Some(WARP)),
            run("job-live", "running", Some(WARP)),
        ],
        &[repository("warp", WARP)],
    );

    assert_eq!(job_ids(&groups[0]), ["job-live", "job-paused", "job-done"]);
    assert!(!groups[0].rows[1].is_finished());
}

#[test]
fn rows_carry_the_node_name() {
    let mut on_second_node = run("job-a", "running", Some(WARP));
    on_second_node.node_name = Some("agent-ctrl-node-1".to_owned());
    let groups = group_runs(&[on_second_node], &[]);

    assert_eq!(
        groups[0].rows[0].node_name.as_deref(),
        Some("agent-ctrl-node-1")
    );
}

#[test]
fn only_runs_the_dashboard_allows_pausing_can_be_paused_and_only_paused_ones_resumed() {
    let mut worker = run("job-worker", "running", None);
    worker.interactive = false;
    worker.pause_allowed = Some(false);
    let mut task_bound = run("job-task-bound", "running", None);
    task_bound.pause_allowed = Some(false);
    let mut old_dashboard = run("job-old-dashboard", "running", None);
    old_dashboard.pause_allowed = None;
    let groups = group_runs(
        &[
            run("job-live", "running", None),
            run("job-paused", "paused", None),
            run("job-waiting", "waiting_input", None),
            worker,
            task_bound,
            old_dashboard,
        ],
        &[],
    );
    let row = |job_id: &str| {
        groups[0]
            .rows
            .iter()
            .find(|row| row.job_id == job_id)
            .unwrap()
            .clone()
    };

    assert!(row("job-live").can_pause() && !row("job-live").can_resume());
    assert!(row("job-paused").can_resume() && !row("job-paused").can_pause());
    assert!(!row("job-waiting").can_pause());
    assert!(!row("job-worker").can_pause());
    assert!(!row("job-task-bound").can_pause());
    assert!(!row("job-old-dashboard").can_pause());
}

fn pause_unknown_session(_: &str) -> anyhow::Result<()> {
    Err(managed_agent_sessions::HttpStatusError {
        status: reqwest::StatusCode::PRECONDITION_FAILED,
        body: r#"{"error":"session id unknown","code":"session_unknown"}"#.to_owned(),
    }
    .into())
}

fn resume_at_capacity(_: &str) -> anyhow::Result<()> {
    Err(managed_agent_sessions::HttpStatusError {
        status: reqwest::StatusCode::CONFLICT,
        body: r#"{"error":"node at capacity","code":"node_at_capacity"}"#.to_owned(),
    }
    .into())
}

/// Runs `act` on a model whose backend is `backend`, waits for the refresh that follows, and
/// returns the action error it leaves.
fn action_error_after(
    backend: FleetBackend,
    act: fn(&mut FleetModel, &mut ModelContext<FleetModel>),
) -> Option<String> {
    App::test((), |mut app| async move {
        let model = app.add_model(|_| FleetModel::with_backend(backend));
        model.update(&mut app, act);
        assert_eventually_within!(
            std::time::Duration::from_secs(5) => model.read(&app, |model, _| {
                model.runs.len() == 2 && !model.is_refreshing()
            }),
            "the refresh after the action never landed"
        );
        model.read(&app, |model, _| model.action_error().map(str::to_owned))
    })
}

#[test]
fn a_pause_refused_for_an_unknown_session_says_why() {
    let error = action_error_after(
        FleetBackend {
            pause: pause_unknown_session,
            ..backend(list_two_runs)
        },
        |model, ctx| model.pause("job-a".to_owned(), ctx),
    )
    .expect("the pause failure is kept");
    assert!(error.contains("cannot be paused"), "{error}");
}

#[test]
fn a_resume_with_every_node_full_says_so() {
    let error = action_error_after(
        FleetBackend {
            resume: resume_at_capacity,
            ..backend(list_two_runs)
        },
        |model, ctx| model.resume("job-a".to_owned(), ctx),
    )
    .expect("the resume failure is kept");
    assert!(error.contains("at capacity"), "{error}");
}

#[test]
fn a_successful_pause_and_resume_leave_no_error() {
    let accept: fn(&str) -> anyhow::Result<()> = |_| Ok(());
    let paused = action_error_after(
        FleetBackend {
            pause: accept,
            ..backend(list_two_runs)
        },
        |model, ctx| model.pause("job-a".to_owned(), ctx),
    );
    let resumed = action_error_after(
        FleetBackend {
            resume: accept,
            ..backend(list_two_runs)
        },
        |model, ctx| model.resume("job-a".to_owned(), ctx),
    );
    assert_eq!((paused, resumed), (None, None));
}

#[cfg(feature = "local_fs")]
#[test]
fn every_startable_agent_has_an_agent_ctrl_profile() {
    for (agent, label) in STARTABLE_AGENTS {
        assert!(agent_ctrl_profile(agent).is_some(), "{label}");
    }
    let agents = STARTABLE_AGENTS.map(|(agent, _)| agent);
    assert!(agents.contains(&CLIAgent::Kimi));
    assert!(agents.contains(&CLIAgent::DeepSeekHarness));
}
