//! Arguments for `agent-event`, which forwards one CLI agent event to the Warp pane its agent
//! runs in.
//!
//! Only the argument shapes live here; the app crate maps and delivers the event.

use std::ffi::OsString;

use clap::{Args, FromArgMatches, ValueEnum};

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

/// `agent-event`'s arguments when `argv` invokes it, parsed apart from the rest of the CLI so a
/// parse error never reaches clap's exit: Kimi Code reads exit code 2 as the hook blocking the
/// agent. `None` when `argv` is some other command.
pub fn from_argv(
    argv: impl IntoIterator<Item = impl Into<OsString>>,
) -> Option<Result<AgentEventArgs, clap::Error>> {
    let mut argv = argv.into_iter().map(Into::into).skip(1).peekable();
    if argv.peek().is_none_or(|command| command != "agent-event") {
        return None;
    }
    let command = AgentEventArgs::augment_args(clap::Command::new("agent-event"));
    Some(
        command
            .try_get_matches_from(argv)
            .and_then(|matches| AgentEventArgs::from_arg_matches(&matches)),
    )
}
