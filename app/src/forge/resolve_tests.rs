use super::preferred_remote;

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// This repository has `origin` on GitHub and `gitlab` on GitLab, so which
/// remote answers is a real choice, not a formality.
#[test]
fn origin_wins_when_a_repository_has_several_remotes() {
    assert_eq!(
        preferred_remote(&names(&["gitlab", "origin", "upstream"])),
        Some("origin")
    );
}

#[test]
fn upstream_is_preferred_over_an_unrecognised_remote() {
    assert_eq!(
        preferred_remote(&names(&["fork", "upstream"])),
        Some("upstream")
    );
}

/// A single remote needs no preference list: it is the only answer available.
#[test]
fn a_lone_remote_is_used_whatever_it_is_called() {
    assert_eq!(preferred_remote(&names(&["glab-base"])), Some("glab-base"));
}

/// Guessing between several unrecognised remotes would mean querying the wrong
/// forge, so nothing is chosen and the caller falls back to `gh`.
#[test]
fn several_unrecognised_remotes_yield_no_choice() {
    assert_eq!(preferred_remote(&names(&["fork", "mirror"])), None);
}

#[test]
fn no_remotes_yield_no_choice() {
    assert_eq!(preferred_remote(&[]), None);
}
