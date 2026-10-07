use settings::macros::define_settings_group;
use settings::{RespectUserSyncSetting, SupportedPlatforms, SyncToCloud};

define_settings_group!(AltScreenReporting, settings: [
    mouse_reporting_enabled: MouseReportingEnabled {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "terminal.mouse_reporting_enabled",
        description: "Whether to forward mouse events to full-screen terminal applications.",
    },
    scroll_reporting_enabled: ScrollReportingEnabled {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "terminal.scroll_reporting_enabled",
        description: "Whether to forward scroll events to full-screen terminal applications.",
    },
    right_click_reporting_enabled: RightClickReportingEnabled {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "terminal.right_click_reporting_enabled",
        description: "Whether to forward right-clicks to full-screen terminal applications that track the mouse. When disabled, right-click opens Warp's context menu (or pastes, per the right-click setting) even inside such applications.",
    },
    focus_reporting_enabled: FocusReportingEnabled {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "terminal.focus_reporting_enabled",
        description: "Whether to forward focus and blur events to full-screen terminal applications.",
    },
]);
