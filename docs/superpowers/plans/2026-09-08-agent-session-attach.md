# Agent Session Attach Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Detaching a session never closes Warp, and a cold-started window offers a picker listing both supervisor-held sessions and closed Claude, Codex and Pi conversations, each openable in a new tab.

**Architecture:** A new `app/src/terminal/agent_session_history/` module enumerates the three agents' own transcript directories on a background thread and caches the result in a singleton model. The existing attach picker grows a second section fed from that cache, and its entry points stop being gated on a reachable supervisor. Detach gains one guard: if closing the pane would close the window, a replacement tab is added first.

**Tech Stack:** Rust, the `warpui` entity/view framework, `serde_json` for transcript heads, `tempfile` for test fixtures.

**Spec:** `docs/superpowers/specs/2026-09-08-agent-session-attach-design.md`

## Global Constraints

- The app crate is named `warp`. Run tests with `cargo test -p warp <filter>`. Integration tests live in `crates/integration`.
- Unit tests go in a sibling file wired with `#[cfg(test)] #[path = "<name>_tests.rs"] mod tests;` at the bottom of the module — the convention used by `app/src/terminal/detached_sessions.rs`.
- **Never run `git add -A` or `git add .` in this repository.** Other sessions edit this working tree concurrently. Every commit stages explicit paths, exactly as written in the plan's commit steps.
- Bounds, used verbatim wherever they appear:
  - `MAX_CANDIDATES: usize = 200` — transcripts whose heads get read, across all agents
  - `MAX_HEAD_LINES: usize = 200` and `MAX_HEAD_BYTES: usize = 256 * 1024` — how far into one transcript a scanner reads
  - `MAX_MENU_ROWS: usize = 20` — rows in the picker's "Recent" section
  - `TITLE_MAX_CHARS: usize = 80` — before a title is cut and an ellipsis appended
  - `GET_STARTED_ROWS: usize = 3` — rows on the get-started pane
- New keystroke: `cmd-shift-L` on Mac, `ctrl-shift-L` on Linux and Windows.
- Section labels in the picker are exactly `Running` and `Recent`. The modal title is exactly `Sessions`. The tab-menu row label is exactly `Attach or resume session…`. The new-tab dropdown row label is exactly `Resume session…`.
- Scanner functions take their root directory as a parameter so tests inject a `tempfile::TempDir` rather than redirecting `HOME`. Only `root()` consults the environment, and it has no test.

---

## Verified transcript shapes

These were read off real files. Do not substitute a guess for any of them.

**Pi** — `~/.pi/agent/sessions/--<slug>--/<timestamp>_<uuid>.jsonl`. Line 1 is the identity record; the first user message follows:

```json
{"type":"session","version":3,"id":"01a080b4-...","timestamp":"2026-09-08T11:08:08.384Z","cwd":"T:\\projects\\dev-ops\\warp"}
{"type":"message","id":"1c50ecb7","message":{"role":"user","content":[{"type":"text","text":"i can..."}]}}
```

**Codex** — `<sessions>/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`. Line 1 is `session_meta`. **The first entry with `role: "user"` is an injected AGENTS.md block, not the prompt** — titling from it produces "# AGENTS.md instructions for …" on every row. The real prompt is an `event_msg`:

```json
{"timestamp":"...","type":"session_meta","payload":{"session_id":"019f17d6-...","cwd":"T:\\projects\\SOTZ3N\\stoz3n-chat-agent","cli_version":"0.142.4"}}
{"timestamp":"...","type":"event_msg","payload":{"type":"user_message","message":"Say OK only.","images":[]}}
```

Reaching that line took 45 KB in the samples, which is why `MAX_HEAD_BYTES` is 256 KB and not 64 KB.

**Claude** — `<config>/projects/<slug>/<uuid>.jsonl`. Early lines are bookkeeping (`last-prompt`, `mode`, `permission-mode`) with no `cwd`. Hook attachments carry `cwd` before any message does. The first real prompt is a `type: "user"` entry whose `message.content` is a plain string (it is an array of typed parts in other transcripts):

```json
{"type":"user","cwd":"T:\\projects\\dev-ops\\warp","sessionId":"357091b5-...","message":{"role":"user","content":"how do we stand with..."}}
```

---

## File structure

| File | Responsibility |
|---|---|
| `app/src/terminal/agent_session_history/mod.rs` | `PastAgentSession`, `Candidate`, shared reading and text helpers, the merge/order pipeline |
| `app/src/terminal/agent_session_history/mod_tests.rs` | helper and ordering tests |
| `app/src/terminal/agent_session_history/pi.rs` + `pi_tests.rs` | Pi's layout only |
| `app/src/terminal/agent_session_history/claude.rs` + `claude_tests.rs` | Claude's layout only |
| `app/src/terminal/agent_session_history/codex.rs` + `codex_tests.rs` | Codex's layout only |
| `app/src/terminal/agent_session_history/model.rs` | the singleton cache and its background refresh |
| `app/src/workspace/view.rs` | detach guard, picker construction, resume action, dropdown row |
| `app/src/workspace/action.rs` | `ResumeAgentSession` |
| `app/src/workspace/mod.rs` | keystroke, dropped flag gate |
| `app/src/tab.rs` | ungated tab-menu row |
| `app/src/pane_group/pane/get_started_view.rs` | recent-session rows |
| `app/src/terminal/cli_agent_sessions/mod.rs` | `live_session_ids()` accessor |
| `crates/integration/src/test/detached_sessions.rs` | last-pane detach round trip |

---

## Task 1: Detaching the last pane keeps the window

**Files:**
- Modify: `app/src/workspace/view.rs:13555-13585` (`detach_session_for_terminal_view`)
- Test: `crates/integration/src/test/detached_sessions.rs`
- Modify: `crates/integration/src/bin/integration.rs:145`, `crates/integration/tests/integration/ui_tests.rs:238`

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: nothing other tasks rely on.

**Background:** `detach_session_for_terminal_view` currently ends with `Ok(()) => self.close_pane(pane_group_id, pane_id, ctx)`. The existing integration test's own doc comment concedes the problem: "The pane detached is a split rather than the tab's only one, so closing it leaves a window standing to attach into."

- [ ] **Step 1: Write the failing integration test**

Add to `crates/integration/src/test/detached_sessions.rs`, after the existing test function:

```rust
/// Detaching a tab's only pane leaves the window standing.
///
/// Closing the last pane closes the tab, closing the last tab closes the
/// window, and closing the last window ends the process -- so before this
/// guard existed, "detach" and "quit, losing the way back" were the same
/// keystroke. The assertion is deliberately about the window still having a
/// usable tab rather than about the process: a test that has already exited
/// cannot report anything.
pub fn test_detaching_the_only_pane_keeps_the_window() -> Builder {
    FeatureFlag::PersistentTerminalSessions.set_enabled(true);

    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(execute_long_running_command_for_pane(0, 0, "sleep 300"))
        .with_step(
            TestStep::new("The only pane's shell is supervisor-owned").add_named_assertion(
                "the pane has a persistent session",
                |app, window_id| {
                    let session_id = pane_session(app, window_id, 0, 0);
                    async_assert!(
                        session_id.is_some(),
                        "the pane's shell must belong to the supervisor before it can be \
                         detached; no session id means the supervisor was unreachable"
                    )
                },
            ),
        )
        .with_step(
            TestStep::new("Detach the only pane's session")
                .with_action(|app, window_id, _| {
                    let workspace = workspace_view(app, window_id);
                    app.update(|ctx| {
                        ctx.dispatch_typed_action_for_view(
                            window_id,
                            workspace.id(),
                            &WorkspaceAction::DetachSession,
                        );
                    });
                })
                .set_timeout(Duration::from_secs(30))
                .set_retries(2)
                .add_assertion(assert_tab_count(1))
                .add_named_assertion("the window still holds a terminal pane", |app, window_id| {
                    let panes = pane_group_view(app, window_id, 0)
                        .read(app, |pane_group, _| pane_group.visible_pane_ids().len());
                    async_assert!(
                        panes == 1,
                        "detaching the only pane must leave a replacement behind; {panes} \
                         panes means the window was about to close"
                    )
                }),
        )
        .with_step(
            TestStep::new("The replacement is a fresh shell, not the detached one")
                .set_timeout(Duration::from_secs(30))
                .set_retries(2)
                .add_named_assertion(
                    "the supervisor is holding a session no pane is on",
                    |app, window_id| {
                        let held = workspace_view(app, window_id).read(app, |_, ctx| {
                            !warp::terminal::detached_sessions::list(ctx).is_empty()
                        });
                        async_assert!(
                            held,
                            "the detached session must be listed as attachable; an empty list \
                             means the handover never reached the supervisor"
                        )
                    },
                ),
        )
}
```

- [ ] **Step 2: Register the test**

In `crates/integration/src/bin/integration.rs`, beside line 145:

```rust
    register_test!(test_detaching_the_only_pane_keeps_the_window);
```

In `crates/integration/tests/integration/ui_tests.rs`, beside line 238:

```rust
    test_detaching_the_only_pane_keeps_the_window,
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p integration test_detaching_the_only_pane_keeps_the_window`
Expected: FAIL — the window closes during the detach step, so the assertions cannot run.

- [ ] **Step 4: Add the guard**

In `app/src/workspace/view.rs`, replace the `match` at the end of `detach_session_for_terminal_view`:

```rust
        match self.hand_session_to_supervisor(pane_group_id, pane_id, ctx) {
            Ok(()) => self.close_pane(pane_group_id, pane_id, ctx),
```

with:

