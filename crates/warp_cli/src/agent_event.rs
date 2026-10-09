//! Arguments for `agent-event`, which forwards one CLI agent event to the Warp pane its agent
//! runs in.
//!
//! Only the argument shapes live here; the app crate maps and delivers the event.

use clap::{Args, ValueEnum};

/// `agent-event --format <FORMAT>`, reading one JSON object from standard input.
#[derive(Debug, Clone, Args)]
pub struct AgentEventArgs {
    /// The shape of the JSON object on standard input.
    #[arg(long, value_enum)]
    pub format: AgentEventFormat,

    /// The version of the Warp hooks running this command, reported as their plugin version.
    #[arg(long, value_name = "VERSION")]
    pub hooks_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AgentEventFormat {
    /// A Kimi Code hook payload.
    KimiHook,
    /// A Warp CLI agent protocol v1 notification.
    Protocol,
}
