//! Installs the Kimi Code hooks that report to Warp through `warp agent-event`.

use std::path::PathBuf;
use std::sync::LazyLock;
use std::{fs, io};

use async_trait::async_trait;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use super::{
    CliAgentPluginManager, PluginInstallError, PluginInstructionStep, PluginInstructions,
    compare_versions,
};
use crate::terminal::agent_session_paths::kimi_home;

// The command's pieces are macros so `MANUAL_HOOK_EXAMPLE` can `concat!` them: one spelling of the
// command serves both the installed hooks and the manual instructions.
macro_rules! hooks_version {
    () => {
        "1.0.0"
    };
}
macro_rules! command_marker {
    () => {
        "agent-event --format kimi-hook"
    };
}
#[cfg(windows)]
macro_rules! agent_event_variable {
    () => {
        "%WARP_AGENT_EVENT_BIN%"
    };
}
#[cfg(not(windows))]
macro_rules! agent_event_variable {
    () => {
        "$WARP_AGENT_EVENT_BIN"
    };
}

/// Bumped whenever the hook entries change. Each hook reports it as its plugin version.
const HOOKS_VERSION: &str = hooks_version!();

const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PermissionRequest",
    "PermissionResult",
    "PostToolUse",
    "Stop",
    "StopFailure",
    "Interrupt",
];

/// What marks a hook entry as Warp's, whichever version wrote it.
const COMMAND_MARKER: &str = command_marker!();

const HOOK_TIMEOUT_SECONDS: i64 = 5;

/// Kimi runs hook commands through `cmd.exe` on Windows and `/bin/sh` elsewhere. The variable is
/// empty outside a Warp pane, so the hook fails there and Kimi carries on.
const AGENT_EVENT_BIN: &str = concat!("\"", agent_event_variable!(), "\"");

/// The same command as [`hook_command`], as a TOML entry for the manual instructions.
const MANUAL_HOOK_EXAMPLE: &str = concat!(
    "[[hooks]]\nevent = \"Stop\"\ncommand = \"\\\"",
    agent_event_variable!(),
    "\\\" ",
    command_marker!(),
    " --hooks-version ",
    hooks_version!(),
    "\"\ntimeout = 5"
);

fn hook_command() -> String {
    format!("{AGENT_EVENT_BIN} {COMMAND_MARKER} --hooks-version {HOOKS_VERSION}")
}

pub(super) struct KimiPluginManager;

#[async_trait]
impl CliAgentPluginManager for KimiPluginManager {
    fn minimum_plugin_version(&self) -> &'static str {
        HOOKS_VERSION
    }

    fn can_auto_install(&self) -> bool {
        true
    }

    fn is_installed(&self) -> bool {
        read_config().is_some_and(|config| installed_hooks_version(&config).is_some())
    }

    fn needs_update(&self) -> bool {
        read_config().is_some_and(|config| hooks_need_update(&config))
    }

    async fn install(&self) -> Result<(), PluginInstallError> {
        write_hooks()
    }

    async fn update(&self) -> Result<(), PluginInstallError> {
        write_hooks()
    }

    fn install_success_message(&self) -> &'static str {
        "Warp hooks installed. Restart Kimi Code to activate."
    }

    fn update_success_message(&self) -> &'static str {
        "Warp hooks updated. Restart Kimi Code to activate."
    }

    fn install_instructions(&self) -> &'static PluginInstructions {
        &INSTRUCTIONS
    }

    fn update_instructions(&self) -> &'static PluginInstructions {
        &INSTRUCTIONS
    }
}

static INSTRUCTIONS: LazyLock<PluginInstructions> = LazyLock::new(|| PluginInstructions {
    title: "Install Warp Hooks for Kimi Code",
    subtitle: "Warp adds hooks to Kimi Code's config.toml that report the session's status to \
               this pane. Your other settings and hooks are kept.",
    steps: &[PluginInstructionStep {
        description: "To add them by hand, put one entry like this per event (SessionStart, \
                      UserPromptSubmit, PermissionRequest, PermissionResult, PostToolUse, Stop, \
                      StopFailure, Interrupt) in ~/.kimi-code/config.toml:",
        command: MANUAL_HOOK_EXAMPLE,
        executable: false,
        link: None,
    }],
    post_install_notes: &["Restart Kimi Code to activate the hooks."],
});