```rust
        match self.hand_session_to_supervisor(pane_group_id, pane_id, ctx) {
            Ok(()) => {
                // A replacement goes in before the close, never after: if the
                // window empties even for an instant, the app is gone and
                // there is nothing left to add a tab to. Detaching one pane of
                // a split still just closes that pane.
                if self.detach_would_empty_the_window(pane_group_id, pane_id, ctx) {
                    self.add_replacement_tab_for_detach(pane_group_id, pane_id, ctx);
                }
                self.close_pane(pane_group_id, pane_id, ctx)
            }
```

Add both helpers immediately after `detach_session_for_terminal_view`:

```rust
    /// Whether closing this pane would take the whole window with it.
    ///
    /// True only for the last pane of the last tab. A tab's only pane in a
    /// window that has other tabs is fine to close: the tab goes, the window
    /// stays.
    fn detach_would_empty_the_window(
        &self,
        pane_group_id: EntityId,
        pane_id: PaneId,
        ctx: &AppContext,
    ) -> bool {
        if self.tabs.len() != 1 {
            return false;
        }
        let Some(pane_group) = self.get_pane_group_view_with_id(pane_group_id) else {
            return false;
        };
        let pane_group = pane_group.as_ref(ctx);
        pane_group.pane_count() == 1 && pane_group.pane_ids().any(|id| id == pane_id)
    }

    /// Opens a plain terminal tab in the departing session's directory.
    ///
    /// The directory is carried over so the replacement lands where the user
    /// was working; when it cannot be read, a default shell in the default
    /// directory is still better than a closed window.
    fn add_replacement_tab_for_detach(
        &mut self,
        pane_group_id: EntityId,
        pane_id: PaneId,
        ctx: &mut ViewContext<Self>,
    ) {
        let initial_directory = self
            .get_pane_group_view_with_id(pane_group_id)
            .and_then(|pane_group| {
                pane_group
                    .as_ref(ctx)
                    .terminal_view_from_pane_id(pane_id, ctx)
            })
            .and_then(|view| view.as_ref(ctx).pwd_if_local(ctx));

        self.add_tab_with_pane_layout(
            PanesLayout::SingleTerminal(Box::new(NewTerminalOptions {
                initial_directory,
                ..Default::default()
            })),
            Arc::new(HashMap::new()),
            None, /*custom_tab_title*/
            ctx,
        );
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p integration test_detaching_the_only_pane_keeps_the_window`
Expected: PASS

- [ ] **Step 6: Confirm the existing round trip still passes**

Run: `cargo test -p integration test_detach_and_attach_a_session`
Expected: PASS — detaching a split must be unchanged.

- [ ] **Step 7: Commit**

```bash
git add app/src/workspace/view.rs crates/integration/src/test/detached_sessions.rs crates/integration/src/bin/integration.rs crates/integration/tests/integration/ui_tests.rs
git commit -m "fix(terminal): keep the window when the detached pane is the last one

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_017zUJLduZivuZHisz9e7YeY"
```

---

## Task 2: The session-history module and its shared helpers

**Files:**
- Create: `app/src/terminal/agent_session_history/mod.rs`
- Create: `app/src/terminal/agent_session_history/mod_tests.rs`
- Modify: `app/src/terminal/mod.rs` (add the module declaration)

**Interfaces:**
- Consumes: `crate::terminal::cli_agent::CLIAgent`.
- Produces, for Tasks 3–6:
  - `pub struct PastAgentSession { pub agent: CLIAgent, pub session_id: String, pub cwd: PathBuf, pub title: Option<String>, pub modified: SystemTime }`
  - `pub(super) struct Candidate { pub agent: CLIAgent, pub path: PathBuf, pub session_id: String, pub modified: SystemTime }`
  - `pub(super) fn read_head(path: &Path) -> Vec<Value>`
  - `pub(super) fn as_title(text: &str) -> Option<String>`
  - `pub(super) fn jsonl_files(dir: &Path) -> Vec<(PathBuf, SystemTime)>`
  - `pub(super) fn subdirs(dir: &Path) -> Vec<PathBuf>`
  - `pub(super) fn home_dir() -> Option<PathBuf>`
  - `pub(super) const MAX_CANDIDATES: usize`

- [ ] **Step 1: Write the failing tests**

Create `app/src/terminal/agent_session_history/mod_tests.rs`:

```rust
use std::fs;

use super::*;

fn write(dir: &std::path::Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, contents).expect("fixture write");
    path
}

#[test]
fn read_head_skips_lines_that_are_not_json() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write(
        dir.path(),
        "a.jsonl",
        "not json at all\n{\"type\":\"session\"}\n\n{\"type\":\"message\"}\n",
    );

    let head = read_head(&path);

    assert_eq!(head.len(), 2);
    assert_eq!(head[0]["type"], "session");
    assert_eq!(head[1]["type"], "message");
}

#[test]
fn read_head_of_a_missing_file_is_empty() {
    let dir = tempfile::tempdir().expect("temp dir");
    assert!(read_head(&dir.path().join("nothing.jsonl")).is_empty());
}

#[test]
fn read_head_of_an_empty_file_is_empty() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write(dir.path(), "empty.jsonl", "");
    assert!(read_head(&path).is_empty());
}

#[test]
fn read_head_stops_at_the_line_budget() {
    let dir = tempfile::tempdir().expect("temp dir");
    let body = "{\"n\":1}\n".repeat(MAX_HEAD_LINES + 50);
    let path = write(dir.path(), "long.jsonl", &body);

    assert_eq!(read_head(&path).len(), MAX_HEAD_LINES);
}

#[test]
fn a_title_is_the_first_non_empty_line() {
    assert_eq!(
        as_title("\n\n  fix the hit test  \nand then some\n"),
        Some("fix the hit test".to_owned())
    );
}

#[test]
fn a_long_title_is_cut_and_marked() {
    let long = "x".repeat(TITLE_MAX_CHARS + 10);

    let title = as_title(&long).expect("a title");

    assert_eq!(title.chars().count(), TITLE_MAX_CHARS + 1);
    assert!(title.ends_with('…'));
}

#[test]
fn blank_text_has_no_title() {
    assert_eq!(as_title("   \n\n  "), None);
}

#[test]
fn jsonl_files_ignores_other_extensions_and_directories() {
    let dir = tempfile::tempdir().expect("temp dir");
    write(dir.path(), "keep.jsonl", "{}\n");
    write(dir.path(), "skip.json", "{}\n");
    fs::create_dir(dir.path().join("nested.jsonl")).expect("fixture dir");

    let found = jsonl_files(dir.path());

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0.file_name().unwrap(), "keep.jsonl");
}

#[test]
fn jsonl_files_of_a_missing_directory_is_empty() {
    let dir = tempfile::tempdir().expect("temp dir");
    assert!(jsonl_files(&dir.path().join("gone")).is_empty());
}

#[test]
fn subdirs_lists_only_directories() {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::create_dir(dir.path().join("one")).expect("fixture dir");
    write(dir.path(), "two", "");

    let found = subdirs(dir.path());

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].file_name().unwrap(), "one");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p warp agent_session_history`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Write the module**

Create `app/src/terminal/agent_session_history/mod.rs`:

