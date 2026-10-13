//! The window's agent sessions, grouped by tab and pane.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Local};
use fuzzy_match::match_indices_case_insensitive;
use managed_agent_sessions::{RunPreviewEvent, RunSummary};
use warp_core::ui::Icon;
use warp_editor::editor::NavigationKey;
use warpui::elements::{
    Border, ChildView, Clipped, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CornerRadius, CrossAxisAlignment, DragBarSide, Element, Expanded, Fill, Flex,
    Hoverable, MainAxisSize, MouseStateHandle, Padding, ParentElement, Radius, Resizable,
    ResizableStateHandle, ScrollStateHandle, Scrollable, ScrollableElement, ScrollbarWidth,
    Shrinkable, Text, UniformList, UniformListState, resizable_state_handle,
};
use warpui::fonts::{Properties, Weight};
use warpui::keymap::FixedBinding;
use warpui::keymap::macros::*;
use warpui::platform::Cursor;
use warpui::text_layout::ClipConfig;
use warpui::{
    AppContext, Entity, FocusContext, ModelHandle, SingletonEntity, TypedActionView, View,
    ViewContext, ViewHandle,
};

use crate::agent_ctrl::fleet_model::{FleetModel, FleetModelEvent, is_finished_state};
use crate::appearance::Appearance;
use crate::editor::{
    EditorView, Event as EditorEvent, PropagateAndNoOpNavigationKeys,
    PropagateHorizontalNavigationKeys, SingleLineEditorOptions, TextOptions,
};
use crate::search::command_palette::agent_sessions::{
    normalized_query, row_has_exact_words, row_title, score_row, text_has_exact_words,
};
use crate::terminal::agent_session_history::{PastAgentSession, ResolvedPath};
use crate::terminal::agent_sessions_index::{
    AgentSessionRow, AgentSessionState, DetachedInput, PaneAgentInput, build_rows, is_row_within,
};
use crate::terminal::cli_agent::CLIAgent;
use crate::terminal::detached_sessions::DetachedListError;
use crate::util::time_format::format_approx_duration_from_now;
use crate::util::truncation::truncate_from_end;
use crate::view_components::action_button::{ActionButtonTheme, SecondaryTheme};
use crate::workspace::PaneViewLocator;
use crate::workspace::agent_session_actions::{AgentSessionAction, action_for};

/// The rows the view draws, grouped as the window is.
// A window lists a few dozen rows at most, so boxing the pane variant would only add indirection.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ViewRow {
    TabHeader {
        tab_index: usize,
        title: String,
    },
    Pane {
        input: PaneAgentInput,
        row: Option<AgentSessionRow>,
        action: ViewRowAction,
    },
    PastToggle {
        tab_index: usize,
        folder: PathBuf,
        count: usize,
        expanded: bool,
    },
    Past {
        row: AgentSessionRow,
        /// The pane it is listed under, which is where it resumes.
        target: PaneViewLocator,
    },
    DetachedHeader {
        count: usize,
    },
    Detached {
        row: AgentSessionRow,
    },
    ManagedHeader,
    Managed {
        run: RunSummary,
    },
    ManagedError(String),
    SupervisorUnreachable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ViewRowAction {
    Act(AgentSessionAction),
    /// Pick a past conversation to resume in `target`.
    ChooseSession {
        target: PaneViewLocator,
    },
    None,
}

/// What a row stands for, independent of where it sits in the list, so a click or hover still
/// reaches its row after the list is rebuilt.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RowKey {
    TabHeader(usize),
    Pane(PaneViewLocator),
    PastToggle {
        tab_index: usize,
        folder: PathBuf,
    },
    Past {
        target: PaneViewLocator,
        agent: Option<CLIAgent>,
        session_id: Option<String>,
    },
    DetachedHeader,
    Detached {
        detached_session: Option<warp_core::SessionId>,
        title: Option<String>,
    },
    ManagedHeader,
    Managed(String),
    ManagedError,
    SupervisorUnreachable,
}

impl ViewRow {
    pub(crate) fn key(&self) -> RowKey {
        match self {
            ViewRow::TabHeader { tab_index, .. } => RowKey::TabHeader(*tab_index),
            ViewRow::Pane { input, .. } => RowKey::Pane(input.locator),
            ViewRow::PastToggle {
                tab_index, folder, ..
            } => RowKey::PastToggle {
                tab_index: *tab_index,
                folder: folder.clone(),
            },
            ViewRow::Past { row, target } => RowKey::Past {
                target: *target,
                agent: row.agent,
                session_id: row.session_id.clone(),
            },
            ViewRow::DetachedHeader { .. } => RowKey::DetachedHeader,
            ViewRow::Detached { row } => RowKey::Detached {
                detached_session: match &row.state {
                    AgentSessionState::Detached { session_id } => Some(*session_id),
                    AgentSessionState::Running { .. }
                    | AgentSessionState::Pending { .. }
                    | AgentSessionState::Past => None,
                },
                title: row.title.clone(),
            },
            ViewRow::ManagedHeader => RowKey::ManagedHeader,
            ViewRow::Managed { run } => RowKey::Managed(run.job_id.clone()),
            ViewRow::ManagedError(_) => RowKey::ManagedError,
            ViewRow::SupervisorUnreachable => RowKey::SupervisorUnreachable,
        }
    }
}

/// For each pane folder, the indices into `rows` of the past sessions that lie within it.
pub(crate) type PastByFolder = HashMap<PathBuf, Vec<usize>>;

/// The (tab index, folder) pairs whose past conversations are shown.
pub(crate) type ExpandedFolders = HashSet<(usize, PathBuf)>;

/// Works out which past sessions lie within which pane folder. It resolves each pane folder on
/// disk, so it belongs where a snapshot is taken, never on a keystroke; past sessions come
/// resolved from the scan.
pub(crate) fn past_by_folder(panes: &[PaneAgentInput], rows: &[AgentSessionRow]) -> PastByFolder {
    let past: Vec<(usize, &AgentSessionRow)> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| matches!(row.state, AgentSessionState::Past))
        .collect();
    let mut by_folder = PastByFolder::new();
    for folder in panes.iter().filter_map(|pane| pane.cwd.as_ref()) {
        if folder.as_os_str().is_empty() || by_folder.contains_key(folder) {
            continue;
        }
        let resolved = ResolvedPath::new(folder);
        let within = past
            .iter()
            .filter(|(_, row)| is_row_within(row, &resolved))
            .map(|(index, _)| *index)
            .collect();
        by_folder.insert(folder.clone(), within);
    }
    by_folder
}

fn pane_folder(pane: &PaneAgentInput) -> Option<&Path> {
    pane.cwd
        .as_deref()
        .filter(|folder| !folder.as_os_str().is_empty())
}

fn folders_with_past(panes: &[PaneAgentInput], past_by_folder: &PastByFolder) -> ExpandedFolders {
    panes
        .iter()
        .filter_map(|pane| {
            let folder = pane_folder(pane)?;
            past_by_folder
                .get(folder)
                .is_some_and(|past| !past.is_empty())
                .then(|| (pane.tab_index, folder.to_path_buf()))
        })
        .collect()
}

