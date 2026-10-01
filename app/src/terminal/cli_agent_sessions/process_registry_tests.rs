use super::*;

fn process(pid: u32, parent: u32, name: &str) -> ProcessInfo {
    ProcessInfo {
        pid,
        parent: Some(parent),
        name: name.to_owned(),
    }
}

fn write_entry(dir: &Path, pid: u32, session_id: &str, cwd: &str) {
    std::fs::write(
        dir.join(format!("{pid}.json")),
        format!(r#"{{"pid":{pid},"sessionId":"{session_id}","cwd":{cwd:?},"kind":"interactive"}}"#),
    )
    .unwrap();
}

#[test]
fn descendants_lists_children_before_grandchildren_and_skips_other_trees() {
    let processes = [
        process(10, 1, "bash.exe"),
        process(20, 10, "sh.exe"),
        process(30, 20, "node.exe"),
        process(21, 10, "git.exe"),
        process(99, 1, "claude.exe"),
    ];

    assert_eq!(descendants(10, &processes), vec![20, 21, 30]);
}

#[test]
fn descendants_survives_a_process_that_claims_itself_as_parent() {
    let processes = [process(10, 10, "System"), process(20, 10, "bash.exe")];

    assert_eq!(descendants(10, &processes), vec![20]);
}

/// The case a transcript guess gets wrong: two Claude instances in one directory. Each pane
/// must get the session of the process under its own shell.
#[test]
fn each_shell_resolves_the_claude_running_under_it() {
    let registry = tempfile::tempdir().unwrap();
    write_entry(registry.path(), 101, "aaaa-1111", r"T:\projects\obachan");
    write_entry(registry.path(), 201, "bbbb-2222", r"T:\projects\obachan");
    let processes = [
        process(100, 1, "bash.exe"),
        process(101, 100, "claude.exe"),
        process(200, 1, "bash.exe"),
        process(201, 200, "claude.exe"),
    ];

    let first = resolve_in(CLIAgent::Claude, 100, &processes, registry.path()).unwrap();
    let second = resolve_in(CLIAgent::Claude, 200, &processes, registry.path()).unwrap();

    assert_eq!(first.session_id, "aaaa-1111");
    assert_eq!(first.cwd.as_deref(), Some(r"T:\projects\obachan"));
    assert_eq!(second.session_id, "bbbb-2222");
}

#[test]
fn pi_is_found_through_its_launcher_script() {
    let registry = tempfile::tempdir().unwrap();
    write_entry(registry.path(), 302, "01a0e78c-324d", r"T:\projects\system");
    let processes = [
        process(300, 1, "bash.exe"),
        process(301, 300, "sh.exe"),
        process(302, 301, "node.exe"),
    ];

    let session = resolve_in(CLIAgent::Pi, 300, &processes, registry.path()).unwrap();

    assert_eq!(session.session_id, "01a0e78c-324d");
}

#[test]
fn a_stale_entry_is_not_credited_to_an_unrelated_process_with_the_same_id() {
    let registry = tempfile::tempdir().unwrap();
    write_entry(registry.path(), 101, "stale", r"T:\projects");
    let processes = [process(100, 1, "bash.exe"), process(101, 100, "git.exe")];

    assert_eq!(
        resolve_in(CLIAgent::Claude, 100, &processes, registry.path()),
        None
    );
}

#[test]
fn an_id_that_is_unsafe_to_type_into_a_shell_is_rejected() {
    let registry = tempfile::tempdir().unwrap();
    write_entry(registry.path(), 101, "x; rm -rf ~", r"T:\projects");
    let processes = [process(100, 1, "bash.exe"), process(101, 100, "claude.exe")];

    assert_eq!(
        resolve_in(CLIAgent::Claude, 100, &processes, registry.path()),
        None
    );
}

#[test]
fn a_shell_without_an_agent_resolves_to_nothing() {
    let registry = tempfile::tempdir().unwrap();
    let processes = [process(100, 1, "bash.exe"), process(101, 100, "vim.exe")];

    assert_eq!(
        resolve_in(CLIAgent::Claude, 100, &processes, registry.path()),
        None
    );
}

#[test]
fn only_agents_with_a_registry_are_supported() {
    assert!(supports(CLIAgent::Claude));
    assert!(supports(CLIAgent::Pi));
    assert!(!supports(CLIAgent::Gemini));
}

#[test]
fn the_pi_extension_is_installed_once_and_rewritten_when_changed() {
    let pi_agent_dir = tempfile::tempdir().unwrap();
    let path = pi_agent_dir
        .path()
        .join("extensions")
        .join(PI_EXTENSION_FILE_NAME);

    assert!(install_pi_extension_in(pi_agent_dir.path()).unwrap());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), PI_EXTENSION_SOURCE);
    assert!(!install_pi_extension_in(pi_agent_dir.path()).unwrap());

    std::fs::write(&path, "// edited").unwrap();
    assert!(install_pi_extension_in(pi_agent_dir.path()).unwrap());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), PI_EXTENSION_SOURCE);
}

#[test]
fn claude_installed_through_npm_is_found_as_node() {
    let registry = tempfile::tempdir().unwrap();
    write_entry(registry.path(), 102, "npm-claude", r"T:\projects");
    let processes = [
        process(100, 1, "bash.exe"),
        process(101, 100, "sh.exe"),
        process(102, 101, "node.exe"),
    ];

    let session = resolve_in(CLIAgent::Claude, 100, &processes, registry.path()).unwrap();

    assert_eq!(session.session_id, "npm-claude");
}

#[test]
fn discoverable_agents_all_have_a_registry() {
    assert!(DISCOVERABLE_AGENTS.into_iter().all(supports));
}
