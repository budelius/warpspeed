use crate::FeatureFlag;

/// Whether a terminal pane forks its login shell at construction time or parks
/// the inputs and forks on first activation.
///
/// Decided per tab whenever a window's tabs are built up front -- when a
/// launch configuration is opened, and when a saved session is restored.
/// Deliberately free of any `local_tty` gating so one policy type backs every
/// terminal manager implementation. `local_tty` is unconditionally active on
/// non-wasm targets (see `app/build.rs`), so on those platforms it is the
/// real `local_tty::TerminalManager` that parks and replays the shell start,
/// not `MockTerminalManager` — that mock only backs the wasm/`remote_tty`
/// configurations where `local_tty` is genuinely absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellStartPolicy {
    /// Fork the shell as part of building the terminal manager.
    Eager,
    /// Park the shell inputs; fork on the first `ensure_shell_started` call.
    Deferred,
}

impl ShellStartPolicy {
    /// The policy for every pane of one tab of a window being built up front.
    ///
    /// Named for its first caller, but the contract is general -- "is this the
    /// tab that will be visible" -- and session restore uses it too, so the
    /// name is narrower than the scope.
    ///
    /// `is_eager_tab` is true for the tab the window will activate. With
    /// [`FeatureFlag::LazyShellStartup`] disabled this always returns
    /// [`ShellStartPolicy::Eager`], preserving the pre-feature behaviour.
    pub fn for_launch_config_tab(is_eager_tab: bool) -> Self {
        if !FeatureFlag::LazyShellStartup.is_enabled() || is_eager_tab {
            Self::Eager
        } else {
            Self::Deferred
        }
    }

    /// Whether this policy parks the shell instead of forking it.
    pub fn is_deferred(self) -> bool {
        matches!(self, Self::Deferred)
    }
}

#[cfg(test)]
#[path = "shell_start_policy_tests.rs"]
mod tests;
