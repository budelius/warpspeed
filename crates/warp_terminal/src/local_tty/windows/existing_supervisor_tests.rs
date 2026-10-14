use std::ffi::OsString;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{CloseHandle, HANDLE};

use super::*;
use crate::SizeInfo;
use crate::local_tty::shell::{DirectShellStarter, ShellStarter};
use crate::local_tty::spawner::{PendingDetach, PtySpawner};
use crate::local_tty::windows::pipe_transport::PipeListener;
use crate::shell::ShellType;

fn unused_pipe_name() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    format!(
        r"\\.\pipe\warp-terminal-supervisor-v{PROTOCOL_VERSION}-{:08x}{:08x}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    )
}

fn peer(responses: Vec<Message>) -> (WindowsTerminalSupervisorClient, JoinHandle<Vec<Message>>) {
    let pipe = unused_pipe_name();
    let name = pipe.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let peer = std::thread::spawn(move || {
        let mut listener = PipeListener::bind(name).unwrap();
        ready_tx.send(()).unwrap();
        let mut requests = Vec::new();
        for response in responses {
            let mut transport = listener.accept().unwrap();
            requests.push(transport.receive_timeout(Duration::from_secs(5)).unwrap());
            transport.send(&response).unwrap();
            let _ = transport.receive_timeout(Duration::from_secs(5));
        }
        requests
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    (
        WindowsTerminalSupervisorClient::for_existing_pipe(pipe).unwrap(),
        peer,
    )
}

fn listed_session(attached: bool) -> SessionSummary {
    SessionSummary {
        session_id: SessionId::from(7),
        metadata: SessionMetadata::default(),
        detached_for_seconds: 0,
        attached,
    }
}

#[test]
fn existing_supervisor_rejects_remote_pipes() {
    assert!(
        WindowsTerminalSupervisorClient::for_existing_pipe(
            r"\\server\pipe\warp-terminal-supervisor-v6-0123456789abcdef".to_owned()
        )
        .is_err()
    );
}

#[test]
fn existing_supervisor_rejects_other_protocol_channels() {
    assert!(
        WindowsTerminalSupervisorClient::for_existing_pipe(
            r"\\.\pipe\warp-terminal-supervisor-v0-0123456789abcdef".to_owned()
        )
        .is_err()
    );
}

#[test]
fn missing_existing_supervisor_is_not_replaced_by_a_new_daemon() {
    let client = WindowsTerminalSupervisorClient::for_existing_pipe(unused_pipe_name()).unwrap();
    let error = client.check_detached(SessionId::from(7)).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("old supervisor is no longer available")
    );
}

#[test]
fn existing_supervisor_checks_protocol_before_listing_sessions() {
    let (client, peer) = peer(vec![Message::Pong {
        protocol_version: 0,
    }]);
    let error = client.check_detached(SessionId::from(7)).unwrap_err();
    assert!(error.to_string().contains("incompatible protocol"));
    assert!(matches!(peer.join().unwrap().as_slice(), [Message::Ping]));
}

#[test]
fn existing_supervisor_rejects_an_attached_session_without_ending_it() {
    let (client, peer) = peer(vec![
        Message::Pong {
            protocol_version: PROTOCOL_VERSION,
        },
        Message::ListSessionsResponse {
            sessions: vec![listed_session(true)],
        },
    ]);
    let error = client.check_detached(SessionId::from(7)).unwrap_err();
    assert!(error.to_string().contains("Another window is attached"));
    assert!(matches!(
        peer.join().unwrap().as_slice(),
        [Message::Ping, Message::ListSessions]
    ));
}

#[test]
fn existing_supervisor_rejects_a_missing_session() {
    let (client, peer) = peer(vec![
        Message::Pong {
            protocol_version: PROTOCOL_VERSION,
        },
        Message::ListSessionsResponse { sessions: vec![] },
    ]);
    let error = client.check_detached(SessionId::from(7)).unwrap_err();
    assert!(error.to_string().contains("no longer holds this session"));
    peer.join().unwrap();
}

#[test]
fn handoff_routes_adopted_sessions_back_to_their_original_supervisor() {
    let (client, peer) = peer(vec![Message::DetachResponse { error: None }]);
    let client = Arc::new(client);
    let preserved = client.register_handle(SessionId::from(7));
    let mut spawner = PtySpawner::new_for_test();
    spawner.adopt_supervisor(SessionId::from(7), client);
    spawner
        .prepare_sessions_for_handoff(vec![PendingDetach::without_screen(SessionId::from(7))])
        .unwrap();
    assert!(preserved.load(Ordering::Acquire));
    assert!(matches!(
        peer.join().unwrap().as_slice(),
        [Message::Detach { .. }]
    ));
}

#[test]
fn exit_preserves_adopted_sessions_at_their_original_supervisor() {
    let (client, peer) = peer(vec![Message::DetachResponse { error: None }]);
    let client = Arc::new(client);
    let preserved = client.register_handle(SessionId::from(7));
    let mut spawner = PtySpawner::new_for_test();
    spawner.adopt_supervisor(SessionId::from(7), client);
    spawner.prepare_for_app_termination(vec![PendingDetach::without_screen(SessionId::from(7))]);
    assert!(preserved.load(Ordering::Acquire));
    assert!(matches!(
        peer.join().unwrap().as_slice(),
        [Message::Detach { .. }]
    ));
}

#[test]
fn adopted_detached_sessions_remain_discoverable() {
    let (client, peer) = peer(vec![Message::ListSessionsResponse {
        sessions: vec![listed_session(false)],
    }]);
    let mut spawner = PtySpawner::new_for_test();
    spawner.adopt_supervisor(SessionId::from(7), Arc::new(client));
    assert_eq!(
        spawner.list_detached_sessions().unwrap(),
        vec![listed_session(false)]
    );
    peer.join().unwrap();
}

#[test]
fn attaching_through_an_existing_supervisor_preserves_the_shell_pid() {
    let name = unused_pipe_name();
    let daemon_pipe = name.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        super::super::supervisor_daemon::run_at_pipe(daemon_pipe, || ready_tx.send(()).unwrap())
            .unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let original = WindowsTerminalSupervisorClient::for_existing_pipe(name.clone()).unwrap();
    let session_id = SessionId::from(u64::MAX - 31);
    let options = PtyOptions {
        size: SizeInfo::new_without_font_metrics(24, 80),
        window_id: None,
        shell_starter: ShellStarter::Direct(DirectShellStarter::new_for_test(
            ShellType::PowerShell,
            PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"),
            vec![
                OsString::from("-NoProfile"),
                OsString::from("-Command"),
                OsString::from("Start-Sleep -Seconds 30"),
            ],
        )),
        start_dir: None,
        env_vars: HashMap::new(),
        enable_ssh_wrapper: false,
        reuse_ssh_control_master: false,
        shell_debug_mode: false,
        honor_ps1: false,
        node_version_chip_enabled: false,
        close_fds: true,
    };
    let first = original.spawn(session_id, &options).unwrap();
    let _ = unsafe { CloseHandle(HANDLE(first.raw as usize as *mut _)) };
    let snapshot = ScreenSnapshot {
        columns: 80,
        rows: 24,
        repaint: b"existing work".to_vec(),
    };
    let metadata = SessionMetadata {
        title: "Existing agent".to_owned(),
        ..Default::default()
    };
    original
        .detach(session_id, snapshot.clone(), metadata.clone())
        .unwrap();

    let client = Arc::new(WindowsTerminalSupervisorClient::for_existing_pipe(name).unwrap());
    client.check_detached(session_id).unwrap();
    let mut spawner = PtySpawner::new_for_test();
    spawner.adopt_supervisor(session_id, client.clone());
    let (result, mut handle, attached_metadata) = spawner
        .attach_pty(session_id, true, (80, 24))
        .unwrap()
        .expect("the existing session must attach");
    assert_eq!(result.pid, first.pid);
    assert_eq!(attached_metadata, metadata);
    assert!(original.is_running(session_id).unwrap());

    spawner
        .detach_session(session_id, snapshot, metadata)
        .unwrap();
    handle.kill().unwrap();
    assert!(
        original.is_running(session_id).unwrap(),
        "detaching must preserve the old shell"
    );
    drop(result);
    original.kill(session_id).unwrap();
}
