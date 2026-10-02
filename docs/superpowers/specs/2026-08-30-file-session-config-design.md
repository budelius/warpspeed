# File-backed session configs

**Status:** design
**Date:** 2026-08-30
**Scope:** one persistent Warp window assembled from reusable tab configs, with lazy restore and agent/session continuity.

## Implementation status

Named TOML sessions, reusable tab configs, runtime window snapshots, lazy pane restoration, shell
metadata, and single-process ownership are implemented. Windows session panes now use the terminal
supervisor and preserve their process tree across orderly closure. CLI-agent snapshots carry typed
resume metadata; Claude Code resumes automatically when the original PTY cannot be reattached.

## Problem

Launch configs currently have two conflicting meanings:

1. the YAML file is a declarative template for windows, tabs, panes, and startup directories;
2. `PersistentLaunchConfigSessions` treats a previously saved `WindowSnapshot` as authoritative and opens it instead of the YAML.

That conflict explains the current JustVIBE mismatch. The Warp and WarpSpeed `JustVIBE.yaml` files are byte-identical and contain five tabs, but WarpSpeed has a persisted window snapshot under `state_dir()/launch_config_sessions` containing three tabs. `launch_config_workspace_source` restores that snapshot before considering the YAML.

A launch config should remain a repeatable template. A persistent working session needs a distinct file-backed identity and explicit state semantics.

## Goals

- Define one session in a small, user-editable TOML file.
- Compose the session from existing tab-config TOML files instead of duplicating pane layouts.
- Open the session as one Warp window in one owning process.
- Persist and restore the complete evolving window state: tab order, pane trees, titles, colors, groups, focus, working directories, shells, terminal history, built-in agents, CLI-agent resume metadata, and persistent PTY identities.
- Lazily start or reattach only panes in the active tab. Background tabs render their saved metadata and history without creating shells.
- Keep launch configs declarative: opening a launch config always reflects its YAML.
- Make reset/rebuild behavior explicit rather than depending on file modification times.

## Non-goals

- Encoding live terminal history or agent transcripts in the user-authored TOML.
- Duplicating tab-config pane schemas in the session-config schema.
- Claiming that a Windows child process survives app exit before the Windows persistent-terminal supervisor in `2026-08-28-persistent-terminal-sessions-design.md` ships.
- Merging edits to a seed tab config into an already-running persistent session automatically.

## User-authored format

Session configs live under the authored-config data directory:

```text
session_configurations/*.toml
```

Every `WarpSpeed*` instance uses the default WarpSpeed authored-config root. On Windows this is:

```text
%APPDATA%\warp\WarpSpeed\data\session_configurations
```

This includes elevated `WarpSpeedAdmin` and isolated `WARPSPEED_PROFILE` instances. Other Warp
channels retain their own authored-config directories.

Example `JustVIBE.toml`:

```toml
version = 1
id = "justvibe"
name = "JustVIBE"
generation = 1
active_tab = "battery-cafe"

[[tabs]]
id = "battery-cafe"
config = "JustVIBE/BatteryCafe.toml"

[[tabs]]
id = "hoh"
config = "JustVIBE/HOH.toml"

[[tabs]]
id = "holylist"
config = "JustVIBE/Holylist.toml"

[[tabs]]
id = "wanderer"
config = "JustVIBE/Wanderer.toml"

[[tabs]]
id = "cefa"
config = "JustVIBE/CEFA.toml"
```

`config` is relative to `tab_configs/`. A tab entry may provide parameter values:

```toml
[[tabs]]
id = "api"
config = "shared/service.toml"

[tabs.params]
repo = 'T:\projects\service'
mode = "agent"
```

IDs are stable machine-readable identities. Display names and titles remain in the tab configs.

`generation` controls rebuilding. Normal edits to tab configs do not discard a live session. Incrementing `generation`, or choosing **Reset session from config**, archives the old snapshot and materializes the current tab configs again.

## Two distinct source kinds

Extend the window source identity currently represented by `LaunchConfigWindowSource` to distinguish:

