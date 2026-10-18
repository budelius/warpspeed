//! Functionality relating to services that the application provides
//! to the host system.
//!
//! For example, on macOS, this module sets up integrations with
//! Finder such that the user can open a new Warp tab or window
//! in a given directory.

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub mod linux;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
pub mod windows;

#[cfg(any(windows, target_os = "linux", target_os = "freebsd", test))]
use url::Url;
use warpui::AppContext;
#[cfg(any(windows, target_os = "linux", target_os = "freebsd"))]
use warpui::SingletonEntity as _;

#[cfg(any(windows, target_os = "linux", target_os = "freebsd"))]
use crate::view_components::DismissibleToast;
#[cfg(any(windows, target_os = "linux", target_os = "freebsd"))]
use crate::workspace::ToastStack;

#[cfg(any(windows, target_os = "linux", target_os = "freebsd", test))]
const FORWARDED_SOURCE_VERSION_QUERY_KEY: &str = "warp_forwarded_source_version";

fn is_warpspeed_app_id(app_id: &warp_core::AppId) -> bool {
    let application_name = app_id.application_name();
    matches!(application_name, "WarpSpeed" | "WarpSpeedAdmin")
        || application_name.starts_with("WarpSpeed-")
}

/// Whether this GUI build requires one primary process per effective AppId.
pub(crate) fn enforces_profile_ownership() -> bool {
    is_warpspeed_app_id(&warp_core::channel::ChannelState::app_id())
}

fn permits_window_process_handoff_for_app_id(app_id: &warp_core::AppId) -> bool {
    !is_warpspeed_app_id(app_id)
}

pub(crate) fn permits_window_process_handoff() -> bool {
    permits_window_process_handoff_for_app_id(&warp_core::channel::ChannelState::app_id())
}

#[cfg(any(windows, target_os = "linux", target_os = "freebsd", test))]
fn profile_ownership_scope_for_app_id(app_id: &warp_core::AppId) -> String {
    app_id
        .to_string()
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(any(windows, target_os = "linux", target_os = "freebsd", test))]
pub(crate) fn profile_ownership_scope() -> String {
    profile_ownership_scope_for_app_id(&warp_core::channel::ChannelState::app_id())
}

#[cfg(any(windows, target_os = "linux", target_os = "freebsd"))]
fn add_forwarded_source_version(urls: &mut [Url]) {
    if !enforces_profile_ownership() {
        return;
    }
    if let Some(url) = urls.first_mut() {
        url.query_pairs_mut().append_pair(
            FORWARDED_SOURCE_VERSION_QUERY_KEY,
            &crate::build_info::version_identity(),
        );
    }
}

#[cfg(any(windows, target_os = "linux", target_os = "freebsd", test))]
fn forwarded_source_version(urls: &[Url]) -> Option<String> {
    urls.iter().find_map(|url| {
        url.query_pairs()
            .find(|(key, _)| key == FORWARDED_SOURCE_VERSION_QUERY_KEY)
            .map(|(_, value)| value.into_owned())
    })
}

#[cfg(any(windows, target_os = "linux", target_os = "freebsd"))]
fn surface_forwarded_version_mismatch(urls: &[Url], ctx: &mut AppContext) {
    if !enforces_profile_ownership() {
        return;
    }
    let Some(source_version) = forwarded_source_version(urls) else {
        return;
    };
    if source_version == crate::build_info::version_identity() {
        return;
    }

    log::warn!("A launch from a different WarpSpeed version was forwarded to the profile owner");
    if let Some(window_id) = ctx.windows().active_window() {
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            toast_stack.add_ephemeral_toast(
                DismissibleToast::error(
                    "A different WarpSpeed version tried to open this profile. Restart WarpSpeed to switch versions."
                        .to_owned(),
                ),
                window_id,
                ctx,
            );
        });
    }
}

pub fn init(_ctx: &mut AppContext) {
    log::info!("Initializing app services");

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    linux::init(_ctx);
    #[cfg(target_os = "macos")]
    mac::init();
    #[cfg(windows)]
    windows::init(_ctx);
}

pub fn teardown(_ctx: &mut AppContext) {
    log::info!("Tearing down app services...");

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    linux::teardown(_ctx);
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
