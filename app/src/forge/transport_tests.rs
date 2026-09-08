use super::may_attach_credential;

#[test]
fn a_credential_travels_to_its_own_host() {
    assert!(may_attach_credential(
        "https://api.github.com/repos/o/r",
        "api.github.com"
    ));
}

/// Case in a hostname is not significant, so a differently-cased URL is still
/// the same host.
#[test]
fn host_matching_ignores_case() {
    assert!(may_attach_credential(
        "https://API.GitHub.com/repos/o/r",
        "api.github.com"
    ));
}

/// The rule that matters. The chain hands back whatever the machine holds,
/// often a broad git credential, and it must not reach a host it was never
/// issued for.
#[test]
fn a_credential_does_not_travel_to_another_host() {
    assert!(!may_attach_credential(
        "https://evil.example.com/repos/o/r",
        "api.github.com"
    ));
}

/// A subdomain is a different host and a different trust boundary. The caller
/// names the host it means rather than inheriting one.
#[test]
fn a_subdomain_does_not_inherit_the_parent_hosts_credential() {
    assert!(!may_attach_credential(
        "https://api.github.com/repos/o/r",
        "github.com"
    ));
    assert!(!may_attach_credential(
        "https://attacker.github.com.evil.example/x",
        "github.com"
    ));
}

/// A token must never travel in clear text, even to the right host.
#[test]
fn a_credential_never_travels_over_plain_http() {
    assert!(!may_attach_credential(
        "http://api.github.com/repos/o/r",
        "api.github.com"
    ));
}

#[test]
fn an_unparsable_url_gets_no_credential() {
    assert!(!may_attach_credential("not a url", "api.github.com"));
    assert!(!may_attach_credential("", "api.github.com"));
}

/// A host reached on a non-default port is a distinct authority, so the
/// credential host has to carry the port too. Matching on the bare hostname
/// would let a credential for `git.example.com` reach a different service on
/// `git.example.com:8443`.
#[test]
fn a_port_is_part_of_the_authority_match() {
    assert!(may_attach_credential(
        "https://git.example.com:8443/api/v3/repos/o/r",
        "git.example.com:8443"
    ));
    assert!(!may_attach_credential(
        "https://git.example.com:8443/api/v3/repos/o/r",
        "git.example.com"
    ));
    assert!(!may_attach_credential(
        "https://git.example.com/api/v3/repos/o/r",
        "git.example.com:8443"
    ));
}
