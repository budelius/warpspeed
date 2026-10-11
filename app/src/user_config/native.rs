use std::io::Write;
use std::path::Path;
use std::{fs, io};

use anyhow::{Result, anyhow};
use itertools::Itertools;
use repo_metadata::RepositoryUpdate;
use serde::Deserialize;
use warpui::{ModelContext, ModelHandle, SingletonEntity};

use super::util::{
    for_each_dir_entry, has_name, is_config_file, parse_model_config_dir_entry,
    parse_multi_launch_config_dir_entry, parse_multi_workflow_dir_entry,
    parse_single_theme_dir_entry, parse_tab_config_dir_entry,
};
use super::{
    LAUNCH_CONFIG_COMMENT, WarpConfigUpdateEvent, custom_model_routers_dir, launch_configs_dir,
    session_configs_dir, tab_configs_dir, themes_dir, workflows_dir,
};
use crate::ai::custom_model_routers::{CustomModelRouter, ModelConfigError};
use crate::features::FeatureFlag;
use crate::launch_configs::launch_config::{
    LaunchConfig, LaunchConfigSource, LaunchConfigWindowSource, WindowTemplate,
};
use crate::session_configs::load_session_configs;
use crate::tab_configs::{TabConfig, TabConfigError};
use crate::themes::theme::WarpThemeConfig;
use crate::warp_managed_paths_watcher::{
    WarpManagedPathsWatcher, WarpManagedPathsWatcherEvent, repository_update_touches_path,
    repository_update_touches_prefix,
};
use crate::workflows::workflow::Workflow;

impl super::WarpConfig {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        // Load launch configs, and workflows from disk asynchronously on a background
        // thread.
        //
        // Themes are required during initialization by `Settings`, so we load this synchronously
        // on startup. We should investigate the possibility of offloading theme loading to a
        // background thread in the future.
        let _ = ctx.spawn(
            async move { load_launch_configs(&launch_configs_dir()) },
            |me, launch_configs, ctx| {
                me.launch_configs = launch_configs;
                ctx.emit(WarpConfigUpdateEvent::LaunchConfigs);
            },
        );
        if FeatureFlag::TabConfigs.is_enabled() {
            // Task selection and launch live in the STOZN Dashboard. Remove the
            // retired local picker before loading authored tab configurations.
            super::remove_stozn_managed_agent_config();
            let _ = ctx.spawn(
                async move { load_tab_configs(&tab_configs_dir()) },
                |me, (tab_configs, tab_config_errors), ctx| {
                    me.tab_configs = tab_configs;
                    me.tab_config_errors = tab_config_errors;
                    ctx.emit(WarpConfigUpdateEvent::TabConfigs);
                    // Don't emit TabConfigErrors on startup — the error toast
                    // should only appear when the user saves a config file,
                    // not on app restart.
                },
            );
            let _ = ctx.spawn(
                async move { load_session_configs(&session_configs_dir(), &tab_configs_dir()) },
                |me, (session_configs, errors), ctx| {
                    for error in errors {
                        warp_core::safe_warn!(
                            safe: ("Failed to load a session config"),
                            full: (
                                "Failed to load session config {}: {}",
                                error.file_path.display(),
                                error.message
                            )
                        );
                    }
                    me.session_configs = session_configs;
                    ctx.emit(WarpConfigUpdateEvent::SessionConfigs);
                },
            );
        }
        let _ = ctx.spawn(
            async move { load_workflows(&workflows_dir()) },
            |me, user_workflows, ctx| {
                me.local_user_workflows = user_workflows;
                ctx.emit(WarpConfigUpdateEvent::LocalUserWorkflows);
            },
        );
        if FeatureFlag::CustomModelRouters.is_enabled() {
            let _ = ctx.spawn(
                async move { load_model_configs(&custom_model_routers_dir()) },
                |me, (models, errors), ctx| {
                    me.custom_model_routers = models;
                    me.custom_model_router_errors = errors;
                    ctx.emit(WarpConfigUpdateEvent::ModelConfigs);
                    // Don't emit ModelConfigErrors on startup — like tab configs,
                    // the error toast should only appear when the user saves a
                    // file, not on app restart.
                },
            );
        }
        ctx.subscribe_to_model(
            &WarpManagedPathsWatcher::handle(ctx),
            Self::handle_warp_managed_paths_event,
        );

