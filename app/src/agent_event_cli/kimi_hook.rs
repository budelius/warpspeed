//! Kimi Code hook payloads, as its `[[hooks]]` commands receive them on stdin, mapped to Warp's
//! CLI agent protocol.
//!
//! Kimi converts only top-level keys to snake_case; nested objects such as `display` and
//! `tool_input` keep the keys their tool gave them.

use std::path::{Path, PathBuf};

use serde_json::Value;
use warp_core::cli_agent_protocol::CLIAgentNotification;

use crate::terminal::agent_session_history::is_safe_session_id;
use crate::terminal::agent_session_paths::{kimi_indexed_session_dirs, kimi_work_dir_key};

/// The `agent` name events carry: `CLIAgent::Kimi`'s command prefix.
const AGENT: &str = "kimi";

/// Fields of a permission request's `display`, in the order the first one present summarises it.
const DISPLAY_SUMMARY_FIELDS: &[&str] = &[
    "description",
    "command",
    "path",
    "query",
    "url",
    "summary",
    "skill_name",
    "agent_name",
];

/// The notification for one hook payload, or `None` for hooks Warp does not track -- including
/// `SessionEnd`, since a pane's agent session ends when the `kimi` process does.
pub(super) fn to_notification(
    payload: &Value,
    hooks_version: Option<&str>,
    kimi_home: Option<&Path>,
) -> Option<CLIAgentNotification> {
    let base = |event: &str| CLIAgentNotification {
        session_id: text(payload, "session_id"),
        plugin_version: hooks_version.map(str::to_owned),
        ..CLIAgentNotification::new(AGENT, event)
    };
    let notification = match payload["hook_event_name"].as_str()? {
        "SessionStart" => {
            // Only `SessionStart` and `SessionEnd` carry the session's directory; agent-level
            // hooks send an empty `cwd`.
            let cwd = text(payload, "cwd");
            let transcript_path = kimi_home
                .zip(text(payload, "session_id"))
                .and_then(|(home, session_id)| session_dir(home, &session_id, cwd.as_deref()))
                .map(|dir| dir.display().to_string());
            CLIAgentNotification {
                cwd,
                transcript_path,
                ..base("session_start")
            }
        }
        "UserPromptSubmit" => CLIAgentNotification {
            query: prompt_text(&payload["prompt"]),
            ..base("prompt_submit")
        },
        "PermissionRequest" => CLIAgentNotification {
            tool_name: text(payload, "tool_name"),
            tool_input: tool_input(payload),
            summary: permission_summary(payload),
            ..base("permission_request")
        },
        "PermissionResult" => base("permission_replied"),
        "PostToolUse" => CLIAgentNotification {
            tool_name: text(payload, "tool_name"),
            tool_input: tool_input(payload),
            ..base("tool_complete")
        },
        "Stop" => base("stop"),
        "StopFailure" => CLIAgentNotification {
            error_type: text(payload, "error_type"),
            response: text(payload, "error_message"),
            ..base("stop_failure")
        },
        "Interrupt" => base("idle_prompt"),
        _ => return None,
    };
    Some(notification)
}

fn text(payload: &Value, key: &str) -> Option<String> {
    payload[key]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The text of a prompt, which Kimi sends as content parts.
fn prompt_text(prompt: &Value) -> Option<String> {
    let text = match prompt.as_str() {
        Some(text) => text.to_owned(),
        None => prompt
            .as_array()?
            .iter()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn tool_input(payload: &Value) -> Option<Value> {
    Some(payload["tool_input"].clone()).filter(|input| !input.is_null())
}

fn permission_summary(payload: &Value) -> Option<String> {
    DISPLAY_SUMMARY_FIELDS
        .iter()
        .find_map(|field| text(&payload["display"], field))
        .or_else(|| text(payload, "action"))
}

/// The session's directory, from Kimi's index, else where Kimi's naming puts it for `cwd`;
/// `None` unless it exists.
fn session_dir(home: &Path, session_id: &str, cwd: Option<&str>) -> Option<PathBuf> {
    if !is_safe_session_id(session_id) {
        return None;
    }
    kimi_indexed_session_dirs(home)
        .and_then(|dirs| {
            dirs.into_iter()
                .find(|(id, _)| id == session_id)
                .map(|(_, dir)| dir)
        })
        .or_else(|| {
            Some(
                home.join("sessions")
                    .join(kimi_work_dir_key(cwd?))
                    .join(session_id),
            )
        })
        .filter(|dir| dir.is_dir())
}

#[cfg(test)]
#[path = "kimi_hook_tests.rs"]
mod tests;
