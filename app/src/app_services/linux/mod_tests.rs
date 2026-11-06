use super::*;

#[test]
fn profile_scoped_dbus_paths_are_distinct_and_valid() {
    let first = application_service_path_for(
        "dev.warp.WarpSpeed-first",
        Some("6465762e776172702e5761727053706565642d6669727374".to_owned()),
    );
    let second = application_service_path_for(
        "dev.warp.WarpSpeed-second",
        Some("6465762e776172702e5761727053706565642d7365636f6e64".to_owned()),
    );

    assert_ne!(first, second);
    assert!(
        first
            .split('/')
            .filter(|component| !component.is_empty())
            .all(|component| component
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_'))
    );
}

#[test]
fn bundled_dbus_path_keeps_the_channel_identity() {
    assert_eq!(
        application_service_path_for("dev.warp.WarpLocal", None),
        "/dev/warp/WarpLocal"
    );
}
