use managed_agent_sessions::StoznIdentity;

use super::{StoznAuthState, stozn_status_text};

fn identity(display_name: Option<&str>) -> StoznIdentity {
    StoznIdentity {
        subject: "user-123".to_owned(),
        role: "member".to_owned(),
        display_name: display_name.map(str::to_owned),
    }
}

#[test]
fn signed_out_has_no_status() {
    assert_eq!(stozn_status_text(&StoznAuthState::SignedOut), None);
}

#[test]
fn starting_status() {
    assert_eq!(
        stozn_status_text(&StoznAuthState::Starting).as_deref(),
        Some("Starting STOZN sign-in…")
    );
}

#[test]
fn waiting_for_browser_status_shows_user_code_only() {
    let state = StoznAuthState::WaitingForBrowser {
        user_code: "ABCD-EFGH".to_owned(),
        verification_url: "https://id.example/device?user_code=ABCD-EFGH".to_owned(),
    };
    assert_eq!(
        stozn_status_text(&state).as_deref(),
        Some("Confirm code ABCD-EFGH in your browser to finish signing in…")
    );
}

#[test]
fn signed_in_status_prefers_display_name() {
    assert_eq!(
        stozn_status_text(&StoznAuthState::SignedIn(identity(Some("Martin")))).as_deref(),
        Some("Signed in to STOZN as Martin.")
    );
}

#[test]
fn signed_in_status_falls_back_to_subject() {
    assert_eq!(
        stozn_status_text(&StoznAuthState::SignedIn(identity(None))).as_deref(),
        Some("Signed in to STOZN as user-123.")
    );
}

#[test]
fn failed_status_is_the_error_string() {
    let error = "STOZN sign-in failed: account lacks the required project role";
    assert_eq!(
        stozn_status_text(&StoznAuthState::Failed(error.to_owned())).as_deref(),
        Some(error)
    );
}
