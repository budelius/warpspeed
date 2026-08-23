# Launch configs lose the per-pane shell

**Status:** design
**Date:** 2026-08-17
**Scope:** launch configurations only. Session restore already works and stays untouched.

## Problem

Save a launch configuration from a session containing WSL panes, reopen it, and every pane
comes back as the default shell. The WSL working directory is lost too.

This matters because launch configurations are not merely a convenience. When session restore
fails on large Windows sessions, saving a numbered launch configuration is the only way to
recover a layout. A recovery mechanism that silently changes every pane's shell does not recover
the session.

## Evidence

The schema already carries the field. `app/src/launch_configs/launch_config.rs`:

```rust
/// Optional shell override for this pane (e.g. `"pwsh"`, `"zsh"`).
/// Sourced from the `shell` field of a tab config pane node.
shell: Option<String>,
```

The open path already honours it. `app/src/pane_group/mod.rs:1405`:

```rust
let chosen_shell: Option<AvailableShell> = shell
    .as_deref()
    .and_then(|name| resolve_tab_config_shell(name, ctx));
```

Session restore populates the equivalent value correctly. `app/src/pane_group/mod.rs:1666`:

```rust
let chosen_shell = terminal_snapshot
    .shell_launch_data
    .as_ref()
    .and_then(|shell| { /* ... */ });
```

Only the save path drops it. `app/src/launch_configs/launch_config.rs:146`, inside
`TryFrom<PaneNodeSnapshot> for PaneTemplateType`:

```rust
LeafContents::Terminal(terminal) => Ok(Self::PaneTemplate {
    cwd: PathBuf::from(terminal.cwd.unwrap_or_default()),
    commands: Vec::new(),
    is_focused: Some(leaf.is_focused),
    pane_mode: PaneMode::Terminal,
    shell: None,          // <-- shell_launch_data is available here and ignored
}),
```

## Two defects

**Defect 1 — the save path discards the shell.** `shell: None` is hardcoded. The same snapshot
carries `shell_launch_data`, which session restore reads. Writing it costs one mapping function.

**Defect 2 — WSL working directories are discarded.** `app/src/pane_group/mod.rs:1421` filters
the template `cwd`:

```rust
Some(cwd).filter(|p| p.exists()),
```

A WSL pane's `cwd` is a Linux path such as `/home/you/project`. It never satisfies `exists()` on
Windows, so the pane opens in the default directory. Upstream acknowledges this one line above:

```rust
// TODO(CORE-3187): On Windows, support WSL directory restoration.
```

## Design

### Encoding the shell

`ShellLaunchData` (`crates/warp_terminal/src/shell/mod.rs:783`) has four variants. Map each to the
`Option<String>` the template stores:

| Variant | Stored string | Resolves via |
|---|---|---|
| `Executable { executable_path, .. }` | the executable path | existing path branch |
| `MSYS2 { executable_path, .. }` | the executable path | existing path branch |
| `WSL { distro }` | `wsl:<distro>` | new prefix branch |
| `DockerSandbox { .. }` | `None` | not restored |

`DockerSandbox` stays unrestored on purpose. The variant exists so callers cannot execute `sbx` as
if it were a shell, and reviving a container from a layout file would surprise the user.

The `wsl:` prefix is new because `resolve_tab_config_shell` cannot express a distro today. It
resolves by path or by command name:

```rust
fn resolve_tab_config_shell(name: &str, ctx: &AppContext) -> Option<AvailableShell> {
    if name.contains(std::path::MAIN_SEPARATOR) {
        return AvailableShell::try_from(name).ok();
    }
    if let Some(matched) = AvailableShells::as_ref(ctx).find_by_command_name(name) {
        return Some(matched);
    }
    AvailableShell::try_from(name).ok()
}
```

A distro name such as `Ubuntu` matches neither branch. Adding a `wsl:` branch keeps existing
strings working and makes the new form unambiguous.

### Restoring the working directory

Resolve the shell before deciding what to do with `cwd`. When the resolved shell is WSL, pass the
path through unfiltered; the path is meaningful inside the distro even though Windows cannot stat
it. Otherwise keep the current `exists()` filter, which correctly guards against directories
deleted since the configuration was saved.

This ordering is the only structural change at the call site: today `cwd` and `chosen_shell` are
computed independently.

### Feature gating

Session restore gates shell selection behind `FeatureFlag::ShellSelector`. Match that on the
resolve side so launch configurations and restore behave alike. Do not gate the save side: writing
a `shell` field that nothing reads is harmless, and an ungated save means configurations saved
today still carry their shell when the flag later turns on.

## Files changed

| File | Change |
|---|---|
| `app/src/launch_configs/launch_config.rs` | map `shell_launch_data` into `shell` instead of `None` |
| `app/src/pane_group/mod.rs` | teach `resolve_tab_config_shell` the `wsl:` prefix; resolve shell before filtering `cwd` |
| `app/src/launch_configs/launch_config_tests.rs` | round-trip tests |

## Testing

Round-trip tests in `launch_config_tests.rs`, one per `ShellLaunchData` variant:

- a WSL pane snapshot produces `shell: Some("wsl:Ubuntu")`
- an executable pane produces its path
- a Docker sandbox pane produces `None`
- `resolve_tab_config_shell("wsl:Ubuntu")` returns the matching WSL shell
- a plain command name such as `pwsh` still resolves, proving the new branch broke nothing

Working-directory tests in `pane_group/mod_tests.rs`:

- a WSL template keeps a Linux `cwd` that does not exist on Windows
- a non-WSL template still drops a `cwd` that no longer exists

Manual check: open a WSL pane, save a launch configuration, reopen it, and confirm both the distro
and the directory return.

## Out of scope

Lazy shell startup, covered by its own spec. Session restore, which already works. Making restore
itself more reliable on large Windows sessions, which is upstream issue #12803.