/// Lays out `rows` (from `build_rows`) by tab and pane, with the detached sessions last. Each
/// folder's past conversations sit once per tab, under a toggle after the last of the tab's panes
/// in that folder. A non-empty `query` keeps only the matching rows, plus the headers and panes
/// they sit under, and shows matching past rows without expanding.
pub(crate) fn view_rows(
    panes: &[PaneAgentInput],
    rows: &[AgentSessionRow],
    past_by_folder: &PastByFolder,
    expanded_folders: &ExpandedFolders,
    query: &str,
) -> Vec<ViewRow> {
    let query = normalized_query(query);
    let visible_past = past_by_folder
        .values()
        .flatten()
        .copied()
        .collect::<HashSet<_>>();
    let has_exact_match = panes.iter().any(|pane| {
        text_has_exact_words(&pane.tab_title, query)
            || pane_folder(pane)
                .is_some_and(|folder| text_has_exact_words(&folder.to_string_lossy(), query))
    }) || rows.iter().enumerate().any(|(index, row)| {
        (!matches!(row.state, AgentSessionState::Past) || visible_past.contains(&index))
            && row_has_exact_words(row, query)
    });
    let matches_row = |row: &AgentSessionRow| {
        query.is_empty()
            || if has_exact_match {
                row_has_exact_words(row, query)
            } else {
                score_row(row, query).is_some()
            }
    };
    let matches_text = |text: &str| {
        if has_exact_match {
            text_has_exact_words(text, query)
        } else {
            match_indices_case_insensitive(text, query).is_some()
        }
    };
    let mut lists_past_of_folder: HashMap<(usize, &Path), PaneViewLocator> = HashMap::new();
    for pane in panes {
        if let Some(folder) = pane_folder(pane) {
            lists_past_of_folder.insert((pane.tab_index, folder), pane.locator);
        }
    }

    let mut view = Vec::new();
    let mut current_tab = None;
    for pane in panes {
        let row = rows.iter().find(|row| match &row.state {
            AgentSessionState::Running { locator } | AgentSessionState::Pending { locator } => {
                *locator == pane.locator
            }
            AgentSessionState::Detached { .. } | AgentSessionState::Past => false,
        });
        let folder = pane_folder(pane);
        let lists_past = folder.is_some_and(|folder| {
            lists_past_of_folder.get(&(pane.tab_index, folder)) == Some(&pane.locator)
        });
        let folder_past: Vec<&AgentSessionRow> = folder
            .filter(|_| lists_past)
            .and_then(|folder| past_by_folder.get(folder))
            .into_iter()
            .flatten()
            .filter_map(|index| rows.get(*index))
            .filter(|row| matches_row(row))
            .collect();

        let pane_matches = query.is_empty()
            || row.is_some_and(matches_row)
            || matches_text(&pane.tab_title)
            || folder.is_some_and(|folder| matches_text(&folder.to_string_lossy()));
        if !pane_matches && folder_past.is_empty() {
            continue;
        }

        if current_tab != Some(pane.tab_index) {
            current_tab = Some(pane.tab_index);
            view.push(ViewRow::TabHeader {
                tab_index: pane.tab_index,
                title: pane.tab_title.clone(),
            });
        }
        let action = match (row, folder) {
            (Some(row), _) => action_for(row, Some(pane.locator))
                .map(ViewRowAction::Act)
                .unwrap_or(ViewRowAction::None),
            (None, Some(_)) => ViewRowAction::ChooseSession {
                target: pane.locator,
            },
            (None, None) => ViewRowAction::None,
        };
        view.push(ViewRow::Pane {
            input: pane.clone(),
            row: row.cloned(),
            action,
        });

        if let Some(folder) = folder.filter(|_| !folder_past.is_empty()) {
            let folder = folder.to_path_buf();
            let expanded =
                !query.is_empty() || expanded_folders.contains(&(pane.tab_index, folder.clone()));
            view.push(ViewRow::PastToggle {
                tab_index: pane.tab_index,
                folder,
                count: folder_past.len(),
                expanded,
            });
            if expanded {
                view.extend(folder_past.into_iter().map(|row| ViewRow::Past {
                    row: row.clone(),
                    target: pane.locator,
                }));
            }
        }
    }

    let detached: Vec<&AgentSessionRow> = rows
        .iter()
        .filter(|row| matches!(row.state, AgentSessionState::Detached { .. }) && matches_row(row))
        .collect();
    if !detached.is_empty() {
        view.push(ViewRow::DetachedHeader {
            count: detached.len(),
        });
        view.extend(
            detached
                .into_iter()
                .map(|row| ViewRow::Detached { row: row.clone() }),
        );
    }
    view
}

/// The detached section to show in place of the supervisor's sessions when it could not be asked.
pub(crate) fn supervisor_rows(error: &DetachedListError) -> Vec<ViewRow> {
    match error {
        DetachedListError::Unsupported => Vec::new(),
        DetachedListError::Unreachable(_) => {
            vec![
                ViewRow::DetachedHeader { count: 0 },
                ViewRow::SupervisorUnreachable,
            ]
        }
    }
}

fn managed_run_search_fields(run: &RunSummary) -> [Option<&str>; 6] {
    [
        Some("AgentCtrl"),
        Some(run.job_id.as_str()),
        Some(run.state.as_str()),
        Some(run.agent_name.as_str()),
        run.repo_url.as_deref(),
        run.branch.as_deref(),
    ]
}

pub(crate) fn managed_view_rows(
    runs: &[RunSummary],
    error: Option<&str>,
    query: &str,
) -> Vec<ViewRow> {
    let query = normalized_query(query);
    let has_exact_match = runs.iter().any(|run| {
        managed_run_search_fields(run)
            .into_iter()
            .flatten()
            .any(|text| text_has_exact_words(text, query))
    });
    let matches = |run: &RunSummary| {
        query.is_empty()
            || managed_run_search_fields(run)
                .into_iter()
                .flatten()
                .any(|text| {
                    if has_exact_match {
                        text_has_exact_words(text, query)
                    } else {
                        match_indices_case_insensitive(text, query).is_some()
                    }
                })
    };
    let mut matching = runs
        .iter()
        .filter(|run| matches(run))
        .cloned()
        .collect::<Vec<_>>();
    matching.sort_by_key(|run| {
        if is_finished_state(&run.state) {
            2
        } else if run.state == "paused" {
            1
        } else {
            0
        }
    });
    if matching.is_empty() && error.is_none() {
        return Vec::new();
    }
    let mut rows = vec![ViewRow::ManagedHeader];
    rows.extend(matching.into_iter().map(|run| ViewRow::Managed { run }));
    if let Some(error) = error {
        rows.push(ViewRow::ManagedError(error.to_owned()));
    }
    rows
}

