// Cloning a session copies config files and checkpoints on disk, and its only
// caller is the `local_fs` half of `Workspace::clone_session_in_use`. Gating the
// module keeps it from compiling as dead code on targets without a filesystem,
// which is a `-D warnings` failure on wasm.
#[cfg(feature = "local_fs")]
pub mod clone_session;
pub mod launch_config;
pub mod save_modal;
pub mod session_in_use_dialog;
pub mod session_store;

use warpui::AppContext;

pub fn init(app: &mut AppContext) {
    save_modal::init(app);
    session_in_use_dialog::init(app);
}
