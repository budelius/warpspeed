use super::ReadOnlyPreferences;
use crate::user_preferences::UserPreferences;
use crate::user_preferences::in_memory::InMemoryPreferences;

#[test]
fn read_only_preferences_delegate_reads_without_mutating_the_inner_store() {
    let inner = InMemoryPreferences::default();
    inner.write_value("theme", "original".to_owned()).unwrap();
    let preferences = ReadOnlyPreferences::new(Box::new(inner));

    let error = preferences
        .write_value("theme", "changed".to_owned())
        .unwrap_err();

    let crate::user_preferences::Error::Unknown(error) = error else {
        panic!("unexpected preference error");
    };
    assert_eq!(
        error.to_string(),
        "preferences are read-only in a window worker"
    );
    assert_eq!(
        preferences.read_value("theme").unwrap(),
        Some("original".to_owned())
    );
}

#[test]
fn read_only_preferences_reject_removals() {
    let preferences = ReadOnlyPreferences::new(Box::new(InMemoryPreferences::default()));

    let error = preferences.remove_value("theme").unwrap_err();

    let crate::user_preferences::Error::Unknown(error) = error else {
        panic!("unexpected preference error");
    };
    assert_eq!(
        error.to_string(),
        "preferences are read-only in a window worker"
    );
}