```rust
//! The sessions the CLI agents themselves remember.
//!
//! The supervisor knows about shells that are still running. This module knows
//! about conversations that have ended: Claude, Codex and Pi each keep every
//! session they have ever held in a directory of JSONL transcripts, and each
//! of their CLIs can be told to reopen one by ID. That history survives a
//! reboot, a supervisor that never came back, and an app that was never asked
//! nicely to quit -- which is exactly when the supervisor's own list is empty
//! and a user is looking for the way back in.
//!
//! Nothing here reads a whole transcript. They reach tens of megabytes, and a
//! menu needs four fields out of the first few lines.

// Each scanner and the cache declare themselves as they land; see Tasks 3-6.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::terminal::cli_agent::CLIAgent;

/// How many transcripts get their heads read, across all three agents.
pub(super) const MAX_CANDIDATES: usize = 200;
/// How far into one transcript a scanner looks before giving up on it.
///
/// Codex writes its injected instruction blocks ahead of the user's first
/// prompt, and those ran to 45 KB in the transcripts this was measured
/// against, so the byte budget is deliberately far above what a title needs.
const MAX_HEAD_LINES: usize = 200;
const MAX_HEAD_BYTES: usize = 256 * 1024;
/// How long a title may be before it is cut.
const TITLE_MAX_CHARS: usize = 80;
/// How many rows the picker's recent section shows.
pub const MAX_MENU_ROWS: usize = 20;

/// A closed agent session: enough to tell it apart, and enough to reopen it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PastAgentSession {
    pub agent: CLIAgent,
    pub session_id: String,
    /// Where the session ran. Empty when the transcript's head never said,
    /// which costs a useful label but not the ability to resume.
    pub cwd: PathBuf,
    pub title: Option<String>,
    pub modified: SystemTime,
}

/// A transcript found by enumeration, before anything inside it was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Candidate {
    pub agent: CLIAgent,
    pub path: PathBuf,
    pub session_id: String,
    pub modified: SystemTime,
}

/// The first entries of a transcript, up to whichever budget runs out first.
///
/// A line that does not parse is skipped rather than ending the read: a
/// half-flushed final line, or a leading blank, says nothing about the lines
/// after it. An unreadable file yields nothing, which is how a transcript
/// deleted between enumeration and this call is handled.
pub(super) fn read_head(path: &Path) -> Vec<Value> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let mut reader = BufReader::new(file);
    let mut entries = Vec::new();
    let mut consumed = 0usize;
    let mut line = String::new();
    while entries.len() < MAX_HEAD_LINES && consumed < MAX_HEAD_BYTES {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(read) => consumed += read,
        }
        if let Ok(value) = serde_json::from_str::<Value>(line.trim()) {
            entries.push(value);
        }
    }
    entries
}

/// One menu-sized line out of a prompt that may be long and multi-line.
pub(super) fn as_title(text: &str) -> Option<String> {
    let first = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let mut title: String = first.chars().take(TITLE_MAX_CHARS).collect();
    if first.chars().count() > TITLE_MAX_CHARS {
        title.push('…');
    }
    Some(title)
}

/// Every `.jsonl` file directly inside `dir`, with its modification time.
///
/// A directory that cannot be read yields nothing. One unreadable project
/// directory must not cost the user every other session on the machine.
pub(super) fn jsonl_files(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let path = entry.path();
            if path.extension()? != "jsonl" {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((path, modified))
        })
        .collect()
}

/// Every immediate subdirectory of `dir`, unreadable ones skipped.
pub(super) fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_type().ok()?.is_dir().then(|| entry.path()))
        .collect()
}

/// The user's home directory.
///
/// `HOME` wins under `#[cfg(test)]` because `dirs::home_dir` ignores it on
/// Windows, which would make a redirected fixture read the real home. The
/// same reasoning is recorded on `home_dir_for_claude_config`.
pub(super) fn home_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
        return Some(PathBuf::from(home));
    }
    dirs::home_dir()
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
```

- [ ] **Step 4: Declare the module**

In `app/src/terminal/mod.rs`, next to the other `pub mod` lines and in alphabetical position (before `pub mod alt_screen_reporting;`):

```rust
pub mod agent_session_history;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p warp agent_session_history`
Expected: PASS — 10 tests. `mod.rs` declares no submodules yet; Tasks 3–6 each add their own `mod` line alongside the file it creates, so every task in this plan ends on a green build.

- [ ] **Step 6: Commit**

```bash
git add app/src/terminal/agent_session_history/mod.rs app/src/terminal/agent_session_history/mod_tests.rs app/src/terminal/mod.rs
git commit -m "feat(terminal): read the head of an agent transcript without reading the transcript

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_017zUJLduZivuZHisz9e7YeY"
```

---

## Task 3: The Pi scanner

**Files:**
- Create: `app/src/terminal/agent_session_history/pi.rs`
- Create: `app/src/terminal/agent_session_history/pi_tests.rs`
- Modify: `app/src/terminal/agent_session_history/mod.rs` (add `mod pi;`)

**Interfaces:**
- Consumes: `Candidate`, `PastAgentSession`, `as_title`, `jsonl_files`, `subdirs`, `home_dir` from Task 2.
- Produces: `pub(super) fn root() -> Option<PathBuf>`, `pub(super) fn enumerate(root: &Path) -> Vec<Candidate>`, `pub(super) fn describe(candidate: &Candidate, head: &[Value]) -> PastAgentSession`.

- [ ] **Step 1: Write the failing tests**

Create `app/src/terminal/agent_session_history/pi_tests.rs`:

```rust
use std::fs;
use std::time::SystemTime;

use serde_json::json;

use super::*;

const SESSION_LINE: &str = r#"{"type":"session","version":3,"id":"01a080b4-333f-7512-ab66-ee3ac5699fd4","timestamp":"2026-09-08T11:08:08.384Z","cwd":"T:\\projects\\dev-ops\\warp"}"#;

fn candidate() -> Candidate {
    Candidate {
        agent: CLIAgent::Pi,
        path: PathBuf::from("unused.jsonl"),
        session_id: "01a080b4-333f-7512-ab66-ee3ac5699fd4".to_owned(),
        modified: SystemTime::UNIX_EPOCH,
    }
}

#[test]
fn a_session_id_is_what_follows_the_last_underscore() {
    let dir = tempfile::tempdir().expect("temp dir");
    let project = dir.path().join("--T--projects-dev-ops-warp--");
    fs::create_dir(&project).expect("fixture dir");
    fs::write(
        project.join("2026-09-08T11-08-08-384Z_01a080b4-333f-7512-ab66-ee3ac5699fd4.jsonl"),
        format!("{SESSION_LINE}\n"),
    )
    .expect("fixture write");

    let found = enumerate(dir.path());

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].agent, CLIAgent::Pi);
    assert_eq!(found[0].session_id, "01a080b4-333f-7512-ab66-ee3ac5699fd4");
}

#[test]
fn a_filename_with_no_underscore_is_not_a_session() {
    let dir = tempfile::tempdir().expect("temp dir");
    let project = dir.path().join("--slug--");
    fs::create_dir(&project).expect("fixture dir");
    fs::write(project.join("stray.jsonl"), "{}\n").expect("fixture write");

    assert!(enumerate(dir.path()).is_empty());
}

#[test]
fn a_missing_root_yields_no_sessions() {
    let dir = tempfile::tempdir().expect("temp dir");
    assert!(enumerate(&dir.path().join("gone")).is_empty());
}

#[test]
fn the_cwd_comes_from_the_session_record() {
    let head = vec![serde_json::from_str(SESSION_LINE).expect("fixture json")];

    let session = describe(&candidate(), &head);

    assert_eq!(session.cwd, PathBuf::from(r"T:\projects\dev-ops\warp"));
}

#[test]
fn the_title_comes_from_the_first_user_message() {
    let head = vec![
        serde_json::from_str(SESSION_LINE).expect("fixture json"),
        json!({"type": "model_change", "modelId": "gpt-5.6-sol"}),
        json!({
            "type": "message",
            "message": {"role": "user", "content": [{"type": "text", "text": "fix the hit test"}]}
        }),
    ];

    let session = describe(&candidate(), &head);

    assert_eq!(session.title, Some("fix the hit test".to_owned()));
}

#[test]
fn a_head_with_no_session_record_still_describes_a_resumable_session() {
    let session = describe(&candidate(), &[]);

    assert_eq!(session.cwd, PathBuf::new());
    assert_eq!(session.title, None);
    assert_eq!(session.session_id, "01a080b4-333f-7512-ab66-ee3ac5699fd4");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p warp agent_session_history::pi`
Expected: FAIL — `pi.rs` does not exist.

- [ ] **Step 3: Write the scanner**

Create `app/src/terminal/agent_session_history/pi.rs`:

```rust
//! Pi's session store: `~/.pi/agent/sessions/--<slug>--/<timestamp>_<uuid>.jsonl`.
//!
//! Pi opens every transcript with a `session` record carrying the ID, the
//! start time and the working directory, so one line answers everything but
//! the title.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Candidate, PastAgentSession, as_title, jsonl_files, subdirs};
use crate::terminal::cli_agent::CLIAgent;

/// Where Pi keeps transcripts.
///
/// Pi publishes no home override that has been checked against the shipped
/// CLI, so none is guessed at -- a wrong env var reads an empty directory and
/// reports, wrongly, that the user has no sessions.
pub(super) fn root() -> Option<PathBuf> {
    super::home_dir().map(|home| home.join(".pi").join("agent").join("sessions"))
}

/// `2026-09-08T11-08-08-384Z_01a080b4-...` -- the ID follows the last `_`.
///
/// The timestamp ahead of it contains dashes but no underscore, which is what
/// makes the last underscore an unambiguous split point.
fn session_id_from_stem(stem: &str) -> Option<String> {
    let (_, id) = stem.rsplit_once('_')?;
    (!id.is_empty()).then(|| id.to_owned())
}

/// Every transcript under `root`, which holds one directory per project.
pub(super) fn enumerate(root: &Path) -> Vec<Candidate> {
    subdirs(root)
        .into_iter()
        .flat_map(|project| jsonl_files(&project))
        .filter_map(|(path, modified)| {
            let session_id = session_id_from_stem(path.file_stem()?.to_str()?)?;
            Some(Candidate {
                agent: CLIAgent::Pi,
                path,
                session_id,
                modified,
            })
        })
        .collect()
}

/// Fills in what the transcript's head says about a candidate.
pub(super) fn describe(candidate: &Candidate, head: &[Value]) -> PastAgentSession {
    let cwd = head
        .iter()
        .find(|entry| entry["type"] == "session")
        .and_then(|entry| entry["cwd"].as_str())
        .map(PathBuf::from)
        .unwrap_or_default();
    let title = head
        .iter()
        .find(|entry| entry["type"] == "message" && entry["message"]["role"] == "user")
        .and_then(|entry| entry["message"]["content"][0]["text"].as_str())
        .and_then(as_title);

    PastAgentSession {
        agent: candidate.agent,
        session_id: candidate.session_id.clone(),
        cwd,
        title,
        modified: candidate.modified,
    }
}

#[cfg(test)]
#[path = "pi_tests.rs"]
mod tests;
```

- [ ] **Step 4: Declare the module**

At the top of `app/src/terminal/agent_session_history/mod.rs`:

```rust
mod pi;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p warp agent_session_history`
Expected: PASS — 16 tests.

- [ ] **Step 6: Commit**

```bash
git add app/src/terminal/agent_session_history/pi.rs app/src/terminal/agent_session_history/pi_tests.rs app/src/terminal/agent_session_history/mod.rs
git commit -m "feat(terminal): find Pi's closed sessions

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_017zUJLduZivuZHisz9e7YeY"
```

---

## Task 4: The Claude scanner

**Files:**
- Create: `app/src/terminal/agent_session_history/claude.rs`
- Create: `app/src/terminal/agent_session_history/claude_tests.rs`
- Modify: `app/src/terminal/agent_session_history/mod.rs` (add `mod claude;`)

**Interfaces:**
- Consumes: `Candidate`, `PastAgentSession`, `as_title`, `jsonl_files`, `subdirs` from Task 2; `claude_transcript::claude_config_dir()` from `crate::ai::agent_sdk::driver::harness`.
- Produces: `pub(super) fn root() -> Option<PathBuf>`, `pub(super) fn enumerate(root: &Path) -> Vec<Candidate>`, `pub(super) fn describe(candidate: &Candidate, head: &[Value]) -> PastAgentSession`.

- [ ] **Step 1: Write the failing tests**

Create `app/src/terminal/agent_session_history/claude_tests.rs`:

```rust
use std::fs;
use std::time::SystemTime;

use serde_json::json;

use super::*;

fn candidate() -> Candidate {
    Candidate {
        agent: CLIAgent::Claude,
        path: PathBuf::from("unused.jsonl"),
        session_id: "357091b5-91fb-4cf4-b4e5-6448c736830e".to_owned(),
        modified: SystemTime::UNIX_EPOCH,
    }
}

#[test]
fn a_session_id_is_the_file_stem() {
    let dir = tempfile::tempdir().expect("temp dir");
    let project = dir.path().join("T--projects-dev-ops-warp");
    fs::create_dir(&project).expect("fixture dir");
    fs::write(
        project.join("357091b5-91fb-4cf4-b4e5-6448c736830e.jsonl"),
        "{}\n",
    )
    .expect("fixture write");

    let found = enumerate(dir.path());

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].agent, CLIAgent::Claude);
    assert_eq!(found[0].session_id, "357091b5-91fb-4cf4-b4e5-6448c736830e");
}

