use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::Context;
use fs4::fs_std::FileExt as _;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;
use warp_core::safe_warn;

use crate::launch_configs::launch_config::{
    LaunchConfig, LaunchConfigSource, LaunchConfigWindowSource, TabTemplate, WindowTemplate,
};
use crate::tab_configs::TabConfig;

const SESSION_CONFIG_VERSION: u32 = 1;

static SESSION_LEASES: LazyLock<Mutex<HashMap<(String, u64), File>>> =
    LazyLock::new(Default::default);

#[derive(Clone, Debug)]
pub struct SessionConfigError {
    pub file_path: PathBuf,
    pub message: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionConfigFile {
    version: u32,
    id: String,
    name: String,
    #[serde(default = "default_generation")]
    generation: u64,
    #[serde(default)]
    active_tab: Option<String>,
    tabs: Vec<SessionConfigTab>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionConfigTab {
    id: String,
    config: PathBuf,
    #[serde(default)]
    params: HashMap<String, String>,
}

fn default_generation() -> u64 {
    1
}

/// `owner.json` lives beside `window-<n>.json` in the same checkpoint tree
/// (`sessions_state_dir` here and `session_store::sessions_dir` are the same
/// path), so `session_store::scan_catalog` must skip it by this name.
pub(crate) const OWNER_FILE_NAME: &str = "owner.json";

/// Which process holds a session lease. Written by the claimer while it holds
/// the exclusive lock, so concurrent writers cannot interleave.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionOwner {
    pub pid: u32,
    pub claimed_at_unix_seconds: i64,
    pub app_id: String,
}

pub enum SessionClaim {
    Claimed,
    /// The owner record is absent or unreadable on a crash-and-reclaim, but the
    /// held lock still proves someone owns the session.
    HeldBy(Option<SessionOwner>),
}

pub fn claim_session(source: &LaunchConfigWindowSource) -> anyhow::Result<SessionClaim> {
    let session_id = source
        .session_id()
        .context("Session config source has no session id")?;
    let generation = source
        .session_generation()
        .context("Session config source has no generation")?;
    let mut leases = SESSION_LEASES.lock();
    claim_in_dir(&sessions_state_dir(), &session_id, generation, &mut leases)
}

fn sessions_state_dir() -> PathBuf {
    warp_core::paths::state_dir().join("session_config_sessions")
}

fn lease_dir(dir: &Path, session_id: &str, generation: u64) -> PathBuf {
    dir.join(session_id).join(generation.to_string())
}

fn claim_in_dir(
    dir: &Path,
    session_id: &str,
    generation: u64,
    leases: &mut HashMap<(String, u64), File>,
) -> anyhow::Result<SessionClaim> {
    let key = (session_id.to_owned(), generation);
    if leases.contains_key(&key) {
        return Ok(SessionClaim::Claimed);
    }

    let lease_dir = lease_dir(dir, session_id, generation);
    fs::create_dir_all(&lease_dir)
        .with_context(|| format!("Failed to create session lease directory {lease_dir:?}"))?;
    let lease_path = lease_dir.join("owner.lock");
    let lease = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lease_path)
        .with_context(|| format!("Failed to open session lease {lease_path:?}"))?;
    match lease.try_lock_exclusive() {
        Ok(true) => {
            // The owner record is best-effort: the lock itself is what makes
            // the claim real. Losing only the cosmetic record of who holds it
            // must not throw away a lease that was already acquired.
            if let Err(error) = write_owner(&lease_dir) {
                safe_warn!(
                    safe: ("Failed to write session owner record"),
                    full: ("Failed to write session owner record: {error:#}")
                );
            }
            leases.insert(key, lease);
            Ok(SessionClaim::Claimed)
        }
        Ok(false) => Ok(SessionClaim::HeldBy(read_owner(
            dir, session_id, generation,
        ))),
        Err(error) => {
            Err(error).with_context(|| format!("Failed to claim session lease {lease_path:?}"))
        }
    }
}

fn write_owner(lease_dir: &Path) -> anyhow::Result<()> {
    let owner = SessionOwner {
        pid: std::process::id(),
        claimed_at_unix_seconds: chrono::Utc::now().timestamp(),
        app_id: warp_core::channel::ChannelState::app_id()
            .application_name()
            .to_owned(),
    };
    let path = lease_dir.join(OWNER_FILE_NAME);
    let contents = serde_json::to_vec(&owner).context("Failed to serialize session owner")?;
    fs::write(&path, contents)
        .with_context(|| format!("Failed to write session owner record {path:?}"))
}

