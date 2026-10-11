# Agent Sessions Picker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** From any pane, see the agent conversations that belong to it and focus, start, attach or resume each with one click — from a command-palette mode and from a window-wide view grouped by tab and pane.

**Architecture:** A pure merge function builds `AgentSessionRow`s from four inputs (live agents, pending restores, supervisor-detached shells, on-disk history). One action module turns a row into Focus / Start / Attach / Resume. Two UIs read the same rows and call the same actions: a palette data source (`QueryFilter::AgentSessions`) and a workspace modal (`AgentSessionsView`). Task 1 first fixes the bug that left restored resume commands typed but never run.

**Tech Stack:** Rust, WarpUI (`warpui`), Warp's search mixer (`warp_search_core`), `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-10-06-agent-sessions-picker-design.md`

## Global Constraints

- Work in worktree `T:/projects/dev-ops/warp-agent-sessions-picker`, branch `feat/agent-sessions-picker`. Build with `CARGO_TARGET_DIR=T:/projects/dev-ops/warp-dev-release/target` to reuse the cache.
- Follow `AGENTS.md`: tests in sibling `*_tests.rs` files attached with `#[cfg(test)] #[path = "..."] mod tests;`; context params named `ctx` and last; no `_`-prefixed unused params; inline format args; comments explain *why* only; no wildcard `_` in new `match`es over our own enums.
- Stage explicit paths only (other sessions edit sibling trees); never `git add -A`.
- Every task ends with `cargo fmt -p warp` and the task's tests passing. Before the final merge: `cargo clippy -p warp --all-targets --tests -- -D warnings`.
- Pre-existing failures: ~114 `workspace::view::tests` panic with `StoznAuthManager ... never registered`. Compare against that baseline; do not try to fix it.
- Windows PowerShell 5.1 has no `&&`. Command chaining must use `;` for PowerShell, `&&` for Bash/Zsh, `; and` for Fish.
- Agents with no resume command (`CliAgentResumeDescriptor::resume_command() == None`) are listed but get no Resume action.

---

### Task 1: A pending command runs once the session's history is ready

**Root cause (confirmed 2026-10-06):** a restored resume command is written into the input editor and marked pending (`Input::set_pending_command`, `app/src/terminal/input.rs:13238`). `TerminalView` retries it on `ModelEvent::BootstrapPrecmdDone` (`app/src/terminal/view.rs:13083`) and on `BlockCompleted`. `Input::can_execute_command` (`input.rs:7390`) refuses with `HistoryNotAppendable` until `History::init_session` has registered the session (`app/src/terminal/model/session.rs:451`, via `model_events.rs:353`). When `BootstrapPrecmdDone` reaches the view first, the command is refused, nothing completes at an idle prompt, and the command stays typed forever. The user confirmed: restored panes show `pi --session …` typed but not run.

**Fix:** `TerminalView` subscribes to the `History` singleton and, on `HistoryEvent::Initialized(session_id)` for its own active session, calls `execute_pending_command`. `Initialized` is emitted only after `init_session_with` has inserted the session (`app/src/terminal/history.rs:550`, emitted at :705), so the retry can no longer hit `HistoryNotAppendable`. Also log the refusal reason so the next failure of this kind is visible in the log.

**Files:**
- Modify: `app/src/terminal/view.rs` (subscription in the constructor near the other model subscriptions; handler next to `ModelEvent::BootstrapPrecmdDone` at ~13083)
- Modify: `app/src/terminal/input.rs:7410-7425` (log the refusal reason)
- Test: `app/src/terminal/view_tests.rs`

**Interfaces:**
- Consumes: `History::handle(ctx)`, `HistoryEvent::Initialized(SessionId)` (`app/src/terminal/history.rs:155`), `TerminalView::execute_pending_command(&mut self, _: (), ctx)` (view.rs:16258), `TerminalView::active_block_session_id(&self) -> Option<SessionId>`.
- Produces: nothing new for later tasks; Task 4's **Start** relies on this working.