```rust
enum WindowConfigSourceKind {
    LaunchConfig,
    SessionConfig,
}
```

A launch-config source is never eligible for evolving-window restore. A session-config source is.

This removes the ambiguous behavior behind the JustVIBE mismatch:

- `JustVIBE.yaml` always opens the five tabs declared in YAML.
- `JustVIBE.toml` opens its saved evolving state when one exists, otherwise it materializes its referenced tab configs.

The persistent-state key contains the session `id` and `generation`, not the display name or file timestamp.

## Materializing a new session

When no compatible state exists:

1. Load the session-config TOML.
2. Resolve every referenced tab-config path.
3. Apply declared parameters through the existing `render_tab_config` path.
4. Build one `WindowTemplate` containing the rendered tabs.
5. Open it through the existing launch-config template path so the active tab is eager and all background tabs use `ShellStartPolicy::Deferred`.
6. Attach the session source identity to the resulting workspace.

A missing or invalid reference rejects the session as a whole and reports the exact session file and tab ID. Partially opening a session would silently change its identity and ordering.

## Persistent state

The user-authored TOML is the seed and identity. Runtime state remains separate under `state_dir()`:

```text
session_config_sessions/<session-id>/<generation>/window.json
```

The stored payload is versioned and contains:

- `WindowSnapshot`;
- serialized terminal block lists needed before SQLite catches up;
- built-in agent conversation IDs and active conversation state already carried by `TerminalPaneSnapshot`;
- per-pane CLI-agent resume descriptors;
- persistent terminal session IDs;
- the session-config source ID and generation.

Writes use a temporary file, `flush`, `sync_all`, and atomic rename, matching the existing launch-session store.

### Save points

Save the session:

- when its window closes;
- during orderly app termination;
- before window process handoff;
- after structural changes, with a debounce: tab add/remove/reorder, pane split/close/move, title/color/group changes, and directory/shell metadata changes.

The close/termination saves remain the correctness boundary. Debounced saves reduce crash loss but do not replace them.

## Working-directory and shell preservation

A lazily materialized pane has no PTY metadata, so `TerminalView::pwd_if_local` and `shell_launch_data_if_local` return `None`. Saving such a pane currently erases its configured directory and shell.

Generalize `RestoredShellSnapshotFields` into parked-shell snapshot fields and populate it for both:

- panes restored from `TerminalPaneSnapshot`;
- panes materialized from a launch/tab/session template.

The fields contain the exact startup directory and `ShellLaunchData` selected for the parked pane. `TerminalPane::snapshot` uses them while `TerminalManager::has_deferred_shell()` is true and switches to live PTY metadata immediately after the shell starts.

This makes repeated save/restore lossless even when a tab has never been activated.

## Lazy restore

Restoring a saved session uses the existing `NewWorkspaceSource::Restored` path:

- the restored active tab starts or reattaches eagerly;
- every background tab remains parked;
- parked panes render saved titles, directories, block history, and agent state;
- first activation consumes the deferred start exactly once;
- a persistent PTY is attached only when its tab activates;
- startup/resume commands remain queued until the shell prompt is ready.

An attached session must not be replaced with a newly forked shell merely because it was restored lazily. Deferred state therefore needs to represent both `StartFresh` and `AttachPersistent(session_id)`.

## Agent restoration

### Built-in Warp agents

Keep the existing full restoration path:

- `WindowSnapshot::conversation_ids_to_restore` releases only conversations belonging to this session;
- `RestoredAgentConversations` seeds terminal panes;
- active/fullscreen conversation state, input config, model override, and execution profile restore from `TerminalPaneSnapshot`.

### CLI agents

Persist a typed resume descriptor rather than a pre-rendered shell command:

```rust
struct CliAgentResumeDescriptor {
    agent: CLIAgent,
    session_id: String,
    cwd: PathBuf,
}
```

On restore:

