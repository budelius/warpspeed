//! Per-process checkpoints keep a crashed window recoverable beside other running instances.

use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use fs4::fs_std::FileExt as _;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::app_state::{AppState, WindowSnapshot};

static RECOVERY: LazyLock<Mutex<Option<Recovery>>> = LazyLock::new(Default::default);

#[derive(Serialize, Deserialize)]
struct Checkpoint {
    version: u32,
    revision: String,
    #[serde(default)]
    supersedes: Vec<String>,
    windows: Vec<WindowSnapshot>,
    active_window_index: Option<usize>,
}

struct Claim {
    path: PathBuf,
    _lease: File,
}

struct Recovery {
    own: Claim,
    pending: Vec<Claim>,
    ready: bool,
}

impl Claim {
    fn acquire(path: PathBuf) -> Result<Option<Self>> {
        let lease = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))?;
        Ok(lease.try_lock_exclusive()?.then_some(Self {
            path,
            _lease: lease,
        }))
    }

    fn remove_checkpoint(&self) -> Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

impl Recovery {
    fn new(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.json", uuid::Uuid::new_v4()));
        let own = Claim::acquire(path)?.context("Recovery checkpoint is already claimed")?;
        Ok(Self {
            own,
            pending: Vec::new(),
            ready: false,
        })
    }

    fn claim_interrupted(&mut self) -> Result<Vec<Checkpoint>> {
        let dir = self
            .own
            .path
            .parent()
            .context("Missing checkpoint directory")?;
        // Claims must be atomic as a set: a replacement checkpoint can supersede other files.
        let Some(_scan) = Claim::acquire(dir.join("scan"))? else {
            return Ok(Vec::new());
        };
        let mut checkpoints = Vec::new();
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let Some(claim) = Claim::acquire(path)? else {
                continue;
            };
            // Keep unreadable or newer checkpoints for manual recovery rather than overwriting them.
            let checkpoint = fs::read(&claim.path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Checkpoint>(&bytes).ok())
                .filter(|checkpoint| checkpoint.version == 1);
            if let Some(checkpoint) = checkpoint {
                let name = claim.path.file_name().context("Missing checkpoint name")?;
                checkpoints.push((name.to_string_lossy().into_owned(), checkpoint));
                self.pending.push(claim);
            } else {
                log::warn!("Skipping an unreadable session recovery checkpoint");
            }
        }
        let superseded = checkpoints
            .iter()
            .flat_map(|(_, checkpoint)| checkpoint.supersedes.iter().cloned())
            .collect::<HashSet<_>>();
        Ok(checkpoints
            .into_iter()
            .filter(|(name, _)| !superseded.contains(name))
            .map(|(_, checkpoint)| checkpoint)
            .collect())
    }

    fn save(&mut self, state: &AppState) -> Result<()> {
        if !self.ready {
            return Ok(());
        }
        crate::launch_configs::session_store::write_json_atomically(
            &self.own.path,
            &Checkpoint {
                version: 1,
                revision: env!("WARP_BUILD_REVISION").to_owned(),
                supersedes: self
                    .pending
                    .iter()
                    .filter_map(|claim| claim.path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .collect(),
                windows: state.windows.clone(),
                active_window_index: state.active_window_index,
            },
        )?;
        // Only retire source checkpoints after all restored windows have been durably saved.
        for claim in &self.pending {
            claim.remove_checkpoint()?;
        }
        self.pending.clear();
        Ok(())
    }
}

/// Starts checkpointing this GUI instance and optionally claims interrupted instances.
pub(crate) fn begin(restore: bool) -> Result<Option<AppState>> {
    let mut recovery = Recovery::new(&warp_core::paths::state_dir().join("session-recovery"))?;
    let checkpoints = if restore {
        recovery.claim_interrupted()?
    } else {
        Vec::new()
    };
    let mut windows = Vec::new();
    let mut active_window_index = None;
    for checkpoint in checkpoints {
        log::info!(
            "Recovering interrupted WarpSpeed instance: windows={} revision={}",
            checkpoint.windows.len(),
            checkpoint.revision
        );
        #[cfg(feature = "crash_reporting")]
        crate::crash_reporting::report_interrupted_session(&checkpoint.revision);
        active_window_index = checkpoint
            .active_window_index
            .filter(|index| *index < checkpoint.windows.len())
            .map(|index| windows.len() + index)
            .or(active_window_index);
        windows.extend(checkpoint.windows);
    }
    *RECOVERY.lock() = Some(recovery);
    Ok((!windows.is_empty()).then_some(AppState {
        windows,
        active_window_index,
        block_lists: Default::default(),
        running_mcp_servers: Vec::new(),
    }))
}

pub(crate) fn is_ready() -> bool {
    RECOVERY
        .lock()
        .as_ref()
        .is_some_and(|recovery| recovery.ready)
}

pub(crate) fn finish_startup(state: Option<&AppState>) -> Result<()> {
    if let Some(recovery) = RECOVERY.lock().as_mut() {
        recovery.ready = true;
        if let Some(state) = state {
            recovery.save(state)?;
        }
    }
    Ok(())
}

pub(crate) fn save(state: &AppState) -> Result<()> {
    if let Some(recovery) = RECOVERY.lock().as_mut() {
        recovery.save(state)?;
    }
    Ok(())
}

/// Called after the persistence writer has drained, never from a panic hook or `Drop`.
pub(crate) fn clean_exit() -> Result<()> {
    if let Some(recovery) = RECOVERY.lock().take() {
        recovery.own.remove_checkpoint()?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "session_recovery_tests.rs"]
mod tests;