/// Rows are one line each so the list can be uniform.
const ROW_HEIGHT: f32 = 30.;
const PANE_INDENT: f32 = 24.;
const PAST_INDENT: f32 = 40.;
const INITIAL_WIDTH: f32 = 1260.;
const INITIAL_HEIGHT: f32 = 760.;
const MIN_WIDTH: f32 = 760.;
const MIN_HEIGHT: f32 = 440.;
const WINDOW_MARGIN: f32 = 48.;
const ROW_TITLE_MAX_CHARS: usize = 80;
const ROW_DETAIL_MAX_CHARS: usize = 100;
const PREVIEW_TITLE_MAX_CHARS: usize = 160;
const PREVIEW_DETAIL_MAX_CHARS: usize = 240;
const PREVIEW_MESSAGE_MAX_CHARS: usize = 1_200;

pub(crate) struct AgentSessionsFocus {
    pub locator: PaneViewLocator,
    pub managed_job_id: Option<String>,
}

pub enum AgentSessionsViewEvent {
    Act(AgentSessionAction),
    ChooseSession { target: PaneViewLocator },
    OpenManaged(String),
    Close,
}

#[derive(Clone, Debug)]
pub enum AgentSessionsViewAction {
    ArrowUp,
    ArrowDown,
    Enter,
    Select(RowKey),
    Activate(RowKey),
}

pub fn init(app: &mut AppContext) {
    app.register_fixed_bindings([
        FixedBinding::new(
            "up",
            AgentSessionsViewAction::ArrowUp,
            id!(AgentSessionsView::ui_name()),
        ),
        FixedBinding::new(
            "down",
            AgentSessionsViewAction::ArrowDown,
            id!(AgentSessionsView::ui_name()),
        ),
        FixedBinding::new(
            "enter",
            AgentSessionsViewAction::Enter,
            id!(AgentSessionsView::ui_name()),
        ),
    ]);
}

/// The window's agent sessions by tab and pane, searchable, one action per row.
pub struct AgentSessionsView {
    query_editor: ViewHandle<EditorView>,
    fleet_model: ModelHandle<FleetModel>,
    panes: Vec<PaneAgentInput>,
    detached: Vec<DetachedInput>,
    supervisor_error: Option<DetachedListError>,
    rows: Vec<AgentSessionRow>,
    past_by_folder: PastByFolder,
    /// The search text `view_rows` was last built from.
    built_query: String,
    /// Whether past sessions have been scanned at least once; until then the list has none.
    history_loaded: bool,
    expanded_folders: ExpandedFolders,
    view_rows: Arc<Vec<ViewRow>>,
    /// Made when a row first appears and kept across renders and rebuilds.
    row_states: HashMap<RowKey, MouseStateHandle>,
    button_states: HashMap<RowKey, MouseStateHandle>,
    selected_index: Option<usize>,
    list_state: UniformListState,
    scroll_state: ScrollStateHandle,
    preview_scroll_state: ClippedScrollStateHandle,
    resizable_width: ResizableStateHandle,
    resizable_height: ResizableStateHandle,
    preferred_managed_job_id: Option<String>,
    pending_managed_open: Option<String>,
}