fn config_path() -> io::Result<PathBuf> {
    kimi_home()
        .map(|home| home.join("config.toml"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "could not determine Kimi Code's home directory",
            )
        })
}

fn read_config() -> Option<String> {
    fs::read_to_string(config_path().ok()?).ok()
}

fn write_hooks() -> Result<(), PluginInstallError> {
    let path = config_path()?;
    let existing = match fs::read_to_string(&path) {
        Ok(config) => config,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let updated =
        with_warp_hooks(&existing, &hook_command()).map_err(|message| PluginInstallError {
            log: format!("{}: {message}\n", path.display()),
            message,
        })?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension("toml.warp-tmp");
    fs::write(&temporary, updated)?;
    fs::rename(&temporary, &path)?;
    Ok(())
}

/// `config` with Warp's hook entries replaced by one per tracked event running `command`, and
/// everything else -- other hooks, comments, formatting -- as it was.
fn with_warp_hooks(config: &str, command: &str) -> Result<String, String> {
    let mut document = config
        .parse::<DocumentMut>()
        .map_err(|error| format!("Kimi Code's config.toml is not valid TOML: {error}"))?;
    let hooks = document
        .entry("hooks")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
    let Some(hooks) = hooks.as_array_of_tables_mut() else {
        return Err(
            "Kimi Code's config.toml defines `hooks` other than as [[hooks]] tables".to_owned(),
        );
    };
    hooks.retain(|hook| !is_warp_hook(hook));
    for event in HOOK_EVENTS {
        let mut hook = Table::new();
        hook.insert("event", value(*event));
        hook.insert("command", value(command));
        hook.insert("timeout", value(HOOK_TIMEOUT_SECONDS));
        hooks.push(hook);
    }
    Ok(document.to_string())
}

fn is_warp_hook(hook: &Table) -> bool {
    hook.get("command")
        .and_then(Item::as_str)
        .is_some_and(|command| command.contains(COMMAND_MARKER))
}

fn warp_hooks(document: &DocumentMut) -> Vec<&Table> {
    document
        .get("hooks")
        .and_then(Item::as_array_of_tables)
        .map(|hooks| hooks.iter().filter(|hook| is_warp_hook(hook)).collect())
        .unwrap_or_default()
}

/// The version a Warp hook entry reports; `0.0.0` for one written before versioning.
fn hook_version(hook: &Table) -> &str {
    hook.get("command")
        .and_then(Item::as_str)
        .and_then(|command| {
            command
                .split_whitespace()
                .skip_while(|word| *word != "--hooks-version")
                .nth(1)
        })
        .unwrap_or("0.0.0")
}

/// The oldest version among Warp's hook entries, or `None` without any.
fn installed_hooks_version(config: &str) -> Option<String> {
    let document = config.parse::<DocumentMut>().ok()?;
    warp_hooks(&document)
        .into_iter()
        .map(hook_version)
        .min_by(|left, right| compare_versions(left, right))
        .map(str::to_owned)
}

/// Whether Warp's installed hooks are older than this build's or miss one of its events.
fn hooks_need_update(config: &str) -> bool {
    let Ok(document) = config.parse::<DocumentMut>() else {
        return false;
    };
    let hooks = warp_hooks(&document);
    if hooks.is_empty() {
        return false;
    }
    let outdated = hooks
        .iter()
        .any(|hook| compare_versions(hook_version(hook), HOOKS_VERSION).is_lt());
    let missing = HOOK_EVENTS.iter().any(|event| {
        !hooks
            .iter()
            .any(|hook| hook.get("event").and_then(Item::as_str) == Some(*event))
    });
    outdated || missing
}

#[cfg(test)]
#[path = "kimi_tests.rs"]
mod tests;
