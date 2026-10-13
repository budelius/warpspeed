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
