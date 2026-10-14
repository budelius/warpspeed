use super::*;
use crate::terminal::cli_agent_sessions::process_registry::{ProcessInfo, RegisteredAgentProcess};

fn process(
    pid: u32,
    parent: Option<u32>,
    name: &str,
    executable: Option<&str>,
    supervisor: bool,
) -> ProcessInfo {
    ProcessInfo {
        pid,
        parent,
        name: name.to_owned(),
        executable: executable.map(PathBuf::from),
        command: if supervisor {
            vec![name.to_owned(), SUPERVISOR_ARGUMENT.to_owned()]
        } else {
            Vec::new()
        },
        start_time: 0,
    }
}

fn agent(pid: u32, session_id: &str, uuid: Option<&str>) -> RegisteredAgentProcess {
    RegisteredAgentProcess {
        agent: CLIAgent::Pi,
        session_id: session_id.to_owned(),
        cwd: Some(PathBuf::from("/work")),
        pid,
        started_at_unix_seconds: 0,
        warp_session_uuid: uuid.map(str::to_owned),
    }
}

#[test]
fn a_process_under_this_builds_supervisor_is_known() {
    let registered = [agent(30, "session", Some("pane"))];
    let processes = [
        process(10, None, "warp", Some("/current/warp"), true),
        process(20, Some(10), "bash", None, false),
        process(30, Some(20), "node", None, false),
    ];

    let scanned = scan_in(
        &registered,
        &processes,
        Path::new("/current/warp"),
        &HashMap::new(),
    );

    assert_eq!(scanned[0].placement, Placement::Known);
}

#[test]
#[cfg(windows)]
fn windows_executable_case_does_not_turn_this_build_into_another_one() {
    let registered = [agent(30, "session", Some("pane"))];
    let processes = [
        process(
            10,
            None,
            "WarpSpeed.exe",
            Some(r"C:\warpspeed\current\warpspeed.exe"),
            true,
        ),
        process(30, Some(10), "node.exe", None, false),
    ];

    let scanned = scan_in(
        &registered,
        &processes,
        Path::new(r"C:\WarpSpeed\Current\WarpSpeed.exe"),
        &HashMap::new(),
    );

    assert_eq!(scanned[0].placement, Placement::Known);
}

#[test]
fn a_process_under_another_build_names_that_build() {
    let registered = [agent(30, "session", None)];
    let processes = [
        process(
            10,
            None,
            "WarpSpeed.exe",
            Some(r"C:\WarpSpeed\versions\997f1e1c\WarpSpeed.exe"),
            true,
        ),
        process(20, Some(10), "pwsh.exe", None, false),
        process(30, Some(20), "node.exe", None, false),
    ];

    let scanned = scan_in(
        &registered,
        &processes,
        Path::new(r"C:\WarpSpeed\current\WarpSpeed.exe"),
        &HashMap::new(),
    );

    assert_eq!(
        scanned[0].placement,
        Placement::HeldByOtherBuild {
            build: "997f1e1c".to_owned(),
            supervisor_pid: 10,
            saved_in: None,
        }
    );
}

#[test]
fn a_broken_parent_chain_uses_the_saved_pane_and_other_supervisor() {
    let registered = [agent(30, "session", Some("ABCDEF"))];
    let processes = [
        process(
            10,
            None,
            "old-warp",
            Some("/opt/warpspeed/versions/old-build/warp"),
            true,
        ),
        process(30, Some(999), "node", None, false),
    ];
    let saved = HashMap::from([(
        "abcdef".to_owned(),
        SavedPane {
            session_name: "M3RP".to_owned(),
            tab_title: "WA3P".to_owned(),
        },
    )]);

    let scanned = scan_in(
        &registered,
        &processes,
        Path::new("/opt/warpspeed/current/warp"),
        &saved,
    );

    assert_eq!(
        scanned[0].placement,
        Placement::HeldByOtherBuild {
            build: "old-build".to_owned(),
            supervisor_pid: 10,
            saved_in: Some(SavedPane {
                session_name: "M3RP".to_owned(),
                tab_title: "WA3P".to_owned(),
            }),
        }
    );
}

#[test]
fn a_saved_pane_does_not_guess_between_two_other_supervisors() {
    let registered = [agent(30, "session", Some("pane"))];
    let processes = [
        process(
            10,
            None,
            "first-warp",
            Some("/opt/warpspeed/versions/first/warp"),
            true,
        ),
        process(
            11,
            None,
            "second-warp",
            Some("/opt/warpspeed/versions/second/warp"),
            true,
        ),
        process(30, Some(999), "node", None, false),
    ];
    let saved = HashMap::from([(
        "pane".to_owned(),
        SavedPane {
            session_name: "M3RP".to_owned(),
            tab_title: "WA3P".to_owned(),
        },
    )]);

    let scanned = scan_in(
        &registered,
        &processes,
        Path::new("/opt/warpspeed/current/warp"),
        &saved,
    );

    assert_eq!(
        scanned[0].placement,
        Placement::NoKnownPane {
            saved_in: Some(SavedPane {
                session_name: "M3RP".to_owned(),
                tab_title: "WA3P".to_owned(),
            }),
        }
    );
}

#[test]
fn an_agent_without_any_terminal_is_reported_with_no_known_pane() {
    let registered = [agent(30, "session", None)];
    let processes = [process(30, None, "node", None, false)];

    let scanned = scan_in(
        &registered,
        &processes,
        Path::new("/current/warp"),
        &HashMap::new(),
    );

    assert_eq!(
        scanned[0].placement,
        Placement::NoKnownPane { saved_in: None }
    );
}

#[test]
fn duplicate_sessions_carry_the_number_of_live_processes() {
    let registered = [agent(30, "duplicate", None), agent(31, "duplicate", None)];
    let processes = [
        process(30, None, "node", None, false),
        process(31, None, "node", None, false),
    ];

    let scanned = scan_in(
        &registered,
        &processes,
        Path::new("/current/warp"),
        &HashMap::new(),
    );

    assert_eq!(scanned[0].process_count, 2);
    assert_eq!(scanned[1].process_count, 2);
}