#[test]
fn a_subagent_directory_is_not_a_session() {
    let dir = tempfile::tempdir().expect("temp dir");
    let project = dir.path().join("T--projects-dev-ops-warp");
    let subagents = project.join("357091b5-91fb-4cf4-b4e5-6448c736830e").join("subagents");
    fs::create_dir_all(&subagents).expect("fixture dir");
    fs::write(subagents.join("agent-aac0b7f3.jsonl"), "{}\n").expect("fixture write");

    assert!(enumerate(dir.path()).is_empty());
}

#[test]
fn the_cwd_comes_from_the_first_entry_that_has_one() {
    let head = vec![
        json!({"type": "last-prompt", "leafUuid": "a5bf3b9c"}),
        json!({"type": "mode", "mode": "normal"}),
        json!({"type": "user", "cwd": r"T:\projects\dev-ops\warp",
               "message": {"role": "user", "content": "how do we stand"}}),
    ];

    let session = describe(&candidate(), &head);

    assert_eq!(session.cwd, PathBuf::from(r"T:\projects\dev-ops\warp"));
    assert_eq!(session.title, Some("how do we stand".to_owned()));
}

#[test]
fn a_content_array_is_read_for_its_first_text_part() {
    let head = vec![json!({
        "type": "user",
        "cwd": "/home/u/p",
        "message": {"role": "user", "content": [
            {"type": "tool_result", "content": "ignored"},
            {"type": "text", "text": "the real prompt"}
        ]}
    })];

    assert_eq!(
        describe(&candidate(), &head).title,
        Some("the real prompt".to_owned())
    );
}

#[test]
fn a_tool_result_only_entry_is_skipped_for_the_title() {
    let head = vec![
        json!({"type": "user", "cwd": "/home/u/p",
               "message": {"role": "user", "content": [{"type": "tool_result", "content": "x"}]}}),
        json!({"type": "user",
               "message": {"role": "user", "content": "the real prompt"}}),
    ];

    assert_eq!(
        describe(&candidate(), &head).title,
        Some("the real prompt".to_owned())
    );
}

#[test]
fn a_transcript_with_only_bookkeeping_is_still_resumable() {
    let head = vec![json!({"type": "bridge-session", "sessionId": "357091b5"})];

    let session = describe(&candidate(), &head);

    assert_eq!(session.cwd, PathBuf::new());
    assert_eq!(session.title, None);
    assert_eq!(session.session_id, "357091b5-91fb-4cf4-b4e5-6448c736830e");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p warp agent_session_history::claude`
Expected: FAIL — `claude.rs` does not exist.

- [ ] **Step 3: Write the scanner**

Create `app/src/terminal/agent_session_history/claude.rs`:

```rust
//! Claude Code's session store: `<config>/projects/<slug>/<uuid>.jsonl`.
//!
//! Unlike the other two, Claude's opening lines are bookkeeping -- a last
//! prompt pointer, a mode, a permission mode -- and neither the working
//! directory nor the first prompt sits at a fixed position. Both are therefore
//! searched for rather than read off line one.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Candidate, PastAgentSession, as_title, jsonl_files, subdirs};
use crate::ai::agent_sdk::driver::harness::claude_transcript::claude_config_dir;
use crate::terminal::cli_agent::CLIAgent;

/// Where Claude keeps transcripts, honouring `$CLAUDE_CONFIG_DIR` through the
/// resolver the transcript-upload path already uses.
pub(super) fn root() -> Option<PathBuf> {
    claude_config_dir().ok().map(|config| config.join("projects"))
}

/// Every transcript under `root`, which holds one directory per project.
///
/// Only the top level of each project directory is read. A session's
/// subagent transcripts live one level deeper, under a directory named for
/// the session, and offering those as sessions of their own would list the
/// same conversation many times over.
pub(super) fn enumerate(root: &Path) -> Vec<Candidate> {
    subdirs(root)
        .into_iter()
        .flat_map(|project| jsonl_files(&project))
        .filter_map(|(path, modified)| {
            let session_id = path.file_stem()?.to_str()?.to_owned();
            (!session_id.is_empty()).then(|| Candidate {
                agent: CLIAgent::Claude,
                path,
                session_id,
                modified,
            })
        })
        .collect()
}

/// Fills in what the transcript's head says about a candidate.
pub(super) fn describe(candidate: &Candidate, head: &[Value]) -> PastAgentSession {
    let cwd = head
        .iter()
        .find_map(|entry| entry["cwd"].as_str())
        .map(PathBuf::from)
        .unwrap_or_default();
    let title = head
        .iter()
        .filter(|entry| entry["message"]["role"] == "user")
        .find_map(|entry| first_text(&entry["message"]["content"]))
        .and_then(as_title);

    PastAgentSession {
        agent: candidate.agent,
        session_id: candidate.session_id.clone(),
        cwd,
        title,
        modified: candidate.modified,
    }
}

/// A user message's text, whether Claude wrote the content as a bare string or
/// as a list of typed parts.
///
/// A turn that is nothing but tool results has no text part, and yields
/// `None` so the search moves on to the next user entry rather than titling a
/// session with a tool's output.
fn first_text(content: &Value) -> Option<&str> {
    if let Some(text) = content.as_str() {
        return Some(text);
    }
    content
        .as_array()?
        .iter()
        .find(|part| part["type"] == "text")
        .and_then(|part| part["text"].as_str())
}

#[cfg(test)]
#[path = "claude_tests.rs"]
mod tests;
```

- [ ] **Step 4: Declare the module**

At the top of `app/src/terminal/agent_session_history/mod.rs`:

```rust
mod claude;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p warp agent_session_history`
Expected: PASS — 22 tests.

- [ ] **Step 6: Commit**

```bash
git add app/src/terminal/agent_session_history/claude.rs app/src/terminal/agent_session_history/claude_tests.rs app/src/terminal/agent_session_history/mod.rs
git commit -m "feat(terminal): find Claude Code's closed sessions

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_017zUJLduZivuZHisz9e7YeY"
```

---

## Task 5: The Codex scanner

**Files:**
- Create: `app/src/terminal/agent_session_history/codex.rs`
- Create: `app/src/terminal/agent_session_history/codex_tests.rs`
- Modify: `app/src/terminal/agent_session_history/mod.rs` (add `mod codex;`)

**Interfaces:**
- Consumes: `Candidate`, `PastAgentSession`, `as_title`, `jsonl_files`, `subdirs`, `MAX_CANDIDATES` from Task 2; `codex_transcript::{codex_sessions_root, parse_session_meta}` from `crate::ai::agent_sdk::driver::harness`.
- Produces: `pub(super) fn root() -> Option<PathBuf>`, `pub(super) fn enumerate(root: &Path) -> Vec<Candidate>`, `pub(super) fn describe(candidate: &Candidate, head: &[Value]) -> PastAgentSession`.

- [ ] **Step 1: Write the failing tests**

Create `app/src/terminal/agent_session_history/codex_tests.rs`:

```rust
use std::fs;
use std::time::SystemTime;

use serde_json::json;

use super::*;

const ROLLOUT: &str =
    "rollout-2026-06-30T11-23-08-019f17d6-eb34-7683-b0bc-833f26bf12ad.jsonl";
const SESSION_ID: &str = "019f17d6-eb34-7683-b0bc-833f26bf12ad";

fn candidate() -> Candidate {
    Candidate {
        agent: CLIAgent::Codex,
        path: PathBuf::from("unused.jsonl"),
        session_id: SESSION_ID.to_owned(),
        modified: SystemTime::UNIX_EPOCH,
    }
}

fn day_dir(root: &std::path::Path) -> PathBuf {
    let day = root.join("2026").join("06").join("30");
    fs::create_dir_all(&day).expect("fixture dir");
    day
}

#[test]
fn a_session_id_is_the_trailing_uuid_past_the_dashed_timestamp() {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(day_dir(dir.path()).join(ROLLOUT), "{}\n").expect("fixture write");

    let found = enumerate(dir.path());

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].agent, CLIAgent::Codex);
    assert_eq!(found[0].session_id, SESSION_ID);
}

#[test]
fn a_file_that_is_not_a_rollout_is_not_a_session() {
    let dir = tempfile::tempdir().expect("temp dir");
    let day = day_dir(dir.path());
    fs::write(day.join("notes.jsonl"), "{}\n").expect("fixture write");
    fs::write(day.join("rollout-nonsense.jsonl"), "{}\n").expect("fixture write");

    assert!(enumerate(dir.path()).is_empty());
}

