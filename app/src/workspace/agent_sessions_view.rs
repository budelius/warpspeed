//! Full-window library for local and AgentCtrl sessions.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Local};
use managed_agent_sessions::{RunPreviewEvent, RunSummary};
use warp_core::ui::Icon;
use warp_editor::editor::NavigationKey;
use warpui::elements::{
    Border, ChildView, Clipped, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CornerRadius, CrossAxisAlignment, Element, Expanded, Fill, Flex, Hoverable,
    MainAxisSize, MouseStateHandle, Padding, ParentElement, Radius, ScrollStateHandle, Scrollable,
    ScrollableElement, ScrollbarWidth, Shrinkable, Text, UniformList, UniformListState,
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
use crate::agent_ctrl::session_picker::{
    managed_action_label, managed_agent_label, managed_run_detail, managed_run_title, matching_runs,
};
use crate::appearance::Appearance;
use crate::editor::{
    EditorView, Event as EditorEvent, PropagateAndNoOpNavigationKeys,
    PropagateHorizontalNavigationKeys, SingleLineEditorOptions, TextOptions,
};
use crate::search::command_palette::agent_sessions::{
    normalized_query, row_has_exact_words, row_title, score_row,
};
use crate::terminal::agent_session_history::{PastAgentSession, ResolvedPath};
use crate::terminal::agent_sessions_index::{
    AgentSessionRow, AgentSessionState, DetachedInput, PaneAgentInput, build_rows_with_scanned,
    is_row_within,
};
use crate::terminal::cli_agent::CLIAgent;
use crate::terminal::cli_agent_sessions::agent_process_scan::{SavedPane, ScannedAgent};
use crate::terminal::detached_sessions::DetachedListError;
use crate::util::time_format::format_approx_duration_from_now;
use crate::util::truncation::truncate_from_end;
use crate::view_components::action_button::{ActionButtonTheme, SecondaryTheme};
use crate::workspace::PaneViewLocator;
use crate::workspace::agent_session_actions::{AgentSessionAction, action_for};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum SessionFilter {
    #[default]
    All,
    CurrentDirectory,
    RunningAndDetached,
    AgentCtrl,
    Past,
}

impl SessionFilter {
    const ALL: [Self; 5] = [
        Self::All,
        Self::CurrentDirectory,
        Self::RunningAndDetached,
        Self::AgentCtrl,
        Self::Past,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All sessions",
            Self::CurrentDirectory => "Current directory",
            Self::RunningAndDetached => "Running & detached",
            Self::AgentCtrl => "AgentCtrl",
            Self::Past => "Past sessions",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ViewSection {
    CurrentDirectory,
    RunningAndDetached,
    AgentCtrl,
    Past,
}

/// One row in the session library. Local sessions are listed once and always carry the pane in
/// which a past conversation should resume.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ViewRow {
    SectionHeader {
        section: ViewSection,
        title: String,
        count: usize,
    },
    DateHeader(String),
    Local {
        row: AgentSessionRow,
        target: Option<PaneViewLocator>,
    },
    Managed {
        run: RunSummary,
    },
    ManagedError(String),
    SupervisorUnreachable,
}

/// What a row stands for, independent of where it sits in the list, so a click or hover still
/// reaches its row after the list is rebuilt.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RowKey {
    Section(ViewSection),
    Date(String),
    Local {
        agent: Option<CLIAgent>,
        session_id: Option<String>,
        pane: Option<PaneViewLocator>,
        detached_session: Option<warp_core::SessionId>,
        process_pid: Option<u32>,
    },
    Managed(String),
    ManagedError,
    SupervisorUnreachable,
}

impl ViewRow {
    pub(crate) fn key(&self) -> RowKey {
        match self {
            ViewRow::SectionHeader { section, .. } => RowKey::Section(*section),
            ViewRow::DateHeader(date) => RowKey::Date(date.clone()),
            ViewRow::Local { row, .. } => RowKey::Local {
                agent: row.agent,
                session_id: row.session_id.clone(),
                pane: match row.state {
                    AgentSessionState::Running { locator }
                    | AgentSessionState::Pending { locator } => Some(locator),
                    AgentSessionState::Detached { .. }
                    | AgentSessionState::KnownProcess { .. }
                    | AgentSessionState::HeldByOtherBuild { .. }
                    | AgentSessionState::NoKnownPane { .. }
                    | AgentSessionState::Past => None,
                },
                detached_session: match row.state {
                    AgentSessionState::Detached { session_id } => Some(session_id),
                    AgentSessionState::Running { .. }
                    | AgentSessionState::Pending { .. }
                    | AgentSessionState::KnownProcess { .. }
                    | AgentSessionState::HeldByOtherBuild { .. }
                    | AgentSessionState::NoKnownPane { .. }
                    | AgentSessionState::Past => None,
                },
                process_pid: match row.state {
                    AgentSessionState::KnownProcess { pid }
                    | AgentSessionState::HeldByOtherBuild { pid, .. }
                    | AgentSessionState::NoKnownPane { pid, .. } => Some(pid),
                    AgentSessionState::Running { .. }
                    | AgentSessionState::Pending { .. }
                    | AgentSessionState::Detached { .. }
                    | AgentSessionState::Past => None,
                },
            },
            ViewRow::Managed { run } => RowKey::Managed(run.job_id.clone()),
            ViewRow::ManagedError(_) => RowKey::ManagedError,
            ViewRow::SupervisorUnreachable => RowKey::SupervisorUnreachable,
        }
    }
}

fn focused_folder(
    panes: &[PaneAgentInput],
    target: Option<PaneViewLocator>,
) -> Option<(PathBuf, ResolvedPath)> {
    let path = panes
        .iter()
        .find(|pane| Some(pane.locator) == target)?
        .cwd
        .as_ref()
        .filter(|path| !path.as_os_str().is_empty())?
        .clone();
    Some((path.clone(), ResolvedPath::new(&path)))
}

fn is_running_or_detached(row: &AgentSessionRow) -> bool {
    !matches!(row.state, AgentSessionState::Past)
}

fn past_date_label(last_used: Option<std::time::SystemTime>) -> String {
    last_used.map_or_else(
        || "Unknown date".to_owned(),
        |last_used| {
            DateTime::<Local>::from(last_used)
                .format("%A, %B %-d, %Y")
                .to_string()
        },
    )
}

fn append_local_section(
    view: &mut Vec<ViewRow>,
    section: ViewSection,
    title: String,
    rows: Vec<&AgentSessionRow>,
    target: Option<PaneViewLocator>,
    group_by_date: bool,
) {
    if rows.is_empty() {
        return;
    }
    view.push(ViewRow::SectionHeader {
        section,
        title,
        count: rows.len(),
    });
    let mut date = None;
    for row in rows {
        if group_by_date {
            let row_date = past_date_label(row.last_used);
            if date.as_deref() != Some(row_date.as_str()) {
                date = Some(row_date.clone());
                view.push(ViewRow::DateHeader(row_date));
            }
        }
        view.push(ViewRow::Local {
            row: row.clone(),
            target,
        });
    }
}

/// Builds the local portion of the library. In the all-sessions view, sessions in the target
/// pane's directory are promoted to the first section and omitted from later sections.
pub(crate) fn view_rows(
    panes: &[PaneAgentInput],
    rows: &[AgentSessionRow],
    target: Option<PaneViewLocator>,
    filter: SessionFilter,
    query: &str,
) -> Vec<ViewRow> {
    let query = normalized_query(query);
    let focused_folder = focused_folder(panes, target);
    let is_current = |row: &AgentSessionRow| {
        focused_folder
            .as_ref()
            .is_some_and(|(_, folder)| is_row_within(row, folder))
    };
    let belongs_to_filter = |row: &AgentSessionRow| match filter {
        SessionFilter::All => true,
        SessionFilter::CurrentDirectory => is_current(row),
        SessionFilter::RunningAndDetached => is_running_or_detached(row),
        SessionFilter::AgentCtrl => false,
        SessionFilter::Past => matches!(row.state, AgentSessionState::Past),
    };
    let has_exact_match = rows
        .iter()
        .filter(|row| belongs_to_filter(row))
        .any(|row| row_has_exact_words(row, query));
    let matches_row = |row: &AgentSessionRow| {
        belongs_to_filter(row)
            && (query.is_empty()
                || if has_exact_match {
                    row_has_exact_words(row, query)
                } else {
                    score_row(row, query).is_some()
                })
    };
    let matching = rows
        .iter()
        .filter(|row| matches_row(row))
        .collect::<Vec<_>>();
    let mut view = Vec::new();

    if matches!(filter, SessionFilter::All | SessionFilter::CurrentDirectory) {
        let current = matching
            .iter()
            .copied()
            .filter(|row| is_current(row))
            .collect();
        let title = focused_folder.as_ref().map_or_else(
            || "Current directory".to_owned(),
            |(path, _)| format!("Current directory · {}", path.display()),
        );
        append_local_section(
            &mut view,
            ViewSection::CurrentDirectory,
            title,
            current,
            target,
            false,
        );
    }

    if matches!(
        filter,
        SessionFilter::All | SessionFilter::RunningAndDetached
    ) {
        let running = matching
            .iter()
            .copied()
            .filter(|row| is_running_or_detached(row))
            .filter(|row| filter != SessionFilter::All || !is_current(row))
            .collect();
        append_local_section(
            &mut view,
            ViewSection::RunningAndDetached,
            "Running & detached".to_owned(),
            running,
            target,
            false,
        );
    }

    if matches!(filter, SessionFilter::All | SessionFilter::Past) {
        let past = matching
            .iter()
            .copied()
            .filter(|row| matches!(row.state, AgentSessionState::Past))
            .filter(|row| filter != SessionFilter::All || !is_current(row))
            .collect();
        append_local_section(
            &mut view,
            ViewSection::Past,
            "Past sessions".to_owned(),
            past,
            target,
            true,
        );
    }
    view
}

/// The running section to show in place of the supervisor's sessions when it could not be asked.
pub(crate) fn supervisor_rows(error: &DetachedListError) -> Vec<ViewRow> {
    match error {
        DetachedListError::Unsupported => Vec::new(),
        DetachedListError::Unreachable(_) => vec![
            ViewRow::SectionHeader {
                section: ViewSection::RunningAndDetached,
                title: "Running & detached".to_owned(),
                count: 0,
            },
            ViewRow::SupervisorUnreachable,
        ],
    }
}

pub(crate) fn managed_view_rows(
    runs: &[RunSummary],
    error: Option<&str>,
    query: &str,
) -> Vec<ViewRow> {
    let matching = matching_runs(runs, query);
    if matching.is_empty() && error.is_none() {
        return Vec::new();
    }
    let mut rows = vec![ViewRow::SectionHeader {
        section: ViewSection::AgentCtrl,
        title: "AgentCtrl sessions".to_owned(),
        count: matching.len(),
    }];
    rows.extend(
        matching
            .into_iter()
            .map(|(run, _)| ViewRow::Managed { run: run.clone() }),
    );
    if let Some(error) = error {
        rows.push(ViewRow::ManagedError(error.to_owned()));
    }
    rows
}

/// Rows are one line each so the list can be uniform.
const ROW_HEIGHT: f32 = 36.;
const ROW_INDENT: f32 = 24.;
const DATE_INDENT: f32 = 24.;
const SIDEBAR_WIDTH: f32 = 232.;
const LIST_WIDTH: f32 = 460.;
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
    EndProcess {
        agent: CLIAgent,
        pid: u32,
        session_id: String,
    },
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
    SetFilter(SessionFilter),
    Close,
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

/// A full-window library of local and AgentCtrl sessions, with filtering and transcript preview.
pub struct AgentSessionsView {
    query_editor: ViewHandle<EditorView>,
    fleet_model: ModelHandle<FleetModel>,
    panes: Vec<PaneAgentInput>,
    detached: Vec<DetachedInput>,
    scanned_agents: Vec<ScannedAgent>,
    supervisor_error: Option<DetachedListError>,
    rows: Vec<AgentSessionRow>,
    target: Option<PaneViewLocator>,
    selected_filter: SessionFilter,
    /// The search text `view_rows` was last built from.
    built_query: String,
    /// Whether past sessions have been scanned at least once; until then the list has none.
    history_loaded: bool,
    view_rows: Arc<Vec<ViewRow>>,
    /// Made when a row first appears and kept across renders and rebuilds.
    row_states: HashMap<RowKey, MouseStateHandle>,
    button_states: HashMap<RowKey, MouseStateHandle>,
    filter_states: HashMap<SessionFilter, MouseStateHandle>,
    close_state: MouseStateHandle,
    selected_index: Option<usize>,
    list_state: UniformListState,
    scroll_state: ScrollStateHandle,
    preview_scroll_state: ClippedScrollStateHandle,
    preferred_managed_job_id: Option<String>,
    pending_managed_open: Option<String>,
    pending_process_end: Option<u32>,
    process_end_errors: HashMap<u32, String>,
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
            editor.set_placeholder_text("Search sessions", ctx);
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

        let filter_states = SessionFilter::ALL
            .into_iter()
            .map(|filter| (filter, MouseStateHandle::default()))
            .collect();
        Self {
            query_editor,
            fleet_model,
            panes: Vec::new(),
            detached: Vec::new(),
            scanned_agents: Vec::new(),
            supervisor_error: None,
            rows: Vec::new(),
            target: None,
            selected_filter: SessionFilter::All,
            built_query: String::new(),
            history_loaded: false,
            view_rows: Arc::new(Vec::new()),
            row_states: HashMap::new(),
            button_states: HashMap::new(),
            filter_states,
            close_state: MouseStateHandle::default(),
            selected_index: None,
            list_state: UniformListState::new(),
            scroll_state: Arc::new(Mutex::new(Default::default())),
            preview_scroll_state: ClippedScrollStateHandle::new(),
            preferred_managed_job_id: None,
            pending_managed_open: None,
            pending_process_end: None,
            process_end_errors: HashMap::new(),
        }
    }

    /// Replaces what the view lists and promotes `focus`'s directory to the first section.
    #[cfg(test)]
    pub(crate) fn set_inputs(
        &mut self,
        panes: Vec<PaneAgentInput>,
        detached: Result<Vec<DetachedInput>, DetachedListError>,
        past: &[PastAgentSession],
        history_loaded: bool,
        focus: Option<AgentSessionsFocus>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.set_inputs_with_scan(panes, detached, (past, &[]), history_loaded, focus, ctx);
    }

    pub(crate) fn set_inputs_with_scan(
        &mut self,
        panes: Vec<PaneAgentInput>,
        detached: Result<Vec<DetachedInput>, DetachedListError>,
        history: (&[PastAgentSession], &[ScannedAgent]),
        history_loaded: bool,
        focus: Option<AgentSessionsFocus>,
        ctx: &mut ViewContext<Self>,
    ) {
        let (past, scanned_agents) = history;
        let (focus, preferred_managed_job_id) = match focus {
            Some(focus) => (Some(focus.locator), focus.managed_job_id),
            None => (None, None),
        };
        let prefer_managed = preferred_managed_job_id.is_some();
        self.panes = panes;
        self.target = focus;
        self.selected_filter = SessionFilter::All;
        self.scanned_agents = scanned_agents.to_vec();
        self.history_loaded = history_loaded;
        self.preferred_managed_job_id = preferred_managed_job_id;
        self.pending_managed_open = None;
        self.pending_process_end = None;
        self.process_end_errors.clear();
        self.selected_index = None;
        (self.detached, self.supervisor_error) = match detached {
            Ok(detached) => (detached, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        self.set_rows(past);
        self.query_editor.update(ctx, |editor, ctx| {
            editor.clear_buffer_and_reset_undo_stack(ctx)
        });
        self.rebuild_view_rows(ctx);
        if !prefer_managed
            && let Some(index) = focus.and_then(|focus| {
                self.view_rows.iter().position(|row| {
                    matches!(
                        row,
                        ViewRow::Local {
                            row: AgentSessionRow {
                                state: AgentSessionState::Running { locator }
                                    | AgentSessionState::Pending { locator },
                                ..
                            },
                            ..
                        } if *locator == focus
                    )
                })
            })
        {
            self.selected_index = Some(index);
        }
        if self.selected_index.is_none() {
            self.selected_index = self.next_selectable(0);
        }
        if let Some(index) = self.selected_index {
            self.list_state.scroll_to(index);
        }
        self.request_selected_preview(false, ctx);
        ctx.notify();
    }

    /// Rebuilds the rows with newly scanned past sessions, keeping the rest of the snapshot.
    pub(crate) fn refresh_snapshot(
        &mut self,
        past: &[PastAgentSession],
        scanned_agents: &[ScannedAgent],
        ctx: &mut ViewContext<Self>,
    ) {
        self.history_loaded = true;
        self.scanned_agents = scanned_agents.to_vec();
        self.set_rows(past);
        self.rebuild_view_rows(ctx);
    }

    fn set_rows(&mut self, past: &[PastAgentSession]) {
        self.rows =
            build_rows_with_scanned(&self.panes, &self.detached, past, &self.scanned_agents);
    }

    pub(crate) fn set_process_end_error(
        &mut self,
        pid: u32,
        error: String,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self
            .rows
            .iter()
            .any(|row| endable_process_pid(row) == Some(pid))
        {
            return;
        }
        self.pending_process_end = None;
        self.process_end_errors.insert(pid, error);
        ctx.notify();
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
            ViewRow::SectionHeader { .. }
            | ViewRow::DateHeader(_)
            | ViewRow::Local { .. }
            | ViewRow::ManagedError(_)
            | ViewRow::SupervisorUnreachable => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn selected_pane(&self) -> Option<PaneViewLocator> {
        self.target
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
            self.target,
            self.selected_filter,
            &query,
        );
        self.built_query = query.clone();
        let past = rows
            .iter()
            .position(|row| {
                matches!(
                    row,
                    ViewRow::SectionHeader {
                        section: ViewSection::Past,
                        ..
                    }
                )
            })
            .map(|index| rows.split_off(index))
            .unwrap_or_default();
        if matches!(
            self.selected_filter,
            SessionFilter::All | SessionFilter::RunningAndDetached
        ) && let Some(error) = &self.supervisor_error
        {
            if rows.iter().any(|row| {
                matches!(
                    row,
                    ViewRow::SectionHeader {
                        section: ViewSection::RunningAndDetached,
                        ..
                    }
                )
            }) {
                rows.push(ViewRow::SupervisorUnreachable);
            } else {
                rows.extend(supervisor_rows(error));
            }
        }
        let model = self.fleet_model.as_ref(ctx);
        if matches!(
            self.selected_filter,
            SessionFilter::All | SessionFilter::AgentCtrl
        ) {
            rows.extend(managed_view_rows(model.runs(), model.last_error(), &query));
        }
        rows.extend(past);
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
        self.selected_index = preferred_index
            .or_else(|| selected.and_then(|selected| self.index_of(&selected)))
            .or_else(|| self.next_selectable(0));
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
            self.pending_process_end = None;
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
                ViewRow::SectionHeader { .. }
                | ViewRow::DateHeader(_)
                | ViewRow::Local { .. }
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
        if self.selected_index != Some(index) {
            self.pending_process_end = None;
        }
        self.selected_index = Some(index);
        self.list_state.scroll_to(index);
        self.request_selected_preview(false, ctx);
        ctx.notify();
    }

    pub(crate) fn open_managed(&mut self, job_id: &str, ctx: &mut ViewContext<Self>) {
        let Some(index) = self.index_of(&RowKey::Managed(job_id.to_owned())) else {
            self.fleet_model.update(ctx, |model, ctx| {
                model.set_action_error(
                    "This AgentCtrl session is no longer available. Refresh and try again."
                        .to_owned(),
                    ctx,
                )
            });
            return;
        };
        self.select(index, ctx);
        self.activate(index, ctx);
    }

    fn activate_selected(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(index) = self.selected_index.or_else(|| self.next_selectable(0)) {
            self.activate(index, ctx);
        }
    }

    fn set_filter(&mut self, filter: SessionFilter, ctx: &mut ViewContext<Self>) {
        if self.selected_filter == filter {
            return;
        }
        self.selected_filter = filter;
        self.selected_index = None;
        self.pending_process_end = None;
        self.preferred_managed_job_id = None;
        self.rebuild_view_rows(ctx);
    }

    fn activate(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        let Some(row) = self.view_rows.get(index).cloned() else {
            return;
        };
        self.selected_index = Some(index);
        match row {
            ViewRow::Local { row, target } => match &row.state {
                AgentSessionState::HeldByOtherBuild { pid, .. }
                | AgentSessionState::NoKnownPane { pid, .. } => {
                    if self.pending_process_end != Some(*pid) {
                        self.process_end_errors.remove(pid);
                    }
                    if !confirm_process_end(&mut self.pending_process_end, *pid) {
                        ctx.notify();
                        return;
                    }
                    if let (Some(agent), Some(session_id)) = (row.agent, row.session_id) {
                        ctx.emit(AgentSessionsViewEvent::EndProcess {
                            agent,
                            pid: *pid,
                            session_id,
                        });
                    }
                }
                AgentSessionState::Running { .. }
                | AgentSessionState::Pending { .. }
                | AgentSessionState::Detached { .. }
                | AgentSessionState::Past => {
                    if let Some(action) = action_for(&row, target) {
                        ctx.emit(AgentSessionsViewEvent::Act(action));
                    }
                }
                AgentSessionState::KnownProcess { .. } => {}
            },
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
            ViewRow::SectionHeader { .. }
            | ViewRow::DateHeader(_)
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
        let pending_process_end = self.pending_process_end;
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
                            pending_process_end,
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
        if let ViewRow::Local { row, .. } = row {
            if row.process_count > 1 {
                content.add_child(preview_error(
                    &format!(
                        "Warning: {} live processes claim this session. End process affects only the selected PID.",
                        row.process_count
                    ),
                    app,
                ));
            }
            if let Some(pid) = endable_process_pid(row)
                && let Some(error) = self.process_end_errors.get(&pid)
            {
                content.add_child(preview_error(error, app));
            }
            if let Some(message) = process_warning(row) {
                content.add_child(preview_error(&message, app));
            }
        }
        match row {
            ViewRow::Local { row, .. } => {
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
            ViewRow::SectionHeader { .. }
            | ViewRow::DateHeader(_)
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
        let (key, label) = preview_action(row, self.pending_process_end)?;
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

fn preview_heading(row: &ViewRow) -> (String, Option<String>) {
    match row {
        ViewRow::Local { row, .. } => (
            session_label(row),
            Some(local_preview_detail(row, &row.cwd)),
        ),
        ViewRow::Managed { run } => {
            let mut details = vec![
                "Remote AgentCtrl".to_owned(),
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
        ViewRow::SectionHeader { title, .. } => (title.clone(), None),
        ViewRow::DateHeader(date) => (date.clone(), None),
        ViewRow::ManagedError(error) => ("AgentCtrl unavailable".to_owned(), Some(error.clone())),
        ViewRow::SupervisorUnreachable => ("Supervisor unreachable".to_owned(), None),
    }
}

fn saved_pane_label(saved: &SavedPane) -> String {
    format!("saved in {} / {}", saved.session_name, saved.tab_title)
}

fn placement_detail(row: &AgentSessionRow) -> Option<String> {
    match &row.state {
        AgentSessionState::KnownProcess { pid } => {
            Some(format!("running under this WarpSpeed build · PID {pid}"))
        }
        AgentSessionState::HeldByOtherBuild {
            build, saved_in, ..
        } => Some(
            [
                Some(format!("held by WarpSpeed build {build}")),
                saved_in.as_ref().map(saved_pane_label),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · "),
        ),
        AgentSessionState::NoKnownPane { saved_in, .. } => Some(
            [
                Some("no known terminal pane".to_owned()),
                saved_in.as_ref().map(saved_pane_label),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · "),
        ),
        AgentSessionState::Running { .. }
        | AgentSessionState::Pending { .. }
        | AgentSessionState::Detached { .. }
        | AgentSessionState::Past => None,
    }
}

fn confirm_process_end(pending: &mut Option<u32>, pid: u32) -> bool {
    if *pending == Some(pid) {
        *pending = None;
        true
    } else {
        *pending = Some(pid);
        false
    }
}

fn endable_process_pid(row: &AgentSessionRow) -> Option<u32> {
    match row.state {
        AgentSessionState::HeldByOtherBuild { pid, .. }
        | AgentSessionState::NoKnownPane { pid, .. } => Some(pid),
        AgentSessionState::Running { .. }
        | AgentSessionState::Pending { .. }
        | AgentSessionState::Detached { .. }
        | AgentSessionState::KnownProcess { .. }
        | AgentSessionState::Past => None,
    }
}

fn process_warning(row: &AgentSessionRow) -> Option<String> {
    match &row.state {
        AgentSessionState::HeldByOtherBuild { pid, build, .. } => Some(format!(
            "Warning: this process is held by WarpSpeed build {build}. End process, then Confirm end, terminates only PID {pid}."
        )),
        AgentSessionState::NoKnownPane { pid, .. } => Some(format!(
            "Warning: no terminal pane owns this process. End process, then Confirm end, terminates only PID {pid}."
        )),
        AgentSessionState::Running { .. }
        | AgentSessionState::Pending { .. }
        | AgentSessionState::KnownProcess { .. }
        | AgentSessionState::Detached { .. }
        | AgentSessionState::Past => None,
    }
}

fn local_preview_detail(row: &AgentSessionRow, cwd: &Path) -> String {
    let mut details = vec!["Local".to_owned(), row.state.label().to_owned()];
    if let Some(placement) = placement_detail(row) {
        details.push(placement);
    }
    if let Some(last_used) = row.last_used {
        details.push(activity_detail_label(DateTime::<Local>::from(last_used)));
    }
    if !cwd.as_os_str().is_empty() {
        details.push(cwd.display().to_string());
    }
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

fn preview_action(
    row: &ViewRow,
    pending_process_end: Option<u32>,
) -> Option<(RowKey, &'static str)> {
    let label = match row {
        ViewRow::Local { row, target } => match row.state {
            AgentSessionState::HeldByOtherBuild { pid, .. }
            | AgentSessionState::NoKnownPane { pid, .. } => {
                if pending_process_end == Some(pid) {
                    "Confirm end"
                } else {
                    "End process…"
                }
            }
            AgentSessionState::Running { .. }
            | AgentSessionState::Pending { .. }
            | AgentSessionState::Detached { .. }
            | AgentSessionState::Past => action_for(row, *target)?.label(),
            AgentSessionState::KnownProcess { .. } => return None,
        },
        ViewRow::Managed { run } => managed_action_label(&run.state),
        ViewRow::SectionHeader { .. }
        | ViewRow::DateHeader(_)
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
        ViewRow::Local { row, target } => {
            matches!(
                row.state,
                AgentSessionState::HeldByOtherBuild { .. } | AgentSessionState::NoKnownPane { .. }
            ) || action_for(row, *target).is_some()
        }
        ViewRow::Managed { .. } => true,
        ViewRow::SectionHeader { .. }
        | ViewRow::DateHeader(_)
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
    pending_process_end: Option<u32>,
    app: &AppContext,
) -> Box<dyn Element> {
    if !is_interactive(row) {
        return render_row_contents(row, is_selected, false, None, pending_process_end, app);
    }

    let content_key = key.clone();
    Hoverable::new(row_state, move |state| {
        render_row_contents(
            row,
            is_selected,
            state.is_hovered(),
            Some((content_key.clone(), button_state.clone())),
            pending_process_end,
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
    pending_process_end: Option<u32>,
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
        ViewRow::Local { row, .. } => (
            row.last_used
                .map(DateTime::<Local>::from)
                .map(activity_age_label),
            Some(row.state.label()),
        ),
        ViewRow::Managed { run } => (managed_activity(run).map(activity_age_label), None),
        ViewRow::SectionHeader { .. }
        | ViewRow::DateHeader(_)
        | ViewRow::ManagedError(_)
        | ViewRow::SupervisorUnreachable => (None, None),
    };
    let (indent, label, detail, button): (f32, Box<dyn Element>, Option<String>, Option<&str>) =
        match row {
            ViewRow::SectionHeader { title, count, .. } => (
                12.,
                clipped_text(
                    format!(
                        "{} ({count})",
                        truncate_from_end(title, ROW_TITLE_MAX_CHARS)
                    ),
                    main_color,
                )
                .with_style(bold())
                .finish(),
                None,
                None,
            ),
            ViewRow::DateHeader(date) => (
                DATE_INDENT,
                clipped_text(date.clone(), sub_color)
                    .with_style(bold())
                    .finish(),
                None,
                None,
            ),
            view_row @ ViewRow::Local { row, .. } => {
                let detail = [
                    placement_detail(row),
                    Some(row.cwd.display().to_string()).filter(|cwd| !cwd.is_empty()),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                (
                    ROW_INDENT,
                    clipped_text(
                        truncate_from_end(&session_label(row), ROW_TITLE_MAX_CHARS),
                        main_color,
                    )
                    .finish(),
                    Some(detail).filter(|detail| !detail.is_empty()),
                    preview_action(view_row, pending_process_end).map(|(_, label)| label),
                )
            }
            ViewRow::Managed { run } => (
                ROW_INDENT,
                clipped_text(managed_run_title(run), main_color).finish(),
                Some(managed_run_detail(run)),
                Some(managed_action_label(&run.state)),
            ),
            ViewRow::ManagedError(error) => (
                ROW_INDENT,
                clipped_text(
                    truncate_from_end(error, ROW_TITLE_MAX_CHARS),
                    theme.ui_error_color(),
                )
                .finish(),
                None,
                None,
            ),
            ViewRow::SupervisorUnreachable => (
                ROW_INDENT,
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

fn filter_count(
    filter: SessionFilter,
    rows: &[AgentSessionRow],
    managed_count: usize,
    current_folder: Option<&ResolvedPath>,
) -> usize {
    match filter {
        SessionFilter::All => rows.len() + managed_count,
        SessionFilter::CurrentDirectory => current_folder.map_or(0, |folder| {
            rows.iter().filter(|row| is_row_within(row, folder)).count()
        }),
        SessionFilter::RunningAndDetached => rows
            .iter()
            .filter(|row| is_running_or_detached(row))
            .count(),
        SessionFilter::AgentCtrl => managed_count,
        SessionFilter::Past => rows
            .iter()
            .filter(|row| matches!(row.state, AgentSessionState::Past))
            .count(),
    }
}

fn render_filter_button(
    filter: SessionFilter,
    count: usize,
    is_selected: bool,
    state: MouseStateHandle,
    app: &AppContext,
) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();
    let font = appearance.ui_font_family();
    let font_size = appearance.ui_font_size();
    Hoverable::new(state, move |state| {
        let color = theme.active_ui_text_color().into_solid();
        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Expanded::new(
                    1.,
                    Text::new_inline(filter.label(), font, font_size)
                        .with_color(color)
                        .finish(),
                )
                .finish(),
            )
            .with_child(
                Text::new_inline(count.to_string(), font, font_size - 1.)
                    .with_color(theme.sub_text_color(theme.surface_2()).into_solid())
                    .finish(),
            )
            .finish();
        let mut container = Container::new(row)
            .with_horizontal_padding(10.)
            .with_vertical_padding(7.)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
        if is_selected || state.is_hovered() {
            container = container.with_background(theme.surface_overlay_1());
        }
        container.finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action(AgentSessionsViewAction::SetFilter(filter));
    })
    .finish()
}

fn render_header(close_state: MouseStateHandle, app: &AppContext) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();
    let font = appearance.ui_font_family();
    let title = Flex::column()
        .with_spacing(2.)
        .with_child(
            Text::new_inline("Session library", font, appearance.ui_font_size() + 2.)
                .with_style(Properties::default().weight(Weight::Bold))
                .with_color(theme.active_ui_text_color().into_solid())
                .finish(),
        )
        .with_child(
            Text::new_inline(
                "Open local and AgentCtrl sessions without leaving this view",
                font,
                appearance.ui_font_size() - 1.,
            )
            .with_color(theme.sub_text_color(theme.surface_2()).into_solid())
            .finish(),
        )
        .finish();
    let close = Hoverable::new(close_state, move |state| {
        let mut container = Container::new(
            ConstrainedBox::new(
                Icon::X
                    .to_warpui_icon(theme.active_ui_text_color())
                    .finish(),
            )
            .with_width(14.)
            .with_height(14.)
            .finish(),
        )
        .with_uniform_padding(8.)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
        if state.is_hovered() {
            container = container.with_background(theme.surface_overlay_1());
        }
        container.finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(|ctx, _, _| ctx.dispatch_typed_action(AgentSessionsViewAction::Close))
    .finish();
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1., title).finish())
            .with_child(close)
            .finish(),
    )
    .with_horizontal_padding(20.)
    .with_vertical_padding(12.)
    .with_border(Border::bottom(1.).with_border_fill(theme.outline()))
    .finish()
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
            AgentSessionsViewAction::SetFilter(filter) => self.set_filter(*filter, ctx),
            AgentSessionsViewAction::Close => ctx.emit(AgentSessionsViewEvent::Close),
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
            .with_horizontal_padding(16.)
            .with_vertical_padding(12.)
            .finish()
        } else {
            self.render_list(app)
        };

        let current_folder = focused_folder(&self.panes, self.target);
        let managed_count = self.fleet_model.as_ref(app).runs().len();
        let mut filters = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(2.);
        for filter in SessionFilter::ALL {
            let count = filter_count(
                filter,
                &self.rows,
                managed_count,
                current_folder.as_ref().map(|(_, folder)| folder),
            );
            let state = self.filter_states.get(&filter).cloned().unwrap_or_default();
            filters.add_child(render_filter_button(
                filter,
                count,
                filter == self.selected_filter,
                state,
                app,
            ));
        }
        let sidebar = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(render_search_box(&self.query_editor, app))
                .with_child(
                    Container::new(filters.finish())
                        .with_horizontal_padding(12.)
                        .finish(),
                )
                .finish(),
        )
        .with_border(Border::right(1.).with_border_fill(theme.outline()))
        .finish();

        let mut master = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
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
                .with_vertical_padding(8.)
                .finish(),
            );
        }
        master.add_child(Shrinkable::new(1., list).finish());
        let master = Container::new(master.finish())
            .with_border(Border::right(1.).with_border_fill(theme.outline()))
            .finish();
        let content = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                ConstrainedBox::new(sidebar)
                    .with_width(SIDEBAR_WIDTH)
                    .finish(),
            )
            .with_child(ConstrainedBox::new(master).with_width(LIST_WIDTH).finish())
            .with_child(Expanded::new(1., self.render_preview(app)).finish())
            .finish();

        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(render_header(self.close_state.clone(), app))
            .with_child(Expanded::new(1., content).finish())
            .finish()
    }
}

#[cfg(test)]
#[path = "agent_sessions_view_tests.rs"]
mod tests;
