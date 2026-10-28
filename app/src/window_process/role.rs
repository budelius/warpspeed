use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Result, bail};

pub(crate) const WORKER_BOOTSTRAP_ENV: &str = "WARP_WINDOW_WORKER_BOOTSTRAP";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GuiProcessRole {
    LegacyOwner,
    Coordinator,
    WindowWorker { bootstrap_path: PathBuf },
}

impl GuiProcessRole {
    pub(crate) fn owns_profile(&self) -> bool {
        matches!(self, Self::LegacyOwner | Self::Coordinator)
    }

    pub(crate) fn resolve(
        isolation_enabled: bool,
        worker_bootstrap: Option<OsString>,
    ) -> Result<Self> {
        match (isolation_enabled, worker_bootstrap) {
            (false, None) => Ok(Self::LegacyOwner),
            (false, Some(_)) => bail!(
                "WarpSpeed window workers cannot start while window process isolation is disabled"
            ),
            (true, None) => Ok(Self::Coordinator),
            (true, Some(path)) if path.is_empty() => {
                bail!("WarpSpeed window worker bootstrap path is empty")
            }
            (true, Some(path)) => Ok(Self::WindowWorker {
                bootstrap_path: PathBuf::from(path),
            }),
        }
    }

    pub(crate) fn from_environment(isolation_enabled: bool) -> Result<Self> {
        #[cfg(not(target_family = "wasm"))]
        let worker_bootstrap = std::env::var_os(WORKER_BOOTSTRAP_ENV);
        #[cfg(target_family = "wasm")]
        let worker_bootstrap = None;

        #[cfg(not(target_family = "wasm"))]
        if worker_bootstrap.is_some() {
            // The capability is one-shot and must not reach shells or helper processes.
            unsafe { std::env::remove_var(WORKER_BOOTSTRAP_ENV) };
        }

        Self::resolve(isolation_enabled, worker_bootstrap)
    }
}

#[cfg(test)]
#[path = "role_tests.rs"]
mod tests;
