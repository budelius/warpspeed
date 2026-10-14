use fuzzy_match::match_indices_case_insensitive;
use managed_agent_sessions::RunSummary;

use super::fleet_model::is_finished_state;
use crate::search::command_palette::agent_sessions::{normalized_query, text_has_exact_words};

pub(crate) fn managed_agent_label(agent_name: &str) -> &str {
    if agent_name.contains("claude") {
        "Claude"
    } else if agent_name.contains("codex") {
        "Codex"
    } else if agent_name.contains("-pi-") || agent_name.ends_with("-pi") || agent_name == "pi" {
        "Pi"
    } else if agent_name.contains("kimi") {
        "Kimi"
    } else if agent_name.contains("dsh") || agent_name.contains("deepseek") {
        "DeepSeek"
    } else {
        agent_name
    }
}

pub(crate) fn managed_run_title(run: &RunSummary) -> String {
    let repo = run.repo_url.as_deref().and_then(|url| {
        url.trim_end_matches('/')
            .trim_end_matches(".git")
            .rsplit(['/', ':'])
            .next()
            .filter(|name| !name.is_empty())
    });
    let id: String = run
        .job_id
        .strip_prefix("job-")
        .unwrap_or(&run.job_id)
        .chars()
        .take(8)
        .collect();
    let agent = managed_agent_label(&run.agent_name);
    match repo {
        Some(repo) => format!("{repo} · {agent} · {id}"),
        None => format!("{agent} · {id}"),
    }
}

pub(crate) fn managed_run_detail(run: &RunSummary) -> String {
    [
        Some("AgentCtrl"),
        Some(run.state.as_str()),
        run.node_name.as_deref(),
        run.branch.as_deref(),
        Some(run.job_id.as_str()),
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join(" · ")
}

pub(crate) fn managed_action_label(state: &str) -> &'static str {
    if state == "paused" {
        "Resume & attach"
    } else if is_finished_state(state) {
        "View transcript"
    } else {
        "Attach here"
    }
}

fn search_fields(run: &RunSummary) -> [Option<&str>; 8] {
    [
        Some("AgentCtrl remote"),
        Some(run.job_id.as_str()),
        Some(run.state.as_str()),
        Some(run.agent_name.as_str()),
        Some(managed_agent_label(&run.agent_name)),
        run.repo_url.as_deref(),
        run.branch.as_deref(),
        run.node_name.as_deref(),
    ]
}

pub(crate) fn matching_runs<'a>(runs: &'a [RunSummary], query: &str) -> Vec<(&'a RunSummary, i64)> {
    let query = normalized_query(query);
    let exact_match = |run: &RunSummary| {
        search_fields(run)
            .into_iter()
            .flatten()
            .any(|text| text_has_exact_words(text, query))
    };
    let has_exact_match = !query.is_empty() && runs.iter().any(exact_match);
    let mut matching = runs
        .iter()
        .filter_map(|run| {
            if query.is_empty() {
                return Some((run, 0));
            }
            if has_exact_match && !exact_match(run) {
                return None;
            }
            let score = search_fields(run)
                .into_iter()
                .flatten()
                .filter_map(|text| {
                    match_indices_case_insensitive(text, query).map(|result| result.score)
                })
                .max();
            let score = if has_exact_match {
                score.unwrap_or(0).saturating_add(1_000_000)
            } else {
                score?
            };
            Some((run, score))
        })
        .collect::<Vec<_>>();
    matching.sort_by_key(|(run, _)| {
        if is_finished_state(&run.state) {
            2
        } else if run.state == "paused" {
            1
        } else {
            0
        }
    });
    matching
}

#[cfg(test)]
#[path = "session_picker_tests.rs"]
mod tests;
