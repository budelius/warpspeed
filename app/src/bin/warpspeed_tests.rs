use super::application_name_for;

#[test]
fn explicit_profiles_are_sanitized_and_isolated() {
    let sanitized = application_name_for(Some(" project/a "), false);
    assert!(sanitized.starts_with("WarpSpeed-projecta-"));
    assert_ne!(sanitized, application_name_for(Some("projecta"), false));
    let project_b = application_name_for(Some("project-b"), false);
    assert!(project_b.starts_with("WarpSpeed-project-b-"));
    assert_eq!(project_b, application_name_for(Some("project-b"), false));
    assert_ne!(
        application_name_for(Some("project-a"), false),
        application_name_for(Some("project-b"), false)
    );
}

#[test]
fn nonempty_profiles_never_collapse_to_the_default_profile() {
    let punctuation = application_name_for(Some("///"), false);
    let unicode = application_name_for(Some("💥"), false);
    let whitespace = application_name_for(Some("  "), true);
    assert!(punctuation.starts_with("WarpSpeed-profile-"));
    assert!(unicode.starts_with("WarpSpeed-profile-"));
    assert!(whitespace.starts_with("WarpSpeed-profile-"));
    assert_ne!(punctuation, unicode);
    assert_ne!(punctuation, whitespace);
    assert_ne!(punctuation, "WarpSpeed");
    assert_ne!(whitespace, "WarpSpeedAdmin");
    assert_ne!(
        application_name_for(Some("profile"), false).to_ascii_lowercase(),
        application_name_for(Some("PROFILE"), false).to_ascii_lowercase()
    );
    let long_a = format!("{}a", "x".repeat(1_000));
    let long_b = format!("{}b", "x".repeat(1_000));
    assert_ne!(
        application_name_for(Some(&long_a), false),
        application_name_for(Some(&long_b), false)
    );
    assert!(application_name_for(Some(&long_a), false).len() < 100);
    assert_eq!(application_name_for(None, false), "WarpSpeed");
    assert_eq!(application_name_for(None, true), "WarpSpeedAdmin");
}