- [ ] **Step 1: Write the failing test** in `app/src/terminal/view_tests.rs`. Build a bootstrapped view with a local session whose history is **not** yet initialized, set a pending command, deliver `BootstrapPrecmdDone`, assert the command is still pending; then initialize the session's history and assert the pending command was consumed. Use `add_window_with_terminal` (`app/src/test_util/terminal.rs:196`), `TerminalView::set_active_local_session_for_test` (view.rs:7944) for the session, `set_login_shell_bootstrapped_for_test`, and the block list's `set_bootstrapped()` (`model/blocks.rs:3111`). Initialize history with `History::handle(ctx).update(ctx, |h, ctx| h.init_session_with(session, async { vec![] }, ctx))` using the same `Session` the view's `Sessions` model holds for that id. Skeleton:

```rust
/// A pending command refused because the session's history was not registered yet must run
/// once it is; nothing else would retry it at an idle prompt.
#[test]
fn pending_command_runs_once_history_is_initialized() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        let cwd = std::env::temp_dir();
        terminal.update(&mut app, |view, ctx| {
            view.set_active_local_session_for_test(&cwd, ctx);
            view.set_login_shell_bootstrapped_for_test();
            view.model.lock().block_list_mut().set_bootstrapped();
            view.set_pending_command("echo resumed", ctx);
            view.execute_pending_command((), ctx);
        });
        assert!(
            terminal.read(&app, |view, ctx| view.input.as_ref(ctx).has_pending_command()),
            "with no history registered for the session the command must stay pending"
        );

        let session_id = terminal.read(&app, |view, _| view.active_block_session_id()).unwrap();
        let session = terminal.read(&app, |view, ctx| view.sessions.as_ref(ctx).get(session_id)).unwrap();
        app.update(|ctx| {
            History::handle(ctx).update(ctx, |history, ctx| {
                history.init_session_with(session, async { Vec::new() }, ctx)
            })
        });

        assert_eventually_within!(
            Duration::from_secs(5) =>
                !terminal.read(&app, |view, ctx| view.input.as_ref(ctx).has_pending_command()),
            "the pending command must run once the session's history is initialized"
        );
    });
}
```

Adjust only what the harness requires (field privacy, exact helper names); keep the two assertions.

- [ ] **Step 2: Run it and confirm it fails** on the second assertion.

Run: `cargo nextest run -p warp --lib -E 'test(pending_command_runs_once_history_is_initialized)'`
Expected: FAIL — "the pending command must run once the session's history is initialized".

- [ ] **Step 3: Implement.** In the `TerminalView` constructor, after the existing model subscriptions:

```rust
ctx.subscribe_to_model(&History::handle(ctx), |me, _, event, ctx| match event {
    HistoryEvent::Initialized(session_id) => {
        if me.active_block_session_id() == Some(*session_id) {
            me.execute_pending_command((), ctx);
        }
    }
});
```

In `Input::execute_pending_command`, replace the silent return with a debug log of the reason:

```rust
if let CanExecuteCommand::No(reason) = self.can_execute_command(ctx) {
    log::debug!("Holding the pending command: {reason:?}");
    return;
}
```

(derive or add `Debug` on `DenyExecutionReason` if missing).

- [ ] **Step 4: Run the test and the pane-group restore tests.**

Run: `cargo nextest run -p warp --lib -E 'test(pending_command_runs_once_history_is_initialized) | test(/^pane_group::/)'`
Expected: all PASS.

- [ ] **Step 5: Commit.**

```bash
git add app/src/terminal/view.rs app/src/terminal/input.rs app/src/terminal/view_tests.rs
git commit -m "fix(terminal): a pending command runs once its session's history is ready"
```

---

### Task 2: Past sessions — all agents, subfolder matching, cached in the background

**Files:**
- Modify: `app/src/terminal/agent_session_history.rs`
- Create: `app/src/terminal/agent_session_history_model.rs`
- Modify: `app/src/terminal/mod.rs` (declare the module), `app/src/lib.rs` (register the singleton where `CLIAgentSessionsModel` is registered)
- Test: `app/src/terminal/agent_session_history_tests.rs`, `app/src/terminal/agent_session_history_model_tests.rs`

