// WarpSpeed -- the standalone build this fork is named for.
//
// Why this binary exists: the `dev`, `local`, `stable`, and `preview` binaries load their
// ChannelConfig at runtime from the private `warp-channel-config` generator, which needs SSH
// access to warpdotdev/warp-channel-config. Without that access they panic at startup. Only
// `oss.rs` carries an inline config and runs standalone -- but it never enables DOGFOOD_FLAGS,
// so `FeatureFlag::LazyShellStartup` would be off and the build could not exercise the feature.
//
// This mirrors oss.rs's inline-config approach with three changes:
//   * a distinct AppId, so it gets its own data directory and runs alongside an installed Warp
//     instead of sharing its settings, database, and session state;
//   * DOGFOOD + PREVIEW flags enabled unconditionally, so LazyShellStartup is actually on;
//   * the AppId varies with elevation and with WARPSPEED_PROFILE, so an elevated instance uses a
//     separate database from a normal one instead of contending for the same SQLite files.
//
// Multiple instances run concurrently because `SingleInstanceManager` is gated behind the
// `release_bundle` cargo feature, which this build does not enable. Instances sharing one AppId
// still share one database; use WARPSPEED_PROFILE to separate them.
//
// This is the fork's entry point: `cargo run -p warp --bin warpspeed`. It is the only binary
// in the tree that both builds without warpdotdev SSH access and runs with dogfood flags on,
// which is what makes the fork's in-progress features reachable at all.
#![cfg_attr(feature = "release_bundle", windows_subsystem = "windows")]

use anyhow::Result;
use warp_core::channel::{Channel, ChannelConfig, ChannelState, OzConfig, WarpServerConfig};
use warp_core::{AppId, features};

/// Whether this process is running with an elevated token.
///
/// Returns `false` on any failure: a wrong answer here only picks a data directory, and the
/// non-elevated one is the safe default.
#[cfg(windows)]
fn is_elevated() -> bool {
    use std::mem::size_of;

    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = Default::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
        .is_ok();

        let _ = CloseHandle(token);

        ok && elevation.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
fn is_elevated() -> bool {
    false
}

/// Picks the application name, which is what separates data directories.
///
/// * `WARPSPEED_PROFILE=foo` -> `WarpSpeed-foo`, for running isolated instances side by side.
/// * elevated, no profile set -> `WarpSpeedAdmin`, so an admin instance never shares a database
///   with a normal one. Sharing would mean two processes on one SQLite file, and files written
///   by the elevated process that the normal one may not be able to rewrite.
/// * otherwise -> `WarpSpeed`.
fn application_name() -> String {
    match std::env::var("WARPSPEED_PROFILE") {
        Ok(profile) if !profile.trim().is_empty() => {
            let profile: String = profile
                .trim()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            format!("WarpSpeed-{profile}")
        }
        _ if is_elevated() => "WarpSpeedAdmin".to_owned(),
        _ => "WarpSpeed".to_owned(),
    }
}

fn main() -> Result<()> {
    let application_name = application_name();

    // Printed so `warpspeed.exe` run from a console says which database it is about to use.
    // The GUI subsystem swallows this in release_bundle builds, which this build is not.
    //
    // On stderr, not stdout: subcommands such as `warpspeed sessions panes --json`
    // write machine-readable output, and a banner ahead of it makes the result
    // unparseable for the scripts that surface exists to serve.
    eprintln!(
        "WarpSpeed starting: app_id={application_name} elevated={}",
        is_elevated()
    );

    let state = ChannelState::new(
        Channel::Local,
        ChannelConfig {
            app_id: AppId::new("dev", "warp", application_name.clone()),
            logfile_name: format!("{}.log", application_name.to_lowercase()).into(),
            server_config: WarpServerConfig::production(),
            oz_config: OzConfig::production(),
            telemetry_config: None,
            crash_reporting_config: None,
            autoupdate_config: None,
            mcp_static_config: None,
        },
    )
    .with_additional_features(features::DOGFOOD_FLAGS)
    .with_additional_features(features::PREVIEW_FLAGS);

    ChannelState::set(state);

    warp::run()
}
