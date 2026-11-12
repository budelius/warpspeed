//! `<warp binary> agent-event` -- how an agent hook or plugin that cannot write to its terminal
//! reports a CLI agent event to the pane its agent runs in.
//!
//! It never fails, never prints, and never takes long. A hook's exit status and output feed back
//! into the agent -- Kimi Code blocks on exit code 2 and adds a `UserPromptSubmit` hook's stdout
//! to the prompt -- so every problem ends in a silent exit within [`DEADLINE`].

mod kimi_hook;

use std::io::Read;
use std::sync::mpsc;
use std::time::Duration;

use instant::Instant;
use warp_cli::agent_event::{AgentEventArgs, AgentEventFormat};
use warp_core::cli_agent_protocol::{CLIAgentNotification, WARP_AGENT_EVENT_TOKEN_ENV};

use crate::remote_control::address;
use crate::remote_control::client::Instance;
use crate::remote_control::protocol::{MAX_CLI_AGENT_EVENT_BYTES, Request, Response};
use crate::terminal::agent_session_paths::kimi_home;

/// Leaves room for process startup inside the 2 s a hook may take.
const DEADLINE: Duration = Duration::from_millis(1500);
const MAX_INPUT_BYTES: u64 = 1024 * 1024;
/// How much of each free-text field an oversized notification keeps.
const MAX_TEXT_BYTES: usize = 16 * 1024;

/// Forwards the event on stdin to the pane whose shell carries `WARP_AGENT_EVENT_TOKEN`.
pub(crate) fn run(args: &AgentEventArgs) {
    let Some(token) = std::env::var(WARP_AGENT_EVENT_TOKEN_ENV)
        .ok()
        .filter(|token| !token.is_empty())
    else {
        return;
    };
    let format = args.format;
    let hooks_version = args.hooks_version.clone();
    let (finished, done) = mpsc::channel();
    // On a thread so a stdin that never closes or an instance that never answers cannot hold
    // the agent up past the deadline.
    std::thread::spawn(move || {
        let mut input = String::new();
        if std::io::stdin()
            .take(MAX_INPUT_BYTES)
            .read_to_string(&mut input)
            .is_ok()
            && let Some(notification) = notification_json(format, &input, hooks_version.as_deref())
        {
            deliver(&token, &notification);
        }
        let _ = finished.send(());
    });
    let _ = done.recv_timeout(DEADLINE);
}

/// The protocol notification `input` describes, as JSON, or `None` when there is nothing to send.
fn notification_json(
    format: AgentEventFormat,
    input: &str,
    hooks_version: Option<&str>,
) -> Option<String> {
    let notification = match format {
        AgentEventFormat::KimiHook => kimi_hook::to_notification(
            &serde_json::from_str(input).ok()?,
            hooks_version,
            kimi_home().as_deref(),
        )?,
        AgentEventFormat::Protocol => serde_json::from_str::<CLIAgentNotification>(input).ok()?,
    };
    fitted_json(notification)
}

/// The notification as JSON within [`MAX_CLI_AGENT_EVENT_BYTES`]: one too large loses its tool
/// input and has its free text cut, and is not sent at all if that is still not enough.
fn fitted_json(mut notification: CLIAgentNotification) -> Option<String> {
    let json = serde_json::to_string(&notification).ok()?;
    if json.len() <= MAX_CLI_AGENT_EVENT_BYTES {
        return Some(json);
    }
    notification.tool_input = None;
    for text in [
        &mut notification.query,
        &mut notification.response,
        &mut notification.summary,
    ]
    .into_iter()
    .flatten()
    {
        truncate_at_char_boundary(text, MAX_TEXT_BYTES);
    }
    let json = serde_json::to_string(&notification).ok()?;
    (json.len() <= MAX_CLI_AGENT_EVENT_BYTES).then_some(json)
}

fn truncate_at_char_boundary(text: &mut String, max_bytes: usize) {
    if text.len() > max_bytes {
        let end = (0..=max_bytes)
            .rev()
            .find(|&index| text.is_char_boundary(index))
            .unwrap_or(0);
        text.truncate(end);
    }
}

/// Offers the notification to every running instance at once, so one that never answers cannot
/// keep it from the instance that owns the token.
///
/// Unlike the `sessions` command, this reaches instances of every protocol version. A hook runs
/// the binary that started its shell, and a newer window that adopted the shell owns the token, so
/// a hook limited to its own version would lose every event of an adopted agent. That is safe only
/// because `CliAgentEvent` and `Done` keep their encoding across versions, which
/// `cli_agent_event_keeps_its_encoding_across_protocol_versions` pins.
fn deliver(token: &str, notification: &str) {
    let attempts = address::read_markers().into_iter().map(|marker| {
        let request = Request::CliAgentEvent {
            token: token.to_owned().into(),
            notification: notification.to_owned(),
        };
        move || {
            Instance::connect(marker).is_ok_and(|instance| {
                matches!(instance.call_within(request, DEADLINE), Ok(Response::Done))
            })
        }
    });
    first_success(attempts, DEADLINE);
}

/// Runs each attempt on its own thread and returns whether one succeeded, as soon as one does,
/// all have failed, or `deadline` has passed.
fn first_success<F>(attempts: impl IntoIterator<Item = F>, deadline: Duration) -> bool
where
    F: FnOnce() -> bool + Send + 'static,
{
    let end = Instant::now() + deadline;
    let (results, outcomes) = mpsc::channel();
    let mut pending = 0;
    for attempt in attempts {
        let results = results.clone();
        std::thread::spawn(move || {
            let _ = results.send(attempt());
        });
        pending += 1;
    }
    while pending > 0 {
        let remaining = end.saturating_duration_since(Instant::now());
        match outcomes.recv_timeout(remaining) {
            Ok(true) => return true,
            Ok(false) => pending -= 1,
            Err(_) => return false,
        }
    }
    false
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
