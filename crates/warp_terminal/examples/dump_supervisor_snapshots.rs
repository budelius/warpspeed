//! Prints what a Windows terminal supervisor is holding, screen contents included.
//!
//! The supervisor only hands a session's screen out on attach, so each detached
//! session is attached, rendered, and handed straight back with the rendered
//! screen and its original metadata. A session another client holds is listed
//! but left alone.
//!
//! ```text
//! cargo run -p warp_terminal --example dump_supervisor_snapshots -- \
//!     [--list] [--json] [--app-name NAME] [--exe PATH] [SESSION_ID...]
//! ```
//!
//! Every installed build runs its own supervisor, named after the build's
//! executable and the data directory. Without `--exe`, every `warpspeed.exe`
//! under the WarpSpeed install root is tried and the first one with a listening
//! supervisor wins. The data directory follows the application name: `WarpSpeed`,
//! `WarpSpeed-<WARPSPEED_PROFILE>`, or `WarpSpeedAdmin` for an elevated instance.

#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    use std::path::PathBuf;
    use std::time::Duration;

    use anyhow::{Context, bail};
    use serde_json::json;
    use warp_core::channel::{Channel, ChannelConfig, ChannelState, OzConfig, WarpServerConfig};
    use warp_core::{AppId, SessionId};
    use warp_terminal::event_listener::ChannelEventListener;
    use warp_terminal::local_tty::screen::HeadlessScreen;
    use warp_terminal::local_tty::supervisor::transport::Transport;
    use warp_terminal::local_tty::supervisor::{Message, SessionSummary};
    use warp_terminal::local_tty::windows::pipe_transport::{
        PipeTransport, supervisor_pipe_name_for,
    };
    use warp_terminal::local_tty::windows::supervisor_client::WindowsTerminalSupervisorClient;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};

    struct Options {
        list_only: bool,
        json: bool,
        app_name: String,
        executable: Option<PathBuf>,
        session_ids: Vec<SessionId>,
    }

    fn default_app_name() -> String {
        match std::env::var("WARPSPEED_PROFILE") {
            Ok(profile) if !profile.trim().is_empty() => {
                let profile: String = profile
                    .trim()
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect();
                format!("WarpSpeed-{profile}")
            }
            _ => "WarpSpeed".to_owned(),
        }
    }

    fn parse_options() -> anyhow::Result<Options> {
        let mut options = Options {
            list_only: false,
            json: false,
            app_name: default_app_name(),
            executable: None,
            session_ids: Vec::new(),
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--list" => options.list_only = true,
                "--json" => options.json = true,
                "--app-name" => {
                    options.app_name = args.next().context("--app-name needs a value")?;
                }
                "--exe" => {
                    options.executable = Some(args.next().context("--exe needs a path")?.into());
                }
                other => {
                    let id: u64 = other
                        .parse()
                        .with_context(|| format!("Expected a numeric session id, got {other:?}"))?;
                    options.session_ids.push(SessionId::from(id));
                }
            }
        }
        Ok(options)
    }

    /// Every WarpSpeed executable that could have started a supervisor, newest install first.
    fn installed_executables() -> Vec<PathBuf> {
        let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
            return Vec::new();
        };
        let versions = PathBuf::from(local_app_data)
            .join("Programs")
            .join("WarpSpeed")
            .join("versions");
        let Ok(entries) = std::fs::read_dir(versions) else {
            return Vec::new();
        };
        let mut executables: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path().join("warpspeed.exe"))
            .filter(|path| path.is_file())
            .collect();
        executables.sort();
        executables.reverse();
        executables
    }

    /// Whether a supervisor answers on `pipe_name`.
    ///
    /// Says hello and goodbye properly: a client that connects and drops without a
    /// message leaves the supervisor blocked until its read times out, and the
    /// request that follows would then find the pipe busy.
    fn supervisor_answers(pipe_name: &str) -> bool {
        let Ok(Some(mut transport)) =
            PipeTransport::connect_waiting(pipe_name, Duration::from_secs(15))
        else {
            return false;
        };
        if transport.send(&Message::Ping).is_err() {
            return false;
        }
        let answered = matches!(
            transport.receive_timeout(Duration::from_secs(5)),
            Ok(Message::Pong { .. })
        );
        let _ = transport.send(&Message::ResponseReceived);
        answered
    }

    /// The executable whose supervisor is listening, or why none is.
    fn find_supervisor(executable: Option<PathBuf>) -> anyhow::Result<PathBuf> {
        let state_directory = warp_core::paths::state_dir();
        let candidates = match executable {
            Some(executable) => vec![executable],
            None => installed_executables(),
        };
        if candidates.is_empty() {
            bail!("No installed WarpSpeed build was found; pass --exe <path to warpspeed.exe>");
        }
        let mut tried = Vec::new();
        for candidate in candidates {
            let pipe_name = supervisor_pipe_name_for(&state_directory, &candidate);
            if supervisor_answers(&pipe_name) {
                return Ok(candidate);
            }
            tried.push(format!("{} -> {pipe_name}", candidate.display()));
        }
        bail!(
            "No terminal supervisor is listening for state directory {}:\n  {}",
            state_directory.display(),
            tried.join("\n  ")
        )
    }

    fn summary_json(summary: &SessionSummary, screen: Option<&str>) -> serde_json::Value {
        json!({
            "session_id": summary.session_id.as_u64(),
            "attached": summary.attached,
            "detached_for_seconds": summary.detached_for_seconds,
            "shell": summary.metadata.shell,
            "working_directory": summary.metadata.working_directory,
            "title": summary.metadata.title,
            "bootstrapped": summary.metadata.bootstrap.is_some(),
            "screen": screen,
        })
    }

    fn print_summary(summary: &SessionSummary) {
        let state = if summary.attached {
            "attached".to_owned()
        } else {
            format!("detached {}s", summary.detached_for_seconds)
        };
        println!(
            "session {} [{state}] shell={:?} cwd={:?} title={:?} bootstrapped={}",
            summary.session_id.as_u64(),
            summary.metadata.shell,
            summary.metadata.working_directory,
            summary.metadata.title,
            summary.metadata.bootstrap.is_some(),
        );
    }

    /// Attaches, renders the screen, and hands the session straight back.
    ///
    /// The lent PTY handle is closed rather than read: the supervisor keeps its own
    /// copy and resumes reading once the session is detached again.
    fn dump_screen(
        client: &WindowsTerminalSupervisorClient,
        session_id: SessionId,
    ) -> anyhow::Result<String> {
        let attached = client
            .attach(session_id)
            .with_context(|| format!("Failed to attach session {}", session_id.as_u64()))?;
        if let Some(endpoint) = attached.endpoint {
            let _ = unsafe { CloseHandle(HANDLE(endpoint.raw as usize as *mut _)) };
        }

        let mut screen =
            HeadlessScreen::from_snapshot(&attached.snapshot, ChannelEventListener::new_headless());
        screen.feed(&attached.buffered_output);
        let mut text = screen.visible_text();
        if attached.output_was_dropped {
            text.push_str("\n[output was dropped while detached]");
        }

        client
            .detach(session_id, screen.snapshot(), attached.metadata)
            .with_context(|| format!("Failed to hand session {} back", session_id.as_u64()))?;
        Ok(text)
    }

    let options = parse_options()?;

    ChannelState::set(ChannelState::new(
        Channel::Local,
        ChannelConfig {
            app_id: AppId::new("dev", "warp", options.app_name.clone()),
            logfile_name: "dump_supervisor_snapshots.log".into(),
            server_config: WarpServerConfig::production(),
            oz_config: OzConfig::production(),
            telemetry_config: None,
            crash_reporting_config: None,
            autoupdate_config: None,
            mcp_static_config: None,
        },
    ));

    let executable = find_supervisor(options.executable.clone())?;
    eprintln!("Using the supervisor of {}", executable.display());

    let client = WindowsTerminalSupervisorClient::for_executable(executable);
    let mut sessions = client.list_sessions()?;
    if !options.session_ids.is_empty() {
        sessions.retain(|summary| options.session_ids.contains(&summary.session_id));
    }

    let mut report = Vec::new();
    for summary in &sessions {
        let screen = if options.list_only || summary.attached {
            None
        } else {
            match dump_screen(&client, summary.session_id) {
                Ok(text) => Some(text),
                Err(error) => {
                    eprintln!("session {}: {error:#}", summary.session_id.as_u64());
                    None
                }
            }
        };

        if options.json {
            report.push(summary_json(summary, screen.as_deref()));
        } else {
            print_summary(summary);
            if let Some(screen) = screen {
                println!("{}", "-".repeat(72));
                println!("{screen}");
                println!("{}", "=".repeat(72));
            }
        }
    }

    if options.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if sessions.is_empty() {
        println!(
            "The supervisor for app {} holds no sessions",
            options.app_name
        );
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("This example only runs on Windows");
}
