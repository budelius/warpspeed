//! `<warp binary> sessions` -- inspecting and moving terminal sessions from a
//! shell.
//!
//! Spelled that way because the binary's name varies: `warpspeed` in this fork,
//! `warp` upstream. Messages that suggest a command work it out at run time from
//! [`crate::remote_control::program_name`] rather than guessing.
//!
//! The subcommands split by who can answer them. Listing or killing detached shells reaches the
//! supervisor directly, while listing agents reads their process registries. Both work with no
//! window open. Detaching, attaching and quitting go to a running instance over the
//! [`crate::remote_control`] channel.
//!
//! Output is a plain table by default and JSON under `--json`. Both are
//! deliberate products rather than debug printing: this is the surface a test
//! drives Warp through, so a script has to be able to read every answer, and a
//! person has to be able to read it at a prompt without one.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use warp_cli::sessions::{SessionsArgs, SessionsCommand};

use crate::remote_control::client;
use crate::remote_control::protocol::{PaneSummary, Request, Response};

/// Runs one `sessions` subcommand and exits.
pub(crate) fn run(args: &SessionsArgs) -> Result<()> {
    let json = args.json;
    match &args.command {
        SessionsCommand::List => list_sessions(json),
        SessionsCommand::Supervisor => supervisor_status(json),
        SessionsCommand::Agents => list_agents(json),
        SessionsCommand::Instances => list_instances(json),
        SessionsCommand::Panes { instance } => list_panes(instance.pid, json),
        SessionsCommand::Windows { instance } => list_windows(instance.pid, json),
        SessionsCommand::Detach { session, instance } => act(
            instance.pid,
            Request::DetachSession {
                session_id: session.session_id,
            },
            json,
            &format!("Detached session {}", session.session_id),
        ),
        SessionsCommand::Attach { session, instance } => {
            attach(instance.pid, session.session_id, json)
        }
        SessionsCommand::Quit {
            terminate,
            instance,
        } => quit(instance.pid, *terminate, json),
        SessionsCommand::Kill { session } => kill_session(session.session_id, json),
        SessionsCommand::Stored { instance } => list_stored(instance.pid, json),
        SessionsCommand::Open { name, instance } => act(
            instance.pid,
            Request::OpenStoredSession { name: name.clone() },
            json,
            &format!("Opened saved session {name}"),
        ),
        SessionsCommand::RestartInCurrentSupervisor {
            terminal_view,
            instance,
        } => act(
            instance.pid,
            Request::RestartSessionInCurrentSupervisor {
                terminal_view_id: terminal_view.terminal_view_id,
            },
            json,
            &format!(
                "Restarted terminal view {} under the current supervisor",
                terminal_view.terminal_view_id
            ),
        ),
    }
}

// -- the supervisor's side -------------------------------------------------

/// The channel this profile's supervisor listens on, whether or not one is
/// there.
///
/// Printed even when nothing answers, because "which channel was it looking
/// for" is the question a failed detach actually raises -- most often the
/// answer is that `WARPSPEED_PROFILE` differs between the shell and the window.
fn supervisor_channel() -> String {
    let channel = crate::terminal::local_tty::session_holder::current_channel();
    if channel.is_empty() {
        "none (this build has no local terminals)".to_owned()
    } else {
        channel
    }
}

/// Whether a supervisor is listening right now.
///
/// A complete `Ping`/`Pong` exchange, not a bare connect. Connecting and
/// dropping is rude to a supervisor that is waiting to be spoken to: it accepts
/// the connection, blocks reading a message that never arrives, and until that
/// read times out the *next* client cannot get in. Two of these in a row --
/// which is all a check followed by a request is -- would make the second one
/// report no supervisor at all, while the daemon sat there holding a session
/// list it was perfectly willing to hand over.
///
/// Deliberately never starts one. Every request path spawns a supervisor when
/// it cannot find one, and a command that only asks a question must not leave a
/// daemon behind as a side effect.
#[cfg(all(feature = "local_tty", windows))]
fn supervisor_reachable() -> bool {
    use warp_terminal::local_tty::windows::pipe_transport::{PipeTransport, supervisor_pipe_name};

    let Ok(Some(mut transport)) =
        PipeTransport::connect_waiting(&supervisor_pipe_name(), std::time::Duration::from_secs(15))
    else {
        return false;
    };
    finish_probe(&mut transport)
}