        Self {
            theme_config: load_theme_configs(&themes_dir()),
            ..Default::default()
        }
    }

    fn handle_warp_managed_paths_event(
        &mut self,
        _: ModelHandle<WarpManagedPathsWatcher>,
        event: &WarpManagedPathsWatcherEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        let WarpManagedPathsWatcherEvent::FilesChanged(update) = event;

        if update_touches_dir(update, &themes_dir()) {
            let theme_dir = themes_dir();
            let _ = ctx.spawn(
                async move { load_theme_configs(&theme_dir) },
                |me, theme_config, ctx| {
                    me.theme_config = theme_config;
                    ctx.emit(WarpConfigUpdateEvent::Themes);
                },
            );
        }

        if update_touches_dir(update, &workflows_dir()) {
            let workflow_dir = workflows_dir();
            let _ = ctx.spawn(
                async move { load_workflows(&workflow_dir) },
                |me, workflows, ctx| {
                    me.local_user_workflows = workflows;
                    ctx.emit(WarpConfigUpdateEvent::LocalUserWorkflows);
                },
            );
        }

        if update_touches_dir(update, &launch_configs_dir()) {
            let launch_config_dir = launch_configs_dir();
            let _ = ctx.spawn(
                async move { load_launch_configs(&launch_config_dir) },
                |me, launch_configs, ctx| {
                    me.launch_configs = launch_configs;
                    ctx.emit(WarpConfigUpdateEvent::LaunchConfigs);
                },
            );
        }

        if FeatureFlag::TabConfigs.is_enabled() && update_touches_dir(update, &tab_configs_dir()) {
            let tab_config_dir = tab_configs_dir();
            let _ = ctx.spawn(
                async move { load_tab_configs(&tab_config_dir) },
                |me, (configs, errors), ctx| {
                    me.tab_configs = configs;
                    me.tab_config_errors = errors.clone();
                    ctx.emit(WarpConfigUpdateEvent::TabConfigs);
                    if !errors.is_empty() {
                        ctx.emit(WarpConfigUpdateEvent::TabConfigErrors(errors));
                    }
                },
            );
        }

        if FeatureFlag::TabConfigs.is_enabled()
            && (update_touches_dir(update, &tab_configs_dir())
                || update_touches_dir(update, &session_configs_dir()))
        {
            let session_config_dir = session_configs_dir();
            let tab_config_dir = tab_configs_dir();
            let _ = ctx.spawn(
                async move { load_session_configs(&session_config_dir, &tab_config_dir) },
                |me, (configs, errors), ctx| {
                    for error in errors {
                        warp_core::safe_warn!(
                            safe: ("Failed to load a session config"),
                            full: (
                                "Failed to load session config {}: {}",
                                error.file_path.display(),
                                error.message
                            )
                        );
                    }
                    me.session_configs = configs;
                    ctx.emit(WarpConfigUpdateEvent::SessionConfigs);
                },
            );
        }

        if FeatureFlag::CustomModelRouters.is_enabled()
            && update_touches_dir(update, &custom_model_routers_dir())
        {
            let dir_path = custom_model_routers_dir();
            let _ = ctx.spawn(
                async move { load_model_configs(&dir_path) },
                |me, (models, errors), ctx| {
                    me.custom_model_routers = models;
                    me.custom_model_router_errors = errors.clone();
                    ctx.emit(WarpConfigUpdateEvent::ModelConfigs);
                    if !errors.is_empty() {
                        ctx.emit(WarpConfigUpdateEvent::ModelConfigErrors(errors));
                    }
                },
            );
        }

        if FeatureFlag::SettingsFile.is_enabled()
            && update_touches_path(update, &crate::settings::user_preferences_toml_file_path())
        {
            ctx.emit(WarpConfigUpdateEvent::Settings);
        }
    }

    /// Writes a custom model router to disk as a YAML file.
    ///
    /// When `existing_path` is provided (editing) the file at that path is
    /// overwritten; otherwise a new file is created under
    /// `custom_model_routers_dir()`. The file name is derived from `name` by
    /// lowercasing and replacing non-alphanumeric characters (except `-`) with
    /// `_`. If the candidate path already exists, a numeric suffix is appended
    /// (`_2`, `_3`, …) until a free slot is found. Returns the path written to.
    #[cfg(feature = "local_fs")]
    pub fn save_custom_model_router(
        name: &str,
        yaml: &str,
        existing_path: Option<&std::path::Path>,
    ) -> anyhow::Result<std::path::PathBuf> {
        let dir = custom_model_routers_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| anyhow::anyhow!("could not create custom_model_routers dir: {e}"))?;
        let path = if let Some(p) = existing_path {
            p.to_path_buf()
        } else {
            let sanitized = name
                .to_lowercase()
                .replace(|c: char| !c.is_alphanumeric() && c != '-', "_");
            let candidate = dir.join(format!("{sanitized}.yaml"));
            if candidate.exists() {
                (2..)
                    .map(|n| dir.join(format!("{sanitized}_{n}.yaml")))
                    .find(|p| !p.exists())
                    .expect("infinite iterator always finds a free slot")
            } else {
                candidate
            }
        };
        std::fs::write(&path, yaml)
            .map_err(|e| anyhow::anyhow!("could not write router file: {e}"))?;
        Ok(path)
    }

    /// Deletes a custom model router file from disk.
    /// The filesystem watcher in [`Self::handle_warp_managed_paths_event`] will
    /// pick up the deletion and reload `custom_model_routers`.
    #[cfg(feature = "local_fs")]
    pub fn delete_custom_model_router(source_path: &std::path::Path) -> anyhow::Result<()> {
        std::fs::remove_file(source_path)
            .map_err(|e| anyhow::anyhow!("could not delete router file: {e}"))
    }

    /// Replaces only the source window while preserving the file's other configurations.
    #[cfg(feature = "local_fs")]
    pub fn overwrite_launch_config_window(
        source: &LaunchConfigWindowSource,
        window: WindowTemplate,
    ) -> Result<LaunchConfig> {
        overwrite_launch_config_window_in_dir(
            &crate::user_config::launch_configs_dir(),
            source,
            window,
        )
    }

    /// This method takes a file name candidate (appends .yaml if missing) and a LaunchConfig as
    /// arguments. It saves the file and returns where it was written, failing if a launch
    /// configuration of that name is already there.
    #[cfg(feature = "local_fs")]
    pub fn save_new_launch_config(
        file_name: String,
        launch_config: LaunchConfig,
    ) -> std::result::Result<SavedLaunchConfig, SaveLaunchConfigError> {
        write_launch_config_file(
            &crate::user_config::launch_configs_dir(),
            file_name,
            launch_config,
            Overwrite::No,
        )
    }

    /// Saves a launch configuration over the file of that name, replacing what is there.  Any
    /// other configurations that file holds are preserved.  A missing file is created, so that
    /// renaming between a failed save and the overwrite still does the obvious thing.
    #[cfg(feature = "local_fs")]
    pub fn overwrite_launch_config(
        file_name: String,
        launch_config: LaunchConfig,
    ) -> std::result::Result<SavedLaunchConfig, SaveLaunchConfigError> {
        write_launch_config_file(
            &crate::user_config::launch_configs_dir(),
            file_name,
            launch_config,
            Overwrite::Yes,
        )
    }
}

