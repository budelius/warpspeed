use fuzzy_match::match_indices_case_insensitive;

use crate::terminal::agent_sessions_index::AgentSessionRow;

const EXACT_WORD_SCORE_BONUS: i64 = 1_000_000;

/// How a row matched a query, with the matched character indices of the fields the row renders.
#[derive(Debug, Default)]
pub(super) struct RowMatch {
    pub score: i64,
    pub title_indices: Vec<usize>,
    pub folder_indices: Vec<usize>,
}

/// The text a row shows as its title: the conversation title, else a short session id, else the
/// agent's name.
pub(crate) fn row_title(row: &AgentSessionRow) -> String {
    if let Some(title) = row.title.as_deref().filter(|title| !title.is_empty()) {
        return title.to_owned();
    }
    if let Some(session_id) = row.session_id.as_deref().filter(|id| !id.is_empty()) {
        return session_id.chars().take(8).collect();
    }
    match row.agent {
        Some(agent) => agent.display_name().to_owned(),
        None => "Shell".to_owned(),
    }
}

pub(super) fn row_folder(row: &AgentSessionRow) -> String {
    row.cwd.display().to_string()
}

pub(crate) fn normalized_query(query: &str) -> &str {
    let query = query.trim();
    query
        .strip_prefix('"')
        .and_then(|query| query.strip_suffix('"'))
        .filter(|query| !query.is_empty())
        .unwrap_or(query)
}

pub(crate) fn text_has_exact_words(text: &str, query: &str) -> bool {
    let query_words = normalized_query(query)
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    !query_words.is_empty()
        && query_words.iter().all(|query_word| {
            text.split(|character: char| !character.is_alphanumeric())
                .any(|word| word.eq_ignore_ascii_case(query_word))
        })
}

pub(crate) fn row_has_exact_words(row: &AgentSessionRow, query: &str) -> bool {
    text_has_exact_words(&row_title(row), query)
        || text_has_exact_words(&row_folder(row), query)
        || row
            .agent
            .is_some_and(|agent| text_has_exact_words(agent.display_name(), query))
        || row
            .session_id
            .as_deref()
            .is_some_and(|id| text_has_exact_words(id, query))
}

/// Fuzzy matches `query` against the row's title, folder, agent name and session id.
pub(super) fn match_row(row: &AgentSessionRow, query: &str) -> Option<RowMatch> {
    let query = normalized_query(query);
    let title = match_indices_case_insensitive(&row_title(row), query);
    let folder = match_indices_case_insensitive(&row_folder(row), query);
    let agent = row
        .agent
        .and_then(|agent| match_indices_case_insensitive(agent.display_name(), query));
    let session_id = row
        .session_id
        .as_deref()
        .and_then(|id| match_indices_case_insensitive(id, query));

    let mut score = [&title, &folder, &agent, &session_id]
        .into_iter()
        .flatten()
        .map(|result| result.score)
        .max()?;
    if row_has_exact_words(row, query) {
        score = score.saturating_add(EXACT_WORD_SCORE_BONUS);
    }
    Some(RowMatch {
        score,
        title_indices: title
            .map(|result| result.matched_indices)
            .unwrap_or_default(),
        folder_indices: folder
            .map(|result| result.matched_indices)
            .unwrap_or_default(),
    })
}

pub(crate) fn score_row(row: &AgentSessionRow, query: &str) -> Option<i64> {
    match_row(row, query).map(|matched| matched.score)
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