#[cfg(all(feature = "local_tty", unix))]
fn supervisor_reachable() -> bool {
    use warp_terminal::local_tty::unix_supervisor::socket::socket_path;
    use warp_terminal::local_tty::unix_supervisor::transport::UnixSocketTransport;

    let Ok(mut transport) = UnixSocketTransport::connect(&socket_path()) else {
        return false;
    };
    finish_probe(&mut transport)
}

#[cfg(not(feature = "local_tty"))]
fn supervisor_reachable() -> bool {
    false
}

/// Says hello and goodbye properly, so the supervisor is free for the request
/// that follows.
#[cfg(feature = "local_tty")]
fn finish_probe(
    transport: &mut impl warp_terminal::local_tty::supervisor::transport::Transport,
) -> bool {
    use std::time::Duration;

    use warp_terminal::local_tty::supervisor::message::Message;

    if transport.send(&Message::Ping).is_err() {
        return false;
    }
    let answered = matches!(
        receive_within(transport, Duration::from_secs(5)),
        Some(Message::Pong { .. })
    );
    // Tells the supervisor it may close its end. Skipping this leaves the
    // instance held open until a timeout, which is the problem this whole
    // function exists to avoid.
    let _ = transport.send(&Message::ResponseReceived);
    answered
}

/// Reads one message, giving up after `timeout`.
///
/// The transports report "nothing has arrived yet" as `Ok(None)` rather than
/// blocking, so polling is what turns that into an answer or a verdict.
#[cfg(feature = "local_tty")]
fn receive_within(
    transport: &mut impl warp_terminal::local_tty::supervisor::transport::Transport,
    timeout: std::time::Duration,
) -> Option<warp_terminal::local_tty::supervisor::message::Message> {
    use std::time::Duration;

    use instant::Instant;

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match transport.receive() {
            Ok(Some(message)) => return Some(message),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
    None
}

#[derive(Serialize)]
struct SupervisorStatus {
    channel: String,
    reachable: bool,
    state_directory: String,
}

fn supervisor_status(json: bool) -> Result<()> {
    let status = SupervisorStatus {
        channel: supervisor_channel(),
        reachable: supervisor_reachable(),
        state_directory: warp_core::paths::state_dir().to_string_lossy().into_owned(),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }
    println!(
        "supervisor  {}",
        if status.reachable {
            "running"
        } else {
            "not running"
        }
    );
    println!("channel     {}", status.channel);
    println!("state dir   {}", status.state_directory);
    Ok(())
}

#[derive(Serialize)]
struct SessionRow {
    session_id: u64,
    shell: String,
    working_directory: String,
    title: String,
    detached_for_seconds: u64,
    attached: bool,
    shell_pid: u32,
    session_uuid_hex: String,
}

#[derive(Serialize)]
struct AgentRow {
    agent: String,
    session_id: String,
    pid: u32,
    started_at_unix_seconds: u64,
    last_active_unix_seconds: Option<u64>,
    state: String,
    build: Option<String>,
    saved_session: Option<String>,
    saved_tab: Option<String>,
    working_directory: Option<String>,
    last_prompt: Option<String>,
    process_count: usize,
}

fn list_agents(json: bool) -> Result<()> {
    use crate::terminal::cli_agent_sessions::agent_process_scan::{Placement, scan};

    let history = crate::terminal::agent_session_history::find_all_resumable()
        .into_iter()
        .map(|session| {
            let last_active_unix_seconds = session
                .modified
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_secs());
            (
                (session.agent, session.session_id),
                (session.last_prompt, last_active_unix_seconds),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    let rows = scan()
        .into_iter()
        .map(|process| {
            let (last_prompt, last_active_unix_seconds) = history
                .get(&(process.agent, process.session_id.clone()))
                .cloned()
                .unwrap_or_default();
            let (state, build, saved_in) = match process.placement {
                Placement::Known => ("known".to_owned(), None, None),
                Placement::HeldByOtherBuild {
                    build, saved_in, ..
                } => ("held_by_other_build".to_owned(), Some(build), saved_in),
                Placement::NoKnownPane { saved_in } => ("no_known_pane".to_owned(), None, saved_in),
            };
            AgentRow {
                agent: process.agent.display_name().to_owned(),
                session_id: process.session_id,
                pid: process.pid,
                started_at_unix_seconds: process.started_at_unix_seconds,
                last_active_unix_seconds,
                state,
                build,
                saved_session: saved_in.as_ref().map(|saved| saved.session_name.clone()),
                saved_tab: saved_in.map(|saved| saved.tab_title),
                working_directory: process.cwd.map(|cwd| cwd.to_string_lossy().into_owned()),
                last_prompt,
                process_count: process.process_count,
            }
        })
        .collect::<Vec<_>>();

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("No registered local agent processes are running.");
        return Ok(());
    }
    println!(
        "{:<12}  {:<8}  {:<24}  {:<12}  NAME",
        "AGENT", "PID", "STATE", "SESSION"
    );
    for row in rows {
        let name = row
            .last_prompt
            .as_deref()
            .or(row.working_directory.as_deref())
            .unwrap_or(&row.session_id);
        let duplicate = if row.process_count > 1 {
            format!(" ({} processes claim this session)", row.process_count)
        } else {
            String::new()
        };
        println!(
            "{:<12}  {:<8}  {:<24}  {:<12}  {}{}",
            truncate(&row.agent, 12),
            row.pid,
            truncate(&row.state, 24),
            truncate(&row.session_id, 12),
            name,
            duplicate,
        );
        let saved_in = match (&row.saved_session, &row.saved_tab) {
            (Some(session), Some(tab)) => Some(format!("saved in {session} / {tab}")),
            (Some(session), None) => Some(format!("saved in {session}")),
            (None, Some(tab)) => Some(format!("saved in {tab}")),
            (None, None) => None,
        };
        let details = [
            row.build.as_deref().map(|build| format!("build {build}")),
            saved_in,
            row.working_directory
                .as_deref()
                .map(|cwd| format!("cwd {cwd}")),
            (row.started_at_unix_seconds > 0)
                .then(|| format!("started {}", row.started_at_unix_seconds)),
            row.last_active_unix_seconds
                .map(|last_active| format!("last active {last_active}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if !details.is_empty() {
            println!("{:<62}{}", "", details.join(" · "));
        }
    }
    Ok(())
}

fn list_sessions(json: bool) -> Result<()> {
    let sessions = supervisor_sessions()?;
    let rows: Vec<SessionRow> = sessions
        .into_iter()
        .map(|session| SessionRow {
            session_id: session.session_id.as_u64(),
            shell: session.metadata.shell,
            working_directory: session.metadata.working_directory,
            title: session.metadata.title,
            detached_for_seconds: session.detached_for_seconds,
            attached: session.attached,
            shell_pid: session.shell_pid,
            session_uuid_hex: session.session_uuid_hex,
        })
        .collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("The supervisor is holding no sessions.");
        return Ok(());
    }
    println!(
        "{:<22}  {:<10}  {:<9}  {:<8}  WHERE",
        "SESSION", "SHELL", "AGE", "STATE"
    );
    for row in rows {
        println!(
            "{:<22}  {:<10}  {:<9}  {:<8}  {}",
            row.session_id,
            truncate(&row.shell, 10),
            crate::terminal::detached_sessions::humanize_age(row.detached_for_seconds),
            if row.attached { "in use" } else { "free" },
            if row.title.is_empty() {
                row.working_directory
            } else {
                format!("{} — {}", row.title, row.working_directory)
            }
        );
    }
    Ok(())
}

#[cfg(feature = "local_tty")]
fn supervisor_sessions()
-> Result<Vec<warp_terminal::local_tty::supervisor::message::SessionSummary>> {
    if !supervisor_reachable() {
        bail!(
            "No terminal session supervisor is running for this profile.\n\
             It would listen on {}.",
            supervisor_channel()
        );
    }
    let spawner = warp_terminal::local_tty::spawner::PtySpawner::new()
        .context("Failed to open a client for the terminal session supervisor")?;
    spawner
        .list_detached_sessions()
        .context("Failed to list the supervisor's sessions")
}

#[cfg(not(feature = "local_tty"))]
fn supervisor_sessions() -> Result<Vec<()>> {
    bail!("This build has no local terminals")
}

#[cfg(feature = "local_tty")]
fn kill_session(session_id: u64, json: bool) -> Result<()> {
    // No reachability check of its own: `supervisor_sessions` makes one, and a
    // second probe is another connection the supervisor has to service before
    // it can answer the request that matters.
    let held = supervisor_sessions()?
        .into_iter()
        .any(|session| session.session_id.as_u64() == session_id);
    if !held {
        bail!(
            "The supervisor is not holding session {session_id}. A session a window is using ends \
             when its pane closes."
        );
    }

    // Reached through the spawner rather than a client of its own so that the
    // preserved-session bookkeeping stays in one place.
    let spawner = warp_terminal::local_tty::spawner::PtySpawner::new()
        .context("Failed to open a client for the terminal session supervisor")?;
    spawner
        .kill_detached_session(session_id.into())
        .with_context(|| format!("Failed to end session {session_id}"))?;
    report_done(json, &format!("Ended session {session_id}"))
}

#[cfg(not(feature = "local_tty"))]
fn kill_session(_session_id: u64, _json: bool) -> Result<()> {
    bail!("This build has no local terminals")
}

// -- a running instance's side ---------------------------------------------

#[derive(Serialize)]
struct InstanceRow {
    pid: u32,
    version: String,
}

fn list_instances(json: bool) -> Result<()> {
    let rows: Vec<InstanceRow> = client::live_instances()
        .iter()
        .map(|instance| InstanceRow {
            pid: instance.pid(),
            version: instance.version().to_owned(),
        })
        .collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!(
            "No running Warp instance is listening.\nThis command reaches instances of the state \
             directory at {:?}.",
            warp_core::paths::state_dir()
        );
        return Ok(());
    }
    println!("{:<8}  BUILD", "PID");
    for row in rows {
        println!("{:<8}  {}", row.pid, row.version);
    }
    Ok(())
}

fn list_windows(pid: Option<u32>, json: bool) -> Result<()> {
    let instance = client::select_instance(pid)?;
    let windows = match instance.call(Request::ListWindows)? {
        Response::Windows { windows } => windows,
        other => bail!(unexpected(other)),
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&windows)?);
        return Ok(());
    }
    if windows.is_empty() {
        println!("This Warp instance has no windows.");
        return Ok(());
    }
    println!(
        "{:<8}  {:<8}  {:<9}  WORKSPACE",
        "INDEX", "WINDOW", "ACTIVE"
    );
    for window in windows {
        println!(
            "{:<8}  {:<8}  {:<9}  {}",
            window.index + 1,
            window.window_id,
            if window.is_active { "yes" } else { "no" },
            if window.has_workspace { "yes" } else { "no" },
        );
    }
    Ok(())
}