#[test]
fn a_missing_root_yields_no_sessions() {
    let dir = tempfile::tempdir().expect("temp dir");
    assert!(enumerate(&dir.path().join("gone")).is_empty());
}

#[test]
fn the_cwd_comes_from_the_session_meta_line() {
    let head = vec![json!({
        "timestamp": "2026-06-30T09:30:42.514Z",
        "type": "session_meta",
        "payload": {
            "id": SESSION_ID,
            "timestamp": "2026-06-30T09:23:08.792Z",
            "cwd": r"T:\projects\SOTZ3N\stoz3n-chat-agent",
            "cli_version": "0.142.4"
        }
    })];

    let session = describe(&candidate(), &head);

    assert_eq!(
        session.cwd,
        PathBuf::from(r"T:\projects\SOTZ3N\stoz3n-chat-agent")
    );
}

#[test]
fn the_title_skips_the_injected_instruction_turn() {
    // Codex writes the AGENTS.md block as a `role: user` response item before
    // the person has said anything. Titling from the first `role: user` entry
    // would name every session after the repository's instructions file.
    let head = vec![
        json!({"type": "response_item", "payload": {
            "type": "message", "role": "user",
            "content": [{"type": "input_text", "text": "# AGENTS.md instructions for T:\\repo"}]
        }}),
        json!({"type": "event_msg", "payload": {
            "type": "user_message", "message": "Say OK only.", "images": []
        }}),
    ];

    assert_eq!(
        describe(&candidate(), &head).title,
        Some("Say OK only.".to_owned())
    );
}

#[test]
fn a_head_with_no_meta_still_describes_a_resumable_session() {
    let session = describe(&candidate(), &[]);

    assert_eq!(session.cwd, PathBuf::new());
    assert_eq!(session.title, None);
    assert_eq!(session.session_id, SESSION_ID);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p warp agent_session_history::codex`
Expected: FAIL — `codex.rs` does not exist.

- [ ] **Step 3: Write the scanner**

Create `app/src/terminal/agent_session_history/codex.rs`:

```rust
//! Codex's rollout store: `<sessions>/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`.
//!
//! Two things here are not shared with the other scanners. The date tree lets
//! enumeration walk newest-first and stop early, and the title cannot come
//! from the first `role: user` entry -- Codex writes the repository's
//! AGENTS.md into one of those before the person has typed anything.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Candidate, MAX_CANDIDATES, PastAgentSession, as_title, jsonl_files, subdirs};
use crate::ai::agent_sdk::driver::harness::codex_transcript::{
    codex_sessions_root, parse_session_meta,
};
use crate::terminal::cli_agent::CLIAgent;

/// Where Codex keeps rollouts, honouring `$CODEX_HOME` through the resolver
/// the transcript-upload path already uses.
pub(super) fn root() -> Option<PathBuf> {
    codex_sessions_root().ok()
}

/// The trailing UUID of `rollout-2026-06-30T11-23-08-019f17d6-eb34-...`.
///
/// The timestamp in the middle is dash-separated too, so the ID is taken as
/// the last five groups and then checked by parsing it. A name that does not
/// end in a UUID is not a rollout, whatever it is.
fn session_id_from_stem(stem: &str) -> Option<String> {
    let rest = stem.strip_prefix("rollout-")?;
    let groups: Vec<&str> = rest.split('-').collect();
    if groups.len() < 5 {
        return None;
    }
    let id = groups[groups.len() - 5..].join("-");
    uuid::Uuid::parse_str(&id).ok().map(|_| id)
}

/// Every rollout under `root`, newest day first.
///
/// The walk stops once it holds `MAX_CANDIDATES`, which is what keeps a
/// machine with years of daily directories from being enumerated in full for
/// a menu that shows twenty rows.
pub(super) fn enumerate(root: &Path) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for year in newest_first(subdirs(root)) {
        for month in newest_first(subdirs(&year)) {
            for day in newest_first(subdirs(&month)) {
                for (path, modified) in jsonl_files(&day) {
                    let Some(session_id) = path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .and_then(session_id_from_stem)
                    else {
                        continue;
                    };
                    candidates.push(Candidate {
                        agent: CLIAgent::Codex,
                        path,
                        session_id,
                        modified,
                    });
                }
                if candidates.len() >= MAX_CANDIDATES {
                    return candidates;
                }
            }
        }
    }
    candidates
}

/// Zero-padded date names sort lexically into chronological order, so
/// reversing them walks the newest first.
fn newest_first(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    dirs.sort_unstable();
    dirs.reverse();
    dirs
}

/// Fills in what the rollout's head says about a candidate.
pub(super) fn describe(candidate: &Candidate, head: &[Value]) -> PastAgentSession {
    let cwd = parse_session_meta(head.first())
        .map(|meta| meta.cwd)
        .unwrap_or_default();
    let title = head
        .iter()
        .find(|entry| {
            entry["type"] == "event_msg" && entry["payload"]["type"] == "user_message"
        })
        .and_then(|entry| entry["payload"]["message"].as_str())
        .and_then(as_title);

    PastAgentSession {
        agent: candidate.agent,
        session_id: candidate.session_id.clone(),
        cwd,
        title,
        modified: candidate.modified,
    }
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
```

- [ ] **Step 4: Declare the module**

At the top of `app/src/terminal/agent_session_history/mod.rs`:

```rust
mod codex;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p warp agent_session_history`
Expected: PASS — 28 tests.

- [ ] **Step 6: Commit**

```bash
git add app/src/terminal/agent_session_history/codex.rs app/src/terminal/agent_session_history/codex_tests.rs app/src/terminal/agent_session_history/mod.rs
git commit -m "feat(terminal): find Codex's closed sessions, titled by the prompt not the instructions

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_017zUJLduZivuZHisz9e7YeY"
```

---

## Task 6: Merging, ordering, and the background cache

**Files:**
- Modify: `app/src/terminal/agent_session_history/mod.rs` (add `scan`, `order_for_display`, `roots`)
- Modify: `app/src/terminal/agent_session_history/mod_tests.rs` (ordering tests)
- Create: `app/src/terminal/agent_session_history/model.rs`
- Modify: `app/src/terminal/cli_agent_sessions/mod.rs` (add `live_session_ids`)

**Interfaces:**
- Consumes: each scanner's `root`, `enumerate` and `describe` from Tasks 3–5.
- Produces, for Tasks 7–10:
  - `pub fn scan() -> Vec<PastAgentSession>` — every candidate, described, newest first, uncapped
  - `pub fn order_for_display(sessions: Vec<PastAgentSession>, focused_cwd: Option<&Path>, live: &HashSet<String>) -> Vec<PastAgentSession>`
  - `pub enum ScanState { NeverScanned, Scanning, Scanned }`
  - `pub struct AgentSessionHistoryModel` with `pub fn sessions(&self) -> &[PastAgentSession]`, `pub fn state(&self) -> ScanState`, `pub fn refresh(&mut self, ctx: &mut ModelContext<Self>)`
  - `CLIAgentSessionsModel::live_session_ids(&self) -> HashSet<String>`

- [ ] **Step 1: Write the failing ordering tests**

Append to `app/src/terminal/agent_session_history/mod_tests.rs`:

```rust
use std::collections::HashSet;
use std::time::Duration;

fn session(id: &str, cwd: &str, seconds: u64) -> PastAgentSession {
    PastAgentSession {
        agent: CLIAgent::Claude,
        session_id: id.to_owned(),
        cwd: PathBuf::from(cwd),
        title: None,
        modified: SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
    }
}

#[test]
fn sessions_from_the_focused_directory_lead() {
    let sessions = vec![
        session("elsewhere-new", "/other", 300),
        session("here-old", "/here", 100),
        session("elsewhere-old", "/other", 50),
        session("here-new", "/here", 200),
    ];

    let ordered = order_for_display(sessions, Some(Path::new("/here")), &HashSet::new());

    let ids: Vec<&str> = ordered.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(
        ids,
        ["here-new", "here-old", "elsewhere-new", "elsewhere-old"]
    );
}

#[test]
fn with_no_focused_directory_the_newest_leads() {
    let sessions = vec![session("old", "/a", 100), session("new", "/b", 200)];

    let ordered = order_for_display(sessions, None, &HashSet::new());

    let ids: Vec<&str> = ordered.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(ids, ["new", "old"]);
}

#[test]
fn a_session_already_open_in_a_pane_is_not_offered() {
    let sessions = vec![session("open", "/a", 200), session("closed", "/a", 100)];
    let live = HashSet::from(["open".to_owned()]);

    let ordered = order_for_display(sessions, None, &live);

    let ids: Vec<&str> = ordered.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(ids, ["closed"]);
}

#[test]
fn the_list_is_capped_after_it_is_ordered() {
    let sessions: Vec<PastAgentSession> = (0..MAX_MENU_ROWS + 5)
        .map(|n| session(&format!("s{n}"), "/a", n as u64))
        .collect();

    let ordered = order_for_display(sessions, None, &HashSet::new());

    assert_eq!(ordered.len(), MAX_MENU_ROWS);
    assert_eq!(
        ordered[0].session_id,
        format!("s{}", MAX_MENU_ROWS + 4),
        "the cap must keep the newest, not whichever happened to be first"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p warp agent_session_history`
Expected: FAIL — `order_for_display` is not defined.

- [ ] **Step 3: Add the pipeline to `mod.rs`**

Add these imports at the top of `app/src/terminal/agent_session_history/mod.rs`:

```rust
use std::collections::HashSet;
```

and this, after `home_dir`:

```rust
/// Every agent's transcript root that resolves on this machine.
///
/// A root that does not resolve is left out rather than reported: an agent the
/// user has never installed is not an error worth a log line on every scan.
#[cfg(not(target_family = "wasm"))]
fn roots() -> Vec<(fn(&Path) -> Vec<Candidate>, PathBuf)> {
    [
        (claude::enumerate as fn(&Path) -> Vec<Candidate>, claude::root()),
        (codex::enumerate as fn(&Path) -> Vec<Candidate>, codex::root()),
        (pi::enumerate as fn(&Path) -> Vec<Candidate>, pi::root()),
    ]
    .into_iter()
    .filter_map(|(enumerate, root)| Some((enumerate, root?)))
    .collect()
}

/// The browser build has no transcript directories to walk.
#[cfg(target_family = "wasm")]
fn roots() -> Vec<(fn(&Path) -> Vec<Candidate>, PathBuf)> {
    Vec::new()
}

/// Every closed session the three agents remember, newest first.
///
/// Blocking, and deliberately so: it is called from a background task, never
/// from the UI thread. Enumeration touches no file contents; only the newest
/// `MAX_CANDIDATES` are opened, so the cost does not grow with how much
/// history is on disk.
pub fn scan() -> Vec<PastAgentSession> {
    let mut candidates: Vec<Candidate> = roots()
        .into_iter()
        .flat_map(|(enumerate, root)| enumerate(&root))
        .collect();
    candidates.sort_by(|a, b| b.modified.cmp(&a.modified));
    candidates.truncate(MAX_CANDIDATES);

    candidates
        .into_iter()
        .map(|candidate| {
            let head = read_head(&candidate.path);
            match candidate.agent {
                CLIAgent::Claude => claude::describe(&candidate, &head),
                CLIAgent::Codex => codex::describe(&candidate, &head),
                _ => pi::describe(&candidate, &head),
            }
        })
        .collect()
}

/// The rows to show, most useful first.
///
/// A session started where the user is standing is the one they mean far more
/// often than a newer one from somewhere else, so those lead; the newest wins
/// within each group. A session a pane is already on is dropped: offering to
/// reopen the conversation on screen promises something that will not happen.
pub fn order_for_display(
    sessions: Vec<PastAgentSession>,
    focused_cwd: Option<&Path>,
    live: &HashSet<String>,
) -> Vec<PastAgentSession> {
    let mut sessions: Vec<PastAgentSession> = sessions
        .into_iter()
        .filter(|session| !live.contains(&session.session_id))
        .collect();
    sessions.sort_by(|a, b| {
        let here = |session: &PastAgentSession| {
            focused_cwd.is_some_and(|cwd| session.cwd == cwd)
        };
        here(b).cmp(&here(a)).then(b.modified.cmp(&a.modified))
    });
    sessions.truncate(MAX_MENU_ROWS);
    sessions
}
```

- [ ] **Step 4: Run the ordering tests to verify they pass**

Run: `cargo test -p warp agent_session_history`
Expected: PASS — 32 tests.

- [ ] **Step 5: Add the live-session accessor**

In `app/src/terminal/cli_agent_sessions/mod.rs`, beside the existing `pub fn session` (around line 382):

```rust
    /// The agent session IDs panes are currently on.
    ///
    /// Read by the session picker, which must not offer to reopen a
    /// conversation the user is already looking at. A session Warp detected
    /// from a command line alone has no ID and contributes nothing.
    pub fn live_session_ids(&self) -> std::collections::HashSet<String> {
        self.sessions
            .values()
            .filter_map(|session| session.session_context.session_id.clone())
            .collect()
    }
```

`CLIAgentSessionContext::session_id` is `Option<String>` (`app/src/terminal/cli_agent_sessions/mod.rs:61`), which is why this filters rather than maps.

- [ ] **Step 6: Write the model**

Create `app/src/terminal/agent_session_history/model.rs`:

```rust
//! The cached result of the last scan.
//!
//! Scanning opens up to `MAX_CANDIDATES` files, so it does not belong on the
//! UI thread. The existing supervisor listing does block there and justifies
//! it as one round trip; that reasoning does not stretch to two hundred file
//! reads. The picker therefore renders whatever this holds and refreshes
//! behind itself.

use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

use super::{PastAgentSession, scan};

/// Whether a scan has ever finished, so the picker can say "looking" rather
/// than "none found" before the first one lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanState {
    NeverScanned,
    Scanning,
    Scanned,
}

#[derive(Debug)]
pub struct AgentSessionHistoryModel {
    sessions: Vec<PastAgentSession>,
    state: ScanState,
}

impl Default for AgentSessionHistoryModel {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            state: ScanState::NeverScanned,
        }
    }
}

