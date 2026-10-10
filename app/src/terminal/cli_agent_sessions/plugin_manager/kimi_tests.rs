use toml_edit::DocumentMut;

use super::{
    HOOK_EVENTS, HOOKS_VERSION, MANUAL_HOOK_EXAMPLE, hook_command, hooks_need_update,
    installed_hooks_version, with_warp_hooks,
};

/// Kimi's `HooksConfigSchema` (`externalHooks/configSection.ts:10-21`, zod `.strict()`).
const KIMI_HOOK_KEYS: &[&str] = &["event", "matcher", "command", "timeout"];
/// Kimi's `HOOK_EVENT_TYPES` (`externalHooks/internal/types.ts:3-24`).
const KIMI_HOOK_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionResult",
    "UserPromptSubmit",
    "UserPromptQueued",
    "TurnStarted",
    "Stop",
    "StopFailure",
    "Interrupt",
    "SessionStart",
    "SessionEnd",
    "SessionHeartbeat",
    "SubagentStart",
    "SubagentStop",
    "TaskStarted",
    "PreCompact",
    "PostCompact",
    "Notification",
];

/// Fails where Kimi would ignore every hook in `config`.
fn assert_kimi_accepts(config: &str) {
    let document = config.parse::<DocumentMut>().expect("valid TOML");
    let hooks = document["hooks"]
        .as_array_of_tables()
        .expect("hooks are [[hooks]] tables");
    for hook in hooks.iter() {
        for (key, _) in hook.iter() {
            assert!(
                KIMI_HOOK_KEYS.contains(&key),
                "Kimi rejects hook key {key:?}"
            );
        }
        let event = hook["event"].as_str().expect("event is a string");
        assert!(KIMI_HOOK_EVENTS.contains(&event), "unknown event {event}");
        assert!(
            !hook["command"]
                .as_str()
                .expect("command is a string")
                .is_empty()
        );
        if let Some(timeout) = hook.get("timeout") {
            let timeout = timeout.as_integer().expect("timeout is an integer");
            assert!((1..=600).contains(&timeout), "timeout {timeout}");
        }
    }
}

/// The events of the hooks that run the current Warp command.
fn warp_hook_events(config: &str) -> Vec<String> {
    let document = config.parse::<DocumentMut>().unwrap();
    document["hooks"]
        .as_array_of_tables()
        .unwrap()
        .iter()
        .filter(|hook| hook["command"].as_str() == Some(hook_command().as_str()))
        .map(|hook| hook["event"].as_str().unwrap().to_owned())
        .collect()
}

const USER_CONFIG: &str = r#"# my settings
default_model = "kimi-k2"   # keep this comment

[[hooks]]
event = "PreToolUse"
matcher = "Shell"
command = "python ~/guard.py"
timeout = 10
"#;

#[test]
fn installs_one_hook_per_event_into_an_empty_config() {
    let config = with_warp_hooks("", &hook_command()).unwrap();
    assert_kimi_accepts(&config);
    assert_eq!(warp_hook_events(&config), HOOK_EVENTS);
    assert_eq!(
        installed_hooks_version(&config).as_deref(),
        Some(HOOKS_VERSION)
    );
    assert!(!hooks_need_update(&config));
}

#[test]
fn keeps_the_users_settings_hooks_and_comments() {
    let config = with_warp_hooks(USER_CONFIG, &hook_command()).unwrap();
    assert!(
        config.starts_with("# my settings\ndefault_model = \"kimi-k2\"   # keep this comment\n"),
        "{config}"
    );
    assert!(
        config.contains(
            "[[hooks]]\nevent = \"PreToolUse\"\nmatcher = \"Shell\"\ncommand = \"python ~/guard.py\"\ntimeout = 10\n"
        ),
        "{config}"
    );
    assert_kimi_accepts(&config);
    assert_eq!(warp_hook_events(&config), HOOK_EVENTS);
}

#[test]
fn installing_twice_changes_nothing() {
    let once = with_warp_hooks(USER_CONFIG, &hook_command()).unwrap();
    let twice = with_warp_hooks(&once, &hook_command()).unwrap();
    assert_eq!(once, twice);
}

#[test]
fn updating_replaces_older_warp_hooks_and_keeps_others() {
    let old = concat!(
        "[[hooks]]\n",
        "event = \"Stop\"\n",
        "command = \"\\\"$WARP_AGENT_EVENT_BIN\\\" agent-event --format kimi-hook --hooks-version 0.9.0\"\n",
        "timeout = 5\n",
        "\n",
        "[[hooks]]\n",
        "event = \"Notification\"\n",
        "command = \"notify-send kimi\"\n",
    );
    assert_eq!(installed_hooks_version(old).as_deref(), Some("0.9.0"));
    assert!(hooks_need_update(old));

    let config = with_warp_hooks(old, &hook_command()).unwrap();

    assert!(!config.contains("0.9.0"), "{config}");
    assert!(config.contains("notify-send kimi"), "{config}");
    assert_eq!(warp_hook_events(&config), HOOK_EVENTS);
    assert!(!hooks_need_update(&config));
}

#[test]
fn a_missing_event_needs_an_update() {
    let partial = format!(
        "[[hooks]]\nevent = \"Stop\"\ncommand = {:?}\ntimeout = 5\n",
        hook_command()
    );
    assert_eq!(
        installed_hooks_version(&partial).as_deref(),
        Some(HOOKS_VERSION)
    );
    assert!(hooks_need_update(&partial));
}

#[test]
fn a_config_without_warp_hooks_is_not_installed() {
    assert_eq!(installed_hooks_version(USER_CONFIG), None);
    assert!(!hooks_need_update(USER_CONFIG));
    assert_eq!(installed_hooks_version(""), None);
}

#[test]
fn refuses_hooks_that_are_not_tables_and_invalid_toml() {
    assert!(with_warp_hooks("hooks = []\n", &hook_command()).is_err());
    assert!(with_warp_hooks("this is = = not toml", &hook_command()).is_err());
}

#[test]
fn the_manual_example_is_the_installed_command() {
    let document = MANUAL_HOOK_EXAMPLE
        .parse::<DocumentMut>()
        .expect("the example is valid TOML");
    assert_eq!(
        document["hooks"][0]["command"].as_str(),
        Some(hook_command().as_str())
    );
    assert_kimi_accepts(MANUAL_HOOK_EXAMPLE);
}
