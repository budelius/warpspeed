use managed_agent_sessions::{
    StoznIdentity, begin_stozn_login, complete_stozn_login, logout_stozn, stored_stozn_identity,
};
use warpui::{Entity, ModelContext, SingletonEntity};

#[cfg(windows)]
fn open_framed_stozn_auth(verification_url: &str) -> bool {
    use command::blocking::Command;
    use std::path::PathBuf;

    let mut candidates = Vec::new();
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(local_app_data)
                .join("Programs")
                .join("Stozn")
                .join("Stozn.exe"),
        );
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        candidates.push(PathBuf::from(program_files).join("Stozn").join("Stozn.exe"));
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

#[derive(Clone, Debug)]
pub enum StoznAuthState {
    SignedOut,
    Starting,
    WaitingForBrowser { user_code: String },
    SignedIn(StoznIdentity),
    Failed(String),
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

    pub fn is_signed_in(&self) -> bool {
        matches!(self.state, StoznAuthState::SignedIn(_))
    }

    pub fn start_login(&mut self, ctx: &mut ModelContext<Self>) {
        if self.is_signed_in() {
            ctx.emit(StoznAuthEvent::LoginCompleted);
            return;
        }
        if matches!(
            self.state,
            StoznAuthState::Starting | StoznAuthState::WaitingForBrowser { .. }
        ) {
            return;
        }
        self.state = StoznAuthState::Starting;
        ctx.emit(StoznAuthEvent::StateChanged);
        ctx.notify();
        let _ = ctx.spawn(
            async move { blocking::unblock(begin_stozn_login).await },
            |me, result, ctx| match result {
                Ok(flow) => {
                    if !open_framed_stozn_auth(flow.verification_url()) {
                        ctx.open_url(flow.verification_url());
                    }
                    me.state = StoznAuthState::WaitingForBrowser {
                        user_code: flow.user_code().to_owned(),
                    };
                    ctx.emit(StoznAuthEvent::StateChanged);
                    ctx.notify();
                    let _ = ctx.spawn(
                        async move { blocking::unblock(move || complete_stozn_login(flow)).await },
                        |me, result, ctx| {
                            match result {
                                Ok(identity) => {
                                    me.state = StoznAuthState::SignedIn(identity);
                                    ctx.emit(StoznAuthEvent::LoginCompleted);
                                }
                                Err(error) => {
                                    me.state = StoznAuthState::Failed(format!("{error:#}"));
                                }
                            }
                            ctx.emit(StoznAuthEvent::StateChanged);
                            ctx.notify();
                        },
                    );
                }
                Err(error) => {
                    me.state = StoznAuthState::Failed(format!("{error:#}"));
                    ctx.emit(StoznAuthEvent::StateChanged);
                    ctx.notify();
                }
            },
        );
    }

    pub fn logout(&mut self, ctx: &mut ModelContext<Self>) {
        match logout_stozn() {
            Ok(()) => self.state = StoznAuthState::SignedOut,
            Err(error) => self.state = StoznAuthState::Failed(format!("{error:#}")),
        }
        ctx.emit(StoznAuthEvent::StateChanged);
        ctx.notify();
    }
}

impl Entity for StoznAuthManager {
    type Event = StoznAuthEvent;
}

impl SingletonEntity for StoznAuthManager {}
