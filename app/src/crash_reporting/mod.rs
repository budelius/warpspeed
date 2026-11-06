#[cfg(all(target_os = "macos", feature = "cocoa_sentry"))]
mod mac;
#[cfg(linux_or_windows)]
mod sentry_minidump;

#[cfg(target_os = "linux")]
mod linux;
mod ojiichan;

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::io::ErrorKind;
use std::ops::DerefMut;
use std::path::PathBuf;
use std::sync::Arc;

use lazy_static::lazy_static;
use parking_lot::{Mutex, RwLock};
use regex::Regex;
use sentry::{ClientInitGuard, IntoDsn, SessionMode};
#[cfg(linux_or_windows)]
pub use sentry_minidump::run_server as run_minidump_server;
use warp_core::channel::Channel;
use warp_server_auth::anonymous_id::get_or_create_anonymous_id;
use warpui::r#async::block_on;
use warpui::rendering::GPUDeviceInfo;
use warpui::windowing::state::ApplicationStage;
use warpui::windowing::{self, StateEvent, WindowManager};
use warpui::{AppContext, SingletonEntity};

use crate::antivirus::{AntivirusInfo, AntivirusInfoEvent};
use crate::auth::{AuthStateProvider, UserUid};
use crate::channel::ChannelState;
use crate::features::FeatureFlag;
use crate::settings::{PrivacySettings, PrivacySettingsChangedEvent};

pub(super) const HEADLESS_PROCESS_TAG: &str = "warp.headless_process";
pub(super) const HEADLESS_EXIT_REASON_TAG: &str = "warp.headless_exit_reason";
pub(super) const HEADLESS_HAD_LIVE_SESSIONS_TAG: &str = "warp.headless_had_live_sessions";

const HEADLESS_CRASH_REPORTING_MARKER: &str = "headless-crash-reporting-enabled";
const TERMINAL_SESSION_SUPERVISOR: &str = "terminal_session_supervisor";

lazy_static! {
    /// The RAII guard returned by the call to initialize the Rust Sentry client must be kept in
    /// scope. When it is destroyed, Sentry monitoring ceases.
    static ref RUST_SENTRY_CLIENT_GUARD: Mutex<RustSentryClientGuard> =
        Mutex::new(RustSentryClientGuard::Uninitialized);

    /// A map from sensitive error messages to "scrubbed" error messages.
    static ref ERROR_MESSAGES_TO_SCRUB: Vec<(Regex, &'static str)> = vec![
         // The following are panic messages for invalid string slicing.
         // See here for source: https://cs.github.com/rust-lang/rust/blob/9c0bc3028a575eece6d4e8fbc6624cb95b9c9893/library/core/src/str/mod.rs?q=%22byte+index+%22+repo%3Arust-lang%2Frust#L100.
        (Regex::new(r"byte index .+ is out of bounds.+").unwrap(), "byte index is out of bounds"),
        (Regex::new(r"byte index .+ is not a char boundary.+").unwrap(), "byte index is not a char boundary"),
        (Regex::new(r"begin <= end .+ when slicing.+").unwrap(), "begin <= end when slicing"),
    ];

    /// The current [`ApplicationStage`] of the application. Used when reporting the
    /// `warp.application_stage` tag to Sentry.
    static ref APPLICATION_LIFECYCLE_STAGE: RwLock<ApplicationStage> = RwLock::new(ApplicationStage::Starting);

    /// The set of tags that we want to attach to all Sentry reports.
    static ref TAGS: RwLock<HashMap<String, String>> = Default::default();
}

/// Sets a kv-pair as a Sentry tag for the rest of the application's lifetime.
pub(crate) fn set_tag<'a, 'b>(key: impl Into<Cow<'a, str>>, value: impl Into<Cow<'b, str>>) {
    set_tag_internal(key.into(), value.into());
}

/// Non-generic internal implementation of [`set_tag`].
fn set_tag_internal(key: Cow<'_, str>, value: Cow<'_, str>) {
    // Avoid setting tags with empty values, as Sentry doesn't allow them.
    if value.is_empty() {
        return;
    }

    #[cfg(linux_or_windows)]
    sentry_minidump::set_tag(key.clone().into_owned(), value.clone().into_owned());

    #[cfg(all(target_os = "macos", feature = "cocoa_sentry"))]
    mac::set_tag(key.as_ref(), value.as_ref());

    TAGS.write().insert(key.into_owned(), value.into_owned());
}

