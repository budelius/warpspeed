use super::*;

#[test]
fn wsl_env_allowlist_includes_client_version_without_notifications_flag() {
    let _guard = FeatureFlag::HOANotifications.override_enabled(false);

    let wslenv = wsl_env_allowlist(false).to_string_lossy().into_owned();

    assert_eq!(
        wslenv.split(':').collect::<Vec<_>>(),
        vec![
            format!("{HONOR_PS1_NAME}/u"),
            format!("{USE_SSH_WRAPPER_NAME}/u"),
            format!("{SSH_REUSE_CONTROL_MASTER_NAME}/u"),
            format!("{SHELL_DEBUG_MODE_NAME}/u"),
            format!("{TERM_PROGRAM_NAME}/u"),
            format!("{IS_LOCAL_SESSION_NAME}/u"),
            format!("{SSH_SOCKET_DIR}/u"),
            format!("{WARP_CLIENT_VERSION_ENV}/u"),
            format!("{TERMINAL_SESSION_UUID_ENV}/u"),
            format!("{FOCUS_URL_ENV}/u"),
            format!("{PROMPT_NODE_VERSION_ENABLED_NAME}/u"),
            format!("{PROMPT_GIT_METADATA_ENABLED_NAME}/u"),
        ],
    );
}

#[test]
fn wsl_env_allowlist_includes_cli_agent_protocol_when_notifications_flag_is_enabled() {
    let _guard = FeatureFlag::HOANotifications.override_enabled(true);

    let wslenv = wsl_env_allowlist(true).to_string_lossy().into_owned();

    assert_eq!(
        wslenv.split(':').collect::<Vec<_>>(),
        vec![
            format!("{HONOR_PS1_NAME}/u"),
            format!("{USE_SSH_WRAPPER_NAME}/u"),
            format!("{SSH_REUSE_CONTROL_MASTER_NAME}/u"),
            format!("{SHELL_DEBUG_MODE_NAME}/u"),
            format!("{TERM_PROGRAM_NAME}/u"),
            format!("{IS_LOCAL_SESSION_NAME}/u"),
            format!("{SSH_SOCKET_DIR}/u"),
            format!("{WARP_CLIENT_VERSION_ENV}/u"),
            format!("{TERMINAL_SESSION_UUID_ENV}/u"),
            format!("{FOCUS_URL_ENV}/u"),
            format!("{PROMPT_NODE_VERSION_ENABLED_NAME}/u"),
            format!("{PROMPT_GIT_METADATA_ENABLED_NAME}/u"),
            format!("{WARP_CLI_AGENT_PROTOCOL_VERSION_ENV}/u"),
            format!("{INITIAL_WORKING_DIR_NAME}/pu"),
        ],
    );
}

fn entry(key: &str, value: &str) -> (OsString, EnvEntry) {
    (
        map_key(key.into()),
        EnvEntry {
            preferred_key: key.into(),
            value: value.into(),
        },
    )
}

#[test]
fn inherited_agent_event_vars_are_dropped_when_the_caller_sets_none() {
    let mut env = BTreeMap::from([
        entry("Warp_Agent_Event_Token", "fedcba9876543210fedcba9876543210"),
        entry(WARP_AGENT_EVENT_BIN_ENV, r"C:\Old\warpspeed.exe"),
        entry("PATH", r"C:\Windows"),
    ]);

    apply_caller_env_vars(&mut env, &HashMap::new());

    assert!(!env.contains_key(&map_key(WARP_AGENT_EVENT_TOKEN_ENV.into())));
    assert!(!env.contains_key(&map_key(WARP_AGENT_EVENT_BIN_ENV.into())));
    assert!(env.contains_key(&map_key("PATH".into())));
}

#[test]
fn caller_agent_event_vars_replace_inherited_ones() {
    let mut env = BTreeMap::from([entry(
        WARP_AGENT_EVENT_TOKEN_ENV,
        "fedcba9876543210fedcba9876543210",
    )]);
    let overrides = HashMap::from([(
        OsString::from(WARP_AGENT_EVENT_TOKEN_ENV),
        OsString::from("0123456789abcdef0123456789abcdef"),
    )]);

    apply_caller_env_vars(&mut env, &overrides);

    assert_eq!(
        env[&map_key(WARP_AGENT_EVENT_TOKEN_ENV.into())].value,
        OsString::from("0123456789abcdef0123456789abcdef")
    );
}
