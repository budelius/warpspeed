use managed_agent_sessions::RunSummary;
use warpui::{AppContext, Entity};

use super::managed_search_item::ManagedSessionSearchItem;
use crate::agent_ctrl::session_picker::matching_runs;
use crate::search::SyncDataSource;
use crate::search::command_palette::agent_sessions::search::{RowMatch, match_row};
use crate::search::command_palette::agent_sessions::search_item::AgentSessionSearchItem;
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::command_palette::separator_search_item::SeparatorSearchItem;
use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::DataSourceRunErrorWrapper;
use crate::terminal::agent_session_history::{PastAgentSession, ResolvedPath};
use crate::terminal::agent_sessions_index::{
    AgentSessionRow, AgentSessionState, DetachedInput, PaneAgentInput, build_rows_with_scanned,
    is_row_within,
};
use crate::terminal::cli_agent_sessions::agent_process_scan::ScannedAgent;
use crate::workspace::PaneViewLocator;
use crate::workspace::agent_session_actions::{AgentSessionAction, action_for};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentSessionSection {
    ThisPane,
    RunningElsewhere,
    Detached,
    Past,
}

impl AgentSessionSection {
    pub(super) fn title(&self) -> &'static str {
        match self {
            AgentSessionSection::ThisPane => "This pane",
            AgentSessionSection::RunningElsewhere => "Running elsewhere",
            AgentSessionSection::Detached => "Detached",
            AgentSessionSection::Past => "Past",
        }
    }

    /// Bottom-up, because the command palette renders results in reverse order.
    fn reverse_order() -> [AgentSessionSection; 4] {
        [
            AgentSessionSection::Past,
            AgentSessionSection::Detached,
            AgentSessionSection::RunningElsewhere,
            AgentSessionSection::ThisPane,
        ]
    }
}

