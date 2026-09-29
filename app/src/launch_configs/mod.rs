// Cloning a session copies config files and checkpoints on disk, and its only
// caller is the `local_fs` half of `Workspace::clone_session_in_use`. Gating the
// module keeps it from compiling as dead code on targets without a filesystem,
// which is a `-D warnings` failure on wasm.
#[cfg(feature = "local_fs")]
pub mod clone_session;
pub mod launch_config;
pub mod save_modal;
// The dialog names `session_configs::SessionOwner`, and `session_configs` only
// exists under `local_fs`, so on wasm this module would not compile at all.
#[cfg(feature = "local_fs")]
pub mod session_in_use_dialog;
pub mod session_store;

use warpui::AppContext;

pub fn init(app: &mut AppContext) {
    save_modal::init(app);
    init_session_in_use_dialog(app);
}

#[cfg(feature = "local_fs")]
fn init_session_in_use_dialog(app: &mut AppContext) {
    session_in_use_dialog::init(app);
}

#[cfg(not(feature = "local_fs"))]
fn init_session_in_use_dialog(_app: &mut AppContext) {}