impl Entity for AgentSessionHistoryModel {
    type Event = ();
}

impl SingletonEntity for AgentSessionHistoryModel {}

impl AgentSessionHistoryModel {
    /// What the last finished scan found, newest first and uncapped. Callers
    /// order and cap it for their own surface.
    pub fn sessions(&self) -> &[PastAgentSession] {
        &self.sessions
    }

    pub fn state(&self) -> ScanState {
        self.state
    }

    /// Starts a scan unless one is already running.
    ///
    /// Results replace the previous list wholesale. A session deleted from
    /// disk between scans should stop being offered, and merging would keep
    /// it forever.
    pub fn refresh(&mut self, ctx: &mut ModelContext<Self>) {
        if self.state == ScanState::Scanning {
            return;
        }
        self.state = ScanState::Scanning;

        let (sender, receiver) = futures::channel::oneshot::channel();
        ctx.background_executor()
            .spawn(async move {
                // The receiver is dropped if the model goes away first, which
                // makes this send fail. There is nobody left to tell.
                let _ = sender.send(scan());
            })
            .detach();

        ctx.spawn(receiver, |me, found, ctx| {
            me.sessions = found.unwrap_or_default();
            me.state = ScanState::Scanned;
            ctx.notify();
        });
    }

    /// Starts a scan on the shared model from anywhere holding an app context.
    pub fn refresh_shared(ctx: &mut AppContext) {
        Self::handle(ctx).update(ctx, |me, ctx| me.refresh(ctx));
    }
}
```

- [ ] **Step 7: Declare the module and check it compiles**

At the top of `app/src/terminal/agent_session_history/mod.rs`:

```rust
pub mod model;
```

Run: `cargo check -p warp`
Expected: clean. If `Self::handle` or `SingletonEntity` needs a different import or an `init` registration, follow the pattern in `app/src/ai/active_agent_views_model.rs:99` verbatim.

- [ ] **Step 8: Run the whole module's tests**

Run: `cargo test -p warp agent_session_history`
Expected: PASS — 32 tests.

- [ ] **Step 9: Commit**

```bash
git add app/src/terminal/agent_session_history/mod.rs app/src/terminal/agent_session_history/mod_tests.rs app/src/terminal/agent_session_history/model.rs app/src/terminal/cli_agent_sessions/mod.rs
git commit -m "feat(terminal): cache the agent session history off the UI thread

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_017zUJLduZivuZHisz9e7YeY"
```

---

## Task 7: Resuming a past session in a new tab

**Files:**
- Modify: `app/src/workspace/action.rs:295` (new variant), `:1066` (the save-requiring arm)
- Modify: `app/src/workspace/view.rs` (dispatch arm near `:25224`, and the handler near `:13778`)

**Interfaces:**
- Consumes: `PastAgentSession` from Task 2; `CliAgentResumeDescriptor::resume_command` from `app/src/app_state.rs:296`.
- Produces: `WorkspaceAction::ResumeAgentSession { agent: CLIAgent, session_id: String, cwd: PathBuf }` for Tasks 8 and 10.

- [ ] **Step 1: Add the action variant**

In `app/src/workspace/action.rs`, immediately after `AttachDetachedSession`:

```rust
    /// Opens a new tab in `cwd` and runs the command that reopens a closed
    /// agent conversation. Unlike `AttachDetachedSession` there is no process
    /// to take over: the agent starts again and replays the transcript.
    ResumeAgentSession {
        agent: crate::terminal::cli_agent::CLIAgent,
        session_id: String,
        cwd: std::path::PathBuf,
    },
```

In the same file, add it to the arm that returns `true` around line 1066, beside `AttachDetachedSession { .. }`:

```rust
            | AttachDetachedSession { .. }
            | ResumeAgentSession { .. }
```

- [ ] **Step 2: Run the build to verify it fails**

Run: `cargo check -p warp`
Expected: FAIL — a non-exhaustive match in `app/src/workspace/view.rs`'s action dispatch.

- [ ] **Step 3: Handle the action**

In `app/src/workspace/view.rs`, beside the `AttachDetachedSession` dispatch arm (around line 25224):

```rust
            ResumeAgentSession {
                agent,
                session_id,
                cwd,
            } => self.resume_agent_session(*agent, session_id.clone(), cwd.clone(), ctx),
```

And add the handler immediately after `attach_detached_session`:

```rust
    /// Opens a tab in the session's directory and queues the agent's own
    /// resume command in it.
    ///
    /// The command is queued rather than run: the shell has not bootstrapped
    /// when the tab appears, and `set_pending_command_queue` is what the
    /// process-handoff path already uses to wait for it. The directory
    /// matters as much as the ID -- Claude resolves a session against the
    /// project directory it was started in.
    fn resume_agent_session(
        &mut self,
        agent: CLIAgent,
        session_id: String,
        cwd: PathBuf,
        ctx: &mut ViewContext<Self>,
    ) {
        let descriptor = CliAgentResumeDescriptor {
            agent,
            session_id: Some(session_id),
            cwd: cwd.clone(),
            title: None,
            captured_unix_seconds: None,
        };
        let Some(command) = descriptor.resume_command() else {
            report_error!(format!(
                "{} cannot reopen a session by id, so there is nothing to resume",
                agent.display_name()
            ));
            return;
        };

        self.add_tab_with_pane_layout(
            PanesLayout::SingleTerminal(Box::new(NewTerminalOptions {
                initial_directory: Some(cwd),
                ..Default::default()
            })),
            Arc::new(HashMap::new()),
            None, /*custom_tab_title*/
            ctx,
        );

        let Some(view) = self
            .tabs
            .last()
            .and_then(|tab| {
                let pane_group = tab.pane_group.as_ref(ctx);
                let pane_id = pane_group.pane_ids().next()?;
                pane_group.terminal_view_from_pane_id(pane_id, ctx)
            })
        else {
            report_error!("The tab opened to resume an agent session has no terminal");
            return;
        };
        view.update(ctx, |view, ctx| {
            view.set_pending_command_queue(vec![command], ctx);
        });
    }