/// Where a launch configuration ended up on disk.
#[cfg(feature = "local_fs")]
#[derive(Debug, PartialEq, Eq)]
pub struct SavedLaunchConfig {
    /// The file name actually used, which is the candidate with `.yaml` appended if it was
    /// missing a recognized suffix.
    pub file_name: String,
    /// Which document within that file holds the configuration.
    pub document_index: usize,
}

/// Why writing a launch configuration file failed.  Callers distinguish the name clash from the
/// rest so they can offer to replace the existing configuration.
#[cfg(feature = "local_fs")]
#[derive(Debug, thiserror::Error)]
pub enum SaveLaunchConfigError {
    #[error("Launch configuration file name is empty")]
    EmptyFileName,
    #[error("Launch configuration file {0} already exists")]
    FileAlreadyExists(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Whether an existing file of the same name may be written over.
#[cfg(feature = "local_fs")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Overwrite {
    Yes,
    No,
}

#[cfg(feature = "local_fs")]
fn write_launch_config_file(
    launch_configs_dir: &Path,
    file_name: String,
    launch_config: LaunchConfig,
    overwrite: Overwrite,
) -> std::result::Result<SavedLaunchConfig, SaveLaunchConfigError> {
    let file_name: String = if is_config_file(&file_name) {
        file_name.trim().into()
    } else {
        format!("{file_name}.yaml")
    };

    if !has_name(file_name.trim()) {
        return Err(SaveLaunchConfigError::EmptyFileName);
    };

    let path = launch_configs_dir.join(&file_name);
    let exists = path.exists();
    if exists && overwrite == Overwrite::No {
        return Err(SaveLaunchConfigError::FileAlreadyExists(file_name));
    }

    let (configs, document_index) = if exists {
        merge_into_existing_launch_configs(&path, launch_config)
    } else {
        (vec![launch_config], 0)
    };

    write_launch_configs(&path, &configs)?;
    Ok(SavedLaunchConfig {
        file_name,
        document_index,
    })
}

/// Places `launch_config` among the documents already in `path`, returning the documents to write
/// and the index the configuration landed at.
///
/// A file Warp wrote holds a single document, which is replaced outright.  A hand-authored file
/// can hold several, so only the document of the same name is replaced and any others are kept; if
/// none matches, the configuration is added rather than displacing what is there.  A file we can't
/// parse holds nothing worth preserving, so it is replaced too.
#[cfg(feature = "local_fs")]
fn merge_into_existing_launch_configs(
    path: &Path,
    launch_config: LaunchConfig,
) -> (Vec<LaunchConfig>, usize) {
    let Ok(file) = fs::File::open(path) else {
        return (vec![launch_config], 0);
    };
    let Ok(mut configs) = serde_yaml::Deserializer::from_reader(io::BufReader::new(file))
        .map(LaunchConfig::deserialize)
        .collect::<std::result::Result<Vec<_>, _>>()
    else {
        return (vec![launch_config], 0);
    };

    if configs.len() < 2 {
        return (vec![launch_config], 0);
    }

    match configs
        .iter()
        .position(|config| config.name == launch_config.name)
    {
        Some(index) => {
            configs[index] = launch_config;
            (configs, index)
        }
        None => {
            configs.push(launch_config);
            let index = configs.len() - 1;
            (configs, index)
        }
    }
}

/// Writes `configs` to `path` through a temporary file, so an interrupted write can't leave a
/// half-written launch configuration behind.
#[cfg(feature = "local_fs")]
fn write_launch_configs(path: &Path, configs: &[LaunchConfig]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Launch configuration path has no parent"))?;
    fs::create_dir_all(parent)?;

    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut writer = io::BufWriter::new(temp.as_file_mut());
        writer.write_all(LAUNCH_CONFIG_COMMENT.as_bytes())?;
        for config in configs {
            serde_yaml::to_writer(&mut writer, config)?;
        }
        writer.flush()?;
    }
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(feature = "local_fs")]
fn overwrite_launch_config_window_in_dir(
    launch_configs_dir: &Path,
    source: &LaunchConfigWindowSource,
    window: WindowTemplate,
) -> Result<LaunchConfig> {
    let managed_dir = fs::canonicalize(launch_configs_dir)?;
    let source_path = fs::canonicalize(&source.path)?;
    if !source_path.starts_with(&managed_dir) {
        return Err(anyhow!(
            "Launch configuration is outside the managed directory"
        ));
    }

    let file = fs::File::open(&source_path)?;
    let reader = io::BufReader::new(file);
    let mut configs = serde_yaml::Deserializer::from_reader(reader)
        .map(LaunchConfig::deserialize)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let config = configs
        .get_mut(source.document_index)
        .ok_or_else(|| anyhow!("Launch configuration document no longer exists"))?;
    if !config.name.eq_ignore_ascii_case(&source.config_name) {
        return Err(anyhow!("Launch configuration document has changed"));
    }
    let target = config
        .windows
        .get_mut(source.window_index)
        .ok_or_else(|| anyhow!("Launch configuration window no longer exists"))?;
    *target = window;

    write_launch_configs(&source_path, &configs)?;

    let mut updated = configs
        .into_iter()
        .nth(source.document_index)
        .expect("already checked launch configuration document index");
    updated.source = Some(LaunchConfigSource {
        path: source.path.clone(),
        document_index: source.document_index,
    });
    Ok(updated)
}

