use crate::FeatureFlag;

/// Whether a terminal pane forks its login shell at construction time or parks
/// the inputs and forks on first activation.
///
/// Decided per tab when a launch configuration is opened. Deliberately free
/// of any `local_tty` gating so one policy type backs every terminal manager
/// implementation. `local_tty` is unconditionally active on non-wasm targets
/// (see `app/build.rs`), so on those platforms it is the real
/// `local_tty::TerminalManager` that parks and replays the shell start, not
/// `MockTerminalManager` — that mock only backs the wasm/`remote_tty`
/// configurations where `local_tty` is genuinely absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellStartPolicy {
    /// Fork the shell as part of building the terminal manager.
    Eager,
    /// Park the shell inputs; fork on the first `ensure_shell_started` call.
    Deferred,
}

impl ShellStartPolicy {
    /// The policy for every pane of one launch-configuration tab.
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
mod tests {
    use super::ShellStartPolicy;
    use crate::FeatureFlag;

    #[test]
    fn eager_tab_is_eager_and_background_tab_defers_when_flag_on() {
        let _flag = FeatureFlag::LazyShellStartup.override_enabled(true);
        assert_eq!(
            ShellStartPolicy::for_launch_config_tab(true),
            ShellStartPolicy::Eager
        );
        assert_eq!(
            ShellStartPolicy::for_launch_config_tab(false),
            ShellStartPolicy::Deferred
        );
    }

    #[test]
    fn every_tab_is_eager_when_flag_off() {
        let _flag = FeatureFlag::LazyShellStartup.override_enabled(false);
        assert_eq!(
            ShellStartPolicy::for_launch_config_tab(true),
            ShellStartPolicy::Eager
        );
        assert_eq!(
            ShellStartPolicy::for_launch_config_tab(false),
            ShellStartPolicy::Eager
        );
    }
}
