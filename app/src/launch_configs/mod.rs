pub mod clone_session;
pub mod launch_config;
pub mod save_modal;
pub mod session_store;

use warpui::AppContext;

pub fn init(app: &mut AppContext) {
    save_modal::init(app);
}