fn list_panes(pid: Option<u32>, json: bool) -> Result<()> {
    let instance = client::select_instance(pid)?;
    let panes = match instance.call(Request::ListPanes)? {
        Response::Panes { panes } => panes,
        other => bail!(unexpected(other)),
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&panes)?);
        return Ok(());
    }
    if panes.is_empty() {
        println!("This Warp instance has no terminal panes.");
        return Ok(());
    }
    println!(
        "{:<22}  {:<8}  {:<10}  {:<9}  WHERE",
        "SESSION", "TAB", "SHELL", "SURVIVES"
    );
    for pane in &panes {
        println!(
            "{:<22}  {:<8}  {:<10}  {:<9}  {}",
            pane.session_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "unsupervised".to_owned()),
            format!("{}.{}", pane.window, pane.tab_index),
            truncate(&pane.shell, 10),
            survives_label(pane),
            if pane.title.is_empty() {
                pane.working_directory.clone()
            } else {
                format!("{} — {}", pane.title, pane.working_directory)
            }
        );
    }
    Ok(())
}

/// Why a pane would or would not come back after a quit.
///
/// The reason matters more than the verdict. "no" on a pane with an agent
/// running in it means the supervisor never owned the shell, which is a
/// different problem from an idle pane that is simply not worth keeping.
fn survives_label(pane: &PaneSummary) -> &'static str {
    match (pane.survives_quit, pane.session_id.is_some()) {
        (true, true) => "yes",
        (true, false) => "no (pty)",
        (false, _) => "no (idle)",
    }
}

