use managed_agent_sessions::{
    StoznIdentity, begin_stozn_login, complete_stozn_login, logout_stozn, stored_stozn_identity,
};
use warpui::{Entity, ModelContext, SingletonEntity};

#[cfg(windows)]
fn open_framed_stozn_auth(verification_url: &str) -> bool {
    use std::path::PathBuf;

    use command::blocking::Command;

    let mut candidates = Vec::new();
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let local_app_data = PathBuf::from(local_app_data);
        candidates.push(
            local_app_data
                .join("Vivaldi")
                .join("Application")
                .join("vivaldi.exe"),
        );
        candidates.push(
            local_app_data
                .join("Google")
                .join("Chrome")
                .join("Application")
                .join("chrome.exe"),
        );
        candidates.push(
            local_app_data
                .join("Microsoft")
                .join("Edge")
                .join("Application")
                .join("msedge.exe"),
        );
    }
    for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(program_files) = std::env::var_os(variable) {
            let program_files = PathBuf::from(program_files);
            candidates.push(
                program_files
                    .join("Google")
                    .join("Chrome")
                    .join("Application")
                    .join("chrome.exe"),
            );
            candidates.push(
                program_files
                    .join("Microsoft")
                    .join("Edge")
                    .join("Application")
                    .join("msedge.exe"),
            );
        }
    }

    candidates
        .into_iter()
        .filter(|path| path.is_file())
        .any(|browser| {
            let mut command = Command::new(browser);
            command
                .arg(format!("--app={verification_url}"))
                .arg("--new-window")
                .env_remove("STOZN_TOKEN")
                .env_remove("WARP_API_KEY");
            command.spawn().is_ok()
        })
}

#[cfg(not(windows))]
fn open_framed_stozn_auth(_verification_url: &str) -> bool {
    false
}

fn open_stozn_verification_url(verification_url: &str, ctx: &mut ModelContext<StoznAuthManager>) {
    if !open_framed_stozn_auth(verification_url) {
        ctx.open_url(verification_url);
    }
}

#[derive(Clone, Debug)]
pub enum StoznAuthState {
    SignedOut,
    Starting,
    WaitingForBrowser {
        user_code: String,
        verification_url: String,
    },
    SignedIn(StoznIdentity),
    Failed(String),
}

impl StoznAuthState {
    fn name(&self) -> &'static str {
        match self {
            StoznAuthState::SignedOut => "SignedOut",
            StoznAuthState::Starting => "Starting",
            StoznAuthState::WaitingForBrowser { .. } => "WaitingForBrowser",
            StoznAuthState::SignedIn(_) => "SignedIn",
            StoznAuthState::Failed(_) => "Failed",
        }
    }
}

/// User-facing description of a STOZN sign-in state, or `None` when there is nothing to report
/// (signed out).
pub fn stozn_status_text(state: &StoznAuthState) -> Option<String> {
    match state {
        StoznAuthState::SignedOut => None,
        StoznAuthState::Starting => Some("Starting STOZN sign-in…".to_owned()),
        StoznAuthState::WaitingForBrowser { user_code, .. } => Some(format!(
            "Confirm code {user_code} in your browser to finish signing in…"
        )),
        StoznAuthState::SignedIn(identity) => Some(format!(
            "Signed in to STOZN as {}.",
            identity
                .display_name
                .as_deref()
                .unwrap_or(&identity.subject)
        )),
        StoznAuthState::Failed(error) => Some(error.clone()),
    }
}

#[derive(Clone, Debug)]
pub enum StoznAuthEvent {
    StateChanged,
    LoginCompleted,
}

pub struct StoznAuthManager {
    state: StoznAuthState,
}

impl StoznAuthManager {
    pub fn new() -> Self {
        Self {
            state: stored_stozn_identity()
                .map(StoznAuthState::SignedIn)
                .unwrap_or(StoznAuthState::SignedOut),
        }
    }

