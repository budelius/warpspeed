use std::path::PathBuf;

use uuid::Uuid;
use warp_core::channel::{Channel, ChannelState};

pub use crate::user_config::{
    WarpConfig, launch_configs_dir, session_configs_dir, tab_configs_dir,
};

/// Uses a fresh authored-config directory under the test root. Windows AppData ignores temporary HOME.
pub fn use_test_config_directory(root: PathBuf) {
    assert_eq!(ChannelState::channel(), Channel::Integration);
    *crate::user_config::AUTHORED_CONFIG_TEST_DIR.lock() =
        Some(root.join(Uuid::new_v4().to_string()));
}