/// Sets the [`GPUDeviceInfo`] of the last opened window for use in logging as a Sentry tag.
pub(crate) fn set_gpu_device_info(gpu_device_info: GPUDeviceInfo) {
    for (key, value) in gpu_device_info.to_sentry_tags() {
        set_tag(key, value);
    }
}

/// Sets the [`AntivirusInfo`] for use in logging as a Sentry tag.
/// Only reports the first detected antivirus product.
pub fn set_antivirus_info(antivirus_info: &AntivirusInfo) {
    for (key, value) in antivirus_info.to_sentry_tags() {
        set_tag(key, value);
    }
}

/// Sets the current application lifecycle stage, for use in logging as a Sentry tag.
fn set_lifecycle_stage(stage: ApplicationStage) {
    #[cfg(linux_or_windows)]
    sentry_minidump::set_tags_from(&stage);

    *APPLICATION_LIFECYCLE_STAGE.write() = stage;
}

/// Sets the detected virtual environment info, for use in logging as a Sentry
/// tag.
fn set_virtual_environment(env: Option<VirtualEnvironment>) {
    for (key, value) in env.to_sentry_tags() {
        set_tag(key, value);
    }
}

fn set_windowing_system(windowing_system: Option<windowing::System>) {
    for (key, value) in windowing_system.to_sentry_tags() {
        set_tag(key, value);
    }
}

/// Checks if crash reporting is currently enabled.
fn is_crash_reporting_enabled(ctx: &mut AppContext) -> bool {
    PrivacySettings::handle(ctx)
        .as_ref(ctx)
        .is_crash_reporting_enabled
}

fn headless_crash_reporting_marker() -> Option<PathBuf> {
    let state_dir = warp_core::paths::state_dir();
    (!state_dir.as_os_str().is_empty()).then(|| state_dir.join(HEADLESS_CRASH_REPORTING_MARKER))
}

fn sync_headless_crash_reporting_marker(enabled: bool) {
    if !is_warpspeed() {
        return;
    }
    let Some(marker) = headless_crash_reporting_marker() else {
        return;
    };
    let result = if enabled {
        marker
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .and_then(|_| std::fs::write(marker, []))
    } else {
        std::fs::remove_file(marker).or_else(|error| {
            if error.kind() == ErrorKind::NotFound {
                Ok(())
            } else {
                Err(error)
            }
        })
    };
    if let Err(error) = result {
        log::warn!("Failed to update headless crash reporting preference: {error}");
    }
}