1. Prefer attaching the original persistent PTY.
2. If the PTY is unavailable and the agent supports resume, generate the resume command at restore time.
3. Queue it on the parked terminal; it runs only when the tab activates.
4. Surface a pane banner when neither attach nor resume is possible.

For Claude, transcript forking remains a last-resort handoff operation. It must not run on every autosave.

## Process ownership

Only one process may own a session ID and generation at a time.

- Acquire an OS-backed lease under `state_dir()/session_config_sessions/<id>/<generation>/owner.lock` before restoring or materializing.
- Hold the lease for the workspace lifetime, releasing it only after the final close snapshot has been written.
- Transfer the lease when restarting a window in a new process: the old process releases it immediately before the handoff, and the replacement claims it before starting deferred shells.
- If the same process already owns it, focus the existing window.
- If another process owns it, focus that process through local control when available; otherwise show an explicit **Session is open in another WarpSpeed process** error.
- Never open an untracked duplicate and never let two processes race to overwrite the snapshot.

Versioned installations and profile runtime isolation remain independent. Every `WarpSpeed*`
profile reads the same authored launch, session, and tab configs, while each profile retains its
own state root, database, ownership leases, and session snapshots.

## Windows process continuity

The file-backed session feature can restore logical state immediately, but exact survival of a running shell or CLI agent on Windows depends on the supervisor described in `2026-08-28-persistent-terminal-sessions-design.md`.

Until that backend ships:

- built-in agents restore from persisted conversations;
- supported CLI agents resume from descriptors;
- ordinary terminal panes recreate their shells in their saved directories;
- the UI must not claim that the original Windows process survived.

After the Windows supervisor ships, the same session snapshot carries its persistent session IDs and reattaches without a file-format change.

## UI

Session configs appear separately from launch configs in the launch/session palette and new-session menu. Each session entry supports:

- **Open / Focus**;
- **Reset from config**;
- **Open config file**;
- **Open state directory** for diagnostics.

The window indicates that it belongs to a persistent session. Saving a launch config from that window creates a normal YAML template; it does not overwrite the session-config TOML.

## Migration of JustVIBE

1. Split the five tabs in `JustVIBE.yaml` into five tab-config TOML files.
2. Create `session_configurations/JustVIBE.toml` referencing them.
3. Archive the existing three-tab launch-session snapshot.
4. Open the new session config and verify five tabs, pane layouts, titles, colors, and directories.
5. Activate only selected tabs and verify unvisited tabs remain shell-free.
6. Close/reopen WarpSpeed and verify the evolving window state and built-in agents restore.
7. Verify a supported CLI agent either reattaches or resumes when its tab is first activated.
8. Keep `JustVIBE.yaml` temporarily as a static recovery template, then remove it after the session config has proved stable.

## Testing

- Parse valid session TOML and reject duplicate tab IDs, duplicate session IDs, missing references, invalid params, and invalid active-tab IDs.
- Materialize ordered tab configs with title, color, pane tree, commands, shell, and params intact.
- A launch config ignores an old evolving-window snapshot.
- A session config restores its compatible snapshot.
- Incrementing generation rebuilds from referenced tab configs.
- A parked template pane round-trips directory and shell without ever starting.
- Session restore starts only the active tab; activation starts or attaches each background pane once.
- Built-in agent conversations and active conversation state round-trip.
- CLI-agent descriptors prefer PTY attach and fall back to lazy resume.
- A second in-process open focuses the owner; a second process cannot acquire the lease.
- Closing a session releases its lease after persistence, and window process handoff transfers ownership to the replacement process.
- Corrupt state falls back to the session seed without modifying the corrupt file.

## Rollout order

1. Separate launch and session source semantics; stop restoring evolving state for YAML launch configs.
2. Fix parked template CWD/shell snapshots.
3. Add session-config parsing, tab-config composition, and menu/palette loading.
4. Move the existing full-window snapshot store to session IDs/generations.
5. Add ownership leases and reset/open-file actions.
6. Persist CLI-agent resume descriptors.
7. Integrate the cross-platform persistent-terminal supervisor as its Windows backend becomes available.
