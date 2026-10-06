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

fn backend(list: fn() -> anyhow::Result<FleetSnapshot>) -> FleetBackend {
    FleetBackend {
        list,
        start: no_start,
        stop: no_stop,
    }
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
