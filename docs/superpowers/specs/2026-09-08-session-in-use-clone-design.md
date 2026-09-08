# Opening a session config that another process owns

**Status:** design
**Date:** 2026-09-08
**Scope:** replace the silent no-op when a session config's lease is held elsewhere with a dialog that names the owner and offers to clone the session or open it from its template; stop session configs from shadowing same-named launch configs in the palette.

## Problem

Opening a session-config-backed launch config in a second Warp process does nothing at all. No
window, no toast, no error. The config still appears in the launch config palette, and selecting it
appears to be ignored.

`open_launch_config` in `app/src/root_view.rs` claims an exclusive lease before opening anything:

```rust
if let Some(source) = session_source.as_ref() {
    match crate::session_configs::claim_session(source) {
        Ok(true) => {}
        Ok(false) => {
            safe_warn!(... "already open in another process");
            return;                     // no window, no user-visible signal
        }
```

`claim_session` (`app/src/session_configs.rs:54`) takes a `try_lock_exclusive` on
`state_dir()/session_config_sessions/<id>/<generation>/owner.lock`. `open_stored_session` has the
same guard and the same silent `return`.

Three things make this worse than a missing error message.

**The lease outlives the window.** `SESSION_LEASES` is a process-wide static `HashMap<(id,
generation), File>`. Nothing ever removes an entry and nothing ever unlocks. Closing the window
does not release the session; only the owning process exiting does. A session is therefore pinned
to whichever process claimed it first, for that process's whole life.

**There is no fallback entry.** `launch_and_session_configs()` is
`launch_configs.iter().chain(&session_configs)` (`app/src/user_config/mod.rs:122`), and the palette
collects it into a `HashMap` keyed by lowercased name
(`app/src/search/command_palette/launch_config/data_source.rs:160`). Session configs come last, so
`OBACHAN.toml` silently overwrites `OBACHAN.yaml`. The unlocked YAML template is not selectable at
all.

**The owner is anonymous.** The lock records nothing about who holds it, so even a correct error
message could not say which window to switch to.

Observed on a machine running ten `warpspeed.exe` processes: the `obachan`, `justvibe`, `lularge`,
`m3rp` and `stozn` leases were all held; `sys` and `tm` were free.

## Goals

- Tell the user that the session is open elsewhere, and in which process.
- Offer a way forward from that dialog rather than only reporting a dead end.
- Let the user clone a busy session into an independent, permanent copy.
- Let the user open the session's template without claiming the lease.
- Make a launch config and a session config that share a name both reachable.

## Non-goals

- Focusing or raising the owning process's window. That needs the `local_control` IPC, which is
  feature-flagged and gated behind Settings > Scripting, and deserves its own design.
- Releasing a lease when its window closes. The lifetime bug is real but orthogonal; fixing it
  would not help a session that is genuinely open elsewhere.
- Forking live operating-system processes, terminal scrollback, or agent conversations into the
  clone. See "What a clone is" below.
- Merging or deduplicating the session and launch config that share a name. Both simply become
  visible.

## Owner identity

`claim_session` stops returning a bare `bool`:

```rust
pub struct SessionOwner {
    pub pid: u32,
    pub claimed_at_unix_seconds: i64,
    pub app_id: String,
}

pub enum SessionClaim {
    Claimed,
    HeldBy(Option<SessionOwner>),
}
```

After taking the lock, the claimer writes `owner.json` beside `owner.lock`. The writer holds the
exclusive lock, so no two processes race on that file.

A failed claim reads `owner.json` back. A stale file cannot mislead: it is only read when the lock
is *held*, which proves the recorded process is still alive. When the file is missing or
unparseable the dialog falls back to "another Warp process".

## The dialog

`app/src/launch_configs/session_in_use_dialog.rs`, following
`tab_configs/remove_confirmation_dialog.rs`, which is the house pattern for a small decision
dialog: `init` registered from `launch_configs/mod.rs`, a `ViewHandle<SessionInUseDialog>` field on
`Workspace`, an `is_session_in_use_dialog_open` flag in `workspace/util.rs`, and a `Dialog` with a
title, a body, and a bottom row of `ActionButton`s.

```text
┌ OBACHAN is already open ──────────────────────────────┐
│ Held by WarpSpeed process 43364, since Sep 04 12:08.  │
│                                                       │
│ Clone copies the layout, working directories and      │
│ shells. Running processes, scrollback and agent       │
│ conversations stay with the original window.          │
│                                                       │
│ [Clone Session]  [Open from Template]      [Cancel]   │
└───────────────────────────────────────────────────────┘
```

`SessionInUseEvent` carries `Clone`, `OpenFromTemplate`, and `Cancel`. Enter clones, escape
cancels.

