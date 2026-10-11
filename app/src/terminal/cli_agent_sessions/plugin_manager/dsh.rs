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

/// The bundled plugin release: the one the installer adds and the oldest one the footer treats as
/// up to date. A macro so the embedded tarball's file name is built from the same literal. Keep in
/// sync with app/resources/dsh-warp/package.json and its packed tarball, and bump them together.
macro_rules! plugin_version {
    () => {
        "0.1.0"
    };
}
const MINIMUM_PLUGIN_VERSION: &str = plugin_version!();

/// The packed plugin, embedded because installed WarpSpeed ships no resources and the package
/// registry refuses anonymous downloads.
const PLUGIN_TARBALL: &[u8] = include_bytes!(concat!(
    "../../../../resources/dsh-warp/stozn-dsh-warp-",
    plugin_version!(),
    ".tgz"
));

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
        let tarball = command_path(&write_tarball(&home)?)?;
        for profile in profiles(&home) {
            self.run_logged("dsh", &plugin_add_args(&profile, &tarball), &mut log)
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
            description: "WarpSpeed bundles the plugin: the Install button writes it to \
                          <dsh home>/warp/ (~/.dsh unless DSH_HOME is set) and adds it to each \
                          profile. To add it to a profile by hand after that, run",
            command: concat!(
                "dsh plugin --profile headless add <dsh home>/warp/stozn-dsh-warp-",
                plugin_version!(),
                ".tgz"
            ),
            executable: false,
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

fn tarball_file_name() -> String {
    format!("stozn-dsh-warp-{MINIMUM_PLUGIN_VERSION}.tgz")
}

/// Writes the bundled tarball to `<home>/warp/` and returns its path. A concurrent install in
/// another pane only ever sees a complete file, because it is written beside and renamed in.
fn write_tarball(home: &Path) -> io::Result<PathBuf> {
    let dir = home.join("warp");
    fs::create_dir_all(&dir)?;
    let path = dir.join(tarball_file_name());
    if fs::read(&path).is_ok_and(|existing| existing == PLUGIN_TARBALL) {
        return Ok(path);
    }
    let temp = tempfile::NamedTempFile::new_in(&dir)?;
    fs::write(temp.path(), PLUGIN_TARBALL)?;
    temp.persist(&path).map_err(|error| error.error)?;
    Ok(path)
}

/// `path` as it can go unquoted into the install command, which the pane's shell (bash,
/// PowerShell or cmd) splits on its own rules. Paths those shells would split or expand are
/// refused rather than quoted, since no one quoting works in all three.
fn command_path(path: &Path) -> Result<String, PluginInstallError> {
    let display = path.display().to_string();
    // Backslashes are escapes to bash; pnpm takes forward slashes on Windows too.
    let command = if cfg!(windows) {
        display.replace('\\', "/")
    } else {
        display.clone()
    };
    let is_unsafe =
        |character: char| character.is_whitespace() || "\"'&|;<>()$\\`%^!".contains(character);
    if command.chars().any(is_unsafe) {
        return Err(PluginInstallError {
            message: format!(
                "cannot install the DeepSeek Harness plugin from {display}: the path has spaces \
                 or shell characters. Set DSH_HOME to a plain path and try again."
            ),
            log: String::new(),
        });
    }
    Ok(command)
}

fn plugin_add_args<'a>(profile: &'a str, tarball: &'a str) -> [&'a str; 5] {
    ["plugin", "--profile", profile, "add", tarball]
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
