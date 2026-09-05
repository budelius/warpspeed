use super::*;

#[test]
fn a_label_names_what_ran_where_and_when() {
    assert_eq!(
        label_for("Claude Code", "pwsh", "C:\\src\\warp", 90),
        "Claude Code — C:\\src\\warp (1m ago)"
    );
}

#[test]
fn a_label_falls_back_to_the_shell_when_there_is_no_title() {
    assert_eq!(
        label_for("", "bash", "/home/me", 0),
        "bash — /home/me (just now)"
    );
}

/// A session detached by an app that died mid-flight hands over nothing, and
/// still has to be selectable.
#[test]
fn a_label_survives_metadata_that_is_entirely_empty() {
    assert_eq!(label_for("", "", "", 7200), "Terminal session (2h ago)");
}

#[test]
fn an_age_uses_the_coarsest_unit_that_still_says_something() {
    assert_eq!(humanize_age(59), "just now");
    assert_eq!(humanize_age(60), "1m ago");
    assert_eq!(humanize_age(3599), "59m ago");
    assert_eq!(humanize_age(3600), "1h ago");
    assert_eq!(humanize_age(86_399), "23h ago");
    assert_eq!(humanize_age(86_400), "1d ago");
}