`open_launch_config` and `open_stored_session` replace their `return` with a call that opens this
dialog on the active workspace. When there is no active workspace — a URI launch into a cold app
with no windows — they fall back to opening from the template. That is a visible window instead of
nothing, which is the point of the change.

## Open from template

`NewWorkspaceSource::FromTemplate { window_template, launch_config_source: None }` — the shape
`launch_config_workspace_source` already builds when `source_is_open`. No lease, no checkpoint
writes, no new palette entry. Closing the window leaves no trace.

## What a clone is

A clone reproduces the **shape** of the session, not its running state. It copies the last
checkpoint the owning instance wrote to disk, which is the freshest cross-process view available.

| Preserved | Not preserved |
|---|---|
| Tab order, titles, colors, groups | Running processes |
| Pane tree and split layout | Terminal scrollback |
| Working directories | Agent conversations |
| Shell selection per pane | CLI-agent resume state |

The right column is not a shortcut. Each of those is owned by the live window: a persistent backing
process has exactly one owner, restoring a window *detaches* its conversations from the previous
holder, and block lists are keyed by pane uuid. Copying any of them would corrupt the original
session.

## Clone implementation

`app/src/launch_configs/clone_session.rs` owns identity selection, the TOML copy, and orchestration.
Step 3 below lives in `session_store.rs` instead, because that module owns the checkpoint format,
the pane snapshot tree, and the catalog.

**1. Pick a free identity.** From `OBACHAN`, try `OBACHAN-2`, `OBACHAN-3`, … until neither
`session_configurations/<stem>.toml` exists nor `<id>` collides with an already-loaded session id.
Yields `id = "obachan-2"`, `name = "OBACHAN (2)"`.

**2. Copy the TOML.** Rewrite only `id`, `name`, and `generation = 1`, using `toml_edit`, which is
already a dependency and edits in place so comments and formatting survive. The `[[tabs]]` entries
point at shared read-only tab configs under `tab_configs/sessions/consolidated/OBACHAN/`, so
nothing else is copied.

**3. Copy the checkpoints.** For each `window-<n>.json` under `<sessions_dir>/obachan/1/`, rewrite
`source` to the clone's source, set `saved_at_unix_seconds` to now, drop `block_lists`, and walk
the `PaneNodeSnapshot` tree sanitizing every `TerminalPaneSnapshot`:

| Field | Clone | Reason |
|---|---|---|
| `cwd`, `shell_launch_data`, `is_read_only`, `input_config`, `llm_model_override`, `active_profile_id` | kept | the point of the clone |
| `uuid` | fresh | keys the block store; must not alias live panes |
| `persistent_session_id` | `None` | a backing process has one owner |
| `conversation_ids_to_restore`, `active_conversation_id` | cleared | restore detaches them from the live window |
| `cli_agent_resume_descriptor` | `None` | resuming one agent session twice concurrently is unsafe |

Write each to `<sessions_dir>/obachan-2/1/window-<n>.json`.

**4. Register in the catalog.** `load_or_rebuild_catalog` trusts `stored-sessions.json` whenever it
parses, so a checkpoint on disk alone stays invisible. `session_store::clone_checkpoints` passes the
new summaries to the existing `update_catalog`. That also bumps the catalog's mtime, which is what
`catalog_revision()` compares, so the palette's `StoredSessionCache` refreshes on its own.

**5. Open it.** Claim the clone's lease — always free, since the id is new — and open through the
normal restore path.

The clone's TOML lands in `session_configurations/`, which the filesystem watcher already turns
into `WarpConfigUpdateEvent::SessionConfigs`, refreshing the palette's config index.

## Palette shadowing

Key `FuzzyLaunchConfigSearcher.configs` and its full-text counterpart by `config.source` — the
`(path, document_index)` pair — falling back to the lowercased name only when `source` is `None`.
Both entries then survive.

`LaunchConfig::render_config_description` already opens its pill row with a hardcoded
`"Configuration"` label that carries no information. Replace it with the source kind, so the two
rows read apart at a glance:

```text
> obachan
  OBACHAN            Session  1 window  6 tabs
  OBACHAN            YAML     1 window  6 tabs
```

## Testing

`clone_session` and the `claim_session` owner round-trip are filesystem functions, unit-tested
against a temporary directory in the style of `session_store_tests.rs`:

- a taken identity advances to `-3`;
- pane sanitization clears all four owned fields and issues fresh uuids;
- the catalog gains exactly the clone's windows and nothing else;
- a second claim of the same id and generation reports `HeldBy` with the recorded owner;
- a claim whose `owner.json` is missing or corrupt reports `HeldBy(None)` rather than failing.

Palette dedup is covered by a `data_source` test asserting that a YAML and a session config sharing
a name both appear.
