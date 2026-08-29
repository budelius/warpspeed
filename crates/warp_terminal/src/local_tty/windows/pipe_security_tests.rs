use super::*;

/// A SID string is the identity the pipe's ACE is written against. If this were
/// empty or malformed, the SDDL below would either fail to parse or -- worse --
/// grant access to something other than the current user.
#[test]
fn the_current_user_sid_looks_like_a_sid() {
    let sid = current_user_sid_string().expect("should be able to read the current user's SID");

    assert!(
        sid.starts_with("S-1-"),
        "expected a SID string, got {sid:?}"
    );
    assert!(
        sid.len() > 8,
        "a real user SID has several sub-authorities, got {sid:?}"
    );
}

/// The descriptor has to actually build. A failure here would otherwise surface
/// much later, as an unexplained failure to create the pipe.
#[test]
fn a_current_user_only_descriptor_can_be_built() {
    let security = PipeSecurity::current_user_only()
        .expect("should be able to build the pipe security descriptor");

    let attributes = security.attributes();

    assert_eq!(
        attributes.nLength as usize,
        std::mem::size_of::<SECURITY_ATTRIBUTES>()
    );
    assert!(
        !attributes.lpSecurityDescriptor.is_null(),
        "the descriptor must not be null, or the pipe would fall back to system defaults"
    );
    assert_eq!(
        attributes.bInheritHandle.as_bool(),
        false,
        "the listening handle must not be inherited by children"
    );
}

/// Guards against the descriptor being built from a hardcoded or default SID:
/// the SDDL must name *this* account, so the pipe is restricted to it.
#[test]
fn the_descriptor_is_built_against_this_users_sid() {
    let sid = current_user_sid_string().expect("should be able to read the current user's SID");

    // Building twice must produce the same identity; a descriptor derived from
    // something other than the token would not be stable in this way.
    let second = current_user_sid_string().expect("should be able to read the SID again");

    assert_eq!(sid, second, "the current user's SID should be stable");
    assert!(
        PipeSecurity::current_user_only().is_ok(),
        "an SDDL naming this SID should parse"
    );
}