/// Which section `row` belongs in, relative to the focused pane and its working directory.
pub(super) fn section_for(
    row: &AgentSessionRow,
    focused: Option<PaneViewLocator>,
    focused_folder: Option<&ResolvedPath>,
) -> AgentSessionSection {
    match &row.state {
        AgentSessionState::Running { locator } | AgentSessionState::Pending { locator } => {
            if Some(*locator) == focused {
                AgentSessionSection::ThisPane
            } else {
                AgentSessionSection::RunningElsewhere
            }
        }
        AgentSessionState::Detached { .. } => AgentSessionSection::Detached,
        AgentSessionState::KnownProcess { .. }
        | AgentSessionState::HeldByOtherBuild { .. }
        | AgentSessionState::NoKnownPane { .. } => AgentSessionSection::RunningElsewhere,
        AgentSessionState::Past => {
            if focused_folder.is_some_and(|folder| is_row_within(row, folder)) {
                AgentSessionSection::ThisPane
            } else {
                AgentSessionSection::Past
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ZeroStateSlot {
    Separator(AgentSessionSection),
    /// Index into the rows the slots were computed from.
    Row(usize),
}

/// The zero-state layout, bottom-up because the palette renders results reversed: each non-empty
/// section's rows, last first, followed by its separator. On screen each section then reads in
/// the rows' given order under its header.
pub(super) fn zero_state_slots(sections: &[AgentSessionSection]) -> Vec<ZeroStateSlot> {
    let mut slots = Vec::new();
    for section in AgentSessionSection::reverse_order() {
        let start = slots.len();
        slots.extend(
            sections
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, row_section)| **row_section == section)
                .map(|(index, _)| ZeroStateSlot::Row(index)),
        );
        if slots.len() > start {
            slots.push(ZeroStateSlot::Separator(section));
        }
    }
    slots
}

struct Entry {
    row: AgentSessionRow,
    action: Option<AgentSessionAction>,
    section: AgentSessionSection,
}

/// Local session snapshots and the latest AgentCtrl runs. Queries never perform filesystem or
/// network IO.
#[derive(Default)]
pub struct DataSource {
    panes: Vec<PaneAgentInput>,
    detached: Vec<DetachedInput>,
    scanned_agents: Vec<ScannedAgent>,
    focused: Option<PaneViewLocator>,
    focused_folder: Option<ResolvedPath>,
    entries: Vec<Entry>,
    managed_runs: Vec<RunSummary>,
    managed_error: Option<String>,
    managed_loading: bool,
    is_loaded: bool,
}

impl DataSource {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn set_snapshot(
        &mut self,
        panes: Vec<PaneAgentInput>,
        detached: Vec<DetachedInput>,
        focused: Option<PaneViewLocator>,
        past: &[PastAgentSession],
    ) {
        self.set_snapshot_with_scan(panes, detached, focused, past, &[]);
    }

    pub(crate) fn set_snapshot_with_scan(
        &mut self,
        panes: Vec<PaneAgentInput>,
        detached: Vec<DetachedInput>,
        focused: Option<PaneViewLocator>,
        past: &[PastAgentSession],
        scanned_agents: &[ScannedAgent],
    ) {
        self.focused_folder = focused
            .and_then(|focused| panes.iter().find(|pane| pane.locator == focused))
            .and_then(|pane| pane.cwd.as_deref())
            .map(ResolvedPath::new);
        self.panes = panes;
        self.detached = detached;
        self.scanned_agents = scanned_agents.to_vec();
        self.focused = focused;
        self.is_loaded = true;
        self.rebuild(past);
    }

    /// Rebuilds the rows against newer past sessions, keeping the panes and detached sessions
    /// the snapshot was taken with.
    #[cfg(test)]
    pub(crate) fn refresh_past(&mut self, past: &[PastAgentSession]) {
        self.refresh_snapshot(past, &[]);
    }

    pub(crate) fn refresh_snapshot(
        &mut self,
        past: &[PastAgentSession],
        scanned_agents: &[ScannedAgent],
    ) {
        if self.is_loaded {
            self.scanned_agents = scanned_agents.to_vec();
            self.rebuild(past);
        }
    }

    pub(crate) fn set_managed_snapshot(
        &mut self,
        runs: Vec<RunSummary>,
        error: Option<String>,
        loading: bool,
    ) {
        self.managed_runs = runs;
        self.managed_error = error;
        self.managed_loading = loading;
    }

    fn managed_results(&self, text: &str) -> Vec<QueryResult<CommandPaletteItemAction>> {
        let mut results = matching_runs(&self.managed_runs, text)
            .into_iter()
            .rev()
            .map(|(run, score)| {
                ManagedSessionSearchItem::new(run.clone(), self.focused, score).into()
            })
            .collect::<Vec<QueryResult<CommandPaletteItemAction>>>();
        if let Some(error) = &self.managed_error {
            results
                .push(SeparatorSearchItem::new(format!("AgentCtrl unavailable: {error}")).into());
        } else if self.managed_loading {
            results.push(SeparatorSearchItem::new("Loading AgentCtrl sessions…".to_owned()).into());
        }
        if !results.is_empty() && text.is_empty() {
            results.push(SeparatorSearchItem::new("AgentCtrl sessions".to_owned()).into());
        }
        results
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn is_loaded(&self) -> bool {
        self.is_loaded
    }

    fn rebuild(&mut self, past: &[PastAgentSession]) {
        self.entries =
            build_rows_with_scanned(&self.panes, &self.detached, past, &self.scanned_agents)
                .into_iter()
                .map(|row| Entry {
                    action: action_for(&row, self.focused),
                    section: section_for(&row, self.focused, self.focused_folder.as_ref()),
                    row,
                })
                .collect();
    }

    fn item(entry: &Entry, matched: RowMatch) -> QueryResult<CommandPaletteItemAction> {
        AgentSessionSearchItem::new(entry.row.clone(), entry.action.clone(), matched).into()
    }
}

impl SyncDataSource for DataSource {
    type Action = CommandPaletteItemAction;

    fn run_query(
        &self,
        query: &Query,
        _app: &AppContext,
    ) -> Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper> {
        let text = query.text.trim();
        let mut results = self.managed_results(text);
        if text.is_empty() {
            let sections: Vec<AgentSessionSection> =
                self.entries.iter().map(|entry| entry.section).collect();
            results.extend(
                zero_state_slots(&sections)
                    .into_iter()
                    .map(|slot| match slot {
                        ZeroStateSlot::Separator(section) => {
                            SeparatorSearchItem::new(section.title().to_owned()).into()
                        }
                        ZeroStateSlot::Row(index) => {
                            Self::item(&self.entries[index], RowMatch::default())
                        }
                    }),
            );
            return Ok(results);
        }

        // Reversed so that, once the palette flips the list, equal scores keep display order.
        results.extend(self.entries.iter().rev().filter_map(|entry| {
            match_row(&entry.row, text).map(|matched| Self::item(entry, matched))
        }));
        Ok(results)
    }
}

impl Entity for DataSource {
    type Event = ();
}

#[cfg(test)]
#[path = "data_source_tests.rs"]
mod tests;