fn is_headless_crash_reporting_enabled() -> bool {
    headless_crash_reporting_marker().is_some_and(|marker| marker.is_file())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalSupervisorExit {
    ControlChannelClosed { had_live_sessions: bool },
    Error,
}

#[derive(Default)]
struct CrashRecoveryMetadata {
    /// Whether the Sentry event was previously _unhandled_.
    was_unhandled_event: bool,
    /// Whether the crash recovery process is currently running, indicating that an unhandled event
    /// should actually be marked as handled.
    is_crash_recovery_process_running: bool,
}

impl CrashRecoveryMetadata {
    #[cfg(enable_crash_recovery)]
    fn new() -> Self {
        Self {
            was_unhandled_event: false,
            is_crash_recovery_process_running:
                crate::crash_recovery::is_crash_recovery_process_running(),
        }
    }

    #[cfg(not(enable_crash_recovery))]
    fn new() -> Self {
        Self {
            was_unhandled_event: false,
            is_crash_recovery_process_running: false,
        }
    }

    fn was_unhandled_event(&mut self) {
        self.was_unhandled_event = true;
    }
}

impl ToSentryTags for CrashRecoveryMetadata {
    fn to_sentry_tags(&self) -> impl IntoIterator<Item = (&str, String)> {
        #[cfg(enable_crash_recovery)]
        {
            /// Converts a `bool` that is meant to be a Sentry value to it's string representation.
            /// Sentry uses `yes` or `no` as the value for booleans, so we follow that convention.
            fn bool_to_sentry_value(value: bool) -> String {
                let sentry_value = if value { "yes" } else { "no" };
                sentry_value.into()
            }

            [
                (
                    "warp.crash_recovery_process.running",
                    bool_to_sentry_value(self.is_crash_recovery_process_running),
                ),
                (
                    "warp.handled_by_crash_recovery_process",
                    bool_to_sentry_value(self.was_unhandled_event),
                ),
            ]
        }

        #[cfg(not(enable_crash_recovery))]
        std::iter::empty()
    }
}

/// Initializes the crash reporting subsystem.  Returns whether or not crash
/// reporting is active.
pub(crate) fn init(ctx: &mut AppContext) -> bool {
    if !ChannelState::is_crash_reporting_available() {
        sync_headless_crash_reporting_marker(false);
        return false;
    }
    if !FeatureFlag::CrashReporting.is_enabled() {
        sync_headless_crash_reporting_marker(false);
        log::info!("Crash reporting FeatureFlag is disabled; not initializing sentry.");
        return false;
    }

    let window_manager = WindowManager::handle(ctx);
    ctx.subscribe_to_model(&window_manager, |_, event, _| match event {
        StateEvent::ValueChanged { current, previous } => {
            if current.stage != previous.stage {
                set_lifecycle_stage(current.stage);
            }
        }
    });

    let antivirus_info = AntivirusInfo::handle(ctx);
    ctx.subscribe_to_model(&antivirus_info, |antivirus_info, event, ctx| match event {
        AntivirusInfoEvent::ScannedComplete => {
            let antivirus_info = antivirus_info.as_ref(ctx);
            set_antivirus_info(antivirus_info);
        }
    });

    let is_crash_reporting_enabled = is_crash_reporting_enabled(ctx);
    sync_headless_crash_reporting_marker(is_crash_reporting_enabled);

    if is_crash_reporting_enabled {
        AuthStateProvider::handle(ctx).update(ctx, |auth_state_provider, ctx| {
            init_sentry(
                auth_state_provider.get().user_id(),
                auth_state_provider.get().user_email(),
                ctx,
            );
        });
    } else {
        log::info!("Crash reporting setting is disabled; not initializing sentry.");
    }

    set_windowing_system(ctx.windows().windowing_system());

    let privacy_settings = PrivacySettings::handle(ctx);
    ctx.subscribe_to_model(&privacy_settings, |_, event, ctx| {
        if let &PrivacySettingsChangedEvent::UpdateIsCrashReportingEnabled { new_value, .. } = event
        {
            sync_headless_crash_reporting_marker(new_value);
            if new_value {
                AuthStateProvider::handle(ctx).update(ctx, |auth_state_provider, ctx| {
                    init_sentry(
                        auth_state_provider.get().user_id(),
                        auth_state_provider.get().user_email(),
                        ctx,
                    );
                });
            } else {
                uninit_sentry();
            }
        }
    });

    // Having initialized the SDK above, we can now set the initial value of
    // some tags.
    set_lifecycle_stage(window_manager.as_ref(ctx).stage());
    init_virtual_environment_tag(ctx);

    is_crash_reporting_enabled
}

#[derive(Default)]
enum RustSentryClientGuard {
    #[default]
    Uninitialized,
    Initialized {
        _guard: ClientInitGuard,
    },
}

fn is_warpspeed() -> bool {
    ChannelState::app_id()
        .application_name()
        .starts_with("WarpSpeed")
}

pub(crate) fn report_interrupted_session(revision: &str) {
    if is_warpspeed() && is_initialized() {
        sentry::capture_event(sentry::protocol::Event {
            message: Some("WarpSpeed interrupted session recovered".to_owned()),
            release: Some(revision.to_owned().into()),
            level: sentry::Level::Error,
            ..Default::default()
        });
    }
}

/// Returns the environment used when reporting events to Sentry.
/// This is the name of the operating system followed by the channel name (i.e. "linux_dev_release").
fn get_environment() -> Cow<'static, str> {
    let channel = ChannelState::channel();

    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            let operating_system = "mac";
        } else if #[cfg(target_os = "linux")] {
            let operating_system = "linux";
        }
        else if #[cfg(target_os = "windows")] {
            let operating_system = "windows";
        } else {
           let operating_system = "";
        }
    };

    let base_environment_name = match channel {
        Channel::Stable => "stable_release",
        Channel::Preview => "preview_release",
        Channel::Local => "local",
        Channel::Integration => "integration_test",
        Channel::Dev => "dev_release",
        Channel::Oss => "oss_release",
    };

    if operating_system.is_empty() {
        base_environment_name.into()
    } else {
        format!("{operating_system}_{base_environment_name}").into()
    }
}

fn scrub_message(message: &mut String) {
    for (regex, replacement) in ERROR_MESSAGES_TO_SCRUB.iter() {
        if regex.is_match(message) {
            *message = format!("(REDACTED) {replacement}");
            return;
        }
    }
}

