use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use fs4::fs_std::FileExt as _;

use super::{SessionConfigFile, materialize_session_config};
use crate::app_state::WindowSnapshot;
use crate::launch_configs::launch_config::{LaunchConfig, LaunchConfigWindowSource};
use crate::tab_configs::session_config::tab_config_from_pane_snapshot;
use crate::user_config::{session_configs_dir, tab_configs_dir};

/// Updates a session template without modifying shared tab configs or live session ownership.
pub(crate) fn overwrite_window(
    source: &LaunchConfigWindowSource,
    window: &WindowSnapshot,
) -> Result<LaunchConfig> {
    overwrite_window_in_dirs(&session_configs_dir(), &tab_configs_dir(), source, window)
}

fn overwrite_window_in_dirs(
    session_dir: &Path,
    tab_dir: &Path,
    source: &LaunchConfigWindowSource,
    window: &WindowSnapshot,
) -> Result<LaunchConfig> {
    ensure!(
        source.is_session_config(),
        "Expected a session configuration"
    );
    ensure!(
        source.window_index == 0,
        "Session configuration has only one window"
    );
    ensure!(!window.tabs.is_empty(), "Cannot save an empty session");
    ensure!(
        window.active_tab_index < window.tabs.len(),
        "Invalid active tab"
    );
    let path = fs::canonicalize(&source.path)?;
    ensure!(
        path.starts_with(fs::canonicalize(session_dir)?),
        "Session configuration is outside the managed directory"
    );
    let lock = fs::File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("toml.save.lock"))?;
    ensure!(
        lock.try_lock_exclusive()?,
        "Session configuration is being saved by another process"
    );
    let original = fs::read_to_string(&path)?;
    let file: SessionConfigFile = toml::from_str(&original)?;
    ensure!(
        file.version == super::SESSION_CONFIG_VERSION,
        "Unsupported session version"
    );
    ensure!(
        file.generation == source.document_index as u64,
        "Session generation has changed"
    );
    ensure!(
        file.name == source.config_name,
        "Session configuration has changed"
    );

    let mut document = original.parse::<toml_edit::DocumentMut>()?;
    fs::create_dir_all(tab_dir)?;
    // Publish new tab files before the manifest; a failed save must leave every old reference valid.
    let saved_tabs = tempfile::Builder::new()
        .prefix("saved-session-")
        .tempdir_in(tab_dir)?;
    let relative_dir = saved_tabs
        .path()
        .file_name()
        .context("Missing tab directory name")?;
    let mut tabs = toml_edit::ArrayOfTables::new();
    let mut configs_by_path = HashMap::new();
    for (index, tab) in window.tabs.iter().enumerate() {
        let id = format!("tab-{}", index + 1);
        // Authored configs can be copied between platforms with different native separators.
        let relative_path = format!("{}/{id}.toml", relative_dir.to_string_lossy());
        let tab_path = tab_dir.join(&relative_path);
        let mut config =
            tab_config_from_pane_snapshot(&tab.root, tab.custom_title.clone(), tab.color());
        config.name = format!(
            "{} — {}",
            file.name,
            tab.custom_title.as_deref().unwrap_or(&id)
        );
        write_atomically(&tab_path, &toml::to_string_pretty(&config)?)?;
        configs_by_path.insert(super::normalized_existing_path(&tab_path), config);
        let mut entry = toml_edit::Table::new();
        entry["id"] = toml_edit::value(id);
        entry["config"] = toml_edit::value(relative_path);
        tabs.push(entry);
    }
    document["tabs"] = toml_edit::Item::ArrayOfTables(tabs);
    document["active_tab"] = toml_edit::value(format!("tab-{}", window.active_tab_index + 1));
    let contents = document.to_string();
    let updated = materialize_session_config(
        toml::from_str(&contents)?,
        source.path.clone(),
        &configs_by_path,
        tab_dir,
    )
    .map_err(anyhow::Error::msg)?;
    ensure!(
        fs::read_to_string(&path)? == original,
        "Session configuration changed while saving"
    );
    write_atomically(&path, &contents)?;
    let _ = saved_tabs.keep();
    Ok(updated)
}

fn write_atomically(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .context("Configuration has no parent directory")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(contents.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
#[path = "save_tests.rs"]
mod tests;
