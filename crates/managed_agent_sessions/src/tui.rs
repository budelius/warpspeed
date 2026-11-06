use std::collections::HashSet;
use std::io::{self, IsTerminal as _};
use std::process::{ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;
use std::{env, thread};

use anyhow::{Context, Result};
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use unicode_width::UnicodeWidthStr as _;
use warp_cli::managed_sessions::ManagedEnvironment;

use super::event_stream::EventFeed;
use super::{
    EnvironmentProfile, ManagedSessionBookmark, OpenedManagedSession, RunEvent, SessionDescriptor,
    dashboard_token, open_managed_session, pause_run_with, post_input_with, resume_run_with,
    retain_new_events, save_bookmarks, stop_run_with, update_bookmark_cursor,
};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_INPUT_CHARS: usize = 8_000;

pub(super) fn can_run() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

pub(super) fn run(environment: ManagedEnvironment, job_id: &str) -> Result<()> {
    let OpenedManagedSession {
        profile,
        descriptor,
        bookmarks,
        client: _,
        existing_cursor: _,
    } = open_managed_session(environment, job_id)?;
    let (updates_tx, updates_rx) = mpsc::channel();
    spawn_event_poller(
        EventFeed::new()?,
        profile,
        job_id.to_owned(),
        updates_tx.clone(),
    );

    let display_name = descriptor.display_name.clone();
    let mut app = ManagedSessionsTui::new(
        profile,
        job_id.to_owned(),
        descriptor,
        bookmarks,
        updates_tx,
        updates_rx,
    );
    loop {
        let mut terminal = ratatui::try_init().context("failed to initialize AgentCtrl TUI")?;
        let result = app.run(&mut terminal);
        let restore_result = ratatui::try_restore().context("failed to restore terminal state");
        let exit = result?;
        restore_result?;

        match exit {
            TuiExit::Detach => break,
            TuiExit::LocalShell(command) => {
                if command.is_none() {
                    println!("AgentCtrl suspended. Exit the local shell to return.");
                }
                app.set_local_shell_result(command.as_deref(), run_local_shell(command.as_deref()));
            }
        }
    }
    println!("Detached from {display_name}. The managed run continues.");
    Ok(())
}

// The command wrapper forces CREATE_NO_WINDOW, which disconnects interactive programs from ConPTY.
#[allow(clippy::disallowed_types)]
fn run_local_shell(command: Option<&str>) -> io::Result<ExitStatus> {
    let configured_shell = env::var_os("WARPSPEED_LOCAL_SHELL");
    let mut process = if let Some(shell) = configured_shell.as_ref() {
        std::process::Command::new(shell)
    } else if cfg!(windows) {
        std::process::Command::new("powershell.exe")
    } else {
        std::process::Command::new(env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into()))
    };

    if let Some(command) = command {
        if cfg!(windows) && configured_shell.is_none() {
            process.args(["-NoLogo", "-Command", command]);
        } else {
            process.args(["-lc", command]);
        }
    } else if cfg!(windows) && configured_shell.is_none() {
        process.arg("-NoLogo");
    } else {
        process.arg("-l");
    }

    process
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
}

fn spawn_event_poller(
    mut feed: EventFeed,
    profile: &'static EnvironmentProfile,
    job_id: String,
    updates: Sender<TuiUpdate>,
) {
    thread::spawn(move || {
        let mut cursor = None;
        let mut known_events = HashSet::new();
        let mut first_request = true;
        let mut retry_delay = Duration::from_millis(500);
        loop {
            let wait_ms = if first_request { 0 } else { 15_000 };
            let page = dashboard_token()
                .and_then(|token| feed.next(profile, &job_id, &token, cursor.as_deref(), wait_ms));
            match page {
                Ok((page, history_gap)) => {
                    first_request = false;
                    retry_delay = Duration::from_millis(500);
                    let terminal = page.terminal;
                    let gap = history_gap || page.gap;
                    let earliest_cursor = page.earliest_cursor;
                    let event_count = page.items.len();
                    let events = retain_new_events(page.items, &mut known_events);
                    if page.next_cursor.is_some() {
                        cursor = page.next_cursor;
                    }
                    if updates
                        .send(TuiUpdate::EventPage {
                            events,
                            cursor: cursor.clone(),
                            terminal,
                            gap,
                            earliest_cursor,
                        })
                        .is_err()
                    {
                        break;
                    }
                    if terminal {
                        break;
                    }
                    if event_count == 100 {
                        first_request = true;
                    } else if cursor.is_none() {
                        thread::sleep(Duration::from_millis(500));
                    }
                }
                Err(error) => {
                    if updates
                        .send(TuiUpdate::PollFailed(error.to_string()))
                        .is_err()
                    {
                        break;
                    }
                    thread::sleep(retry_delay);
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(5));
                    first_request = true;
                }
            }
        }
    });
}

