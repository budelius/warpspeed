use super::*;

fn run(job_id: &str, state: &str, repo: &str, node: &str) -> RunSummary {
    serde_json::from_value(serde_json::json!({
        "job_id": job_id,
        "state": state,
        "agent_name": "staging-pi-interactive",
        "repo_url": repo,
        "node_name": node
    }))
    .unwrap()
}

#[test]
fn remote_sessions_include_all_states_and_repositories() {
    let runs = vec![
        run(
            "job-finished",
            "completed",
            "https://git/other.git",
            "node-2",
        ),
        run("job-paused", "paused", "https://git/repo.git", "node-1"),
        run("job-running", "running", "https://git/third.git", "node-3"),
    ];
    let ids = matching_runs(&runs, "")
        .into_iter()
        .map(|(run, _)| run.job_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["job-running", "job-paused", "job-finished"]);
}

#[test]
fn remote_search_matches_node_names() {
    let runs = vec![
        run("job-one", "running", "https://git/repo.git", "node-1"),
        run("job-two", "paused", "https://git/repo.git", "node-2"),
    ];
    let matches = matching_runs(&runs, "node-2");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].0.job_id, "job-two");
}

#[test]
fn exact_repository_words_win_over_partial_matches() {
    let runs = vec![
        run("job-one", "running", "https://git/me.git", "node-1"),
        run("job-two", "running", "https://git/somewhere.git", "node-1"),
    ];
    let matches = matching_runs(&runs, "\"me\"");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].0.job_id, "job-one");
}

#[test]
fn unmatched_remote_search_is_empty() {
    assert!(
        matching_runs(
            &[run("job-one", "running", "https://git/repo.git", "node-1")],
            "unmatched-query-xyz"
        )
        .is_empty()
    );
}

#[test]
fn remote_titles_distinguish_runs_of_the_same_repository() {
    let first = run(
        "job-12345678-abcd",
        "running",
        "https://git/repo.git",
        "node-1",
    );
    let second = run(
        "job-87654321-abcd",
        "paused",
        "https://git/repo.git",
        "node-1",
    );
    assert_eq!(managed_run_title(&first), "repo · Pi · 12345678");
    assert_eq!(managed_run_title(&second), "repo · Pi · 87654321");
}

#[test]
fn remote_actions_do_not_offer_resume_for_finished_runs() {
    assert_eq!(managed_action_label("paused"), "Resume & attach");
    assert_eq!(managed_action_label("running"), "Attach here");
    assert_eq!(managed_action_label("failed"), "View transcript");
    assert_eq!(managed_action_label("stopped"), "View transcript");
}
