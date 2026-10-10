//! Installs `@stozn/dsh-warp`, the Cordis plugin that reports DeepSeek Harness sessions to Warp,
//! into the user's dsh profiles.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::{fs, io};

use async_trait::async_trait;
use serde_json::Value;

use super::{
    CliAgentPluginManager, PluginInstallError, PluginInstructionStep, PluginInstructions,
    compare_versions, run_cli_command_logged,
};
use crate::terminal::agent_session_paths::dsh_home;
use crate::terminal::model::session::LocalCommandExecutor;
use crate::terminal::shell::ShellType;

const PACKAGE: &str = "@stozn/dsh-warp";
/// The release the installer pins (`dsh plugin add @stozn/dsh-warp@<version>`), so a newer publish
/// is never pulled in unreviewed, and the oldest one the footer treats as up to date. Keep in
/// sync with app/resources/dsh-warp/package.json and bump them together.
const MINIMUM_PLUGIN_VERSION: &str = "0.1.0";
const REGISTRY_LINE: &str = "@stozn:registry=https://git-dev.obachan.dev/api/packages/stozn/npm/";
/// The profile `dsh --profile headless` uses, which is how dsh runs in a pane.
const DEFAULT_PROFILE: &str = "headless";
const PNPM_MISSING: &str = "Installing the DeepSeek Harness plugin needs pnpm on PATH. Install it \
                            with `npm install -g pnpm`, then try again.";

pub(super) struct DshPluginManager {
    executor: LocalCommandExecutor,
    path_env_var: Option<String>,
}

impl DshPluginManager {
    pub(super) fn new(
        shell_path: Option<PathBuf>,
        shell_type: Option<ShellType>,
        path_env_var: Option<String>,
    ) -> Self {
        Self {
            executor: LocalCommandExecutor::new(shell_path, shell_type.unwrap_or(ShellType::Bash)),
            path_env_var,
        }
    }

    async fn run_logged(
        &self,
        cli: &str,
        args: &[&str],
        log: &mut String,
    ) -> Result<(), PluginInstallError> {
        let env_vars = self
            .path_env_var
            .as_deref()
            .map(|path| HashMap::from([("PATH".to_owned(), path.to_owned())]));
        run_cli_command_logged(cli, args, &self.executor, env_vars, log).await
    }

    async fn add_to_every_profile(&self) -> Result<(), PluginInstallError> {
        let home = dsh_home().ok_or_else(|| PluginInstallError {
            message: "could not determine DeepSeek Harness's home directory".to_owned(),
            log: String::new(),
        })?;
        let mut log = String::new();
        if let Err(error) = self.run_logged("pnpm", &["--version"], &mut log).await {
            return Err(PluginInstallError {
                message: PNPM_MISSING.to_owned(),
                log: error.log,
            });
        }
        let spec = format!("{PACKAGE}@{MINIMUM_PLUGIN_VERSION}");
        for profile in profiles(&home) {
            ensure_registry(&home.join("profiles").join(&profile))?;
            self.run_logged(
                "dsh",
                &["plugin", "--profile", &profile, "add", &spec],
                &mut log,
            )
            .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl CliAgentPluginManager for DshPluginManager {
    fn minimum_plugin_version(&self) -> &'static str {
        MINIMUM_PLUGIN_VERSION
    }

    fn can_auto_install(&self) -> bool {
        true
    }

    fn is_installed(&self) -> bool {
        !installed_versions().is_empty()
    }

    fn needs_update(&self) -> bool {
        installed_versions()
            .iter()
            .any(|version| compare_versions(version, MINIMUM_PLUGIN_VERSION).is_lt())
    }

    async fn install(&self) -> Result<(), PluginInstallError> {
        self.add_to_every_profile().await
    }

    async fn update(&self) -> Result<(), PluginInstallError> {
        self.add_to_every_profile().await
    }

    fn install_success_message(&self) -> &'static str {
        "Warp plugin installed. Start DeepSeek Harness again to activate."
    }

    fn update_success_message(&self) -> &'static str {
        "Warp plugin updated. Start DeepSeek Harness again to activate."
    }

    fn install_instructions(&self) -> &'static PluginInstructions {
        &INSTRUCTIONS
    }

    fn update_instructions(&self) -> &'static PluginInstructions {
        &INSTRUCTIONS
    }
}

static INSTRUCTIONS: LazyLock<PluginInstructions> = LazyLock::new(|| PluginInstructions {
    title: "Install the Warp Plugin for DeepSeek Harness",
    subtitle: "The plugin reports the session's status to this pane. It is installed into each \
               dsh profile with pnpm.",
    steps: &[
        PluginInstructionStep {
            description: "Install pnpm if it is not on PATH",
            command: "npm install -g pnpm",
            executable: true,
            link: None,
        },
        PluginInstructionStep {
            description: "Let the profile's pnpm find the plugin: add this line to \
                          ~/.dsh/profiles/headless/.npmrc",
            command: REGISTRY_LINE,
            executable: false,
            link: None,
        },
        PluginInstructionStep {
            description: "Install the plugin into the profile (repeat for each profile you use)",
            command: "dsh plugin --profile headless add @stozn/dsh-warp@0.1.0",
            executable: true,
            link: None,
        },
    ],
    post_install_notes: &["Start DeepSeek Harness again to activate the plugin."],
});

/// The profiles to install into: each profile directory under `<home>/profiles`, or the headless
/// profile when there is none yet. Names that would need shell quoting are left out.
fn profiles(home: &Path) -> Vec<String> {
    let mut profiles = fs::read_dir(home.join("profiles"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| is_plain_profile_name(name))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    profiles.sort();
    if profiles.is_empty() {
        profiles.push(DEFAULT_PROFILE.to_owned());
    }
    profiles
}

fn is_plain_profile_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

/// `.npmrc` contents that resolve `@stozn` packages from the Gitea registry, or `None` when
/// `existing` already names a registry for the scope.
fn with_registry_line(existing: &str) -> Option<String> {
    if existing
        .lines()
        .any(|line| line.trim_start().starts_with("@stozn:registry="))
    {
        return None;
    }
    let mut updated = existing.to_owned();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(REGISTRY_LINE);
    updated.push('\n');
    Some(updated)
}

fn ensure_registry(profile_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(profile_dir)?;
    let path = profile_dir.join(".npmrc");
    let existing = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    if let Some(updated) = with_registry_line(&existing) {
        fs::write(path, updated)?;
    }
    Ok(())
}

/// The plugin version installed in a profile: `None` when the profile does not depend on it,
/// `0.0.0` when it does but the package is missing from `node_modules`.
fn installed_version(profile_dir: &Path) -> Option<String> {
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(profile_dir.join("package.json")).ok()?).ok()?;
    manifest["dependencies"].get(PACKAGE)?;
    let installed = fs::read_to_string(
        profile_dir
            .join("node_modules")
            .join("@stozn")
            .join("dsh-warp")
            .join("package.json"),
    )
    .ok()
    .and_then(|contents| serde_json::from_str::<Value>(&contents).ok());
    Some(
        installed
            .as_ref()
            .and_then(|package| package["version"].as_str())
            .unwrap_or("0.0.0")
            .to_owned(),
    )
}

/// The plugin version in each existing profile that has it.
fn installed_versions() -> Vec<String> {
    let Some(home) = dsh_home() else {
        return Vec::new();
    };
    profiles(&home)
        .into_iter()
        .filter_map(|profile| installed_version(&home.join("profiles").join(profile)))
        .collect()
}

#[cfg(test)]
#[path = "dsh_tests.rs"]
mod tests;
