use std::fs;

use warp::cmd_or_ctrl_shift;
use warp::integration_testing::launch_configs::{
    WarpConfig, session_configs_dir, tab_configs_dir, use_test_config_directory,
};
use warp::integration_testing::step::new_step_with_default_assertions;
use warp::integration_testing::terminal::wait_until_bootstrapped_single_pane_for_tab;
use warp::launch_configs::save_modal::LaunchConfigSaveModal;
use warp_core::channel::{Channel, ChannelState};
use warpui_core::integration::TestStep;
use warpui_core::{SingletonEntity, async_assert};

use super::new_builder;
use crate::Builder;

const SESSION_FILE: &str = "save-target-picker.toml";
const TAB_FILE: &str = "save-target-picker-shared.toml";
const SESSION: &str = r#"version = 1
id = "save-target-picker"
name = "Picker Session"
generation = 7
[[tabs]]
id = "original"
config = "save-target-picker-shared.toml"
"#;
const TAB: &str = r#"name = "Picker Shared Tab"
[[panes]]
id = "main"
type = "terminal"
"#;

pub fn test_save_configuration_target_picker() -> Builder {
    new_builder()
        .with_setup(|utils| {
            use_test_config_directory(utils.test_dir().join("authored-configs"));
            assert_eq!(ChannelState::channel(), Channel::Integration);
            assert!(
                !ChannelState::app_id()
                    .application_name()
                    .starts_with("WarpSpeed")
            );
            fs::create_dir_all(session_configs_dir()).unwrap();
            fs::create_dir_all(tab_configs_dir()).unwrap();
            fs::write(session_configs_dir().join(SESSION_FILE), SESSION).unwrap();
            fs::write(tab_configs_dir().join(TAB_FILE), TAB).unwrap();
        })
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(
            TestStep::new("Session target is available").add_named_assertion(
                "Session config is loaded",
                |app, _| {
                    let loaded = app.read(|ctx| {
                        WarpConfig::as_ref(ctx)
                            .launch_and_session_configs()
                            .any(|config| config.name == "Picker Session")
                    });
                    async_assert!(loaded, "Expected the isolated session fixture to be loaded")
                },
            ),
        )
        .with_step(
            new_step_with_default_assertions("Open Save through the command palette")
                .with_keystrokes(&[cmd_or_ctrl_shift("p")])
                .with_typed_characters(&["save or update session or launch configuration"])
                .with_keystrokes(&["enter"]),
        )
        .with_step(
            new_step_with_default_assertions("Select an existing session")
                .with_click_on_saved_position("save_modal:target")
                .with_typed_characters(&["Session: Picker Session"])
                .with_keystrokes(&["enter"])
                .add_named_assertion("Selection identifies the TOML", |app, window_id| {
                    let selected = app
                        .views_of_type::<LaunchConfigSaveModal>(window_id)
                        .unwrap_or_default()
                        .first()
                        .is_some_and(|view| {
                            view.read(app, |modal, _| {
                                modal.selected_target().is_some_and(|source| {
                                    source.path.ends_with(SESSION_FILE)
                                        && source.document_index == 7
                                })
                            })
                        });
                    async_assert!(selected, "Expected session target, not a new YAML export")
                })
                .add_named_assertion("Selecting a target must not write it", |_, _| {
                    async_assert!(
                        fs::read_to_string(session_configs_dir().join(SESSION_FILE)).unwrap()
                            == SESSION,
                        "Selecting a target must not overwrite it"
                    )
                }),
        )
        .with_step(
            new_step_with_default_assertions("Update the chosen session")
                .with_keystrokes(&["enter"]),
        )
        .with_step(
            TestStep::new("Only the chosen template is updated")
                .add_named_assertion("Session uses private tab templates", |_, _| {
                    let content =
                        fs::read_to_string(session_configs_dir().join(SESSION_FILE)).unwrap();
                    async_assert!(
                        content.contains("saved-session-"),
                        "Expected private snapshot tab templates"
                    )
                })
                .add_named_assertion("Session generation is unchanged", |_, _| {
                    let content =
                        fs::read_to_string(session_configs_dir().join(SESSION_FILE)).unwrap();
                    async_assert!(
                        content.contains("generation = 7"),
                        "Session generation must not change"
                    )
                })
                .add_named_assertion("Shared tab template is unchanged", |_, _| {
                    async_assert!(
                        fs::read_to_string(tab_configs_dir().join(TAB_FILE)).unwrap() == TAB,
                        "Shared tab template was modified"
                    )
                }),
        )
}
