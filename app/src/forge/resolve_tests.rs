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

/// Sends a real request to api.github.com using the machine's own credential,
/// so it is ignored by default and never runs in CI. Run it by hand to confirm
/// the whole path works end to end -- remote parsing, credential resolution,
/// the transport's host guard, the request, and the parse:
///
/// ```text
/// cargo test -p warp --lib forge::resolve -- --ignored --nocapture
/// ```
///
/// It reads a public repository and prints no credential.
#[test]
#[ignore = "makes a real request to api.github.com"]
fn repository_info_round_trips_against_github() {
    // `CARGO_MANIFEST_DIR` is the `app` crate; its parent is the repository,
    // whose `origin` is https://github.com/warpdotdev/Warp.git.
    let repo_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("app crate has a parent directory")
        .to_owned();

    let info = tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(super::repository_info(&repo_path))
        .expect("repository lookup should succeed against GitHub");

    println!(
        "forge answered: name={:?} owner={:?} host={:?}",
        info.name, info.owner, info.host
    );
    // The forge reports the repository's *canonical* name, which need not match
    // the casing in the remote URL: `origin` spells it `Warp`, GitHub calls it
    // `warp`. `gh repo view --json name` returns the same canonical spelling, so
    // this matches what the shell path produced rather than diverging from it.
    assert!(
        info.name.eq_ignore_ascii_case("warp"),
        "unexpected repository name {:?}",
        info.name
    );
    assert_eq!(info.owner.as_deref(), Some("warpdotdev"));
    assert_eq!(info.host.as_deref(), Some("github.com"));
}

/// The pull request endpoint is a different URL, with a different response
/// shape, so verifying `/repos` says nothing about it. Ignored for the same
/// reason as the test above: it makes a real request.
///
/// ```text
/// cargo test -p warp --lib forge::resolve -- --ignored --nocapture
/// ```
///
/// The assertion is deliberately weak. This branch has no pull request on
/// `warpdotdev/Warp`, so the answer is `None` — but reaching `None` proves the
/// request was authorised and the empty list parsed, where an auth or shape
/// problem would have surfaced as an error instead.
#[test]
#[ignore = "makes a real request to api.github.com"]
fn pr_lookup_round_trips_against_github() {
    let repo_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("app crate has a parent directory")
        .to_owned();

    let outcome = tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(super::pr_for_branch(&repo_path));

    match outcome {
        Ok(pr) => println!("forge answered: pr={pr:?}"),
        Err(error) => panic!("the pull request lookup should have been authorised: {error}"),
    }
}
