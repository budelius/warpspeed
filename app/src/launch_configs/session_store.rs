use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ai::blocklist::SerializedBlockListItem;
use crate::app_state::{LeafContents, PaneNodeSnapshot, PaneUuid, WindowSnapshot};
use crate::launch_configs::launch_config::LaunchConfigWindowSource;

const SESSION_FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct StoredPaneBlocks {
    uuid: Vec<u8>,
    blocks: Vec<SerializedBlockListItem>,
}

#[derive(Serialize, Deserialize)]
struct StoredWindowSession {
    version: u32,
    source: LaunchConfigWindowSource,
    window: WindowSnapshot,
    #[serde(default)]
    block_lists: Vec<StoredPaneBlocks>,
}

pub(crate) struct RestoredWindowSession {
    pub window: WindowSnapshot,
    pub block_lists: Arc<HashMap<PaneUuid, Vec<SerializedBlockListItem>>>,
}

pub(crate) fn sessions_dir() -> PathBuf {
    warp_core::paths::state_dir().join("launch_config_sessions")
}

fn session_file_name(source: &LaunchConfigWindowSource) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.path.to_string_lossy().as_bytes());
    hasher.update(source.document_index.to_le_bytes());
    hasher.update(source.window_index.to_le_bytes());
    format!("{:x}.json", hasher.finalize())
}

fn session_path(dir: &Path, source: &LaunchConfigWindowSource) -> PathBuf {
    dir.join(session_file_name(source))
}

#[cfg(feature = "local_fs")]
fn save_to_dir(
    dir: &Path,
    source: &LaunchConfigWindowSource,
    window: &WindowSnapshot,
    block_lists: &HashMap<PaneUuid, Vec<SerializedBlockListItem>>,
    preserve_existing_blocks: bool,
) -> Result<()> {
    use std::io::{BufWriter, Write};

    std::fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create launch config session directory {dir:?}"))?;

    let mut window = window.clone();
    window.launch_config_source = Some(source.clone());
    let pane_uuids = terminal_pane_uuids(&window);
    let path = session_path(dir, source);
    let stored_blocks = if preserve_existing_blocks {
        std::fs::File::open(&path)
            .ok()
            .and_then(|file| {
                serde_json::from_reader::<_, StoredWindowSession>(std::io::BufReader::new(file))
                    .ok()
            })
            .map(|stored| stored.block_lists)
            .unwrap_or_default()
            .into_iter()
            .filter(|pane| pane_uuids.contains(&pane.uuid))
            .collect()
    } else {
        block_lists
            .iter()
            .filter(|(uuid, _)| pane_uuids.contains(&uuid.0))
            .map(|(uuid, blocks)| StoredPaneBlocks {
                uuid: uuid.0.clone(),
                blocks: blocks.clone(),
            })
            .collect()
    };
    let stored = StoredWindowSession {
        version: SESSION_FORMAT_VERSION,
        source: source.clone(),
        window,
        block_lists: stored_blocks,
    };

    let mut temp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("Failed to create temporary session file in {dir:?}"))?;
    {
        let mut writer = BufWriter::new(temp.as_file_mut());
        serde_json::to_writer(&mut writer, &stored)?;
        writer.flush()?;
    }
    temp.as_file().sync_all()?;
    temp.persist(&path)
        .map_err(|error| error.error)
        .with_context(|| format!("Failed to replace launch config session at {path:?}"))?;
    Ok(())
}

#[cfg(not(feature = "local_fs"))]
fn save_to_dir(
    _dir: &Path,
    _source: &LaunchConfigWindowSource,
    _window: &WindowSnapshot,
    _block_lists: &HashMap<PaneUuid, Vec<SerializedBlockListItem>>,
    _preserve_existing_blocks: bool,
) -> Result<()> {
    Ok(())
}

pub(crate) fn save(source: &LaunchConfigWindowSource, window: &WindowSnapshot) -> Result<()> {
    save_to_dir(&sessions_dir(), source, window, &HashMap::new(), true)
}

pub(crate) fn save_windows(
    windows: &[WindowSnapshot],
    block_lists: &HashMap<PaneUuid, Vec<SerializedBlockListItem>>,
) -> Result<()> {
    for window in windows {
        if let Some(source) = window.launch_config_source.as_ref() {
            save_to_dir(&sessions_dir(), source, window, block_lists, false)?;
        }
    }
    Ok(())
}

#[cfg(feature = "local_fs")]
fn load_from_dir(
    dir: &Path,
    source: &LaunchConfigWindowSource,
) -> Result<Option<RestoredWindowSession>> {
    let path = session_path(dir, source);
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to open launch config session at {path:?}"));
        }
    };
    let stored: StoredWindowSession = serde_json::from_reader(std::io::BufReader::new(file))
        .with_context(|| format!("Failed to parse launch config session at {path:?}"))?;
    if stored.version != SESSION_FORMAT_VERSION {
        bail!(
            "Unsupported launch config session version {} at {path:?}",
            stored.version
        );
    }
    if stored.source != *source {
        bail!("Launch config session source does not match {path:?}");
    }
    Ok(Some(RestoredWindowSession {
        window: stored.window,
        block_lists: Arc::new(
            stored
                .block_lists
                .into_iter()
                .map(|pane| (PaneUuid(pane.uuid), pane.blocks))
                .collect(),
        ),
    }))
}

#[cfg(not(feature = "local_fs"))]
fn load_from_dir(
    _dir: &Path,
    _source: &LaunchConfigWindowSource,
) -> Result<Option<RestoredWindowSession>> {
    Ok(None)
}

pub(crate) fn load(source: &LaunchConfigWindowSource) -> Result<Option<RestoredWindowSession>> {
    load_from_dir(&sessions_dir(), source)
}

/// The uuids of every terminal pane in a window snapshot.
///
/// Callers restoring a window use these to read only the blocks they need.
pub(crate) fn terminal_pane_uuids(window: &WindowSnapshot) -> Vec<Vec<u8>> {
    fn collect(node: &PaneNodeSnapshot, uuids: &mut Vec<Vec<u8>>) {
        match node {
            PaneNodeSnapshot::Branch(branch) => {
                for (_, child) in &branch.children {
                    collect(child, uuids);
                }
            }
            PaneNodeSnapshot::Leaf(leaf) => {
                if let LeafContents::Terminal(terminal) = &leaf.contents {
                    uuids.push(terminal.uuid.clone());
                }
            }
        }
    }

    let mut uuids = Vec::new();
    for tab in &window.tabs {
        collect(&tab.root, &mut uuids);
    }
    uuids
}

#[cfg(test)]
#[path = "session_store_tests.rs"]
mod tests;