impl AgentSessionsView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let query_editor = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let mut editor = EditorView::single_line(
                SingleLineEditorOptions {
                    text: TextOptions::ui_text(Some(14.), appearance),
                    select_all_on_focus: true,
                    clear_selections_on_blur: true,
                    propagate_and_no_op_vertical_navigation_keys:
                        PropagateAndNoOpNavigationKeys::Always,
                    propagate_horizontal_navigation_keys: PropagateHorizontalNavigationKeys::Always,
                    ..Default::default()
                },
                ctx,
            );
            editor.set_placeholder_text("Search", ctx);
            editor
        });
        ctx.subscribe_to_view(&query_editor, |me, _, event, ctx| {
            me.handle_query_editor_event(event, ctx);
        });
        let fleet_model = FleetModel::handle(ctx);
        ctx.subscribe_to_model(&fleet_model, |me, _, event, ctx| match event {
            FleetModelEvent::Updated => me.rebuild_view_rows(ctx),
            FleetModelEvent::PreviewUpdated => ctx.notify(),
            FleetModelEvent::ResumeForAttachSucceeded { job_id, requester }
                if *requester == ctx.view_id()
                    && me.pending_managed_open.as_deref() == Some(job_id) =>
            {
                me.pending_managed_open = None;
                ctx.emit(AgentSessionsViewEvent::OpenManaged(job_id.clone()))
            }
            FleetModelEvent::ResumeForAttachSucceeded { .. } => {}
        });

        Self {
            query_editor,
            fleet_model,
            panes: Vec::new(),
            detached: Vec::new(),
            supervisor_error: None,
            rows: Vec::new(),
            past_by_folder: PastByFolder::new(),
            built_query: String::new(),
            history_loaded: false,
            expanded_folders: ExpandedFolders::new(),
            view_rows: Arc::new(Vec::new()),
            row_states: HashMap::new(),
            button_states: HashMap::new(),
            selected_index: None,
            list_state: UniformListState::new(),
            scroll_state: Arc::new(Mutex::new(Default::default())),
            preview_scroll_state: ClippedScrollStateHandle::new(),
            resizable_width: resizable_state_handle(INITIAL_WIDTH),
            resizable_height: resizable_state_handle(INITIAL_HEIGHT),
            preferred_managed_job_id: None,
            pending_managed_open: None,
        }
    }

    /// Replaces what the view lists and selects `focus`'s pane, clearing the search and any
    /// expanded folders.
    pub(crate) fn set_inputs(
        &mut self,
        panes: Vec<PaneAgentInput>,
        detached: Result<Vec<DetachedInput>, DetachedListError>,
        past: &[PastAgentSession],
        history_loaded: bool,
        focus: Option<AgentSessionsFocus>,
        ctx: &mut ViewContext<Self>,
    ) {
        let (focus, preferred_managed_job_id) = match focus {
            Some(focus) => (Some(focus.locator), focus.managed_job_id),
            None => (None, None),
        };
        self.panes = panes;
        self.history_loaded = history_loaded;
        self.preferred_managed_job_id = preferred_managed_job_id;
        self.pending_managed_open = None;
        self.selected_index = None;
        (self.detached, self.supervisor_error) = match detached {
            Ok(detached) => (detached, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        self.set_rows(past);
        self.expanded_folders = folders_with_past(&self.panes, &self.past_by_folder);
        self.query_editor.update(ctx, |editor, ctx| {
            editor.clear_buffer_and_reset_undo_stack(ctx)
        });
        self.rebuild_view_rows(ctx);
        if self.selected_index.is_none() {
            self.selected_index = focus.and_then(|focus| {
                self.view_rows.iter().position(
                    |row| matches!(row, ViewRow::Pane { input, .. } if input.locator == focus),
                )
            });
        }
        if let Some(index) = self.selected_index {
            self.list_state.scroll_to(index);
        }
        self.request_selected_preview(false, ctx);
        ctx.notify();
    }

    /// Rebuilds the rows with newly scanned past sessions, keeping the rest of the snapshot.
    pub(crate) fn refresh_past(&mut self, past: &[PastAgentSession], ctx: &mut ViewContext<Self>) {
        let expand_loaded_history = !self.history_loaded;
        self.history_loaded = true;
        self.set_rows(past);
        if expand_loaded_history {
            self.expanded_folders = folders_with_past(&self.panes, &self.past_by_folder);
        }
        self.rebuild_view_rows(ctx);
    }

    fn set_rows(&mut self, past: &[PastAgentSession]) {
        self.rows = build_rows(&self.panes, &self.detached, past);
        self.past_by_folder = past_by_folder(&self.panes, &self.rows);
    }

    #[cfg(test)]
    pub(crate) fn set_query(&mut self, query: &str, ctx: &mut ViewContext<Self>) {
        self.query_editor
            .update(ctx, |editor, ctx| editor.set_buffer_text(query, ctx));
    }

    #[cfg(test)]
    pub(crate) fn selected_managed_job_id(&self) -> Option<&str> {
        match self.view_rows.get(self.selected_index?)? {
            ViewRow::Managed { run } => Some(&run.job_id),
            ViewRow::TabHeader { .. }
            | ViewRow::Pane { .. }
            | ViewRow::PastToggle { .. }
            | ViewRow::Past { .. }
            | ViewRow::DetachedHeader { .. }
            | ViewRow::Detached { .. }
            | ViewRow::ManagedHeader
            | ViewRow::ManagedError(_)
            | ViewRow::SupervisorUnreachable => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn selected_pane(&self) -> Option<PaneViewLocator> {
        match self.view_rows.get(self.selected_index?)? {
            ViewRow::Pane { input, .. } => Some(input.locator),
            ViewRow::TabHeader { .. }
            | ViewRow::PastToggle { .. }
            | ViewRow::Past { .. }
            | ViewRow::DetachedHeader { .. }
            | ViewRow::Detached { .. }
            | ViewRow::ManagedHeader
            | ViewRow::Managed { .. }
            | ViewRow::ManagedError(_)
            | ViewRow::SupervisorUnreachable => None,
        }
    }

    fn rebuild_view_rows(&mut self, ctx: &mut ViewContext<Self>) {
        let selected = self
            .selected_index
            .and_then(|index| self.view_rows.get(index))
            .map(ViewRow::key);
        let query = self.query_editor.as_ref(ctx).buffer_text(ctx);
        let mut rows = view_rows(
            &self.panes,
            &self.rows,
            &self.past_by_folder,
            &self.expanded_folders,
            &query,
        );
        self.built_query = query.clone();
        if let Some(error) = &self.supervisor_error {
            rows.extend(supervisor_rows(error));
        }
        let model = self.fleet_model.as_ref(ctx);
        rows.extend(managed_view_rows(model.runs(), model.last_error(), &query));
        for row in &rows {
            let key = row.key();
            self.row_states.entry(key.clone()).or_default();
            self.button_states.entry(key).or_default();
        }
        self.view_rows = Arc::new(rows);
        let preferred_index = self
            .preferred_managed_job_id
            .as_deref()
            .and_then(|job_id| self.index_of(&RowKey::Managed(job_id.to_owned())));
        if preferred_index.is_some() {
            self.preferred_managed_job_id = None;
        }
        self.selected_index =
            preferred_index.or_else(|| selected.and_then(|selected| self.index_of(&selected)));
        if let Some(index) = self.selected_index {
            self.list_state.scroll_to(index);
        }
        self.request_selected_preview(false, ctx);
        ctx.notify();
    }

    fn handle_query_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Edited(_) => {
                // Clearing the search in `set_inputs` reports an edit only after the rows are
                // rebuilt and the asked-for pane selected; rebuilding again would lose that.
                if self.query_editor.as_ref(ctx).buffer_text(ctx) == self.built_query {
                    return;
                }
                self.preferred_managed_job_id = None;
                self.rebuild_view_rows(ctx);
                self.selected_index = self.next_selectable(0);
                if let Some(index) = self.selected_index {
                    self.list_state.scroll_to(index);
                }
            }
            EditorEvent::Navigate(NavigationKey::Down) => self.move_selection(true, ctx),
            EditorEvent::Navigate(NavigationKey::Up) => self.move_selection(false, ctx),
            EditorEvent::Enter => self.activate_selected(ctx),
            EditorEvent::Escape => ctx.emit(AgentSessionsViewEvent::Close),
            EditorEvent::Navigate(
                NavigationKey::Tab
                | NavigationKey::ShiftTab
                | NavigationKey::PageUp
                | NavigationKey::PageDown
                | NavigationKey::Left
                | NavigationKey::Right,
            )
            | EditorEvent::Activate
            | EditorEvent::Blurred
            | EditorEvent::Focused
            | EditorEvent::SelectionChanged
            | EditorEvent::AutosuggestionAccepted { .. }
            | EditorEvent::ShiftEnter
            | EditorEvent::AltEnter
            | EditorEvent::CtrlEnter
            | EditorEvent::BackspaceAtBeginningOfBuffer
            | EditorEvent::BackspaceOnEmptyBuffer
            | EditorEvent::CtrlC { .. }
            | EditorEvent::BufferReplaced
            | EditorEvent::BufferReinitialized
            | EditorEvent::CmdUpOnFirstRow
            | EditorEvent::Copy
            | EditorEvent::UnhandledModifierKeyOnEditor(_)
            | EditorEvent::ClearParentSelections
            | EditorEvent::CmdEnter
            | EditorEvent::TryToShowXRay(_)
            | EditorEvent::HideXRay
            | EditorEvent::InsertLastWordPrevCommand
            | EditorEvent::Search { .. }
            | EditorEvent::ExCommand
            | EditorEvent::VimStatusUpdate
            | EditorEvent::Paste
            | EditorEvent::MiddleClickPaste
            | EditorEvent::DeleteAllLeft
            | EditorEvent::EmacsBindingUsed
            | EditorEvent::UpdatePeers { .. }
            | EditorEvent::SetAIContextMenuOpen(_)
            | EditorEvent::AcceptAIContextMenuItem(_)
            | EditorEvent::SelectAIContextMenuCategory(_)
            | EditorEvent::ProcessingAttachedImages(_)
            | EditorEvent::VoiceStateUpdated { .. }
            | EditorEvent::DroppedImageFiles(_)
            | EditorEvent::IgnoreAutosuggestion { .. } => {}
        }
    }

    fn index_of(&self, key: &RowKey) -> Option<usize> {
        self.view_rows.iter().position(|row| row.key() == *key)
    }

    fn is_selectable(&self, index: usize) -> bool {
        self.view_rows.get(index).is_some_and(is_interactive)
    }

    fn next_selectable(&self, start: usize) -> Option<usize> {
        (start..self.view_rows.len()).find(|index| self.is_selectable(*index))
    }

    fn move_selection(&mut self, down: bool, ctx: &mut ViewContext<Self>) {
        let next = match (self.selected_index, down) {
            (None, true) => self.next_selectable(0),
            (None, false) => (0..self.view_rows.len())
                .rev()
                .find(|index| self.is_selectable(*index)),
            (Some(index), true) => self.next_selectable(index + 1),
            (Some(index), false) => (0..index).rev().find(|index| self.is_selectable(*index)),
        };
        if let Some(index) = next {
            self.preferred_managed_job_id = None;
            self.selected_index = Some(index);
            self.list_state.scroll_to(index);
            self.request_selected_preview(false, ctx);
            ctx.notify();
        }
    }

    fn request_selected_preview(&mut self, force: bool, ctx: &mut ViewContext<Self>) {
        let job_id = self
            .selected_index
            .and_then(|index| self.view_rows.get(index))
            .and_then(|row| match row {
                ViewRow::Managed { run } => Some(run.job_id.clone()),
                ViewRow::Pane { .. }
                | ViewRow::TabHeader { .. }
                | ViewRow::PastToggle { .. }
                | ViewRow::Past { .. }
                | ViewRow::DetachedHeader { .. }
                | ViewRow::Detached { .. }
                | ViewRow::ManagedHeader
                | ViewRow::ManagedError(_)
                | ViewRow::SupervisorUnreachable => None,
            });
        let view_id = ctx.view_id();
        self.fleet_model.update(ctx, |model, ctx| {
            model.select_preview(view_id, job_id, force, ctx)
        });
    }

    fn select(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        if !self.is_selectable(index) {
            return;
        }
        self.preferred_managed_job_id = None;
        self.selected_index = Some(index);
        self.list_state.scroll_to(index);
        self.request_selected_preview(false, ctx);
        ctx.notify();
    }

    fn activate_selected(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(index) = self.selected_index.or_else(|| self.next_selectable(0)) {
            self.activate(index, ctx);
        }
    }

    fn past_action(&self, index: usize) -> Option<AgentSessionAction> {
        match self.view_rows.get(index)? {
            ViewRow::Past { row, target } => action_for(row, Some(*target)),
            ViewRow::TabHeader { .. }
            | ViewRow::Pane { .. }
            | ViewRow::PastToggle { .. }
            | ViewRow::DetachedHeader { .. }
            | ViewRow::Detached { .. }
            | ViewRow::ManagedHeader
            | ViewRow::Managed { .. }
            | ViewRow::ManagedError(_)
            | ViewRow::SupervisorUnreachable => None,
        }
    }

    fn activate(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        let Some(row) = self.view_rows.get(index).cloned() else {
            return;
        };
        self.selected_index = Some(index);
        match row {
            ViewRow::Pane { action, .. } => match action {
                ViewRowAction::Act(action) => ctx.emit(AgentSessionsViewEvent::Act(action)),
                ViewRowAction::ChooseSession { target } => {
                    ctx.emit(AgentSessionsViewEvent::ChooseSession { target })
                }
                ViewRowAction::None => {}
            },
            ViewRow::PastToggle {
                tab_index, folder, ..
            } => {
                let expansion = (tab_index, folder);
                if !self.expanded_folders.remove(&expansion) {
                    self.expanded_folders.insert(expansion);
                }
                self.selected_index = Some(index);
                self.rebuild_view_rows(ctx);
            }
            ViewRow::Past { .. } => {
                if let Some(action) = self.past_action(index) {
                    ctx.emit(AgentSessionsViewEvent::Act(action));
                }
            }
            ViewRow::Detached { row } => {
                if let Some(action) = action_for(&row, None) {
                    ctx.emit(AgentSessionsViewEvent::Act(action));
                }
            }
            ViewRow::Managed { run } if run.state == "paused" => {
                let requester = ctx.view_id();
                self.pending_managed_open = Some(run.job_id.clone());
                self.fleet_model.update(ctx, |model, ctx| {
                    model.resume_for_attach(run.job_id, requester, ctx)
                });
            }
            ViewRow::Managed { run } if is_finished_state(&run.state) => {
                self.request_selected_preview(true, ctx);
            }
            ViewRow::Managed { run } => {
                ctx.emit(AgentSessionsViewEvent::OpenManaged(run.job_id));
            }
            ViewRow::TabHeader { .. }
            | ViewRow::DetachedHeader { .. }
            | ViewRow::ManagedHeader
            | ViewRow::ManagedError(_)
            | ViewRow::SupervisorUnreachable => {}
        }
    }

    fn render_list(&self, app: &AppContext) -> Box<dyn Element> {
        let theme = Appearance::as_ref(app).theme();
        let view_rows = self.view_rows.clone();
        let row_states = self.row_states.clone();
        let button_states = self.button_states.clone();
        let selected_index = self.selected_index;
        let list = UniformList::new(
            self.list_state.clone(),
            view_rows.len(),
            move |range: Range<usize>, app: &AppContext| {
                range
                    .filter_map(|index| {
                        let row = view_rows.get(index)?;
                        let key = row.key();
                        let row_state = row_states.get(&key)?.clone();
                        let button_state = button_states.get(&key)?.clone();
                        Some(render_row(
                            row,
                            key,
                            selected_index == Some(index),
                            row_state,
                            button_state,
                            app,
                        ))
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
            },
        )
        .finish_scrollable();

        Scrollable::vertical(
            self.scroll_state.clone(),
            list,
            ScrollbarWidth::Auto,
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            Fill::None,
        )
        .with_overlayed_scrollbar()
        .finish()
    }

    fn render_preview(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let font_size = appearance.ui_font_size();
        let main_color = theme.active_ui_text_color().into_solid();
        let sub_color = theme.sub_text_color(theme.surface_2()).into_solid();
        let Some(row) = self
            .selected_index
            .and_then(|index| self.view_rows.get(index))
        else {
            return Container::new(
                Text::new(
                    "Select a session to inspect its transcript",
                    font,
                    font_size,
                )
                .with_color(sub_color)
                .finish(),
            )
            .with_uniform_padding(20.)
            .finish();
        };

        let (title, detail) = preview_heading(row);
        let mut content = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(12.)
            .with_child(
                Text::new(
                    truncate_from_end(&title, PREVIEW_TITLE_MAX_CHARS),
                    font,
                    font_size + 2.,
                )
                .with_style(Properties::default().weight(Weight::Bold))
                .with_color(main_color)
                .finish(),
            );
        if let Some(detail) = detail {
            content.add_child(
                Text::new(
                    truncate_from_end(&detail, PREVIEW_DETAIL_MAX_CHARS),
                    font,
                    font_size - 1.,
                )
                .with_color(sub_color)
                .finish(),
            );
        }
        if let Some(error) = self.fleet_model.as_ref(app).action_error() {
            content.add_child(
                Container::new(
                    Text::new(error.to_owned(), font, font_size)
                        .with_color(theme.ui_error_color())
                        .finish(),
                )
                .with_uniform_padding(10.)
                .with_background(theme.surface_overlay_1())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
                .finish(),
            );
        }
        if let Some(action) = self.render_preview_action(row, app) {
            content.add_child(action);
        }
        match row {
            ViewRow::Pane { row: Some(row), .. }
            | ViewRow::Past { row, .. }
            | ViewRow::Detached { row } => {
                if row.preview.is_empty() {
                    content.add_child(preview_placeholder(
                        "No transcript preview is available for this session.",
                        app,
                    ));
                } else {
                    for message in &row.preview {
                        content.add_child(render_preview_message(
                            message.role.label(),
                            &message.text,
                            app,
                        ));
                    }
                }
            }
            ViewRow::Managed { run } => {
                let model = self.fleet_model.as_ref(app);
                if model.is_previewing(&run.job_id) {
                    content.add_child(preview_placeholder("Loading transcript…", app));
                } else {
                    match model.preview(&run.job_id) {
                        Some(Ok(preview)) if !preview.events.is_empty() => {
                            if preview.history_gap {
                                content.add_child(preview_placeholder(
                                    "Earlier retained history is unavailable.",
                                    app,
                                ));
                            }
                            if preview.truncated {
                                content.add_child(preview_placeholder(
                                    "Preview limited to the most recent fetched events.",
                                    app,
                                ));
                            }
                            for event in &preview.events {
                                content.add_child(render_managed_preview_event(event, app));
                            }
                        }
                        Some(Ok(_)) => content.add_child(preview_placeholder(
                            "This run has no transcript events yet.",
                            app,
                        )),
                        Some(Err(error)) => content.add_child(preview_error(error, app)),
                        None => content.add_child(preview_placeholder("Loading transcript…", app)),
                    }
                }
            }
            ViewRow::Pane { row: None, .. }
            | ViewRow::TabHeader { .. }
            | ViewRow::PastToggle { .. }
            | ViewRow::DetachedHeader { .. }
            | ViewRow::ManagedHeader
            | ViewRow::ManagedError(_)
            | ViewRow::SupervisorUnreachable => content.add_child(preview_placeholder(
                "Select a session row to inspect its transcript.",
                app,
            )),
        }

        ClippedScrollable::vertical(
            self.preview_scroll_state.clone(),
            Container::new(content.finish())
                .with_uniform_padding(20.)
                .finish(),
            ScrollbarWidth::Auto,
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            Fill::None,
        )
        .finish()
    }

    fn render_preview_action(&self, row: &ViewRow, app: &AppContext) -> Option<Box<dyn Element>> {
        let (key, label) = preview_action(row)?;
        let state = self.button_states.get(&key)?.clone();
        Some(
            Hoverable::new(state, move |state| {
                render_button(label, state.is_hovered(), app)
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(AgentSessionsViewAction::Activate(key.clone()));
            })
            .finish(),
        )
    }
}

fn session_label(row: &AgentSessionRow) -> String {
    let title = row_title(row);
    match row.agent.map(|agent| agent.display_name()) {
        Some(agent) if agent != title => format!("{agent} · {title}"),
        Some(_) | None => title,
    }
}

fn managed_agent_label(agent_name: &str) -> &str {
    if agent_name.contains("claude") {
        "Claude"
    } else if agent_name.contains("codex") {
        "Codex"
    } else if agent_name.contains("-pi-") || agent_name.ends_with("-pi") {
        "Pi"
    } else {
        agent_name
    }
}

fn managed_run_title(run: &RunSummary) -> String {
    run.repo_url
        .as_deref()
        .and_then(|url| {
            url.trim_end_matches('/')
                .trim_end_matches(".git")
                .rsplit(['/', ':'])
                .next()
        })
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| run.job_id.chars().take(16).collect())
}

fn managed_run_detail(run: &RunSummary) -> String {
    let mut details = vec![managed_agent_label(&run.agent_name), run.state.as_str()];
    if let Some(branch) = run.branch.as_deref().filter(|branch| !branch.is_empty()) {
        details.push(branch);
    }
    details.join(" · ")
}

fn managed_action_label(state: &str) -> &'static str {
    if state == "paused" {
        "Resume & attach"
    } else if is_finished_state(state) {
        "View transcript"
    } else {
        "Attach here"
    }
}

fn preview_heading(row: &ViewRow) -> (String, Option<String>) {
    match row {
        ViewRow::Pane {
            input,
            row: Some(row),
            ..
        } => (
            session_label(row),
            Some(local_preview_detail(
                row,
                input.cwd.as_ref().unwrap_or(&row.cwd),
            )),
        ),
        ViewRow::Pane {
            input, row: None, ..
        } => (
            format!("Pane {}", input.pane_number),
            input.cwd.as_ref().map(|cwd| cwd.display().to_string()),
        ),
        ViewRow::Past { row, .. } | ViewRow::Detached { row } => (
            session_label(row),
            Some(local_preview_detail(row, &row.cwd)),
        ),
        ViewRow::Managed { run } => {
            let mut details = vec![
                "AgentCtrl".to_owned(),
                managed_agent_label(&run.agent_name).to_owned(),
                run.state.clone(),
            ];
            if let Some(activity) = managed_activity(run) {
                details.push(activity_detail_label(activity));
            }
            if let Some(branch) = run.branch.as_deref().filter(|branch| !branch.is_empty()) {
                details.push(branch.to_owned());
            }
            (managed_run_title(run), Some(details.join(" · ")))
        }
        ViewRow::TabHeader { title, .. } => (title.clone(), None),
        ViewRow::PastToggle { .. } => ("Past sessions".to_owned(), None),
        ViewRow::DetachedHeader { .. } => ("Detached sessions".to_owned(), None),
        ViewRow::ManagedHeader => ("AgentCtrl sessions".to_owned(), None),
        ViewRow::ManagedError(error) => ("AgentCtrl unavailable".to_owned(), Some(error.clone())),
        ViewRow::SupervisorUnreachable => ("Supervisor unreachable".to_owned(), None),
    }
}

fn local_preview_detail(row: &AgentSessionRow, cwd: &Path) -> String {
    let mut details = vec!["Local".to_owned(), row.state.label().to_owned()];
    if let Some(last_used) = row.last_used {
        details.push(activity_detail_label(DateTime::<Local>::from(last_used)));
    }
    details.push(cwd.display().to_string());
    details.join(" · ")
}

fn managed_activity(run: &RunSummary) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(run.last_activity_at.as_deref()?)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Local))
}

