use super::{ForgeKind, RepoRef};

#[test]
fn repo_ref_parses_an_https_remote() {
    let parsed = RepoRef::from_remote_url("https://github.com/warpdotdev/Warp.git").unwrap();
    assert_eq!(parsed.host, "github.com");
    assert_eq!(parsed.owner, "warpdotdev");
    assert_eq!(parsed.name, "Warp");
}

/// This repository has one remote of each form, so both have to parse.
#[test]
fn repo_ref_parses_an_scp_style_ssh_remote() {
    let parsed = RepoRef::from_remote_url("git@gitlab.com:martin-wieser/warpspeed.git").unwrap();
    assert_eq!(parsed.host, "gitlab.com");
    assert_eq!(parsed.owner, "martin-wieser");
    assert_eq!(parsed.name, "warpspeed");
}

#[test]
fn repo_ref_parses_an_ssh_url_with_a_port() {
    let parsed = RepoRef::from_remote_url("ssh://git@git.example.com:2222/owner/name.git").unwrap();
    assert_eq!(parsed.host, "git.example.com");
    assert_eq!(parsed.owner, "owner");
    assert_eq!(parsed.name, "name");
}

/// GitLab nests projects under groups and subgroups, so the owner is a path,
/// not a single segment. Taking only the first segment would address the wrong
/// project.
#[test]
fn repo_ref_keeps_a_nested_gitlab_namespace() {
    let parsed = RepoRef::from_remote_url("https://gitlab.com/group/subgroup/project.git").unwrap();
    assert_eq!(parsed.host, "gitlab.com");
    assert_eq!(parsed.owner, "group/subgroup");
    assert_eq!(parsed.name, "project");
}

#[test]
fn repo_ref_tolerates_a_missing_git_suffix_and_a_trailing_slash() {
    let parsed = RepoRef::from_remote_url("https://github.com/owner/name/").unwrap();
    assert_eq!(parsed.owner, "owner");
    assert_eq!(parsed.name, "name");
}

#[test]
fn repo_ref_rejects_a_url_with_no_project_path() {
    assert!(RepoRef::from_remote_url("https://github.com/").is_none());
    assert!(RepoRef::from_remote_url("https://github.com/owner").is_none());
    assert!(RepoRef::from_remote_url("not a url at all").is_none());
    assert!(RepoRef::from_remote_url("").is_none());
}

/// A port is part of how you reach the host, so it survives into the API base
/// URL even though it is not part of the host name used for credential lookup.
#[test]
fn repo_ref_reports_the_port_separately_from_the_host() {
    let parsed = RepoRef::from_remote_url("https://git.example.com:8443/owner/name.git").unwrap();
    assert_eq!(parsed.host, "git.example.com");
    assert_eq!(parsed.port, Some(8443));
}

#[test]
fn forge_kind_is_known_for_the_public_hosts() {
    assert_eq!(
        ForgeKind::for_known_host("github.com"),
        Some(ForgeKind::GitHub)
    );
    assert_eq!(
        ForgeKind::for_known_host("gitlab.com"),
        Some(ForgeKind::GitLab)
    );
    assert_eq!(
        ForgeKind::for_known_host("git-tm.tourist-mobile.com"),
        None,
        "a self-hosted host is not knowable by name and must be probed"
    );
}
