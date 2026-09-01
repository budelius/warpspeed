use std::sync::LazyLock;

pub mod clipboard;
mod network;
mod registry;
mod snap_layouts;
mod system_caption_buttons;
mod window_attribute;
mod window_ext;

pub use clipboard::*;
pub use network::*;
pub use registry::*;
// Only the seam the rest of the crate uses. The subclass's decision logic stays private to the
// module that acts on it.
pub use snap_layouts::{ButtonRect, SnapLayoutsState, install, to_physical};
pub use system_caption_buttons::*;
pub use window_attribute::*;
pub use window_ext::WindowExt;

/// The first Windows 11 build. Rounded window corners and Snap Layouts both arrived with it.
pub const WINDOWS_11_BUILD: u32 = 22000;

static WINDOWS_VERSION: LazyLock<windows_version::OsVersion> =
    LazyLock::new(windows_version::OsVersion::current);

/// Whether this process is running on Windows 11 or later.
pub fn is_windows_11() -> bool {
    WINDOWS_VERSION.build >= WINDOWS_11_BUILD
}