fn activity_age_label(last_active: DateTime<Local>) -> String {
    format!(
        "last active {}",
        format_approx_duration_from_now(last_active)
    )
}

fn activity_detail_label(last_active: DateTime<Local>) -> String {
    let age = format_approx_duration_from_now(last_active);
    format!(
        "last active {age} ({})",
        last_active.format("%Y-%m-%d %H:%M")
    )
}

fn preview_action(row: &ViewRow) -> Option<(RowKey, &'static str)> {
    let label = match row {
        ViewRow::Pane { action, .. } => match action {
            ViewRowAction::Act(action) => action.label(),
            ViewRowAction::ChooseSession { .. } => "Choose…",
            ViewRowAction::None => return None,
        },
        ViewRow::Past { row, target } => action_for(row, Some(*target))?.label(),
        ViewRow::Detached { row } => action_for(row, None)?.label(),
        ViewRow::Managed { run } => managed_action_label(&run.state),
        ViewRow::TabHeader { .. }
        | ViewRow::PastToggle { .. }
        | ViewRow::DetachedHeader { .. }
        | ViewRow::ManagedHeader
        | ViewRow::ManagedError(_)
        | ViewRow::SupervisorUnreachable => return None,
    };
    Some((row.key(), label))
}

fn preview_placeholder(message: &str, app: &AppContext) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    Text::new(
        message.to_owned(),
        appearance.ui_font_family(),
        appearance.ui_font_size(),
    )
    .with_color(
        appearance
            .theme()
            .sub_text_color(appearance.theme().surface_2())
            .into_solid(),
    )
    .finish()
}

