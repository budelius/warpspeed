use crate::FeatureFlag;

/// Whether a terminal pane forks its login shell at construction time or parks
/// the inputs and forks on first activation.
///
/// Decided per tab when a launch configuration is opened, then refined per pane
/// by [`ShellStartPolicy::refine_for_pane`]. Deliberately free of any
/// `local_tty` gating so the decision is observable in test builds, where
/// `MockTerminalManager` records it and no shell exists to fork.
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

    /// Narrows a tab-level policy for one pane.
    ///
    /// A pane with pending startup commands always starts eagerly: deferring it
    /// would raise the question of when those commands run and what the user
    /// sees until they do, and forcing it eager sidesteps that entirely.
    pub fn refine_for_pane(self, has_commands: bool) -> Self {
        if has_commands { Self::Eager } else { self }
    }

    /// Whether this policy parks the shell instead of forking it.
    pub fn is_deferred(self) -> bool {
        matches!(self, Self::Deferred)
    }
}

#[cfg(test)]
mod tests {
    use crate::FeatureFlag;
    use super::ShellStartPolicy;

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

    #[test]
    fn pending_commands_force_eager() {
        assert_eq!(
            ShellStartPolicy::Deferred.refine_for_pane(true),
            ShellStartPolicy::Eager
        );
        assert_eq!(
            ShellStartPolicy::Deferred.refine_for_pane(false),
            ShellStartPolicy::Deferred
        );
        assert_eq!(
            ShellStartPolicy::Eager.refine_for_pane(false),
            ShellStartPolicy::Eager
        );
    }
}
