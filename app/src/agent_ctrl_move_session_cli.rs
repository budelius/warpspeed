//! `<warp binary> agent-ctrl move-session` -- moving a recorded agent session that no pane holds.
//!
//! A `dsh --profile headless` run has ended by the time it can be moved, and its pane drops the
//! agent session when the command finishes, so the pane's "Move session" never sees it.

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Local;
use managed_agent_sessions::{
    MovedSessionImport, import_moved_session, stop_run, wait_until_resumed,
};
use warp_cli::managed_sessions::{ManagedEnvironment, ManagedSessionsArgs, ManagedSessionsCommand};

use crate::terminal::CLIAgent;
use crate::terminal::agent_session_handoff::{
    HandoffSource, move_idempotency_key, prepare_handoff,
};
use crate::terminal::agent_session_history::{self, PastAgentSession};

const RESUME_TIMEOUT: Duration = Duration::from_secs(120);

/// A parsed `agent-ctrl move-session`.
#[derive(Debug, Clone)]
pub(crate) struct MoveSessionRequest {
    pub agent: String,
    pub session_id: String,
    pub environment: ManagedEnvironment,
}

/// The `move-session` request in `args`, which this module runs rather than
/// `managed_agent_sessions`.
pub(crate) fn move_session_request(args: &ManagedSessionsArgs) -> Option<MoveSessionRequest> {
    match &args.command {
        ManagedSessionsCommand::MoveSession {
            agent,
            session,
            environment,
        } => Some(MoveSessionRequest {
            agent: agent.clone(),
            session_id: session.clone(),
            environment: *environment,
        }),
        ManagedSessionsCommand::List
        | ManagedSessionsCommand::Start { .. }
        | ManagedSessionsCommand::Repos { .. }
        | ManagedSessionsCommand::MoveAll { .. }
        | ManagedSessionsCommand::Open { .. }
        | ManagedSessionsCommand::Attach { .. }
        | ManagedSessionsCommand::Input { .. }
        | ManagedSessionsCommand::Close { .. }
        | ManagedSessionsCommand::Stop { .. }
        | ManagedSessionsCommand::Pause { .. }
        | ManagedSessionsCommand::Resume { .. } => None,
    }
}

/// The agent named `label` whose sessions end with their command, so no pane holds them.
fn paneless_agent(label: &str) -> Option<CLIAgent> {
    match label {
        "dsh" => Some(CLIAgent::DeepSeekHarness),
        _ => None,
    }
}

/// The handoff source for recorded session `session_id` of `agent`, in the directory it ran in.
fn recorded_source(
    agent: CLIAgent,
    session_id: &str,
    history: &[PastAgentSession],
) -> Result<HandoffSource> {
    let past = history
        .iter()
        .find(|past| past.agent == agent && past.session_id == session_id)
        .with_context(|| format!("no recorded {} session {session_id}", agent.display_name()))?;
    Ok(HandoffSource {
        agent,
        session_id: session_id.to_owned(),
        cwd: past.cwd.clone(),
        transcript_path: None,
    })
}

/// Runs `agent-ctrl move-session` and exits.
pub(crate) fn run(request: MoveSessionRequest) -> Result<()> {
    let agent = paneless_agent(&request.agent).with_context(|| {
        format!(
            "move-session moves dsh sessions; move a {} session from its pane",
            request.agent
        )
    })?;
    let source = recorded_source(
        agent,
        &request.session_id,
        &agent_session_history::find_all(agent),
    )?;
    let home = dirs::home_dir().context("no home directory")?;
    let prepared = prepare_handoff(&source, &home, Local::now())?;
    let pushed_branch = format!("{}/{}", prepared.remote, prepared.branch);
    println!("Pushed the working tree to {pushed_branch}.");
    let source_cwd = source.cwd.display().to_string();
    let job_id = import_moved_session(
        request.environment,
        MovedSessionImport {
            agent_name: prepared.agent_name,
            session_id: &source.session_id,
            idempotency_key: move_idempotency_key(&source.session_id, &prepared.commit),
            session_file: prepared.session_file,
            repo_url: &prepared.repo_url,
            handoff_branch: &prepared.branch,
            source_cwd: &source_cwd,
        },
    )
    .with_context(|| format!("import failed; the handoff branch {pushed_branch} can be deleted"))?;
    println!("Imported as {job_id}; waiting for it to resume…");
    if let Err(error) = wait_until_resumed(request.environment, &job_id, RESUME_TIMEOUT) {
        let stopped = match stop_run(request.environment, &job_id, false) {
            Ok(()) => format!("The run {job_id} was asked to stop."),
            Err(stop_error) => {
                format!("Stop it with: warpspeed agent-ctrl stop {job_id} ({stop_error:#})")
            }
        };
        return Err(error.context(format!("{job_id} did not resume. {stopped}")));
    }
    println!("Resumed. Open it with: warpspeed agent-ctrl open {job_id}");
    Ok(())
}

#[cfg(test)]
#[path = "agent_ctrl_move_session_cli_tests.rs"]
mod tests;