fn configured_sentry_client_options(require_headless_marker: bool) -> sentry::ClientOptions {
    let mut sentry_options = sentry_client_options();
    sentry_options.before_send = Some(Arc::new(move |mut event| {
        if require_headless_marker {
            if !is_headless_crash_reporting_enabled() {
                return None;
            }
            event.tags.insert(
                HEADLESS_PROCESS_TAG.to_owned(),
                TERMINAL_SESSION_SUPERVISOR.to_owned(),
            );
        }

        let mut crash_recovery_metadata = CrashRecoveryMetadata::new();

        for exception in event.exception.iter_mut() {
            exception.value.as_mut().map(scrub_message);

            // Native crash recovery will handle this exception. Marking it handled prevents a
            // duplicate unhandled event while retaining the Rust diagnostic.
            if crash_recovery_metadata.is_crash_recovery_process_running
                && let Some(mechanism) = exception.mechanism.as_mut()
            {
                if let Some(false) = mechanism.handled {
                    crash_recovery_metadata.was_unhandled_event();
                }

                mechanism.handled = Some(true);
            }
        }

        for (k, v) in APPLICATION_LIFECYCLE_STAGE.read().to_sentry_tags() {
            event.tags.insert(k.to_string(), v);
        }
        for (k, v) in TAGS.read().iter() {
            event.tags.insert(k.clone(), v.clone());
        }

        Some(event)
    }));
    if is_warpspeed() {
        ojiichan::configure(&mut sentry_options);
        sentry_options.release = Some(env!("WARP_BUILD_REVISION").into());
    }
    sentry_options
}

pub(crate) fn init_terminal_supervisor_crash_reporting() -> bool {
    if !is_warpspeed()
        || !ChannelState::is_crash_reporting_available()
        || !FeatureFlag::CrashReporting.is_enabled()
        || !is_headless_crash_reporting_enabled()
    {
        return false;
    }

    let mut sentry_options = configured_sentry_client_options(true);
    sentry_options.auto_session_tracking = false;
    sentry_options.attach_stacktrace = false;
    *RUST_SENTRY_CLIENT_GUARD.lock() = RustSentryClientGuard::Initialized {
        _guard: sentry::init(sentry_options),
    };
    true
}

pub(crate) fn report_terminal_supervisor_exit(exit: TerminalSupervisorExit) {
    if !is_initialized() {
        return;
    }

    let (reason, had_live_sessions) = match exit {
        TerminalSupervisorExit::ControlChannelClosed { had_live_sessions } => (
            "control_channel_closed",
            if had_live_sessions { "yes" } else { "no" },
        ),
        TerminalSupervisorExit::Error => ("error", "unknown"),
    };
    sentry::with_scope(
        |scope| {
            scope.set_tag(HEADLESS_EXIT_REASON_TAG, reason);
            scope.set_tag(HEADLESS_HAD_LIVE_SESSIONS_TAG, had_live_sessions);
        },
        || {
            sentry::capture_message(
                "Terminal session supervisor exited unexpectedly",
                sentry::Level::Error,
            );
        },
    );
}

/// Initializes Rust and Cocoa Sentry, which hooks into the panic handler for the rust app and
/// uncaught exception handler of the mac runtime, respectively.
///
/// This must be called from the main thread to capture panics/crashes across the entire
/// application.
fn init_sentry(user_id: Option<UserUid>, email: Option<String>, ctx: &mut AppContext) {
    let key = release_version();

    let environment = Some(get_environment());

    log::info!("Initializing crash reporting {environment:?} with tag {key:?}...");

    fn before_breadcrumb(crumb: sentry::Breadcrumb) -> Option<sentry::Breadcrumb> {
        #[cfg(linux_or_windows)]
        sentry_minidump::forward_breadcrumb(crumb.clone());
        #[cfg(all(target_os = "macos", feature = "cocoa_sentry"))]
        mac::forward_breadcrumb(&crumb);

        Some(crumb)
    }

    let mut sentry_options = configured_sentry_client_options(false);
    sentry_options.before_breadcrumb = Some(Arc::new(Box::new(before_breadcrumb)));

    *RUST_SENTRY_CLIENT_GUARD.lock() = RustSentryClientGuard::Initialized {
        _guard: sentry::init(sentry_options),
    };

    // Native minidumps can contain terminal buffers and credentials. Interrupted instances are
    // reported from their recovery checkpoint on the next launch instead.
    if is_warpspeed() {
        return;
    }

    // Initialize the appropriate native Sentry SDK.
    #[cfg(enable_crash_recovery)]
    {
        use crate::crash_recovery::{CrashRecovery, is_crash_recovery_process_running};

        // If the crash recovery process is running, defer initialization of Sentry native until the
        // crash recovery process is torn down. Unlike Sentry Rust, we can't easily mark events as
        // handled before they are sent to Sentry. Instead, we defer initialization to avoid
        // erroneously reporting crashes when they would be successfully handled by the crash
        // recovery process.
        if is_crash_recovery_process_running() {
            ctx.subscribe_to_model(&CrashRecovery::handle(ctx), |_handle, event, ctx| {
                if matches!(
                    event,
                    crate::crash_recovery::Event::CrashRecoveryProcessTornDown
                ) {
                    log::info!("Initializing Sentry native");
                    sentry_minidump::init();

                    let auth_state_provider = crate::AuthStateProvider::handle(ctx).as_ref(ctx);
                    let auth_state = auth_state_provider.get();
                    let user_id = auth_state.user_id();
                    let email = auth_state.user_email();
                    set_optional_user_information(user_id, email, ctx);
                }
            });
        } else {
            sentry_minidump::init()
        }
    }

    #[cfg(target_os = "macos")]
    if FeatureFlag::CocoaSentry.is_enabled() {
        init_cocoa_sentry();
    }

    set_optional_user_information(user_id, email, ctx);
}