```

Add `use crate::app_state::CliAgentResumeDescriptor;` and `use crate::terminal::cli_agent::CLIAgent;` to `view.rs`'s imports if they are not already present.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p warp`
Expected: clean.

- [ ] **Step 5: Verify the command text is what the agents accept**

Run: `cargo test -p warp app_state`
Expected: PASS — `app/src/app_state_tests.rs:21` already asserts `pi --session session-1`; this task reuses `resume_command` rather than reimplementing it, so that test covers the strings.

- [ ] **Step 6: Commit**

```bash
git add app/src/workspace/action.rs app/src/workspace/view.rs
git commit -m "feat(workspace): reopen a closed agent conversation in a new tab

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_017zUJLduZivuZHisz9e7YeY"
```

---

## Task 8: The two-section picker

**Files:**
- Modify: `app/src/workspace/view.rs:2309-2332` (modal title), `:13726-13774` (`toggle_attach_session_menu`)

**Interfaces:**
- Consumes: `detached_sessions::list` (existing), `AgentSessionHistoryModel`, `order_for_display`, `ScanState` from Task 6, `WorkspaceAction::ResumeAgentSession` from Task 7, `CLIAgentSessionsModel::live_session_ids` from Task 6.
- Produces: nothing other tasks rely on.

- [ ] **Step 1: Retitle the modal**

In `app/src/workspace/view.rs:2318`, change:

```rust
            Modal::new(Some("Attach detached session".to_string()), menu, ctx).with_modal_style(
```

to:

```rust
            Modal::new(Some("Sessions".to_string()), menu, ctx).with_modal_style(
```

- [ ] **Step 2: Rewrite the picker**

Replace the whole body of `toggle_attach_session_menu` (from `let sessions = detached_sessions::list(ctx);` through `ctx.notify();`) with:

```rust
        // The supervisor list is a blocking round trip and stays one: it is a
        // single IPC call, and a cached answer would offer sessions that have
        // since been reaped. The history list is already in hand -- the model
        // scanned it on a background thread -- and is refreshed behind this
        // opening rather than in front of it.
        // Read before anything borrows `ctx` immutably: `focused_terminal_view`
        // takes `&mut ViewContext`, and the model reads below do not.
        let focused_cwd = self.focused_pane_working_directory(ctx);

        let running = detached_sessions::list(ctx);
        let history = AgentSessionHistoryModel::as_ref(ctx);
        let scan_state = history.state();
        let recent = agent_session_history::order_for_display(
            history.sessions().to_vec(),
            focused_cwd.as_deref(),
            &CLIAgentSessionsModel::as_ref(ctx).live_session_ids(),
        );
        AgentSessionHistoryModel::refresh_shared(ctx);

        let mut items: Vec<MenuItem<WorkspaceAction>> = Vec::new();

        if !running.is_empty() {
            items.push(Self::session_menu_header("Running"));
            items.extend(running.into_iter().map(|session| {
                MenuItemFields::new(session.label)
                    .with_on_select_action(WorkspaceAction::AttachDetachedSession {
                        session_id: session.session_id,
                    })
                    .with_icon(icons::Icon::Terminal)
                    .into_item()
            }));
        }

        if !recent.is_empty() {
            if !items.is_empty() {
                items.push(MenuItem::Separator);
            }
            items.push(Self::session_menu_header("Recent"));
            items.extend(recent.into_iter().map(|session| {
                let mut fields = MenuItemFields::new(Self::past_session_label(&session));
                if let Some(icon) = session.agent.icon() {
                    fields = fields.with_icon(icon);
                }
                fields
                    .with_on_select_action(WorkspaceAction::ResumeAgentSession {
                        agent: session.agent,
                        session_id: session.session_id,
                        cwd: session.cwd,
                    })
                    .into_item()
            }));
        }

        // An empty row rather than a toast. A toast dismisses the surface the
        // user just opened and leaves them exactly where they started, which
        // is the dead end this picker exists to end.
        if items.is_empty() {
            items.push(
                MenuItemFields::new(match scan_state {
                    ScanState::Scanned => {
                        "No sessions found in ~/.claude, ~/.codex or ~/.pi".to_owned()
                    }
                    ScanState::NeverScanned | ScanState::Scanning => {
                        "Looking for sessions…".to_owned()
                    }
                })
                .with_disabled(true)
                .into_item(),
            );
        }

        self.attach_session_menu_modal
            .view
            .update(ctx, |modal, ctx| {
                modal.body().update(ctx, |menu, ctx| {
                    menu.set_items(items, ctx);
                });
            });
        self.attach_session_menu_modal.open();
        ctx.notify();
```

- [ ] **Step 3: Add the two helpers**

Immediately after `close_attach_session_menu`:

```rust
    /// A non-clickable section label inside the session picker.
    fn session_menu_header(label: &str) -> MenuItem<WorkspaceAction> {
        MenuItem::Header {
            fields: MenuItemFields::new(label.to_owned()).with_no_interaction_on_hover(),
            clickable: false,
            right_side_fields: None,
        }
    }

    /// What one closed agent session reads as in the picker.
    ///
    /// The title leads because it is the only part that distinguishes two
    /// sessions in the same directory; a session whose head never yielded one
    /// falls back to the agent's name so the row is never blank.
    fn past_session_label(session: &agent_session_history::PastAgentSession) -> String {
        let what = session
            .title
            .clone()
            .unwrap_or_else(|| session.agent.display_name().to_owned());
        let directory = session.cwd.to_string_lossy();
        if directory.is_empty() {
            what
        } else {
            format!("{what} — {directory}")
        }
    }

    /// The directory the focused pane is sitting in, if it is a local shell.
    ///
    /// `&mut ViewContext` because `focused_terminal_view` wants one
    /// (`app/src/workspace/view.rs:12971`), which is why callers resolve this
    /// before taking any immutable borrow of the context.
    fn focused_pane_working_directory(&self, ctx: &mut ViewContext<Self>) -> Option<PathBuf> {
        let view = self.focused_terminal_view(ctx)?;
        view.as_ref(ctx).pwd_if_local(ctx)
    }
```

Add to `view.rs`'s imports:

```rust
use crate::terminal::agent_session_history::{self, model::{AgentSessionHistoryModel, ScanState}};
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p warp`
Expected: clean.

- [ ] **Step 5: Confirm the existing picker test still passes**

Run: `cargo test -p warp detached_sessions`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add app/src/workspace/view.rs
git commit -m "feat(workspace): show running and recent sessions in one picker

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_017zUJLduZivuZHisz9e7YeY"
```

---

## Task 9: Ways in that do not depend on the supervisor

**Files:**
- Modify: `app/src/workspace/mod.rs:1017-1024` (keystroke, dropped gate)
- Modify: `app/src/tab.rs:663-672` (ungate and relabel)
- Modify: `app/src/workspace/view.rs:7241-7250` (new-tab dropdown row)
- Test: `app/src/util/bindings_tests.rs`

**Interfaces:**
- Consumes: `WorkspaceAction::ToggleAttachSessionMenu` (existing).
- Produces: nothing other tasks rely on.

- [ ] **Step 1: Write the failing keybinding test**

Append to `app/src/util/bindings_tests.rs`:

```rust
/// The session picker has a keystroke, and it is the one that was checked.
///
/// `cmd-shift-R` and `ctrl-shift-R` were the obvious mnemonic and are both
/// already taken -- by resume-conversation and by workflows -- which is what
/// this test exists to keep someone from rediscovering. The keymap offers
/// lookup by name (`Keymap::get_binding_by_name`) but no enumeration, so this
/// pins the chosen keystroke rather than proving the absence of a clash.
#[test]
fn test_session_picker_has_its_keybinding() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            crate::workspace::init(ctx);

            let displayed = keybinding_name_to_display_string(
                "workspace:attach_detached_session",
                ctx,
            );
            assert!(
                displayed.is_some(),
                "the session picker must keep a default keystroke; without one the only ways \
                 in are a right-click submenu and a palette entry whose name has to be guessed"
            );
            assert!(
                displayed.as_deref().is_some_and(|shown| shown.ends_with('L')),
                "expected the picker to be bound to shift-L, got {displayed:?}"
            );
        });
    });
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p warp test_session_picker_has_its_keybinding`
Expected: FAIL — the binding is registered with `Trigger::Empty`, so the display string is `None`.

- [ ] **Step 3: Add the keystroke and drop the flag gate**

In `app/src/workspace/mod.rs`, replace the `EditableBinding` for `workspace:attach_detached_session`:

```rust
        EditableBinding::new(
            "workspace:attach_detached_session",
            "Attach detached session",
            WorkspaceAction::ToggleAttachSessionMenu,
        )
        .with_group(bindings::BindingGroup::Terminal.as_str())
        .with_enabled(|| FeatureFlag::PersistentTerminalSessions.is_enabled())
        .with_context_predicate(id!("Workspace")),
