use super::*;

fn process(pid: u32, executable: &str, command: &[&str]) -> ProcessInfo {
    ProcessInfo {
        pid,
        parent: None,
        name: "warpspeed".to_owned(),
        executable: Some(PathBuf::from(executable)),
        command: command
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect(),
        start_time: 1,
    }
}

#[test]
fn one_build_groups_its_app_and_supervisor_processes() {
    let executable = "/opt/warpspeed/versions/20261021-test/warpspeed";
    let processes = vec![
        process(10, executable, &[executable]),
        process(20, executable, &[executable, SUPERVISOR_ARGUMENT]),
        process(30, executable, &[executable, "sessions", "list"]),
    ];
    let markers = vec![InstanceMarker {
        pid: 10,
        address: "control".to_owned(),
        version: "20261021-build".to_owned(),
    }];

    let profiles = HashMap::from([(10, None), (20, None), (30, None)]);

    let sources = build_sources(&processes, &markers, None, &profiles);

    assert_eq!(
        sources,
        vec![BuildSource {
            executable: PathBuf::from(executable),
            version: "20261021-build".to_owned(),
            profile: None,
            marker_instances: vec![(10, "20261021-build".to_owned())],
            supervisor_pids: vec![20],
            app_process_pids: vec![10, 30],
        }]
    );
}

#[test]
fn identical_builds_in_separate_profiles_stay_separate() {
    let executable = "/opt/warpspeed/warpspeed";
    let processes = vec![
        process(10, executable, &[executable]),
        process(20, executable, &[executable, SUPERVISOR_ARGUMENT]),
        process(30, executable, &[executable]),
        process(40, executable, &[executable, SUPERVISOR_ARGUMENT]),
    ];
    let profiles = HashMap::from([
        (10, Some("alpha".to_owned())),
        (20, Some("alpha".to_owned())),
        (30, Some("beta".to_owned())),
        (40, Some("beta".to_owned())),
    ]);

    let sources = build_sources(&processes, &[], None, &profiles);

    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0].profile.as_deref(), Some("alpha"));
    assert_eq!(sources[0].app_process_pids, vec![10]);
    assert_eq!(sources[0].supervisor_pids, vec![20]);
    assert_eq!(sources[1].profile.as_deref(), Some("beta"));
    assert_eq!(sources[1].app_process_pids, vec![30]);
    assert_eq!(sources[1].supervisor_pids, vec![40]);
}

#[test]
fn summary_counts_every_supervisor_process() {
    let overview = SystemOverview {
        builds: vec![BuildOverview {
            version: "build".to_owned(),
            profile: None,
            executable: PathBuf::from("warpspeed"),
            instances: Vec::new(),
            supervisor_pids: vec![20, 30],
            supervisor: None,
            supervisor_error: None,
            other_process_pids: Vec::new(),
        }],
    };

    assert_eq!(overview.process_count(), 2);
    assert_eq!(overview.supervisor_count(), 2);
}

#[test]
fn panes_are_grouped_by_window_and_sorted_by_tab() {
    let pane = |window: &str, tab_index: usize, session_id: u64| PaneReport {
        terminal_view_id: Some(session_id),
        session_id: Some(session_id),
        window: window.to_owned(),
        tab_index,
        title: "pane".to_owned(),
        shell: "pwsh".to_owned(),
        working_directory: r"C:\work".to_owned(),
        has_running_agent: false,
        is_busy: false,
        survives_quit: true,
        agent: None,
        agent_session_id: None,
        agent_not_resumed: false,
    };

    let windows = group_panes(
        vec![
            pane("window-b", 0, 3),
            pane("window-a", 1, 2),
            pane("window-a", 0, 1),
        ],
        None,
    );

    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0].id, "window-a");
    assert_eq!(windows[0].panes[0].session_id, Some(1));
    assert_eq!(windows[0].panes[1].session_id, Some(2));
    assert_eq!(windows[1].id, "window-b");
    assert_eq!(windows[1].panes[0].session_id, Some(3));
}

#[test]
fn command_output_ignores_startup_text_before_json() {
    let sessions: Vec<SupervisorSessionOverview> = parse_json_output(
        br#"WarpSpeed starting: app_id=WarpSpeed
[{"session_id":7,"attached":true}]"#,
    )
    .unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, 7);
    assert!(sessions[0].attached);
}

#[test]
fn supervisor_protocol_is_read_from_windows_and_unix_channels() {
    assert_eq!(
        protocol_version(r"\\.\pipe\warp-terminal-supervisor-v7-abc"),
        Some(7)
    );
    assert_eq!(
        protocol_version("/run/user/1000/warp-supervisor/v6-1.sock"),
        Some(6)
    );
    assert_eq!(protocol_version("unknown"), None);
}