pub fn load_theme_configs(theme_path: &Path) -> WarpThemeConfig {
    let mut theme_configs = WarpThemeConfig::new();
    for_each_dir_entry(theme_path, parse_single_theme_dir_entry)
        .into_iter()
        .for_each(|(theme_name, theme)| theme_configs.add_new_theme(theme_name, theme));
    theme_configs
}

/// Loads all workflows relative to the `workflow_path`.  A YAML file might
/// contain multiple workflows.
pub fn load_workflows(workflow_path: &Path) -> Vec<Workflow> {
    for_each_dir_entry(workflow_path, parse_multi_workflow_dir_entry)
        .into_iter()
        .flatten()
        .collect_vec()
}

/// Loads all launch configs relative to the `launch_config_path`. Each workflow is assumed to be in an
/// individual YAML file.
pub fn load_launch_configs(launch_config_path: &Path) -> Vec<LaunchConfig> {
    for_each_dir_entry(launch_config_path, parse_multi_launch_config_dir_entry)
        .into_iter()
        .flatten()
        .collect_vec()
}

/// Loads custom model routers from the config directory at `dir_path`
/// (`~/.warp/custom_model_routers/`), where each file defines a single router.
/// Returns the parsed routers (sorted by display name) and any per-file
/// parse/validation errors. If the directory does not exist, returns empty vecs.
pub fn load_model_configs(dir_path: &Path) -> (Vec<CustomModelRouter>, Vec<ModelConfigError>) {
    let results = for_each_dir_entry(dir_path, parse_model_config_dir_entry);
    let mut models = Vec::new();
    let mut errors = Vec::new();
    for result in results {
        match result {
            Ok(model) => models.push(model),
            Err(error) => errors.push(error),
        }
    }
    models.sort_by(|a, b| {
        let a_name = a.info.display_name.to_lowercase();
        let b_name = b.info.display_name.to_lowercase();
        a_name
            .cmp(&b_name)
            .then_with(|| a.info.display_name.cmp(&b.info.display_name))
    });
    (models, errors)
}

/// Loads all tab configs from `tab_config_path`. Each tab config is an individual TOML file.
///
/// Returns successfully parsed configs and any errors for files that failed to parse.
pub fn load_tab_configs(tab_config_path: &Path) -> (Vec<TabConfig>, Vec<TabConfigError>) {
    let results = for_each_dir_entry(tab_config_path, parse_tab_config_dir_entry);
    let mut configs = Vec::new();
    let mut errors = Vec::new();
    for result in results {
        match result {
            Ok(config) => configs.push(config),
            Err(error) => errors.push(error),
        }
    }
    configs.sort_by(|a, b| {
        let a_name = a.name.to_lowercase();
        let b_name = b.name.to_lowercase();
        a_name.cmp(&b_name).then_with(|| a.name.cmp(&b.name))
    });
    (configs, errors)
}

fn update_touches_dir(update: &RepositoryUpdate, path: &Path) -> bool {
    let canonical_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    repository_update_touches_prefix(update, path)
        || repository_update_touches_prefix(update, &canonical_path)
}

fn update_touches_path(update: &RepositoryUpdate, path: &Path) -> bool {
    let canonical_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    repository_update_touches_path(update, path)
        || repository_update_touches_path(update, &canonical_path)
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