/// Baseline Sentry client options.
fn sentry_client_options() -> sentry::ClientOptions {
    sentry::ClientOptions {
        dsn: ChannelState::sentry_url()
            .into_dsn()
            .expect("Invalid Sentry DSN"),

        release: Some(release_version().into()),
        environment: Some(get_environment()),
        auto_session_tracking: true,
        session_mode: SessionMode::Application,
        ..Default::default()
    }
}

/// Returns whether the Rust Sentry client is currently initialized.
pub(crate) fn is_initialized() -> bool {
    matches!(
        &*RUST_SENTRY_CLIENT_GUARD.lock(),
        RustSentryClientGuard::Initialized { .. }
    )
}

pub(crate) fn uninit_terminal_supervisor_crash_reporting() {
    let client_guard = std::mem::take(RUST_SENTRY_CLIENT_GUARD.lock().deref_mut());
    std::mem::drop(client_guard);
}

/// Uninitializes sentry, effectively ending reporting on crashes and errors.
pub fn uninit_sentry() {
    // Take the client guard out of the mutex, replacing it with
    // `Uninitialized`.
    let client_guard = std::mem::take(RUST_SENTRY_CLIENT_GUARD.lock().deref_mut());
    if matches!(client_guard, RustSentryClientGuard::Initialized { .. }) {
        log::info!("Uninitializing crash reporting...");

        #[cfg(linux_or_windows)]
        sentry_minidump::uninit();
        #[cfg(target_os = "macos")]
        if FeatureFlag::CocoaSentry.is_enabled() {
            uninit_cocoa_sentry();
        }

        // Drop the client guard, uninitializing the Sentry Rust SDK.
        std::mem::drop(client_guard);
    }
}

/// Initializes sentry hooking into the uncaught exception handler of the mac runtime
/// which allows us to catch errors within obj-c.
pub fn init_cocoa_sentry() {
    #[cfg(all(target_os = "macos", feature = "cocoa_sentry"))]
    {
        mac::init_cocoa_sentry();

        for (k, v) in TAGS.read().iter() {
            mac::set_tag(k, v);
        }
    }
}

pub fn uninit_cocoa_sentry() {
    #[cfg(all(target_os = "macos", feature = "cocoa_sentry"))]
    mac::uninit_cocoa_sentry();
}

pub fn crash() {
    #[cfg(linux_or_windows)]
    sentry_minidump::crash();
    #[cfg(all(target_os = "macos", feature = "cocoa_sentry"))]
    mac::crash();
}