fn preview_error(message: &str, app: &AppContext) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    Text::new(
        message.to_owned(),
        appearance.ui_font_family(),
        appearance.ui_font_size(),
    )
    .with_color(appearance.theme().ui_error_color())
    .finish()
}

fn render_preview_message(label: &str, message: &str, app: &AppContext) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();
    let font = appearance.ui_font_family();
    let font_size = appearance.ui_font_size();
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.)
            .with_child(
                Text::new(label.to_owned(), font, font_size - 1.)
                    .with_style(Properties::default().weight(Weight::Bold))
                    .with_color(theme.sub_text_color(theme.surface_2()).into_solid())
                    .finish(),
            )
            .with_child(
                Text::new(
                    truncate_from_end(message, PREVIEW_MESSAGE_MAX_CHARS),
                    font,
                    font_size,
                )
                .with_color(theme.active_ui_text_color().into_solid())
                .finish(),
            )
            .finish(),
    )
    .with_uniform_padding(10.)
    .with_background(theme.surface_overlay_1())
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
    .finish()
}

fn render_managed_preview_event(event: &RunPreviewEvent, app: &AppContext) -> Box<dyn Element> {
    let mut label = event.kind.label().to_owned();
    if event.redacted {
        label.push_str(" · redacted");
    }
    if event.truncated {
        label.push_str(" · truncated");
    }
    render_preview_message(&label, &event.content, app)
}

