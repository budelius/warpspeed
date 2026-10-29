use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};

use anyhow::{Context, Result};

use super::bootstrap_file;
use super::protocol::WorkerBootstrap;
use super::role::WORKER_BOOTSTRAP_ENV;

pub(crate) struct StartingWorker {
    child: Child,
    bootstrap_path: PathBuf,
}

impl StartingWorker {
    pub(crate) fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
        self.child
            .try_wait()
            .context("failed to inspect starting window worker")
    }

    pub(crate) fn into_running(self) -> Child {
        self.child
    }

    pub(crate) fn abort(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.bootstrap_path);
    }
}

pub(crate) fn launch(state_dir: &Path, bootstrap: &WorkerBootstrap) -> Result<StartingWorker> {
    let executable = std::env::current_exe().context("failed to resolve WarpSpeed executable")?;
    launch_with_executable(&executable, state_dir, bootstrap)
}

fn launch_with_executable(
    executable: &Path,
    state_dir: &Path,
    bootstrap: &WorkerBootstrap,
) -> Result<StartingWorker> {
    let bootstrap_path = bootstrap_file::write(state_dir, bootstrap)?;
    let mut command = command::blocking::Command::new(executable);
    command
        .env(WORKER_BOOTSTRAP_ENV, &bootstrap_path)
        .env_remove("WARP_WINDOW_PROCESS_HANDOFF")
        .independent_process_group()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    let child = command.spawn_with_required_breakaway();
    #[cfg(not(windows))]
    let child = command.spawn();

    match child {
        Ok(child) => Ok(StartingWorker {
            child,
            bootstrap_path,
        }),
        Err(error) => {
            let _ = std::fs::remove_file(&bootstrap_path);
            Err(error).context("failed to launch isolated window worker")
        }
    }
}

#[cfg(test)]
#[path = "spawn_tests.rs"]
mod tests;