```

with:

```rust
        // No flag gate and no keystroke-free registration. The picker's other
        // half reads the agents' own transcript directories, which are there
        // whether or not the supervisor is: gating the way in on the feature
        // that failed is what left a restarted app with nothing to click.
        EditableBinding::new(
            "workspace:attach_detached_session",
            "Attach or resume session",
            WorkspaceAction::ToggleAttachSessionMenu,
        )
        .with_group(bindings::BindingGroup::Terminal.as_str())
        .with_mac_key_binding("cmd-shift-L")
        .with_linux_or_windows_key_binding("ctrl-shift-L")
        .with_context_predicate(id!("Workspace")),
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p warp test_session_picker_has_its_keybinding`
Expected: PASS

- [ ] **Step 5: Ungate and relabel the tab-menu row**

In `app/src/tab.rs`, replace:

```rust
        // Offered whenever the supervisor can be asked, rather than only when
        // it is holding something: finding that out is a round trip, and doing
        // it on every right-click to decide whether to draw a row is not worth
        // it. The picker says so when there is nothing to attach.
        if detached_sessions::are_supported(ctx) {
            items.push(
                MenuItemFields::new("Attach detached session")
                    .with_on_select_action(WorkspaceAction::ToggleAttachSessionMenu)
                    .into_item(),
            );
        }
```

with:

```rust
        // Always drawn. It used to be conditional on the supervisor being
        // reachable, which hid the row in the one situation where a user is
        // hunting for it -- and the picker's recent section does not need a
        // supervisor at all. The picker says what it found.
        items.push(
            MenuItemFields::new("Attach or resume session…")
                .with_on_select_action(WorkspaceAction::ToggleAttachSessionMenu)
                .into_item(),
        );
```

Remove the now-unused `detached_sessions` import from `app/src/tab.rs` if nothing else in the file uses it.

- [ ] **Step 6: Add the new-tab dropdown row**

In `app/src/workspace/view.rs`, in `unified_new_session_menu_items`, immediately before the final `menu_items` return and after the "Reopen closed session" push:

```rust
        menu_items.push(
            MenuItemFields::new("Resume session…")
                .with_on_select_action(WorkspaceAction::ToggleAttachSessionMenu)
                .with_icon(icons::Icon::Terminal)
                .with_key_shortcut_label(
                    keybinding_name_to_display_string("workspace:attach_detached_session", ctx)
                        .unwrap_or_default(),
                )
                .into_item(),
        );
```

If `keybinding_name_to_display_string` is not already imported in `view.rs`, import it from wherever `get_started_view.rs` gets it.

- [ ] **Step 7: Verify it compiles and the suite is green**

Run: `cargo check -p warp && cargo test -p warp bindings`
Expected: clean, PASS.

- [ ] **Step 8: Commit**

```bash
git add app/src/workspace/mod.rs app/src/tab.rs app/src/workspace/view.rs app/src/util/bindings_tests.rs
git commit -m "fix(workspace): stop hiding the session picker behind the supervisor

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_017zUJLduZivuZHisz9e7YeY"
```

---

## Task 10: Recent sessions on the get-started pane

**Files:**
- Modify: `app/src/pane_group/pane/get_started_view.rs:187-292` (`render_main_content`), `:30-50` (`init`)

**Interfaces:**
- Consumes: `AgentSessionHistoryModel`, `order_for_display`, `PastAgentSession` from Task 6; `WorkspaceAction::ResumeAgentSession` from Task 7.
- Produces: nothing other tasks rely on.

**Background:** This is the surface a cold-started window actually shows, and it is where the user asked "how do I open a session?". The pane is a hand-built `Flex::column`; the new rows follow the existing "New session in ~" button's construction exactly.

- [ ] **Step 1: Start a scan when the pane appears**

In `app/src/pane_group/pane/get_started_view.rs`, at the end of `GetStartedView::new` (before it returns), add:

```rust
        // Kick a scan so the rows below are populated by the time the user has
        // read the heading. A window opened with no supervisor still gets a
        // list; that is the point of this surface.
        AgentSessionHistoryModel::refresh_shared(ctx);
```

Give the struct a place to keep the rows' hover state. A handle created during
`render` is a fresh one every frame, which loses the hover the moment the pointer
moves, so they are made once here and indexed by row. Add the field to
`pub struct GetStartedView`:

```rust
    /// One per recent-session row, so hover survives a repaint.
    recent_session_mouse_states: Vec<MouseStateHandle>,
```

and initialise it in the same constructor:

```rust
            recent_session_mouse_states: (0..GET_STARTED_ROWS)
                .map(|_| MouseStateHandle::new())
                .collect(),
```

- [ ] **Step 2: Render the rows**

In `render_main_content`, after the existing terminal-session button and inside the same `with_children([...])` array, add a trailing element built by a new method. Change the array's last entry to be followed by:

```rust
                self.render_recent_sessions(app),
```

and add the method to the `impl GetStartedView` block:

```rust
    /// The newest few closed agent sessions, each reopening on click.
    ///
    /// Nothing is drawn when there are none: an empty heading on the welcome
    /// screen tells a new user that something is missing when nothing is.
    fn render_recent_sessions(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        let history = AgentSessionHistoryModel::as_ref(app);
        let recent = agent_session_history::order_for_display(
            history.sessions().to_vec(),
            None, /*focused_cwd -- this pane has no shell of its own*/
            &CLIAgentSessionsModel::as_ref(app).live_session_ids(),
        );
        let recent: Vec<PastAgentSession> = recent.into_iter().take(GET_STARTED_ROWS).collect();
        if recent.is_empty() {
            return Empty::new().finish();
        }

        let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Center);
        column.add_child(
            Container::new(
                appearance
                    .ui_builder()
                    .paragraph("Recent sessions")
                    .with_style(UiComponentStyles {
                        font_size: Some(12.),
                        font_color: Some(
                            theme.disabled_text_color(theme.background()).into_solid(),
                        ),
                        ..Default::default()
                    })
                    .build()
                    .finish(),
            )
            .with_margin_top(20.)
            .with_margin_bottom(6.)
            .finish(),
        );

        for (row, session) in recent.into_iter().enumerate() {
            let label = match &session.title {
                Some(title) => title.clone(),
                None => session.agent.display_name().to_owned(),
            };
            let action = WorkspaceAction::ResumeAgentSession {
                agent: session.agent,
                session_id: session.session_id.clone(),
                cwd: session.cwd.clone(),
            };
            let icon = session
                .agent
                .icon()
                .unwrap_or(icons::Icon::Terminal)
                .to_warpui_icon(theme.foreground());

            column.add_child(
                appearance
                    .ui_builder()
                    .button(
                        ButtonVariant::Text,
                        self.recent_session_mouse_states[row].clone(),
                    )
                    .with_style(UiComponentStyles {
                        padding: Some(Coords::uniform(8.)),
                        ..Default::default()
                    })
                    .with_hovered_styles(UiComponentStyles {
                        border_radius: Some(CornerRadius::with_all(Radius::Pixels(4.))),
                        background: Some(
                            theme.background().blend(&theme.surface_overlay_1()).into(),
                        ),
                        ..Default::default()
                    })
                    .with_text_and_icon_label(TextAndIcon::new(
                        TextAndIconAlignment::IconFirst,
                        format!("  {label}"),
                        icon,
                        MainAxisSize::Min,
                        MainAxisAlignment::Center,
                        vec2f(16., 16.),
                    ))
                    .build()
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(action.clone());
                    })
                    .with_cursor(Cursor::PointingHand)
                    .finish(),
            );
        }

        column.finish()
    }
```

Add to the file's imports:

```rust
use crate::terminal::agent_session_history::{self, model::AgentSessionHistoryModel, PastAgentSession};
use crate::terminal::cli_agent_sessions::CLIAgentSessionsModel;
use crate::workspace::WorkspaceAction;
```

and, at module level:

```rust
/// How many recent sessions the welcome screen offers before it becomes a list.
const GET_STARTED_ROWS: usize = 3;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p warp`
Expected: clean.

- [ ] **Step 4: Run the app and confirm by eye**

Run the app, open a window whose active tab is the get-started pane, and confirm: a "Recent sessions" heading with up to three rows, each carrying its agent's icon, and clicking one opens a tab that runs the resume command. Then press `ctrl-shift-L` and confirm the picker opens with a "Recent" section.

- [ ] **Step 5: Run the whole suite**

Run: `cargo test -p warp agent_session_history && cargo test -p warp bindings && cargo check -p warp`
Expected: PASS, PASS, clean.

- [ ] **Step 6: Commit**

```bash
git add app/src/pane_group/pane/get_started_view.rs
git commit -m "feat(get-started): offer recent agent sessions on the welcome screen

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_017zUJLduZivuZHisz9e7YeY"
```

---

## Spec coverage

| Spec section | Task |
|---|---|
| Part 1 — Detach keeps the window | 1 |
| Part 2 — the three layouts, roots reused | 3, 4, 5 |
| Part 2 — two-stage scanning, the 200 cap | 2, 5, 6 |
| Part 2 — slug never decoded, `cwd` from the file | 3, 4, 5 |
| Part 2 — background scan, `AgentSessionHistoryModel` | 6 |
| Part 3 — two sections, ordering, icons, live filter, empty row | 6, 8 |
| Part 3 — resuming | 7 |
| Part 4 — keystroke, palette gate, tab row, dropdown | 9 |
| Part 4 — get-started rows | 10 |
| Errors — per-entry skip, never empty the list | 2, 3, 4, 5 |
| Testing — per-scanner, ordering, integration, bindings | 1–6, 9 |

**Deviation from the spec, deliberate:** the spec proposed redirecting `HOME` in fixtures. Scanner functions take their root as a parameter instead, so tests pass a `TempDir` directly and never touch process-wide environment state — which also keeps them safe to run in parallel. `home_dir()` keeps the `#[cfg(test)]` `HOME` behaviour for `root()` alone.