fn is_interactive(row: &ViewRow) -> bool {
    match row {
        ViewRow::Pane { action, .. } => *action != ViewRowAction::None,
        ViewRow::PastToggle { .. } | ViewRow::Detached { .. } | ViewRow::Managed { .. } => true,
        ViewRow::Past { row, .. } => action_for(row, None).is_some(),
        ViewRow::TabHeader { .. }
        | ViewRow::DetachedHeader { .. }
        | ViewRow::ManagedHeader
        | ViewRow::ManagedError(_)
        | ViewRow::SupervisorUnreachable => false,
    }
}

fn render_row(
    row: &ViewRow,
    key: RowKey,
    is_selected: bool,
    row_state: MouseStateHandle,
    button_state: MouseStateHandle,
    app: &AppContext,
) -> Box<dyn Element> {
    if !is_interactive(row) {
        return render_row_contents(row, is_selected, false, None, app);
    }

    let content_key = key.clone();
    Hoverable::new(row_state, move |state| {
        render_row_contents(
            row,
            is_selected,
            state.is_hovered(),
            Some((content_key.clone(), button_state.clone())),
            app,
        )
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action(AgentSessionsViewAction::Select(key.clone()));
    })
    .finish()
}

fn render_row_contents(
    row: &ViewRow,
    is_selected: bool,
    is_hovered: bool,
    action_button: Option<(RowKey, MouseStateHandle)>,
    app: &AppContext,
) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();
    let font = appearance.ui_font_family();
    let font_size = appearance.ui_font_size();
    let main_color = theme.active_ui_text_color().into_solid();
    let sub_color = theme.sub_text_color(theme.surface_2()).into_solid();
    let text = |text: String, color| Text::new_inline(text, font, font_size).with_color(color);
    let clipped_text =
        |content: String, color| text(content, color).with_clip(ClipConfig::ellipsis());
    let bold = || Properties::default().weight(Weight::Bold);

    let (activity, state) = match row {
        ViewRow::Pane { row, .. } => (
            row.as_ref()
                .and_then(|row| row.last_used)
                .map(DateTime::<Local>::from)
                .map(activity_age_label),
            Some(row.as_ref().map_or("—", |row| row.state.label())),
        ),
        ViewRow::Past { row, .. } | ViewRow::Detached { row } => (
            row.last_used
                .map(DateTime::<Local>::from)
                .map(activity_age_label),
            Some(row.state.label()),
        ),
        ViewRow::Managed { run } => (managed_activity(run).map(activity_age_label), None),
        ViewRow::TabHeader { .. }
        | ViewRow::PastToggle { .. }
        | ViewRow::DetachedHeader { .. }
        | ViewRow::ManagedHeader
        | ViewRow::ManagedError(_)
        | ViewRow::SupervisorUnreachable => (None, None),
    };
    let (indent, label, detail, button): (f32, Box<dyn Element>, Option<String>, Option<&str>) =
        match row {
            ViewRow::TabHeader { title, .. } => (
                12.,
                clipped_text(truncate_from_end(title, ROW_TITLE_MAX_CHARS), main_color)
                    .with_style(bold())
                    .finish(),
                None,
                None,
            ),
            ViewRow::Pane { input, row, action } => {
                let label = row
                    .as_ref()
                    .map_or_else(|| "Shell".to_owned(), session_label);
                let button = match action {
                    ViewRowAction::Act(action) => Some(action.label()),
                    ViewRowAction::ChooseSession { .. } => Some("Choose…"),
                    ViewRowAction::None => None,
                };
                let label = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.)
                    .with_child(text(format!("pane {}", input.pane_number), sub_color).finish())
                    .with_child(
                        Shrinkable::new(
                            1.,
                            clipped_text(
                                truncate_from_end(&label, ROW_TITLE_MAX_CHARS),
                                main_color,
                            )
                            .finish(),
                        )
                        .finish(),
                    )
                    .finish();
                (
                    PANE_INDENT,
                    label,
                    input.cwd.as_ref().map(|cwd| cwd.display().to_string()),
                    button,
                )
            }
            ViewRow::PastToggle {
                count, expanded, ..
            } => {
                let chevron = if *expanded {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                };
                let label = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(4.)
                    .with_child(
                        ConstrainedBox::new(chevron.to_warpui_icon(sub_color.into()).finish())
                            .with_width(12.)
                            .with_height(12.)
                            .finish(),
                    )
                    .with_child(
                        clipped_text(format!("Past sessions in this folder ({count})"), sub_color)
                            .finish(),
                    )
                    .finish();
                (PANE_INDENT, label, None, None)
            }
            ViewRow::Past { row, .. } => (
                PAST_INDENT,
                clipped_text(
                    truncate_from_end(&session_label(row), ROW_TITLE_MAX_CHARS),
                    main_color,
                )
                .finish(),
                None,
                action_for(row, None).map(|action| action.label()),
            ),
            ViewRow::DetachedHeader { count } => {
                let title = match count {
                    0 => "Detached in supervisor".to_owned(),
                    count => format!("Detached in supervisor ({count})"),
                };
                (
                    12.,
                    clipped_text(title, main_color).with_style(bold()).finish(),
                    None,
                    None,
                )
            }
            ViewRow::Detached { row } => (
                PANE_INDENT,
                clipped_text(
                    truncate_from_end(&session_label(row), ROW_TITLE_MAX_CHARS),
                    main_color,
                )
                .finish(),
                Some(row.cwd.display().to_string()).filter(|cwd| !cwd.is_empty()),
                Some("Attach"),
            ),
            ViewRow::ManagedHeader => (
                12.,
                text("AgentCtrl".to_owned(), main_color)
                    .with_style(bold())
                    .finish(),
                None,
                None,
            ),
            ViewRow::Managed { run } => (
                PANE_INDENT,
                clipped_text(managed_run_title(run), main_color).finish(),
                Some(managed_run_detail(run)),
                Some(managed_action_label(&run.state)),
            ),
            ViewRow::ManagedError(error) => (
                PANE_INDENT,
                clipped_text(
                    truncate_from_end(error, ROW_TITLE_MAX_CHARS),
                    theme.ui_error_color(),
                )
                .finish(),
                None,
                None,
            ),
            ViewRow::SupervisorUnreachable => (
                PANE_INDENT,
                text("Supervisor unreachable".to_owned(), sub_color).finish(),
                None,
                None,
            ),
        };

    let detail = detail.map_or_else(
        || Flex::row().finish(),
        |detail| clipped_text(truncate_from_end(&detail, ROW_DETAIL_MAX_CHARS), sub_color).finish(),
    );
    let mut line = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(8.)
        .with_child(Shrinkable::new(1., label).finish())
        .with_child(Shrinkable::new(1., detail).finish());
    if let Some(activity) = activity {
        line = line.with_child(text(activity, sub_color).finish());
    }
    if let Some(state) = state {
        line = line.with_child(text(state.to_owned(), sub_color).finish());
    }
    if let (Some(button), Some((key, button_state))) = (button, action_button) {
        line = line.with_child(
            Hoverable::new(button_state, move |state| {
                render_button(button, state.is_hovered(), app)
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(AgentSessionsViewAction::Activate(key.clone()));
            })
            .finish(),
        );
    }

    let mut container = Container::new(line.finish())
        .with_padding_left(indent)
        .with_padding_right(12.);
    if is_selected || is_hovered {
        container = container.with_background(theme.surface_overlay_1());
    }
    ConstrainedBox::new(container.finish())
        .with_height(ROW_HEIGHT)
        .finish()
}