/// Sets the user id if `Some`, otherwise sets the current user ID to be an anonymous ID indicating
/// the user hasn't logged in yet.
fn set_optional_user_information(
    user_id: Option<UserUid>,
    email: Option<String>,
    ctx: &mut AppContext,
) {
    if is_warpspeed() {
        return;
    }
    let user_id = user_id.map(|uid| uid.as_string()).unwrap_or_else(|| {
        // If the user isn't signed in, set an anonymous ID.  This allows us to
        // compute more accurate crash-free user metrics.
        let anonymous_id = get_or_create_anonymous_id(ctx);
        format!("anon.{anonymous_id}")
    });
    // Only send along emails if we're on WarpDev.
    // We try to keep PII out of Sentry as much as possible.
    let email = if ChannelState::channel() == Channel::Dev {
        email
    } else {
        None
    };

    // Set user for Rust sentry.
    sentry::configure_scope(|scope| {
        scope.set_user(Some(sentry::User {
            id: Some(user_id.clone()),
            email,
            ip_address: None,
            username: None,
            other: BTreeMap::new(),
        }));
    });

    #[cfg(linux_or_windows)]
    sentry_minidump::set_user_id(user_id.as_str());
    #[cfg(all(target_os = "macos", feature = "cocoa_sentry"))]
    mac::set_user_id(user_id.as_str());
}

pub fn set_user_id(user_id: UserUid, email: Option<String>, ctx: &mut AppContext) {
    // On macOS, Sentry will error if we try to set a user without initializing the SDK.
    // If crash reporting was disabled, but the user enables it later, we'll set user info as part of initialization.
    if matches!(
        &*RUST_SENTRY_CLIENT_GUARD.lock(),
        RustSentryClientGuard::Initialized { .. }
    ) {
        set_optional_user_information(Some(user_id), email, ctx);
    } else {
        log::info!("Sentry is not initialized; not setting Sentry user info");
    }
}

fn release_version() -> &'static str {
    ChannelState::app_version().unwrap_or("<no tag>")
}

/// Sets the warp.client_type Sentry tag.
pub fn set_client_type_tag(client_id: &str) {
    set_tag("warp.client_type", client_id);
}

/// Initializes the warp.virtual_env Sentry tag group.
fn init_virtual_environment_tag(ctx: &mut AppContext) {
    let (tx, rx) = async_channel::unbounded();

    // Compute the virtual environment in a background thread, as we don't want
    // to block application startup at all.
    std::thread::spawn(move || {
        let virt_env = VirtualEnvironment::detect();
        let _ = block_on(tx.send(virt_env));
    });
    // Once we've computed the value, we want to update the primary Sentry hub,
    // which means calling `set_virtual_environment` from the main thread.
    ctx.foreground_executor()
        .spawn(async move {
            if let Ok(virt_env) = rx.recv().await {
                set_virtual_environment(virt_env);
            }
        })
        .detach();
}

/// Represents a virtualized environment that the operating system is running
/// under.
#[derive(Clone)]
struct VirtualEnvironment {
    name: String,
}

impl VirtualEnvironment {
    /// Detects the current virtual environment, if any.
    fn detect() -> Option<Self> {
        cfg_if::cfg_if! {
            if #[cfg(target_os = "linux")] {
                linux::get_virtualized_environment()
            } else {
                None
            }
        }
    }
}

trait ToSentryTags {
    fn to_sentry_tags(&self) -> impl IntoIterator<Item = (&str, String)>;
}

impl ToSentryTags for ApplicationStage {
    fn to_sentry_tags(&self) -> impl IntoIterator<Item = (&str, String)> {
        [("warp.application_stage", self.to_string())]
    }
}

impl ToSentryTags for GPUDeviceInfo {
    fn to_sentry_tags(&self) -> impl IntoIterator<Item = (&str, String)> {
        [
            ("warp.gpu.device.name", self.device_name.to_string()),
            ("warp.gpu.device.type", self.device_type.to_string()),
            ("warp.gpu.backend", self.backend.to_string()),
            ("warp.gpu.driver.name", self.driver_name.to_string()),
            ("warp.gpu.driver.info", self.driver_info.to_string()),
        ]
    }
}

impl ToSentryTags for Option<VirtualEnvironment> {
    fn to_sentry_tags(&self) -> impl IntoIterator<Item = (&str, String)> {
        let env = self.clone();
        [(
            "warp.virtual_env.name",
            env.map(|env| env.name).unwrap_or_else(|| "none".to_owned()),
        )]
    }
}

impl ToSentryTags for Option<windowing::System> {
    fn to_sentry_tags(&self) -> impl IntoIterator<Item = (&str, String)> {
        [(
            "warp.window.system",
            self.as_ref()
                .map(|windowing_system| windowing_system.to_string())
                .unwrap_or_else(|| "unknown".to_owned()),
        )]
    }
}

impl ToSentryTags for &AntivirusInfo {
    fn to_sentry_tags(&self) -> impl IntoIterator<Item = (&str, String)> {
        [(
            "warp.window.antivirus.name",
            self.get().unwrap_or("none").into(),
        )]
    }
}
