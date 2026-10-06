use std::path::{Path, PathBuf};

use warpui::{AppContext, Entity};

use crate::search::SyncDataSource;
use crate::search::command_palette::agent_sessions::search::{RowMatch, match_row};
use crate::search::command_palette::agent_sessions::search_item::AgentSessionSearchItem;
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::command_palette::separator_search_item::SeparatorSearchItem;
use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::DataSourceRunErrorWrapper;
use crate::terminal::agent_session_history::{PastAgentSession, is_within};
use crate::terminal::agent_sessions_index::{
    AgentSessionRow, AgentSessionState, DetachedInput, PaneAgentInput, build_rows,
};
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
///
/// Matching a past row against the focused folder resolves both paths on disk, so call this when
/// a snapshot is taken rather than per query.
pub(super) fn section_for(
    row: &AgentSessionRow,
    focused: Option<PaneViewLocator>,
    focused_cwd: Option<&Path>,
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
        AgentSessionState::Past => {
            let in_focused_folder = focused_cwd.is_some_and(|folder| {
                !folder.as_os_str().is_empty()
                    && !row.cwd.as_os_str().is_empty()
                    && is_within(&row.cwd, folder)
            });
            if in_focused_folder {
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

/// The zero-state layout: each non-empty section's rows followed by its separator, sections
/// bottom-up, rows kept in their given order.
pub(super) fn zero_state_slots(sections: &[AgentSessionSection]) -> Vec<ZeroStateSlot> {
    let mut slots = Vec::new();
    for section in AgentSessionSection::reverse_order() {
        let start = slots.len();
        slots.extend(
            sections
                .iter()
                .enumerate()
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

/// Agent sessions as they stood when the palette opened. Queries only read this snapshot: taking
/// one asks the supervisor and resolves paths on disk, too slow to repeat per keystroke.
#[derive(Default)]
pub struct DataSource {
    panes: Vec<PaneAgentInput>,
    detached: Vec<DetachedInput>,
    focused: Option<PaneViewLocator>,
    entries: Vec<Entry>,
    is_loaded: bool,
}

impl DataSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set_snapshot(
        &mut self,
        panes: Vec<PaneAgentInput>,
        detached: Vec<DetachedInput>,
        focused: Option<PaneViewLocator>,
        past: &[PastAgentSession],
    ) {
        self.panes = panes;
        self.detached = detached;
        self.focused = focused;
        self.is_loaded = true;
        self.rebuild(past);
    }

    /// Rebuilds the rows against newer past sessions, keeping the panes and detached sessions
    /// the snapshot was taken with.
    pub(crate) fn refresh_past(&mut self, past: &[PastAgentSession]) {
        if self.is_loaded {
            self.rebuild(past);
        }
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn is_loaded(&self) -> bool {
        self.is_loaded
    }

    fn rebuild(&mut self, past: &[PastAgentSession]) {
        let focused_cwd: Option<PathBuf> = self.focused.and_then(|focused| {
            self.panes
                .iter()
                .find(|pane| pane.locator == focused)
                .and_then(|pane| pane.cwd.clone())
        });
        self.entries = build_rows(&self.panes, &self.detached, past)
            .into_iter()
            .map(|row| Entry {
                action: action_for(&row, self.focused),
                section: section_for(&row, self.focused, focused_cwd.as_deref()),
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
        if text.is_empty() {
            let sections: Vec<AgentSessionSection> =
                self.entries.iter().map(|entry| entry.section).collect();
            return Ok(zero_state_slots(&sections)
                .into_iter()
                .map(|slot| match slot {
                    ZeroStateSlot::Separator(section) => {
                        SeparatorSearchItem::new(section.title().to_owned()).into()
                    }
                    ZeroStateSlot::Row(index) => {
                        Self::item(&self.entries[index], RowMatch::default())
                    }
                })
                .collect());
        }

        Ok(self
            .entries
            .iter()
            .filter_map(|entry| {
                match_row(&entry.row, text).map(|matched| Self::item(entry, matched))
            })
            .collect())
    }
}

impl Entity for DataSource {
    type Event = ();
}