fn read_owner(dir: &Path, session_id: &str, generation: u64) -> Option<SessionOwner> {
    let path = lease_dir(dir, session_id, generation).join(OWNER_FILE_NAME);
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn normalized_existing_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The session id a config file name stands for. A config is only valid when
/// its declared id normalizes to the same thing, so this is also the identity
/// two differently spelled file names would collide on.
pub(crate) fn normalized_session_id(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

fn materialize_session_config(
    file: SessionConfigFile,
    file_path: PathBuf,
    tab_configs_by_path: &HashMap<PathBuf, TabConfig>,
    tab_configs_dir: &Path,
) -> Result<LaunchConfig, String> {
    if file.version != SESSION_CONFIG_VERSION {
        return Err(format!(
            "unsupported session config version {}; expected {SESSION_CONFIG_VERSION}",
            file.version
        ));
    }
    if file.id.trim().is_empty() {
        return Err("session id cannot be empty".to_owned());
    }
    let file_id = file_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(normalized_session_id)
        .ok_or_else(|| "session config file has no valid file name".to_owned())?;
    if normalized_session_id(&file.id) != file_id {
        return Err(format!(
            "session id {:?} must match the config file name identity {file_id:?}",
            file.id
        ));
    }
    if file.name.trim().is_empty() {
        return Err("session name cannot be empty".to_owned());
    }
    if file.tabs.is_empty() {
        return Err("session must reference at least one tab config".to_owned());
    }

    let mut tab_ids = HashSet::new();
    let mut ordered_tab_ids = Vec::with_capacity(file.tabs.len());
    let mut tabs = Vec::with_capacity(file.tabs.len());
    for session_tab in file.tabs {
        if !tab_ids.insert(session_tab.id.clone()) {
            return Err(format!("duplicate tab id {:?}", session_tab.id));
        }
        ordered_tab_ids.push(session_tab.id.clone());

        let referenced_path = if session_tab.config.is_absolute() {
            session_tab.config
        } else {
            tab_configs_dir.join(session_tab.config)
        };
        let referenced_path = normalized_existing_path(&referenced_path);
        let tab_config = tab_configs_by_path.get(&referenced_path).ok_or_else(|| {
            format!(
                "tab {:?} references a missing or invalid tab config at {}",
                session_tab.id,
                referenced_path.display()
            )
        })?;

        let mut params = tab_config.default_param_values();
        params.extend(session_tab.params);
        let (title, layout) = crate::tab_configs::render_tab_config(tab_config, &params, None);
        tabs.push(TabTemplate {
            title,
            layout,
            commands: Vec::new(),
            color: tab_config.color,
        });
    }

    let active_tab_index = match file.active_tab {
        Some(active_tab) => Some(
            ordered_tab_ids
                .iter()
                .position(|tab_id| tab_id == &active_tab)
                .ok_or_else(|| format!("active_tab references unknown tab id {active_tab:?}"))?,
        ),
        None => Some(0),
    };

    Ok(LaunchConfig {
        name: file.name,
        active_window_index: Some(0),
        windows: vec![WindowTemplate {
            active_tab_index,
            tabs,
        }],
        source: Some(LaunchConfigSource {
            path: file_path,
            document_index: file.generation as usize,
        }),
    })
}

pub fn load_session_configs(
    session_configs_dir: &Path,
    tab_configs_dir: &Path,
) -> (Vec<LaunchConfig>, Vec<SessionConfigError>) {
    let (tab_configs, tab_config_errors) = crate::user_config::load_tab_configs(tab_configs_dir);
    let tab_configs_by_path = tab_configs
        .into_iter()
        .filter_map(|config| {
            let path = normalized_existing_path(config.source_path.as_ref()?);
            Some((path, config))
        })
        .collect::<HashMap<_, _>>();

    let mut errors = tab_config_errors
        .into_iter()
        .map(|error| SessionConfigError {
            file_path: error.file_path,
            message: error.error_message,
        })
        .collect::<Vec<_>>();
    let mut configs = Vec::new();
    let mut session_ids = HashSet::new();

    if session_configs_dir.is_dir() {
        for entry in WalkDir::new(session_configs_dir)
            .follow_links(true)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_type().is_file()
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "toml")
            })
        {
            let path = entry.path().to_path_buf();
            let result = fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|contents| {
                    toml::from_str::<SessionConfigFile>(&contents)
                        .map_err(|error| error.to_string())
                })
                .and_then(|file| {
                    if !session_ids.insert(file.id.clone()) {
                        return Err(format!("duplicate session id {:?}", file.id));
                    }
                    materialize_session_config(
                        file,
                        path.clone(),
                        &tab_configs_by_path,
                        tab_configs_dir,
                    )
                });

            match result {
                Ok(config) => configs.push(config),
                Err(message) => errors.push(SessionConfigError {
                    file_path: path,
                    message,
                }),
            }
        }
    }

    configs.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    (configs, errors)
}

#[cfg(test)]
#[path = "session_configs_tests.rs"]
mod tests;
