//! The window's agent sessions, grouped by tab and pane.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Local};
use fuzzy_match::match_indices_case_insensitive;
use warp_core::ui::Icon;
use warp_editor::editor::NavigationKey;
use warpui::elements::{
    Border, ChildView, Clipped, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment,
    Element, Fill, Flex, Hoverable, MainAxisSize, MouseStateHandle, Padding, ParentElement, Radius,
    ScrollStateHandle, Scrollable, ScrollableElement, ScrollbarWidth, Shrinkable, Text,
    UniformList, UniformListState,
};
use warpui::fonts::{Properties, Weight};
use warpui::keymap::FixedBinding;
use warpui::keymap::macros::*;
use warpui::platform::Cursor;
use warpui::{
    AppContext, Entity, FocusContext, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::appearance::Appearance;
use crate::editor::{
    EditorView, Event as EditorEvent, PropagateAndNoOpNavigationKeys,
    PropagateHorizontalNavigationKeys, SingleLineEditorOptions, TextOptions,
};
use crate::search::command_palette::agent_sessions::{row_title, score_row};
use crate::terminal::agent_session_history::{PastAgentSession, ResolvedPath, is_within_resolved};
use crate::terminal::agent_sessions_index::{
    AgentSessionRow, AgentSessionState, DetachedInput, PaneAgentInput, build_rows,
};
use crate::terminal::detached_sessions::DetachedListError;
use crate::util::time_format::format_approx_duration_from_now;
use crate::workspace::PaneViewLocator;
use crate::workspace::agent_session_actions::{AgentSessionAction, action_for};

/// The rows the view draws, grouped as the window is.
// A window lists a few dozen rows at most, so boxing the pane variant would only add indirection.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ViewRow {
    TabHeader {
        title: String,
    },
    Pane {
        input: PaneAgentInput,
        row: Option<AgentSessionRow>,
        action: ViewRowAction,
    },
    PastToggle {
        folder: PathBuf,
        count: usize,
        expanded: bool,
    },
    Past {
        row: AgentSessionRow,
    },
    DetachedHeader {
        count: usize,
    },
    Detached {
        row: AgentSessionRow,
    },
    SupervisorUnreachable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ViewRowAction {
    Act(AgentSessionAction),
    /// Pick a past conversation of `folder` to resume in `target`.
    ResumeInFolder {
        folder: PathBuf,
        target: PaneViewLocator,
    },
    None,
}

/// For each pane folder, the indices into `rows` of the past sessions that lie within it.
pub(crate) type PastByFolder = HashMap<PathBuf, Vec<usize>>;

/// Works out which past sessions lie within which pane folder. It resolves every path on disk
/// once, so it belongs where a snapshot is taken, never on a keystroke.
pub(crate) fn past_by_folder(panes: &[PaneAgentInput], rows: &[AgentSessionRow]) -> PastByFolder {
    let past: Vec<(usize, ResolvedPath)> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| matches!(row.state, AgentSessionState::Past))
        .map(|(index, row)| (index, ResolvedPath::new(&row.cwd)))
        .collect();
    let mut by_folder = PastByFolder::new();
    for folder in panes.iter().filter_map(|pane| pane.cwd.as_ref()) {
        if folder.as_os_str().is_empty() || by_folder.contains_key(folder) {
            continue;
        }
        let resolved = ResolvedPath::new(folder);
        let within = past
            .iter()
            .filter(|(_, path)| is_within_resolved(path, &resolved))
            .map(|(index, _)| *index)
            .collect();
        by_folder.insert(folder.clone(), within);
    }
    by_folder
}