fn render_button(label: &str, is_hovered: bool, app: &AppContext) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let button_theme = SecondaryTheme;
    let background = button_theme.background(is_hovered, appearance);
    let label = Text::new_inline(
        label.to_owned(),
        appearance.ui_font_family(),
        appearance.ui_font_size() - 1.,
    )
    .with_color(button_theme.text_color(is_hovered, background, appearance))
    .finish();
    let mut button = Container::new(label)
        .with_horizontal_padding(8.)
        .with_vertical_padding(2.)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
    if let Some(background) = background {
        button = button.with_background(background);
    }
    if let Some(border) = button_theme.border(appearance) {
        button = button.with_border(Border::all(1.).with_border_color(border));
    }
    button.finish()
}

fn render_search_box(query_editor: &ViewHandle<EditorView>, app: &AppContext) -> Box<dyn Element> {
    let theme = Appearance::as_ref(app).theme();
    let search = Container::new(Clipped::new(ChildView::new(query_editor).finish()).finish())
        .with_padding(Padding::uniform(6.).with_left(12.).with_right(12.))
        .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .finish();
    Container::new(search)
        .with_horizontal_padding(12.)
        .with_vertical_padding(8.)
        .finish()
}

impl Entity for AgentSessionsView {
    type Event = AgentSessionsViewEvent;
}

impl TypedActionView for AgentSessionsView {
    type Action = AgentSessionsViewAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            AgentSessionsViewAction::ArrowUp => self.move_selection(false, ctx),
            AgentSessionsViewAction::ArrowDown => self.move_selection(true, ctx),
            AgentSessionsViewAction::Enter => self.activate_selected(ctx),
            AgentSessionsViewAction::Select(key) => {
                if let Some(index) = self.index_of(key) {
                    self.select(index, ctx);
                }
            }
            AgentSessionsViewAction::Activate(key) => {
                if let Some(index) = self.index_of(key) {
                    self.activate(index, ctx);
                }
            }
        }
    }
}

impl View for AgentSessionsView {
    fn ui_name() -> &'static str {
        "AgentSessionsView"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            ctx.focus(&self.query_editor);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let list = if self.view_rows.is_empty() {
            Container::new(
                Text::new_inline(
                    "No matching sessions",
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(theme.sub_text_color(theme.surface_2()).into_solid())
                .finish(),
            )
            .with_horizontal_padding(12.)
            .with_vertical_padding(8.)
            .finish()
        } else {
            self.render_list(app)
        };
        let mut master = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(render_search_box(&self.query_editor, app));
        if !self.history_loaded {
            master = master.with_child(
                Container::new(
                    Text::new_inline(
                        "Looking for past sessions…",
                        appearance.ui_font_family(),
                        appearance.ui_font_size() - 1.,
                    )
                    .with_color(theme.sub_text_color(theme.surface_2()).into_solid())
                    .finish(),
                )
                .with_horizontal_padding(12.)
                .with_padding_bottom(4.)
                .finish(),
            );
        }
        master.add_child(Shrinkable::new(1.0, list).finish());
        let master = Container::new(master.finish())
            .with_border(Border::right(1.).with_border_fill(theme.outline()))
            .finish();
        let content = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(ConstrainedBox::new(master).with_width(430.).finish())
            .with_child(Expanded::new(1., self.render_preview(app)).finish())
            .finish();
        let width_resizable = Resizable::new(self.resizable_width.clone(), content)
            .with_dragbar_side(DragBarSide::Right)
            .with_bounds_callback(Box::new(|window_size| {
                let max = (window_size.x() - WINDOW_MARGIN).max(MIN_WIDTH);
                (MIN_WIDTH, max)
            }))
            .on_resize(|ctx, _| ctx.notify())
            .finish();
        Resizable::new(self.resizable_height.clone(), width_resizable)
            .with_dragbar_side(DragBarSide::Bottom)
            .with_bounds_callback(Box::new(|window_size| {
                let max = (window_size.y() - WINDOW_MARGIN).max(MIN_HEIGHT);
                (MIN_HEIGHT, max)
            }))
            .on_resize(|ctx, _| ctx.notify())
            .finish()
    }
}

#[cfg(test)]
#[path = "agent_sessions_view_tests.rs"]
mod tests;