    pub fn state(&self) -> &StoznAuthState {
        &self.state
    }

    fn set_state(&mut self, state: StoznAuthState, ctx: &mut ModelContext<Self>) {
        let previous = self.state.name();
        match &state {
            StoznAuthState::WaitingForBrowser { user_code, .. } => log::info!(
                "STOZN sign-in state {previous} -> WaitingForBrowser (user code {user_code})"
            ),
            StoznAuthState::Failed(error) => {
                log::warn!("STOZN sign-in state {previous} -> Failed: {error}")
            }
            StoznAuthState::SignedOut | StoznAuthState::Starting | StoznAuthState::SignedIn(_) => {
                log::info!("STOZN sign-in state {previous} -> {}", state.name())
            }
        }
        self.state = state;
        ctx.emit(StoznAuthEvent::StateChanged);
        ctx.notify();
    }

    /// Whether a stored sign-in exists. A cached sign-in whose credential was removed (for example
    /// after the issuer rejected a refresh) drops to signed out.
    pub fn is_signed_in(&mut self, ctx: &mut ModelContext<Self>) -> bool {
        if matches!(self.state, StoznAuthState::SignedIn(_)) && stored_stozn_identity().is_none() {
            log::info!("STOZN stored credential is gone; dropping cached sign-in");
            self.set_state(StoznAuthState::SignedOut, ctx);
        }
        matches!(self.state, StoznAuthState::SignedIn(_))
    }

    pub fn start_login(&mut self, ctx: &mut ModelContext<Self>) {
        if self.is_signed_in(ctx) {
            ctx.emit(StoznAuthEvent::LoginCompleted);
            return;
        }
        match &self.state {
            StoznAuthState::Starting => {
                log::info!("Ignoring STOZN sign-in request: a sign-in is already starting");
                return;
            }
            StoznAuthState::WaitingForBrowser {
                verification_url, ..
            } => {
                log::info!(
                    "STOZN sign-in already waiting for the browser; reopening the verification page"
                );
                open_stozn_verification_url(verification_url, ctx);
                return;
            }
            StoznAuthState::SignedOut | StoznAuthState::SignedIn(_) | StoznAuthState::Failed(_) => {
            }
        }
        self.set_state(StoznAuthState::Starting, ctx);
        let _ = ctx.spawn(
            async move { blocking::unblock(begin_stozn_login).await },
            |me, result, ctx| match result {
                Ok(flow) => {
                    open_stozn_verification_url(flow.verification_url(), ctx);
                    me.set_state(
                        StoznAuthState::WaitingForBrowser {
                            user_code: flow.user_code().to_owned(),
                            verification_url: flow.verification_url().to_owned(),
                        },
                        ctx,
                    );
                    let _ = ctx.spawn(
                        async move { blocking::unblock(move || complete_stozn_login(flow)).await },
                        |me, result, ctx| match result {
                            Ok(identity) => {
                                me.set_state(StoznAuthState::SignedIn(identity), ctx);
                                ctx.emit(StoznAuthEvent::LoginCompleted);
                            }
                            Err(error) => {
                                me.set_state(StoznAuthState::Failed(format!("{error:#}")), ctx)
                            }
                        },
                    );
                }
                Err(error) => me.set_state(StoznAuthState::Failed(format!("{error:#}")), ctx),
            },
        );
    }

    pub fn logout(&mut self, ctx: &mut ModelContext<Self>) {
        let state = match logout_stozn() {
            Ok(()) => StoznAuthState::SignedOut,
            Err(error) => StoznAuthState::Failed(format!("{error:#}")),
        };
        self.set_state(state, ctx);
    }
}

impl Entity for StoznAuthManager {
    type Event = StoznAuthEvent;
}

impl SingletonEntity for StoznAuthManager {}

#[cfg(test)]
#[path = "stozn_auth_tests.rs"]
mod tests;