/// Lays out `rows` (from `build_rows`) by tab and pane, with each pane's past conversations under
/// a toggle and the detached sessions last. A non-empty `query` keeps only the matching rows, plus
/// the headers and panes they sit under, and shows matching past rows without expanding.
pub(crate) fn view_rows(
    panes: &[PaneAgentInput],
    rows: &[AgentSessionRow],
    past_by_folder: &PastByFolder,
    expanded_folders: &HashSet<PathBuf>,
    query: &str,
) -> Vec<ViewRow> {
    let query = query.trim();
    let matches_row = |row: &AgentSessionRow| query.is_empty() || score_row(row, query).is_some();
    let matches_text = |text: &str| match_indices_case_insensitive(text, query).is_some();
    let mut view = Vec::new();
    let mut current_tab = None;
    for pane in panes {
        let row = rows.iter().find(|row| match &row.state {
            AgentSessionState::Running { locator } | AgentSessionState::Pending { locator } => {
                *locator == pane.locator
            }
            AgentSessionState::Detached { .. } | AgentSessionState::Past => false,
        });
        let folder = pane
            .cwd
            .clone()
            .filter(|folder| !folder.as_os_str().is_empty());
        let folder_past: Vec<&AgentSessionRow> = folder
            .as_ref()
            .and_then(|folder| past_by_folder.get(folder))
            .into_iter()
            .flatten()
            .filter_map(|index| rows.get(*index))
            .filter(|row| matches_row(row))
            .collect();

        let pane_matches = query.is_empty()
            || row.is_some_and(matches_row)
            || matches_text(&pane.tab_title)
            || folder
                .as_ref()
                .is_some_and(|folder| matches_text(&folder.to_string_lossy()));
        if !pane_matches && folder_past.is_empty() {
            continue;
        }

        if current_tab != Some(pane.tab_index) {
            current_tab = Some(pane.tab_index);
            view.push(ViewRow::TabHeader {
                title: pane.tab_title.clone(),
            });
        }
        let action = match (row, &folder) {
            (Some(row), _) => action_for(row, Some(pane.locator))
                .map(ViewRowAction::Act)
                .unwrap_or(ViewRowAction::None),
            (None, Some(folder)) => ViewRowAction::ResumeInFolder {
                folder: folder.clone(),
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
            let expanded = !query.is_empty() || expanded_folders.contains(&folder);
            view.push(ViewRow::PastToggle {
                folder,
                count: folder_past.len(),
                expanded,
            });
            if expanded {
                view.extend(
                    folder_past
                        .into_iter()
                        .map(|row| ViewRow::Past { row: row.clone() }),
                );
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

/// Rows are one line each so the list can be uniform.
const ROW_HEIGHT: f32 = 30.;
const PANE_INDENT: f32 = 24.;
const PAST_INDENT: f32 = 40.;

pub enum AgentSessionsViewEvent {
    Act(AgentSessionAction),
    ResumeInFolder {
        folder: PathBuf,
        target: PaneViewLocator,
    },
    Close,
}

#[derive(Clone, Debug)]
pub enum AgentSessionsViewAction {
    ArrowUp,
    ArrowDown,
    Enter,
    Activate(usize),
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
    panes: Vec<PaneAgentInput>,
    detached: Vec<DetachedInput>,
    supervisor_error: Option<DetachedListError>,
    rows: Vec<AgentSessionRow>,
    past_by_folder: PastByFolder,
    /// The search text `view_rows` was last built from.
    built_query: String,
    /// Whether past sessions have been scanned at least once; until then the list has none.
    history_loaded: bool,
    expanded_folders: HashSet<PathBuf>,
    view_rows: Arc<Vec<ViewRow>>,
    /// One per row position, grown as the list grows and never made while rendering, so a row's
    /// button keeps its hover and click state across renders.
    button_states: Vec<MouseStateHandle>,
    selected_index: Option<usize>,
    list_state: UniformListState,
    scroll_state: ScrollStateHandle,
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

        Self {
            query_editor,
            panes: Vec::new(),
            detached: Vec::new(),
            supervisor_error: None,
            rows: Vec::new(),
            past_by_folder: PastByFolder::new(),
            built_query: String::new(),
            history_loaded: false,
            expanded_folders: HashSet::new(),
            view_rows: Arc::new(Vec::new()),
            button_states: Vec::new(),
            selected_index: None,
            list_state: UniformListState::new(),
            scroll_state: Arc::new(Mutex::new(Default::default())),
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
        focus: Option<PaneViewLocator>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.panes = panes;
        self.history_loaded = history_loaded;
        (self.detached, self.supervisor_error) = match detached {
            Ok(detached) => (detached, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        self.set_rows(past);
        self.expanded_folders.clear();
        self.query_editor.update(ctx, |editor, ctx| {
            editor.clear_buffer_and_reset_undo_stack(ctx)
        });
        self.rebuild_view_rows(ctx);
        self.selected_index = focus.and_then(|focus| {
            self.view_rows.iter().position(
                |row| matches!(row, ViewRow::Pane { input, .. } if input.locator == focus),
            )
        });
        if let Some(index) = self.selected_index {
            self.list_state.scroll_to(index);
        }
        ctx.notify();
    }

    /// Rebuilds the rows with newly scanned past sessions, keeping the rest of the snapshot.
    pub(crate) fn refresh_past(&mut self, past: &[PastAgentSession], ctx: &mut ViewContext<Self>) {
        self.history_loaded = true;
        let selected = self
            .selected_index
            .and_then(|index| self.view_rows.get(index).cloned());
        self.set_rows(past);
        self.rebuild_view_rows(ctx);
        self.selected_index = selected
            .and_then(|selected| self.view_rows.iter().position(|row| *row == selected))
            .filter(|index| self.is_selectable(*index));
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
    pub(crate) fn selected_pane(&self) -> Option<PaneViewLocator> {
        match self.view_rows.get(self.selected_index?)? {
            ViewRow::Pane { input, .. } => Some(input.locator),
            ViewRow::TabHeader { .. }
            | ViewRow::PastToggle { .. }
            | ViewRow::Past { .. }
            | ViewRow::DetachedHeader { .. }
            | ViewRow::Detached { .. }
            | ViewRow::SupervisorUnreachable => None,
        }
    }

    fn rebuild_view_rows(&mut self, ctx: &mut ViewContext<Self>) {
        let query = self.query_editor.as_ref(ctx).buffer_text(ctx);
        let mut rows = view_rows(
            &self.panes,
            &self.rows,
            &self.past_by_folder,
            &self.expanded_folders,
            &query,
        );
        self.built_query = query;
        if let Some(error) = &self.supervisor_error {
            rows.extend(supervisor_rows(error));
        }
        if self.button_states.len() < rows.len() {
            self.button_states
                .resize_with(rows.len(), MouseStateHandle::default);
        }
        self.view_rows = Arc::new(rows);
        self.selected_index = self
            .selected_index
            .filter(|index| self.is_selectable(*index));
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
            _ => {}
        }
    }

    fn is_selectable(&self, index: usize) -> bool {
        self.view_rows.get(index).is_some_and(|row| match row {
            ViewRow::Pane { action, .. } => *action != ViewRowAction::None,
            ViewRow::PastToggle { .. } | ViewRow::Detached { .. } => true,
            ViewRow::Past { .. } => self.past_action(index).is_some(),
            ViewRow::TabHeader { .. }
            | ViewRow::DetachedHeader { .. }
            | ViewRow::SupervisorUnreachable => false,
        })
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
            self.selected_index = Some(index);
            self.list_state.scroll_to(index);
            ctx.notify();
        }
    }

    fn activate_selected(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(index) = self.selected_index.or_else(|| self.next_selectable(0)) {
            self.activate(index, ctx);
        }
    }

    /// The pane a past row is listed under, which is where it resumes.
    fn owning_pane(&self, index: usize) -> Option<PaneViewLocator> {
        self.view_rows[..index]
            .iter()
            .rev()
            .find_map(|row| match row {
                ViewRow::Pane { input, .. } => Some(input.locator),
                ViewRow::TabHeader { .. }
                | ViewRow::PastToggle { .. }
                | ViewRow::Past { .. }
                | ViewRow::DetachedHeader { .. }
                | ViewRow::Detached { .. }
                | ViewRow::SupervisorUnreachable => None,
            })
    }

    fn past_action(&self, index: usize) -> Option<AgentSessionAction> {
        match self.view_rows.get(index)? {
            ViewRow::Past { row } => action_for(row, self.owning_pane(index)),
            ViewRow::TabHeader { .. }
            | ViewRow::Pane { .. }
            | ViewRow::PastToggle { .. }
            | ViewRow::DetachedHeader { .. }
            | ViewRow::Detached { .. }
            | ViewRow::SupervisorUnreachable => None,
        }
    }

    fn activate(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        let Some(row) = self.view_rows.get(index).cloned() else {
            return;
        };
        match row {
            ViewRow::Pane { action, .. } => match action {
                ViewRowAction::Act(action) => ctx.emit(AgentSessionsViewEvent::Act(action)),
                ViewRowAction::ResumeInFolder { folder, target } => {
                    ctx.emit(AgentSessionsViewEvent::ResumeInFolder { folder, target })
                }
                ViewRowAction::None => {}
            },
            ViewRow::PastToggle { folder, .. } => {
                if !self.expanded_folders.remove(&folder) {
                    self.expanded_folders.insert(folder);
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
            ViewRow::TabHeader { .. }
            | ViewRow::DetachedHeader { .. }
            | ViewRow::SupervisorUnreachable => {}
        }
    }

    fn render_list(&self, app: &AppContext) -> Box<dyn Element> {
        let theme = Appearance::as_ref(app).theme();
        let view_rows = self.view_rows.clone();
        let button_states = self.button_states.clone();
        let selected_index = self.selected_index;
        let list = UniformList::new(
            self.list_state.clone(),
            view_rows.len(),
            move |range: Range<usize>, app: &AppContext| {
                range
                    .filter_map(|index| {
                        let row = view_rows.get(index)?;
                        let button_state = button_states.get(index)?.clone();
                        Some(render_row(
                            row,
                            index,
                            selected_index == Some(index),
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
}

fn action_label(action: &AgentSessionAction) -> &'static str {
    match action {
        AgentSessionAction::Focus(_) => "Focus",
        AgentSessionAction::Start(_) => "Start",
        AgentSessionAction::Attach(_) => "Attach",
        AgentSessionAction::Resume { .. } => "Resume",
    }
}

fn session_label(row: &AgentSessionRow) -> String {
    let title = row_title(row);
    match row.agent.map(|agent| agent.display_name()) {
        Some(agent) if agent != title => format!("{agent} · {title}"),
        Some(_) | None => title,
    }
}

fn render_row(
    row: &ViewRow,
    index: usize,
    is_selected: bool,
    button_state: MouseStateHandle,
    app: &AppContext,
) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();
    let font = appearance.ui_font_family();
    let font_size = appearance.ui_font_size();
    let main_color = theme.active_ui_text_color().into_solid();
    let sub_color = theme.sub_text_color(theme.surface_2()).into_solid();
    let text = |text: String, color| Text::new_inline(text, font, font_size).with_color(color);
    let bold = || Properties::default().weight(Weight::Bold);

    let (indent, label, detail, button): (f32, Box<dyn Element>, Option<String>, Option<&str>) =
        match row {
            ViewRow::TabHeader { title } => (
                12.,
                text(title.clone(), main_color).with_style(bold()).finish(),
                None,
                None,
            ),
            ViewRow::Pane { input, row, action } => {
                let label = row
                    .as_ref()
                    .map_or_else(|| "Shell".to_owned(), session_label);
                let button = match action {
                    ViewRowAction::Act(action) => Some(action_label(action)),
                    ViewRowAction::ResumeInFolder { .. } => Some("Resume…"),
                    ViewRowAction::None => None,
                };
                let label = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.)
                    .with_child(text(format!("pane {}", input.pane_number), sub_color).finish())
                    .with_child(text(label, main_color).finish())
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
                    .with_child(text(format!("Past in this folder ({count})"), sub_color).finish())
                    .finish();
                (PANE_INDENT, label, None, None)
            }
            ViewRow::Past { row } => (
                PAST_INDENT,
                text(session_label(row), main_color).finish(),
                row.last_used.map(|last_used| {
                    format_approx_duration_from_now(DateTime::<Local>::from(last_used))
                }),
                action_for(row, None).map(|action| action_label(&action)),
            ),
            ViewRow::DetachedHeader { count } => {
                let title = match count {
                    0 => "Detached in supervisor".to_owned(),
                    count => format!("Detached in supervisor ({count})"),
                };
                (
                    12.,
                    text(title, main_color).with_style(bold()).finish(),
                    None,
                    None,
                )
            }
            ViewRow::Detached { row } => (
                PANE_INDENT,
                text(session_label(row), main_color).finish(),
                Some(row.cwd.display().to_string()).filter(|cwd| !cwd.is_empty()),
                Some("Attach"),
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
        |detail| Clipped::new(text(detail, sub_color).finish()).finish(),
    );
    let mut line = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(8.)
        .with_child(label)
        .with_child(Shrinkable::new(1.0, detail).finish());
    let is_toggle = matches!(row, ViewRow::PastToggle { .. });
    if let Some(button) = button {
        line = line.with_child(render_button(button, index, button_state.clone(), app));
    }

    let mut container = Container::new(line.finish())
        .with_padding_left(indent)
        .with_padding_right(12.);
    if is_selected {
        container = container.with_background(theme.surface_overlay_1());
    }
    let row_element = ConstrainedBox::new(container.finish())
        .with_height(ROW_HEIGHT)
        .finish();

    if !is_toggle {
        return row_element;
    }
    Hoverable::new(button_state, move |_| row_element)
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(AgentSessionsViewAction::Activate(index));
        })
        .finish()
}

fn render_button(
    label: &str,
    index: usize,
    mouse_state: MouseStateHandle,
    app: &AppContext,
) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();
    let label = Text::new_inline(
        label.to_owned(),
        appearance.ui_font_family(),
        appearance.ui_font_size() - 1.,
    )
    .with_color(theme.active_ui_text_color().into_solid())
    .finish();
    let border = theme.surface_3();
    let hover_background = theme.surface_overlay_1();
    Hoverable::new(mouse_state, move |state| {
        let mut button = Container::new(label)
            .with_horizontal_padding(8.)
            .with_vertical_padding(2.)
            .with_border(Border::all(1.).with_border_fill(border))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
        if state.is_hovered() {
            button = button.with_background(hover_background);
        }
        button.finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action(AgentSessionsViewAction::Activate(index));
    })
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
            AgentSessionsViewAction::Activate(index) => self.activate(*index, ctx),
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
        let content = if self.view_rows.is_empty() {
            Container::new(
                Text::new_inline(
                    "No agent sessions in this window",
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
        let mut column = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(render_search_box(&self.query_editor, app));
        if !self.history_loaded {
            column = column.with_child(
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
        column
            .with_child(Shrinkable::new(1.0, content).finish())
            .finish()
    }
}

#[cfg(test)]
#[path = "agent_sessions_view_tests.rs"]
mod tests;
