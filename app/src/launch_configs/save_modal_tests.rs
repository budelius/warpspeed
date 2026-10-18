use warpui::platform::WindowStyle;
use warpui::{App, SingletonEntity, TypedActionView};

use super::{
    ActionRequest, FailureType, LaunchConfigSaveAction, LaunchConfigSaveModal, SaveState,
    target_label,
};
use crate::launch_configs::launch_config::{
    LaunchConfigSource, make_mock_single_window_launch_config,
};
use crate::user_config::WarpConfig;
use crate::workspace::view::tests::initialize_app;

#[test]
fn enter_saves_before_anything_has_been_written() {
    assert_eq!(
        LaunchConfigSaveAction::from_state(&SaveState::NotSaved),
        LaunchConfigSaveAction::Save
    );
}

#[test]
fn enter_opens_the_file_once_the_config_is_written() {
    assert_eq!(
        LaunchConfigSaveAction::from_state(&SaveState::Success),
        LaunchConfigSaveAction::OpenFile
    );
}

/// A name clash leaves the editor up so the name can be corrected, so enter has to retry the
/// save rather than closing the modal and losing the typed name.
#[test]
fn enter_retries_the_save_after_a_name_clash() {
    assert_eq!(
        LaunchConfigSaveAction::from_state(&SaveState::Failure(FailureType::FileAlreadyExists)),
        LaunchConfigSaveAction::Save
    );
}

#[test]
fn enter_retries_after_a_failed_write() {
    assert_eq!(
        LaunchConfigSaveAction::from_state(&SaveState::Failure(FailureType::Other)),
        LaunchConfigSaveAction::Save
    );
}

#[test]
fn save_picker_distinguishes_sessions_and_each_launch_window_with_the_same_name() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let mut launch = make_mock_single_window_launch_config();
        launch.name = "IWAKAN".into();
        launch.windows.push(launch.windows[0].clone());
        launch.source = Some(LaunchConfigSource {
            path: "IWAKAN.yaml".into(),
            document_index: 2,
        });
        let mut session = launch.clone();
        session.windows.truncate(1);
        session.source = Some(LaunchConfigSource {
            path: "IWAKAN.toml".into(),
            document_index: 7,
        });
        app.update(|ctx| {
            WarpConfig::handle(ctx).update(ctx, |configs, ctx| {
                configs.append_launch_config(&launch, ctx);
                configs.append_launch_config(&session, ctx);
            });
        });
        let (_, modal) = app.add_window(WindowStyle::NotStealFocus, LaunchConfigSaveModal::new);
        modal.update(&mut app, |modal, ctx| {
            modal.request_snapshot(ctx.window_id(), session.window_source(0), ctx);
        });
        modal.read(&app, |modal, ctx| {
            assert_eq!(modal.selected_target(), session.window_source(0).as_ref());
            assert_eq!(modal.target_picker.as_ref(ctx).len(), 4);
            assert_eq!(
                modal.target_picker.as_ref(ctx).selected_item_label(),
                Some(target_label(&session.window_source(0).unwrap()))
            );
            assert_ne!(
                target_label(&launch.window_source(0).unwrap()),
                target_label(&launch.window_source(1).unwrap())
            );
        });
    });
}

#[test]
fn save_picker_switching_targets_clears_a_prior_save_result_without_writing() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let (_, modal) = app.add_window(WindowStyle::NotStealFocus, LaunchConfigSaveModal::new);
        let mut config = make_mock_single_window_launch_config();
        config.source = Some(LaunchConfigSource {
            path: "other.toml".into(),
            document_index: 1,
        });
        modal.update(&mut app, |modal, ctx| {
            modal.set_save_state(SaveState::Success, Some("previous.yaml".into()));
            modal.handle_action(
                &ActionRequest::Action(LaunchConfigSaveAction::SelectTarget(
                    config.window_source(0),
                )),
                ctx,
            );
            assert_eq!(modal.selected_target(), config.window_source(0).as_ref());
            assert!(modal.save_state == SaveState::NotSaved);
            assert!(modal.file_path.is_none());
            modal.handle_action(
                &ActionRequest::Action(LaunchConfigSaveAction::SelectTarget(None)),
                ctx,
            );
            assert_eq!(modal.selected_target(), None);
            assert!(modal.save_state == SaveState::NotSaved);
        });
    });
}

#[test]
fn save_picker_reopening_discards_the_previous_selection_and_result() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let (_, modal) = app.add_window(WindowStyle::NotStealFocus, LaunchConfigSaveModal::new);
        let mut config = make_mock_single_window_launch_config();
        config.source = Some(LaunchConfigSource {
            path: "other.toml".into(),
            document_index: 1,
        });
        modal.update(&mut app, |modal, ctx| {
            modal.request_snapshot(ctx.window_id(), config.window_source(0), ctx);
            modal.set_save_state(SaveState::Success, Some("previous.yaml".into()));
            modal.close(ctx);
            modal.request_snapshot(ctx.window_id(), None, ctx);
        });
        modal.read(&app, |modal, ctx| {
            assert_eq!(modal.selected_target(), None);
            assert!(modal.save_state == SaveState::NotSaved);
            assert!(modal.file_path.is_none());
            assert_eq!(
                modal
                    .target_picker
                    .as_ref(ctx)
                    .selected_item_label()
                    .as_deref(),
                Some("Save as new launch configuration (YAML)")
            );
        });
    });
}
