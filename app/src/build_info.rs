//! What this build is, where it is running from, and which directories it uses.
//!
//! Every running Warp process is otherwise anonymous. `--version` reports
//! `GIT_RELEASE_TAG`, which only tagged release builds have, so a developer
//! build, a CI artifact and an installed build all answer `<unknown>` and the
//! only way to tell two of them apart is the mtime of the executable. Once
//! several are running at once -- a couple out of `target/`, one installed --
//! there is no way to ask any of them what it is.
//!
//! Reported through `warp --build-info`, included in `--dump-debug-info`, and
//! written to the log at startup so an already-running instance can be
//! identified after the fact.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use warp_core::channel::ChannelState;

/// The revision this binary was built from, stamped by `build.rs`.
const BUILD_REVISION: &str = env!("WARP_BUILD_REVISION");
/// When this binary was built, if the build passed `WARP_BUILD_TIMESTAMP`.
///
/// Usually empty. `built_at` prefers the executable's modification time, which
/// is the same fact without the cost -- see [`built_at`].
const BUILD_TIMESTAMP: &str = env!("WARP_BUILD_TIMESTAMP");

/// The directory name the Windows installer gives a version, as produced by
/// `script/windows/install-warpspeed.ps1`: `<timestamp>-<revision>[-dirty]`,
/// under `.../WarpSpeed/versions/`.
const INSTALLED_VERSIONS_DIR: &str = "versions";

fn unknown_if_empty(value: &str) -> &str {
    let value = value.trim();
    if value.is_empty() { "unknown" } else { value }
}

/// The installed version an executable belongs to, if it sits in the layout the
/// installer creates.
///
/// Returns `None` for a binary run straight out of `target/`, which is the
/// honest answer: such a build has no version identity beyond its revision.
fn installed_version(executable: &Path) -> Option<String> {
    let version_dir = executable.parent()?;
    let versions_dir = version_dir.parent()?;
    if versions_dir.file_name()? != INSTALLED_VERSIONS_DIR {
        return None;
    }
    Some(version_dir.file_name()?.to_string_lossy().into_owned())
}

/// Where this process is running from, and the directories it reads and writes.
struct RuntimePaths {
    executable: Option<PathBuf>,
    installed_version: Option<String>,
    launch_configs: PathBuf,
    data: PathBuf,
    state: PathBuf,
    config: PathBuf,
}

impl RuntimePaths {
    fn collect() -> Self {
        let executable = std::env::current_exe().ok();
        let installed_version = executable.as_deref().and_then(installed_version);
        Self {
            executable,
            installed_version,
            launch_configs: crate::user_config::launch_configs_dir(),
            data: warp_core::paths::data_dir(),
            state: warp_core::paths::state_dir(),
            config: warp_core::paths::base_config_dir(),
        }
    }
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

/// When this binary was built.
///
/// Taken from the executable's modification time rather than stamped in at
/// compile time. A timestamp baked into the binary has to come from an env var,
/// and `build.rs` declares `rerun-if-env-changed` on it, so a packaging script
/// passing a fresh timestamp would force a full rebuild and relink of the app
/// crate on every single build -- minutes, to record something the filesystem
/// already knows. The link time is the build time, and `Copy-Item` preserves it
/// when the installer stages a version, so this stays right for installed
/// copies too.
///
/// `WARP_BUILD_TIMESTAMP` still wins when a build explicitly sets it, for
/// packaging that wants to pin the value.
fn built_at(executable: Option<&Path>) -> String {
    let stamped = BUILD_TIMESTAMP.trim();
    if !stamped.is_empty() {
        return stamped.to_owned();
    }

    let Some(modified) = executable
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok())
    else {
        return String::from("unknown");
    };

    let seconds = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default();
    chrono::DateTime::from_timestamp(seconds, 0)
        .map(|timestamp| timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| String::from("unknown"))
}

/// A multi-line report identifying this build and where it runs.
///
/// Rendered as `key: value` lines rather than anything prettier because it is
/// pasted into bug reports and grepped.
pub(crate) fn report() -> String {
    let paths = RuntimePaths::collect();
    let mut report = String::new();

    let _ = writeln!(report, "Build");
    let _ = writeln!(
        report,
        "  release tag:   {}",
        ChannelState::app_version().unwrap_or("none (not a tagged release build)")
    );
    let _ = writeln!(report, "  revision:      {}", unknown_if_empty(BUILD_REVISION));
    let _ = writeln!(
        report,
        "  built at:      {}",
        built_at(paths.executable.as_deref())
    );
    let _ = writeln!(
        report,
        "  profile:       {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    let _ = writeln!(report, "  channel:       {:?}", ChannelState::channel());

    let _ = writeln!(report, "Running");
    let _ = writeln!(
        report,
        "  executable:    {}",
        paths
            .executable
            .as_deref()
            .map(display_path)
            .unwrap_or_else(|| String::from("unknown"))
    );
    let _ = writeln!(
        report,
        "  installed as:  {}",
        paths
            .installed_version
            .clone()
            .unwrap_or_else(|| String::from("not installed (running from a build directory)"))
    );
    let _ = writeln!(report, "  pid:           {}", std::process::id());

    let _ = writeln!(report, "Directories");
    let _ = writeln!(
        report,
        "  launch configs: {}",
        display_path(&paths.launch_configs)
    );
    let _ = writeln!(report, "  config:         {}", display_path(&paths.config));
    let _ = writeln!(report, "  data:           {}", display_path(&paths.data));
    let _ = writeln!(report, "  state:          {}", display_path(&paths.state));

    report
}

/// Prints the report; backs `warp --build-info`.
pub(crate) fn run() -> anyhow::Result<()> {
    print!("{}", report());
    Ok(())
}

/// Writes the report to the log, so an instance that is already running can
/// still be identified later.
pub(crate) fn log() {
    for line in report().lines() {
        log::info!("{line}");
    }
}

#[cfg(test)]
#[path = "build_info_tests.rs"]
mod tests;