enum TuiExit {
    Detach,
    LocalShell(Option<String>),
}

#[derive(Debug, PartialEq, Eq)]
enum ComposerCommand {
    Send(String),
    Action(SessionAction),
    Detach,
    Help,
    Status,
    LocalShell(Option<String>),
    Unknown(String),
}

fn parse_composer_command(input: &str) -> ComposerCommand {
    let input = input.trim();
    if let Some(message) = input.strip_prefix("//") {
        return ComposerCommand::Send(format!("/{message}"));
    }
    if let Some(command) = input.strip_prefix('!') {
        let command = command.trim();
        return ComposerCommand::LocalShell((!command.is_empty()).then(|| command.to_owned()));
    }
    let Some(command) = input.strip_prefix('/') else {
        return ComposerCommand::Send(input.to_owned());
    };
    let (name, arguments) = command
        .split_once(char::is_whitespace)
        .map_or((command, ""), |(name, arguments)| (name, arguments.trim()));
    match (name, arguments) {
        ("help", "") => ComposerCommand::Help,
        ("detach" | "exit" | "quit", "") => ComposerCommand::Detach,
        ("close", "") => ComposerCommand::Action(SessionAction::Close),
        ("stop", "") => ComposerCommand::Action(SessionAction::Stop),
        ("pause", "") => ComposerCommand::Action(SessionAction::Pause),
        ("resume", "") => ComposerCommand::Action(SessionAction::Resume),
        ("status", "") => ComposerCommand::Status,
        ("shell" | "local", "") => ComposerCommand::LocalShell(None),
        ("shell" | "local", command) => ComposerCommand::LocalShell(Some(command.to_owned())),
        _ => ComposerCommand::Unknown(input.to_owned()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionAction {
    Send,
    Close,
    Pause,
    Resume,
    Stop,
}

impl SessionAction {
    fn pending_message(self) -> &'static str {
        match self {
            Self::Send => "Sending message…",
            Self::Close => "Closing agent input…",
            Self::Pause => "Requesting pause…",
            Self::Resume => "Requesting resume…",
            Self::Stop => "Requesting stop…",
        }
    }

    fn success_message(self) -> &'static str {
        match self {
            Self::Send => "Message sent",
            Self::Close => "Agent input closed",
            Self::Pause => "Run paused",
            Self::Resume => "Run resumed",
            Self::Stop => "Stop requested",
        }
    }
}

enum TuiUpdate {
    EventPage {
        events: Vec<RunEvent>,
        cursor: Option<String>,
        terminal: bool,
        gap: bool,
        earliest_cursor: Option<String>,
    },
    PollFailed(String),
    ActionFinished {
        action: SessionAction,
        result: std::result::Result<(), String>,
    },
}

#[derive(Clone, Copy)]
enum StatusTone {
    Normal,
    Success,
    Warning,
    Error,
}

struct StatusMessage {
    text: String,
    tone: StatusTone,
}

struct ManagedSessionsTui {
    profile: &'static EnvironmentProfile,
    job_id: String,
    descriptor: SessionDescriptor,
    bookmarks: Vec<ManagedSessionBookmark>,
    updates_tx: Sender<TuiUpdate>,
    updates_rx: Receiver<TuiUpdate>,
    events: Vec<RunEvent>,
    input: String,
    input_cursor: usize,
    scroll: u16,
    max_scroll: u16,
    follow_tail: bool,
    terminal: bool,
    input_closed: bool,
    history_gap: bool,
    help_visible: bool,
    exit: Option<TuiExit>,
    status: StatusMessage,
}

impl ManagedSessionsTui {
    fn new(
        profile: &'static EnvironmentProfile,
        job_id: String,
        descriptor: SessionDescriptor,
        bookmarks: Vec<ManagedSessionBookmark>,
        updates_tx: Sender<TuiUpdate>,
        updates_rx: Receiver<TuiUpdate>,
    ) -> Self {
        Self {
            profile,
            job_id,
            descriptor,
            bookmarks,
            updates_tx,
            updates_rx,
            events: Vec::new(),
            input: String::new(),
            input_cursor: 0,
            scroll: 0,
            max_scroll: 0,
            follow_tail: true,
            terminal: false,
            input_closed: false,
            history_gap: false,
            help_visible: false,
            exit: None,
            status: StatusMessage {
                text: "Loading conversation…".to_owned(),
                tone: StatusTone::Normal,
            },
        }
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<TuiExit> {
        while self.exit.is_none() {
            self.apply_updates();
            terminal.draw(|frame| self.draw(frame))?;
            if event::poll(EVENT_POLL_INTERVAL)? {
                self.handle_event(event::read()?);
            }
        }
        Ok(self.exit.take().expect("TUI exit should be set"))
    }

    fn apply_updates(&mut self) {
        while let Ok(update) = self.updates_rx.try_recv() {
            match update {
                TuiUpdate::EventPage {
                    events,
                    cursor,
                    terminal,
                    gap,
                    earliest_cursor,
                } => {
                    let received_events = !events.is_empty();
                    self.update_run_state_from_events(&events);
                    self.events.extend(events);
                    self.terminal = terminal;
                    self.history_gap |= gap;
                    update_bookmark_cursor(
                        &mut self.bookmarks,
                        self.profile,
                        &self.job_id,
                        cursor,
                        terminal,
                    );
                    if let Err(error) = save_bookmarks(&self.bookmarks) {
                        self.set_status(
                            format!("Could not save the resume position: {error}"),
                            StatusTone::Error,
                        );
                    } else if gap {
                        let retained_from = earliest_cursor
                            .map(|cursor| format!(" from cursor {cursor}"))
                            .unwrap_or_default();
                        self.set_status(
                            format!("Earlier event history is unavailable{retained_from}"),
                            StatusTone::Warning,
                        );
                    } else if terminal {
                        self.set_status("Run finished".to_owned(), StatusTone::Normal);
                    } else if received_events {
                        self.set_status(
                            "Live · following new events".to_owned(),
                            StatusTone::Success,
                        );
                    }
                }
                TuiUpdate::PollFailed(error) => {
                    self.set_status(
                        format!("Connection lost; retrying: {error}"),
                        StatusTone::Error,
                    );
                }
                TuiUpdate::ActionFinished { action, result } => match result {
                    Ok(()) => {
                        let state_result = match action {
                            SessionAction::Close => {
                                self.input_closed = true;
                                Ok(())
                            }
                            SessionAction::Pause => self.set_run_state("paused", "resume", "pause"),
                            SessionAction::Resume => {
                                self.set_run_state("running", "pause", "resume")
                            }
                            SessionAction::Send | SessionAction::Stop => Ok(()),
                        };
                        if let Err(error) = state_result {
                            self.set_status(
                                format!(
                                    "Action succeeded, but the run state was not saved: {error}"
                                ),
                                StatusTone::Error,
                            );
                        } else {
                            self.set_status(
                                action.success_message().to_owned(),
                                StatusTone::Success,
                            );
                        }
                    }
                    Err(error) => {
                        self.set_status(error, StatusTone::Error);
                    }
                },
            }
        }
    }

    fn set_status(&mut self, text: String, tone: StatusTone) {
        self.status = StatusMessage { text, tone };
    }

    fn set_run_state(
        &mut self,
        state: &str,
        add_capability: &str,
        remove_capability: &str,
    ) -> Result<()> {
        self.update_run_state(state, add_capability, remove_capability);
        save_bookmarks(&self.bookmarks)
    }

    fn update_run_state_from_events(&mut self, events: &[RunEvent]) {
        let state = events.iter().rev().find_map(|event| {
            if event.event_type != "status" {
                return None;
            }
            match event.content.trim().to_ascii_lowercase().as_str() {
                "paused" => Some(("paused", "resume", "pause")),
                "resumed" => Some(("running", "pause", "resume")),
                _ => None,
            }
        });
        if let Some((state, add_capability, remove_capability)) = state {
            self.update_run_state(state, add_capability, remove_capability);
        }
    }

    fn update_run_state(&mut self, state: &str, add_capability: &str, remove_capability: &str) {
        self.descriptor.state = state.to_owned();
        self.descriptor
            .capabilities
            .retain(|capability| capability != remove_capability);
        if !self
            .descriptor
            .capabilities
            .iter()
            .any(|capability| capability == add_capability)
        {
            self.descriptor.capabilities.push(add_capability.to_owned());
        }
        if let Some(bookmark) = self.bookmarks.iter_mut().find(|bookmark| {
            bookmark.environment_alias == self.profile.alias
                && bookmark.fleet_id == self.profile.fleet_id
                && bookmark.job_id == self.job_id
        }) {
            bookmark.last_known_state = Some(state.to_owned());
        }
    }

    fn set_local_shell_result(&mut self, command: Option<&str>, result: io::Result<ExitStatus>) {
        self.help_visible = false;
        match result {
            Ok(status) if status.success() => {
                let message = command.map_or_else(
                    || "Returned from local shell".to_owned(),
                    |command| format!("Local command finished: {command}"),
                );
                self.set_status(message, StatusTone::Success);
            }
            Ok(status) => {
                let code = status.code().map_or_else(
                    || "by signal".to_owned(),
                    |code| format!("with status {code}"),
                );
                self.set_status(format!("Local shell exited {code}"), StatusTone::Warning);
            }
            Err(error) => {
                self.set_status(
                    format!("Could not start local shell: {error}"),
                    StatusTone::Error,
                );
            }
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.handle_key(key);
            }
            Event::Paste(text) if !self.help_visible => {
                self.insert_text(&text.replace(['\r', '\n'], " "));
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => self.scroll_up(3),
                MouseEventKind::ScrollDown => self.scroll_down(3),
                MouseEventKind::Down(_)
                | MouseEventKind::Up(_)
                | MouseEventKind::Drag(_)
                | MouseEventKind::Moved
                | MouseEventKind::ScrollLeft
                | MouseEventKind::ScrollRight => {}
            },
            Event::FocusGained | Event::FocusLost | Event::Resize(_, _) => {}
            _ => {}
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => self.exit = Some(TuiExit::Detach),
                KeyCode::Char('d') => self.start_action(SessionAction::Close, None),
                KeyCode::Char('o') => self.exit = Some(TuiExit::LocalShell(None)),
                KeyCode::Char('x') => self.start_action(SessionAction::Stop, None),
                KeyCode::Char(_)
                | KeyCode::Backspace
                | KeyCode::Enter
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Tab
                | KeyCode::BackTab
                | KeyCode::Delete
                | KeyCode::Insert
                | KeyCode::F(_)
                | KeyCode::Null
                | KeyCode::Esc
                | KeyCode::CapsLock
                | KeyCode::ScrollLock
                | KeyCode::NumLock
                | KeyCode::PrintScreen
                | KeyCode::Pause
                | KeyCode::Menu
                | KeyCode::KeypadBegin
                | KeyCode::Media(_)
                | KeyCode::Modifier(_) => {}
            }
            return;
        }

        if self.help_visible {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::F(1)) {
                self.help_visible = false;
            }
            return;
        }

        match key.code {
            KeyCode::Esc => self.exit = Some(TuiExit::Detach),
            KeyCode::Enter => self.submit_input(),
            KeyCode::Char(character) => self.insert_char(character),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.move_cursor_left(),
            KeyCode::Right => self.move_cursor_right(),
            KeyCode::Home => self.input_cursor = 0,
            KeyCode::End => self.input_cursor = self.input.len(),
            KeyCode::Up | KeyCode::PageUp => self.scroll_up(5),
            KeyCode::Down | KeyCode::PageDown => self.scroll_down(5),
            KeyCode::F(1) => self.help_visible = true,
            KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Insert
            | KeyCode::F(_)
            | KeyCode::Null
            | KeyCode::CapsLock
            | KeyCode::ScrollLock
            | KeyCode::NumLock
            | KeyCode::PrintScreen
            | KeyCode::Pause
            | KeyCode::Menu
            | KeyCode::KeypadBegin
            | KeyCode::Media(_)
            | KeyCode::Modifier(_) => {}
        }
    }

    fn insert_text(&mut self, text: &str) {
        let remaining = MAX_INPUT_CHARS.saturating_sub(self.input.chars().count());
        let insertion: String = safe_terminal_text(text).chars().take(remaining).collect();
        self.input.insert_str(self.input_cursor, &insertion);
        self.input_cursor += insertion.len();
    }

    fn insert_char(&mut self, character: char) {
        if self.input.chars().count() >= MAX_INPUT_CHARS || character.is_control() {
            return;
        }
        self.input.insert(self.input_cursor, character);
        self.input_cursor += character.len_utf8();
    }

    fn backspace(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        let previous = self.input[..self.input_cursor]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0);
        self.input.drain(previous..self.input_cursor);
        self.input_cursor = previous;
    }

    fn delete(&mut self) {
        let Some(character) = self.input[self.input_cursor..].chars().next() else {
            return;
        };
        self.input
            .drain(self.input_cursor..self.input_cursor + character.len_utf8());
    }

    fn move_cursor_left(&mut self) {
        self.input_cursor = self.input[..self.input_cursor]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0);
    }

    fn move_cursor_right(&mut self) {
        if let Some(character) = self.input[self.input_cursor..].chars().next() {
            self.input_cursor += character.len_utf8();
        }
    }

    fn submit_input(&mut self) {
        if self.input.trim().is_empty() {
            return;
        }
        match parse_composer_command(&self.input) {
            ComposerCommand::Send(text) => {
                if self.terminal || self.input_closed {
                    return;
                }
                self.clear_composer();
                self.follow_tail = true;
                self.start_action(SessionAction::Send, Some(text));
            }
            ComposerCommand::Action(action) => {
                self.clear_composer();
                self.start_action(action, None);
            }
            ComposerCommand::Detach => {
                self.clear_composer();
                self.exit = Some(TuiExit::Detach);
            }
            ComposerCommand::Help => {
                self.clear_composer();
                self.help_visible = true;
            }
            ComposerCommand::Status => {
                self.clear_composer();
                self.set_status(
                    format!(
                        "Run {} · capabilities: {}",
                        self.descriptor.state,
                        self.descriptor.capabilities.join(", ")
                    ),
                    StatusTone::Normal,
                );
            }
            ComposerCommand::LocalShell(command) => {
                self.clear_composer();
                self.exit = Some(TuiExit::LocalShell(command));
            }
            ComposerCommand::Unknown(command) => {
                self.set_status(
                    format!("Unknown command {command}. Type /help for commands."),
                    StatusTone::Warning,
                );
            }
        }
    }

    fn clear_composer(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
    }

    fn start_action(&mut self, action: SessionAction, text: Option<String>) {
        if self.terminal || matches!(action, SessionAction::Close) && self.input_closed {
            return;
        }
        if matches!(action, SessionAction::Pause) && self.descriptor.state == "paused" {
            self.set_status("Run is already paused".to_owned(), StatusTone::Normal);
            return;
        }
        if matches!(action, SessionAction::Resume) && self.descriptor.state == "running" {
            self.set_status("Run is already running".to_owned(), StatusTone::Normal);
            return;
        }
        if matches!(action, SessionAction::Stop)
            && !self
                .descriptor
                .capabilities
                .iter()
                .any(|capability| capability == "stop")
        {
            self.set_status("This run cannot be stopped".to_owned(), StatusTone::Warning);
            return;
        }
        self.set_status(action.pending_message().to_owned(), StatusTone::Normal);
        let profile = self.profile;
        let job_id = self.job_id.clone();
        let updates = self.updates_tx.clone();
        thread::spawn(move || {
            let result = dashboard_token()
                .and_then(|token| match action {
                    SessionAction::Send => {
                        post_input_with(profile, &token, &job_id, text.as_deref())
                    }
                    SessionAction::Close => post_input_with(profile, &token, &job_id, None),
                    SessionAction::Pause => pause_run_with(profile, &token, &job_id),
                    SessionAction::Resume => resume_run_with(profile, &token, &job_id),
                    SessionAction::Stop => stop_run_with(profile, &token, &job_id, false),
                })
                .map(drop)
                .map_err(|error| error.to_string());
            let _ = updates.send(TuiUpdate::ActionFinished { action, result });
        });
    }

    fn scroll_up(&mut self, rows: u16) {
        self.follow_tail = false;
        self.scroll = self.scroll.saturating_sub(rows);
    }

    fn scroll_down(&mut self, rows: u16) {
        self.scroll = self.scroll.saturating_add(rows).min(self.max_scroll);
        self.follow_tail = self.scroll == self.max_scroll;
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [header_area, transcript_area, input_area, footer_area] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        self.draw_header(frame, header_area);
        self.draw_transcript(frame, transcript_area);
        self.draw_input(frame, input_area);
        self.draw_footer(frame, footer_area);
        if self.help_visible {
            self.draw_help(frame);
        }
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let state = if self.terminal {
            "finished"
        } else {
            &self.descriptor.state
        };
        let state_style = if self.terminal {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        };
        let lines = vec![
            Line::from(vec![
                Span::styled(
                    safe_terminal_text(&self.descriptor.display_name),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(safe_terminal_text(state), state_style),
            ]),
            Line::from(vec![
                Span::styled(
                    safe_terminal_text(&self.descriptor.agent_name),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(format!("  ·  {}  ·  {}", self.profile.alias, self.job_id)),
            ]),
        ];
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" AgentCtrl ")
                    .border_style(Style::default().fg(Color::DarkGray)),
            ),
            area,
        );
    }

    fn draw_transcript(&mut self, frame: &mut Frame, area: Rect) {
        let text = Text::from(transcript_lines(&self.events, self.history_gap));
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(
                " Event history / fallback · {} events ",
                self.events.len()
            ))
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        let paragraph = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
        let line_count = paragraph.line_count(area.width);
        self.max_scroll = line_count
            .saturating_sub(inner.height as usize)
            .min(u16::MAX as usize) as u16;
        if self.follow_tail {
            self.scroll = self.max_scroll;
        } else {
            self.scroll = self.scroll.min(self.max_scroll);
        }
        frame.render_widget(paragraph.scroll((self.scroll, 0)), area);
    }

    fn draw_input(&self, frame: &mut Frame, area: Rect) {
        let disabled = self.terminal || self.input_closed;
        let (content, style) = if disabled {
            (
                if self.terminal {
                    "Run finished"
                } else {
                    "Agent input is closed"
                },
                Style::default().fg(Color::DarkGray),
            )
        } else if self.input.is_empty() {
            (
                " Type a follow-up message…",
                Style::default().fg(Color::DarkGray),
            )
        } else {
            (self.input.as_str(), Style::default())
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Message ")
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(area);
        let cursor_width = self.input[..self.input_cursor].width();
        let horizontal_scroll = cursor_width.saturating_sub(inner.width.saturating_sub(1) as usize);
        frame.render_widget(
            Paragraph::new(content)
                .style(style)
                .block(block)
                .scroll((0, horizontal_scroll.min(u16::MAX as usize) as u16)),
            area,
        );
        if !disabled && !self.help_visible && inner.width > 0 && inner.height > 0 {
            let cursor_x = cursor_width
                .saturating_sub(horizontal_scroll)
                .min(inner.width.saturating_sub(1) as usize) as u16;
            frame.set_cursor_position(Position::new(inner.x + cursor_x, inner.y));
        }
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let status_style = match self.status.tone {
            StatusTone::Normal => Style::default().fg(Color::DarkGray),
            StatusTone::Success => Style::default().fg(Color::Green),
            StatusTone::Warning => Style::default().fg(Color::Yellow),
            StatusTone::Error => Style::default().fg(Color::Red),
        };
        let line = Line::from(vec![
            Span::styled(
                format!(" {}", safe_terminal_text(&self.status.text)),
                status_style,
            ),
            Span::styled(
                "  ·  Enter send  ↑/↓ scroll  /help commands  Ctrl-O shell  Esc detach",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    fn draw_help(&self, frame: &mut Frame) {
        let outer = frame.area();
        let width = outer.width.min(76);
        let height = outer.height.min(17);
        let area = Rect::new(
            outer.x + outer.width.saturating_sub(width) / 2,
            outer.y + outer.height.saturating_sub(height) / 2,
            width,
            height,
        );
        let lines = vec![
            Line::from("Text + Enter       send an agent message"),
            Line::from("//text             send a message beginning with /"),
            Line::from("/pause  /resume    control the managed run"),
            Line::from("/close  /stop      close input or stop the run"),
            Line::from("/status             show state and capabilities"),
            Line::from("/detach             leave while the run continues"),
            Line::default(),
            Line::from("/shell              open an interactive local shell"),
            Line::from("/shell <command>    run a local shell command"),
            Line::from("!<command>          local shell command (same as /shell)"),
            Line::default(),
            Line::from(Span::styled(
                "Exit the local program to return · F1/Esc closes help",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" AgentCtrl commands ")
                    .border_style(Style::default().fg(Color::Cyan)),
            ),
            area,
        );
    }
}

fn transcript_lines(events: &[RunEvent], history_gap: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if history_gap {
        lines.push(Line::from(Span::styled(
            "Earlier retained history is unavailable.",
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::default());
    }
    if events.is_empty() {
        lines.push(Line::from(Span::styled(
            "Waiting for agent events…",
            Style::default().fg(Color::DarkGray),
        )));
        return lines;
    }

    for event in events {
        let (label, style) = event_label(&event.event_type);
        let mut heading = vec![
            Span::styled(label, style.add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("  #{:<4}", event.sequence),
                Style::default().fg(Color::DarkGray),
            ),
        ];
        if event.redacted {
            heading.push(Span::styled(
                " [redacted]",
                Style::default().fg(Color::Yellow),
            ));
        }
        if event.truncated {
            heading.push(Span::styled(
                " [truncated]",
                Style::default().fg(Color::Yellow),
            ));
        }
        lines.push(Line::from(heading));
        for content_line in event.content.split('\n') {
            lines.push(Line::from(format!(
                "  {}",
                safe_terminal_text(content_line)
            )));
        }
        lines.push(Line::default());
    }
    lines
}

fn event_label(event_type: &str) -> (&'static str, Style) {
    match event_type {
        "assistant" => ("AGENT", Style::default().fg(Color::Cyan)),
        "user_input" => ("YOU", Style::default().fg(Color::Green)),
        "tool_use" => ("TOOL", Style::default().fg(Color::Magenta)),
        "stdout" => ("OUTPUT", Style::default()),
        "stderr" => ("ERROR", Style::default().fg(Color::Red)),
        "status" => ("STATUS", Style::default().fg(Color::Yellow)),
        "system" => ("SYSTEM", Style::default().fg(Color::DarkGray)),
        _ => ("EVENT", Style::default().fg(Color::DarkGray)),
    }
}

fn safe_terminal_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character == '\t' {
                ' '
            } else if character.is_control() {
                '�'
            } else {
                character
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "tui_tests.rs"]
mod tests;