/// Attaches a session and confirms it really was taken over.
///
/// Opening the tab and reporting success is not enough, and neither is asking
/// the pane. A pane records the session id it was *asked* for the moment it is
/// created, before its shell has started, so it claims the session during the
/// very window in which taking it over can still fail -- and on Windows it
/// always does, after which the pane quietly falls back to a brand new shell
/// under a new id. Checking the pane too early therefore confirms an intention
/// rather than an outcome.
///
/// The supervisor is the side that knows. It marks a session as in use when it
/// hands it over and puts it back when the handover fails, so once the dust has
/// settled, "the supervisor says this session is in use, and a pane says it is
/// holding it" is true only if the attach actually happened.
fn attach(pid: Option<u32>, session_id: u64, json: bool) -> Result<()> {
    use std::time::Duration;

    use instant::Instant;

    /// How long to let the pane start its shell -- and fail, if it is going to
    /// -- before believing anything either side says.
    const SETTLE: Duration = Duration::from_secs(8);

    /// How long to keep asking after that.
    const CONFIRM: Duration = Duration::from_secs(15);

    let instance = client::select_instance(pid)?;
    match instance.call(Request::AttachSession { session_id })? {
        Response::Done => {}
        Response::Failed { error } => bail!(error),
        other => bail!(unexpected(other)),
    }

    std::thread::sleep(SETTLE);

    let deadline = Instant::now() + CONFIRM;
    loop {
        let held_by_a_pane = matches!(
            instance.call(Request::ListPanes),
            Ok(Response::Panes { panes })
                if panes.iter().any(|pane| pane.session_id == Some(session_id))
        );
        let supervisor_says_in_use = supervisor_sessions().is_ok_and(|sessions| {
            sessions
                .iter()
                .any(|session| session.session_id.as_u64() == session_id && session.attached)
        });
        if held_by_a_pane && supervisor_says_in_use {
            return report_done(json, &format!("Attached session {session_id} in a new tab"));
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    bail!(
        "A tab was opened for session {session_id}, but nothing is holding it: taking the session          over failed and the tab has a new shell in it instead. The session itself is unharmed and          `{} sessions list` will still show it; retry the attach after the failed tab closes.",
        crate::remote_control::program_name()
    )
}

fn list_stored(pid: Option<u32>, json: bool) -> Result<()> {
    let instance = client::select_instance(pid)?;
    let sessions = match instance.call(Request::ListStoredSessions)? {
        Response::StoredSessions { sessions } => sessions,
        Response::Failed { error } => bail!(error),
        other => bail!(unexpected(other)),
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&sessions)?);
        return Ok(());
    }
    if sessions.is_empty() {
        println!("There are no saved sessions.");
        return Ok(());
    }
    println!(
        "{:<20}  {:<10}  {:<17}  {:>4}  {:>5}  {:>6}",
        "NAME", "KIND", "SAVED", "TABS", "PANES", "AGENTS"
    );
    for session in &sessions {
        let saved = chrono::DateTime::from_timestamp(session.saved_at_unix_seconds, 0)
            .map(|time| {
                time.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_default();
        println!(
            "{:<20}  {:<10}  {:<17}  {:>4}  {:>5}  {:>6}",
            truncate(&session.name, 20),
            if session.autosaved {
                "autosaved"
            } else {
                "stored"
            },
            saved,
            session.tabs,
            session.panes,
            session.agents,
        );
    }
    Ok(())
}

fn act(pid: Option<u32>, request: Request, json: bool, done: &str) -> Result<()> {
    let instance = client::select_instance(pid)?;
    match instance.call(request)? {
        Response::Done => report_done(json, done),
        Response::Failed { error } => bail!(error),
        other => bail!(unexpected(other)),
    }
}

/// Asks an instance to quit, and reports it as done once it has actually gone.
///
/// A quit that returned as soon as the request was accepted would be useless to
/// the thing this exists for: a test that quits Warp and then looks at what the
/// supervisor kept has to know the teardown finished, not that it started.
fn quit(pid: Option<u32>, terminate: bool, json: bool) -> Result<()> {
    let instance = client::select_instance(pid)?;
    let pid = instance.pid();
    match instance.call(Request::Quit {
        close_windows: !terminate,
    }) {
        // A quit that tears the process down before the reply is written looks
        // exactly like a dropped connection, and it is the expected outcome.
        Ok(Response::Done) | Err(_) => {}
        Ok(Response::Failed { error }) => bail!(error),
        Ok(other) => bail!(unexpected(other)),
    }
    drop(instance);

    if wait_until_gone(pid) {
        return report_done(json, &format!("Warp instance {pid} has quit"));
    }
    if terminate {
        bail!(
            "Warp instance {pid} was asked to terminate and was still running {} seconds later. A              dialog asking to confirm the quit will do this.",
            QUIT_TIMEOUT.as_secs()
        );
    }
    // Closing every window ends the process on Windows and Linux but not on
    // macOS, where an app with no windows is still running. Saying so beats
    // reporting a timeout for what is that platform's normal behaviour.
    report_done(
        json,
        &format!("Closed every window of Warp instance {pid}; the process is still running"),
    )
}

/// How long to wait for a quit to finish.
const QUIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Whether an instance has stopped answering.
///
/// Asked by trying to reach it, not by looking for its marker file. A marker is
/// an advertisement, and one can outlive the process that wrote it -- a crash,
/// a kill, or simply an exit that runs no destructors, which is how the event
/// loop ends on Windows. "Has it gone" is a question only the channel can
/// answer.
fn wait_until_gone(pid: u32) -> bool {
    use std::time::Duration;

    use instant::Instant;

    let deadline = Instant::now() + QUIT_TIMEOUT;
    while Instant::now() < deadline {
        if !client::is_listening(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

// -- shared output ---------------------------------------------------------

#[derive(Serialize)]
struct DoneReport<'a> {
    ok: bool,
    message: &'a str,
}

fn report_done(json: bool, message: &str) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&DoneReport { ok: true, message })?
        );
    } else {
        println!("{message}");
    }
    Ok(())
}

fn unexpected(response: Response) -> String {
    format!("The Warp instance sent an answer this command did not ask for: {response:?}")
}

/// Shortens a column value, so one long shell path does not push every other
/// column off the line.
fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    let kept: String = value.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
#[path = "sessions_cli_tests.rs"]
mod tests;