**Interfaces:**
- Produces:
  - `pub(crate) fn is_within(path: &Path, folder: &Path) -> bool` — `path` equals `folder` or is under it, using the same normalization as the private `same_directory` (canonicalize; on Windows compare case-insensitively with `\` mapped to `/`).
  - `pub(crate) const RESUMABLE_AGENTS: [CLIAgent; 3] = [CLIAgent::Claude, CLIAgent::Codex, CLIAgent::Pi];`
  - `pub(crate) fn find_all_resumable() -> Vec<PastAgentSession>` — `find_all` for each of `RESUMABLE_AGENTS`, merged, newest `modified` first.
  - `pub struct AgentSessionHistoryModel` (`SingletonEntity`, `Event = AgentSessionHistoryEvent`) with `pub fn sessions(&self) -> &[PastAgentSession]`, `pub fn is_loaded(&self) -> bool`, `pub fn refresh(&mut self, ctx: &mut ModelContext<Self>)` (runs `find_all_resumable` on `ctx.background_executor()`, stores the result, emits `AgentSessionHistoryEvent::Updated`). A refresh while one is running is coalesced (one in flight at a time).

- [ ] **Step 1: Failing tests** in `agent_session_history_tests.rs`:

```rust
#[test]
fn is_within_matches_the_folder_and_its_subfolders() {
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join(".claude").join("worktrees").join("fix");
    std::fs::create_dir_all(&worktree).unwrap();
    assert!(is_within(root.path(), root.path()));
    assert!(is_within(&worktree, root.path()));
    assert!(!is_within(root.path(), &worktree));
    assert!(!is_within(Path::new("/elsewhere"), root.path()));
}
```

and in `agent_session_history_model_tests.rs` a test that `refresh` populates `sessions()` from a fixture HOME containing one Pi session file (reuse the Pi fixture builder already in `agent_session_history_tests.rs`) and emits `Updated`.

- [ ] **Step 2: Run, confirm FAIL** (`is_within` / model not defined).

Run: `cargo nextest run -p warp --lib -E 'test(/agent_session_history/)'`

- [ ] **Step 3: Implement** `is_within`, `RESUMABLE_AGENTS`, `find_all_resumable`, and the model. Register `AgentSessionHistoryModel` next to `CLIAgentSessionsModel` registration (search `add_singleton_model(|_| CLIAgentSessionsModel::new())` in `app/src/lib.rs` and in test init helpers `app/src/pane_group/mod_tests.rs:200`, `app/src/workspace/view_tests.rs` `initialize_app`).

- [ ] **Step 4: Run, confirm PASS.**

- [ ] **Step 5: Commit** — `feat(sessions): list resumable agent sessions across agents, cached in the background`.

---

### Task 3: Rows — one per conversation, in its strongest state

**Files:**
- Create: `app/src/terminal/agent_sessions_index.rs`, `app/src/terminal/agent_sessions_index_tests.rs`
- Modify: `app/src/terminal/mod.rs`
- Modify: `app/src/pane_group/pane/terminal_pane.rs` (public accessor for the pending restore), `app/src/pane_group/mod.rs` (wrapper by pane id)
- Modify: `app/src/terminal/detached_sessions.rs` (structured fields)

**Interfaces:**
- Consumes: `PastAgentSession` (Task 2), `CliAgentResumeDescriptor` (`app/src/app_state.rs:269`), `PaneViewLocator` (`app/src/workspace/util.rs:13`), `warp_core::SessionId`, `CLIAgent`.
- Produces (all `pub(crate)`):

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AgentSessionState {
    Running { locator: PaneViewLocator },
    Pending { locator: PaneViewLocator },
    Detached { session_id: warp_core::SessionId },
    Past,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentSessionRow {
    pub agent: Option<CLIAgent>,          // None for a detached shell whose agent is unknown
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub last_used: Option<SystemTime>,
    pub state: AgentSessionState,
}

/// One terminal pane of the window, in tab order, as the window view groups it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaneAgentInput {
    pub locator: PaneViewLocator,
    pub tab_index: usize,
    pub tab_title: String,
    pub cwd: Option<PathBuf>,
    pub running: Option<CliAgentResumeDescriptor>,   // from CLIAgentSessionsModel
    pub pending: Option<CliAgentResumeDescriptor>,   // restored, not yet running
}

pub(crate) struct DetachedInput {
    pub session_id: warp_core::SessionId,
    pub title: String,
    pub cwd: PathBuf,
    pub detached_for_seconds: u64,
}

pub(crate) fn build_rows(
    panes: &[PaneAgentInput],
    detached: &[DetachedInput],
    past: &[PastAgentSession],
) -> Vec<AgentSessionRow>;
```

  Precedence: `Running` > `Detached` > `Pending` > `Past`, keyed by `(agent, session_id)`. A `Past` row whose `(agent, session_id)` matches a running or pending pane is dropped. Rows without a session id are kept (one per pane). Output order: running and pending rows in tab/pane order, then detached (most recently detached first), then past (newest first).

  A detached shell's agent is unknown (`SessionMetadata` has no agent field, `crates/warp_terminal/src/local_tty/supervisor/message.rs:148`); it becomes a `Detached` row with `agent: None`, and a `Past` row is merged into it only when the past session's `title` equals the detached `title` and its cwd is within the detached cwd.

- `TerminalPane`: `pub(crate) fn pending_agent_resume(&self, app: &AppContext) -> Option<CliAgentResumeDescriptor>` — the existing private `restored_agent_still_resuming` (terminal_pane.rs:246) made `pub(crate)` under this name.
- `PaneGroup`: `pub(crate) fn pending_agent_resume(&self, pane_id: PaneId, app: &AppContext) -> Option<CliAgentResumeDescriptor>` via `downcast_pane_by_id::<TerminalPane>`.
- `DetachedSession` gains `pub cwd: PathBuf`, `pub title: String`, `pub detached_for_seconds: u64` (filled from `SessionSummary.metadata` in `list`); `label` stays.
- `detached_sessions::list_checked(ctx: &AppContext) -> Result<Vec<DetachedSession>, DetachedListError>` where `DetachedListError::{Unsupported, Unreachable(String)}`; `list` keeps its signature and calls it. The pickers use `list_checked` so an unreachable supervisor shows one "Supervisor unreachable" row instead of an empty section.

- [ ] **Step 1: Failing tests** in `agent_sessions_index_tests.rs`, pure (no `App`):

```rust
fn claude(id: &str, cwd: &str) -> CliAgentResumeDescriptor {
    CliAgentResumeDescriptor { agent: CLIAgent::Claude, session_id: Some(id.into()), cwd: cwd.into(), title: None, captured_unix_seconds: None }
}

#[test]
fn a_running_conversation_appears_once_as_running() {
    let pane = PaneAgentInput { running: Some(claude("a", "/p")), ..pane_at(0, "/p") };
    let past = vec![past_session(CLIAgent::Claude, "a", "/p")];
    let rows = build_rows(&[pane], &[], &past);
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].state, AgentSessionState::Running { .. }));
}

#[test]
fn a_pending_restore_outranks_its_past_entry() { /* pending "b" + past "b" -> one Pending row */ }

#[test]
fn past_sessions_follow_live_ones_newest_first() { /* two past rows ordered by modified desc after a running row */ }

#[test]
fn an_agent_without_a_session_id_is_kept_per_pane() { /* two panes, running with session_id None -> two rows */ }
```

(`pane_at(index, cwd)` and `past_session(agent, id, cwd)` are small builders in the test file.)

- [ ] **Step 2: Run, confirm FAIL.** `cargo nextest run -p warp --lib -E 'test(/agent_sessions_index/)'`
- [ ] **Step 3: Implement** `build_rows` and the accessors.
- [ ] **Step 4: Run, confirm PASS** (plus `detached_sessions` tests).
- [ ] **Step 5: Commit** — `feat(sessions): merge live, pending, detached and past agent sessions into one list`.

---

### Task 4: Actions — Focus, Start, Attach, Resume

**Files:**
- Create: `app/src/workspace/agent_session_actions.rs`, `app/src/workspace/agent_session_actions_tests.rs`
- Modify: `app/src/workspace/mod.rs` (module), `app/src/workspace/action.rs` (new action), `app/src/workspace/view.rs` (handler + window walk), `app/src/pane_group/mod.rs` (public split-into-folder)

**Interfaces:**
- Consumes: Task 3 rows; `Workspace::focus_pane(PaneViewLocator, ctx)` (view.rs:6289); `Workspace::attach_session_by_id(SessionId, ctx) -> anyhow::Result<()>` (view.rs:14681, `local_tty`); `PaneGroup::terminal_manager_for_pane_id` + `TerminalManager::ensure_shell_started`; `TerminalView::execute_command_or_set_pending`; `PaneGroup::add_session_in_directory` (mod.rs:6914, private); `process_registry::running_session_ids()`; `ShellFamily` (`crates/warp_util/src/path.rs:210`) and `TerminalView::shell_family(ctx)`; `toast_stack` error toasts.
- Produces:

```rust
/// What a row's click does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentSessionAction {
    Focus(PaneViewLocator),
    Start(PaneViewLocator),
    Attach(warp_core::SessionId),
    Resume { agent: CLIAgent, session_id: String, cwd: PathBuf, target: Option<PaneViewLocator> },
}

pub(crate) fn action_for(row: &AgentSessionRow, target: Option<PaneViewLocator>) -> Option<AgentSessionAction>;

/// `cd <cwd>` joined to `resume` with the shell's separator.
pub(crate) fn resume_command_line(cwd: &Path, resume: &str, shell: ShellKind) -> String;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShellKind { Posix, Fish, PowerShell }
```

  - `WorkspaceAction::AgentSession(AgentSessionAction)` — listed in the `false` arm of `should_save_app_state_on_action` (action.rs:987) and handled in `Workspace::handle_action`.
  - `Workspace::agent_session_inputs(&self, ctx: &AppContext) -> (Vec<PaneAgentInput>, Vec<DetachedInput>)` — walks `self.tabs` like `terminal_panes` (view.rs:14481): for each terminal pane, `running` from `CLIAgentSessionsModel::session(view_id)` (+ `session_id(view_id)`), `pending` from `PaneGroup::pending_agent_resume`, `cwd` from `pane_group.terminal_working_directory`; detached from `detached_sessions::list`.
  - `PaneGroup::split_terminal_in_directory(&mut self, base: PaneId, directory: PathBuf, ctx) -> TerminalPaneId` — public wrapper over `add_session_in_directory(Direction::Right, Some(base), None, Some(directory), None, DefaultSessionModeBehavior::Ignore /* the terminal default */, ctx)`.

  Behaviour of `Workspace::perform_agent_session_action`:
  - `Focus` → `focus_pane`.
  - `Start` → `ensure_shell_started` on the pane's manager, then `focus_pane`.
  - `Attach` → `attach_session_by_id`; on `Err` show `DismissibleToast::error(format!("Couldn't attach the session: {error:#}"))`.
  - `Resume` → if `session_id` is in `running_session_ids()` or any pane's running descriptor, focus that pane (or toast "This conversation is already running outside this window"). Otherwise build the command from `CliAgentResumeDescriptor { agent, session_id: Some(..), cwd, .. }.resume_command()`; if the target pane's view is idle (`is_login_shell_bootstrapped() && !is_long_running() && !has_pending_command_or_awaiting_completion(ctx)` and no running agent) run `resume_command_line(...)` there; else `split_terminal_in_directory(target, cwd)` and run the bare resume command in the new pane (it already starts in `cwd`).

- [ ] **Step 1: Failing tests** (pure, in `agent_session_actions_tests.rs`):

```rust
#[test]
fn resume_command_line_uses_each_shells_separator() {
    let cwd = Path::new("T:/p/x");
    assert_eq!(resume_command_line(cwd, "pi --session 1", ShellKind::Posix), "cd T:/p/x && pi --session 1");
    assert_eq!(resume_command_line(cwd, "pi --session 1", ShellKind::PowerShell), "cd T:/p/x; pi --session 1");
    assert_eq!(resume_command_line(cwd, "pi --session 1", ShellKind::Fish), "cd T:/p/x; and pi --session 1");
}

#[test]
fn a_path_with_spaces_is_escaped() { /* "T:/my dir" -> escaped per ShellFamily::shell_escape */ }

#[test]
fn each_state_maps_to_its_action() {
    // Running -> Focus, Pending -> Start, Detached -> Attach, Past -> Resume (with target)
}

#[test]
fn a_past_row_without_a_resume_command_has_no_action() { /* agent Gemini -> None */ }
```

- [ ] **Step 2: Run, confirm FAIL.** `cargo nextest run -p warp --lib -E 'test(/agent_session_actions/)'`
- [ ] **Step 3: Implement** the module, the action variant and handler, `agent_session_inputs`, and `split_terminal_in_directory`.
- [ ] **Step 4: Run, confirm PASS.** Also run `cargo nextest run -p warp --lib -E 'test(/^pane_group::/)'`.
- [ ] **Step 5: Commit** — `feat(sessions): focus, start, attach or resume an agent session from one action`.

---

### Task 5: Palette — the "Agent sessions" mode

**Files:**
- Modify: `crates/warp_search_core/src/data_source.rs` (variant + atom `"agents:"` + placeholder "Search agent sessions" + display name "Agent sessions" + icon `None` or the terminal icon path)
- Modify: `app/src/search/command_palette/filter_chip_renderer.rs:97` (icon colour arm), `zero_state.rs:80` (show chip after Conversations), `data_sources.rs` (register), `mixer.rs` (new `CommandPaletteItemAction::AgentSession { action: AgentSessionAction }` + `ItemSummary` arm + `to_summary`), `view.rs` (`handle_result_accepted` arm dispatching `WorkspaceAction::AgentSession(action)`; `is_mode_enabled` pair; palette-binding intercept arm)
- Modify: `app/src/palette.rs` (`PaletteMode::AgentSessions`), `app/src/workspace/view.rs` (`open_palette` arm; `open_agent_sessions_palette(folder: Option<String>, ctx)` using `open_palette_action(PaletteMode::AgentSessions, PaletteSource::..., folder.as_deref(), ctx)`), `app/src/workspace/mod.rs` (binding `"workspace:search_agent_sessions"`, "Agent sessions: search", `WorkspaceAction::OpenPalette { mode: PaletteMode::AgentSessions, .. }`)
- Create: `app/src/search/command_palette/agent_sessions/{mod.rs,data_source.rs,search_item.rs,search.rs,search_tests.rs}`

**Interfaces:**
- Consumes: Task 3 `build_rows`, Task 4 `action_for`, `Workspace::agent_session_inputs`, `AgentSessionHistoryModel` (Task 2; call `refresh` when the mode opens).
- Produces: `agent_sessions::DataSource` (`SyncDataSource<Action = CommandPaletteItemAction>`), sections **This pane**, **Running elsewhere**, **Detached**, **Past** pushed in reverse order with `SeparatorSearchItem` (pattern: `conversations/data_source.rs:148-227`); fuzzy match over title, folder, agent display name, session id with `fuzzy_match::match_indices_case_insensitive` (pattern: `conversations/search.rs:95-171`); `pub(crate) fn score_row(row: &AgentSessionRow, query: &str) -> Option<i64>`.

- [ ] **Step 1: Failing tests** in `agent_sessions/search_tests.rs`: `score_row` matches on each field (title, folder segment, `"claude"`, id prefix) and returns `None` for no match; zero-state section order.
- [ ] **Step 2: Run, confirm FAIL.** `cargo nextest run -p warp --lib -E 'test(/command_palette::agent_sessions/)'`
- [ ] **Step 3: Implement** the data source, item rendering (icon = `CLIAgent::icon()` falling back to `Icon::Terminal`; title or short id; folder; `format_approx_duration_from_now`; state hint "Focus"/"Start"/"Attach"/"Resume"), and all registration points listed above. Add a mixer-level test modelled on `data_sources_tests.rs:214-262` that registers the source under `QueryFilter::AgentSessions` and gets results.
- [ ] **Step 4: Run, confirm PASS**, plus `cargo nextest run -p warp --lib -E 'test(/command_palette/)'`.
- [ ] **Step 5: Commit** — `feat(palette): search agent sessions and act on them`.

---

### Task 6: Window view — agent sessions by tab and pane

**Files:**
- Create: `app/src/workspace/agent_sessions_view.rs`, `app/src/workspace/agent_sessions_view_tests.rs`
- Modify: `app/src/workspace/view.rs` (field `agent_sessions_modal: ModalViewState<Modal<AgentSessionsView>>`, build/open/close like `session_config_modal` at 2214-2245/2738-2765, render next to `attach_session_menu_modal` at ~29529, handle `AgentSessionsViewEvent::{Act(AgentSessionAction), ResumeInFolder { folder, target }, Close}`), `app/src/workspace/action.rs` (`ShowAgentSessions { focus: Option<PaneViewLocator> }`, false arm), `app/src/workspace/mod.rs` (binding `"workspace:show_agent_sessions"`, "Agent sessions: show window")
- Modify: `app/src/terminal/view/action.rs` (`TerminalAction::ShowAgentSessions` + Debug arm), `app/src/terminal/view.rs` (handler → `dispatch_typed_action_deferred(WorkspaceAction::ShowAgentSessionsForTerminalView { terminal_view_id })`; accessibility list arm ~27311), `app/src/terminal/view/pane_impl.rs:804-830` (menu item "Agent sessions…")

**Interfaces:**
- Consumes: Task 3 rows and `PaneAgentInput` (tab grouping), Task 4 actions, Task 5 `open_agent_sessions_palette`.
- Produces:

```rust
/// The rows the view draws, grouped as the window is.
pub(crate) enum ViewRow {
    TabHeader { title: String },
    Pane { input: PaneAgentInput, row: Option<AgentSessionRow>, action: ViewRowAction },
    PastToggle { folder: PathBuf, count: usize, expanded: bool },
    Past { row: AgentSessionRow },
    DetachedHeader { count: usize },
    Detached { row: AgentSessionRow },
}

pub(crate) enum ViewRowAction { Act(AgentSessionAction), ResumeInFolder { folder: PathBuf, target: PaneViewLocator }, None }

pub(crate) fn view_rows(
    panes: &[PaneAgentInput],
    rows: &[AgentSessionRow],
    expanded_folders: &HashSet<PathBuf>,
    query: &str,
) -> Vec<ViewRow>;
```

  `AgentSessionsView` holds a single-line `EditorView` (pattern: `workspace/view/conversation_list/view.rs:226-246`, placeholder "Search"), `expanded_folders`, the inputs, and renders `view_rows` with buttons that emit `AgentSessionsViewEvent::Act(..)` / `ResumeInFolder` / toggle expansion. Up/Down/Enter bindings like `register_conversation_list_view_bindings` (conversation_list/view.rs:192-210); Escape closes via the `Modal`.

- [ ] **Step 1: Failing tests** for `view_rows` (pure): panes grouped under their tab headers in tab order; a running pane row gets `Act(Focus)`, pending `Act(Start)`, a plain shell `ResumeInFolder`; past rows for a folder appear only when it is expanded, counting subfolder sessions; query filters rows but keeps their tab header; detached section last.
- [ ] **Step 2: Run, confirm FAIL.** `cargo nextest run -p warp --lib -E 'test(/agent_sessions_view/)'`
- [ ] **Step 3: Implement** `view_rows`, the view, the modal wiring, the pane-menu item and both bindings. "Resume…" (`ResumeInFolder`) closes the modal and calls `open_agent_sessions_palette(Some(folder))` with the pane as target.
- [ ] **Step 4: Run, confirm PASS**, plus the pane-menu label test pattern from `workspace/view_tests.rs:~6860` (`move_session_is_offered`) for "Agent sessions…".
- [ ] **Step 5: Commit** — `feat(sessions): show a window's agent sessions by tab and pane`.

---

### Task 7: Verify, merge, install

- [ ] `cargo fmt -p warp -p warp_search_core` and `cargo clippy -p warp -p warp_search_core --all-targets --tests -- -D warnings` — clean.
- [ ] `cargo nextest run -p warp --lib --no-fail-fast -E 'test(/^pane_group::|^terminal::|^search::|^workspace::|^launch_configs/)'` — only the known `StoznAuthManager` failures remain.
- [ ] Push `feat/agent-sessions-picker` to `gitea` and `gitlab`; merge into `dev` (`--no-ff`), push `dev` to both; build and install with `script/windows/install-warpspeed.ps1` (never restart running WarpSpeed processes).
- [ ] Ask the user to verify in a newly launched WarpSpeed: pane menu → "Agent sessions…", palette `agents:`, and that a restored agent now starts on its own when its tab opens.
