use super::{FailureType, LaunchConfigSaveAction, SaveState};

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

/// Replacing an existing configuration destroys what is there, so it never happens on enter.
#[test]
fn enter_never_overwrites() {
    for state in [
        SaveState::NotSaved,
        SaveState::Success,
        SaveState::Failure(FailureType::FileAlreadyExists),
        SaveState::Failure(FailureType::Other),
    ] {
        assert_ne!(
            LaunchConfigSaveAction::from_state(&state),
            LaunchConfigSaveAction::Overwrite
        );
    }
}

#[test]
fn enter_closes_a_failure_that_cannot_be_retried() {
    assert_eq!(
        LaunchConfigSaveAction::from_state(&SaveState::Failure(FailureType::Other)),
        LaunchConfigSaveAction::Close
    );
}
