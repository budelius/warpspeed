use super::*;

#[test]
fn mutex_and_pipe_names_are_scoped_by_profile() {
    let first = "6465762e776172702e5761727053706565642d6669727374";
    let second = "6465762e776172702e5761727053706565642d7365636f6e64";

    assert_ne!(
        single_instance_mutex_name_for_scope(first),
        single_instance_mutex_name_for_scope(second)
    );
    assert_ne!(
        uri_named_pipe_name_for_scope(first),
        uri_named_pipe_name_for_scope(second)
    );
    assert!(single_instance_mutex_name_for_scope(first).starts_with("Local\\"));
}
