//! Arguments for the `sessions` subcommand, the terminal-session control
//! surface.
//!
//! Which process answers a subcommand decides what it can do.
//!
//! [`SessionsCommand::List`] and [`SessionsCommand::Kill`] talk to the
//! *supervisor*: the daemon that owns detached shells and outlives the app.
//! [`SessionsCommand::Agents`] reads the agents' process registries directly.
//! All three work with no Warp window open.
//!
//! Everything else talks to a *running Warp instance*, because the work is the
//! app's to do. Detaching reads the pane's screen and closes it; attaching
//! opens a tab. Neither is something a separate process can perform on the
//! app's behalf, so these subcommands ask a window to do it and report what it
//! said.
//!
//! Only the argument shapes live here. `warp_cli` has no view of terminals or
//! windows, so the implementation is in the app crate, reached the same way
//! `--build-info` is.

use clap::{Args, Subcommand};

/// `sessions <subcommand>`.
#[derive(Debug, Clone, Args)]
pub struct SessionsArgs {
    #[command(subcommand)]
    pub command: SessionsCommand,

    /// Print machine-readable JSON instead of a table.
    ///
    /// Global to the group because every subcommand here is something a script
    /// wants to read back, including the ones whose human output is one line.
    #[arg(long, global = true)]
    pub json: bool,
}

/// Which Warp instance a subcommand is addressed to.
///
/// Left out, it means "the only one running". That is the common case at a
/// prompt, and being explicit about a single obvious target is friction with no
/// payoff. With several running, the ambiguity is real and the error says so
/// rather than picking one.
#[derive(Debug, Clone, Args)]
pub struct InstanceArg {
    /// The process id of the Warp instance to address.
    ///
    /// Optional while exactly one instance is running. `sessions instances`
    /// lists the choices.
    #[arg(long = "instance", value_name = "PID")]
    pub pid: Option<u32>,
}

/// A session id, as `sessions list` prints it.
#[derive(Debug, Clone, Args)]
pub struct SessionArg {
    /// The id of the session to act on.
    #[arg(long = "session", value_name = "ID")]
    pub session_id: u64,
}

/// A terminal view id, as `sessions panes` prints it.
#[derive(Debug, Clone, Args)]
pub struct TerminalViewArg {
    /// The id of the terminal view to act on.
    #[arg(long = "terminal-view", value_name = "ID")]
    pub terminal_view_id: u64,
}

#[derive(Debug, Clone, Subcommand)]
pub enum SessionsCommand {
    /// List the sessions the supervisor is holding.
    ///
    /// These are shells with no window attached: detached by hand, or left
    /// running when Warp last quit. Each one can be taken back with `attach`.
    List,

    /// Report whether a supervisor is reachable, and which one.
    ///
    /// Prints the channel it listens on even when nothing answers, which is the
    /// first thing worth knowing when a detach has failed.
    Supervisor,

    /// List every registered local agent process and where its terminal is.
    Agents,

    /// List the running Warp instances that accept these commands.
    Instances,

    /// List the terminal panes of a running Warp instance.
    ///
    /// The session id in each row is what `detach` takes.
    Panes {
        #[command(flatten)]
        instance: InstanceArg,
    },

    /// List every window of a running Warp instance.
    Windows {
        #[command(flatten)]
        instance: InstanceArg,
    },

    /// Hand a live pane's session to the supervisor and close the pane.
    ///
    /// The shell keeps running. What was in the pane is preserved and comes
    /// back on the next `attach`.
    Detach {
        #[command(flatten)]
        session: SessionArg,

        #[command(flatten)]
        instance: InstanceArg,
    },

    /// Open a tab in a running Warp instance holding a detached session.
    Attach {
        #[command(flatten)]
        session: SessionArg,

        #[command(flatten)]
        instance: InstanceArg,
    },

    /// List the saved sessions a running Warp instance can reopen, newest first.
    Stored {
        #[command(flatten)]
        instance: InstanceArg,
    },

    /// Reopen a saved session in a running Warp instance, as picking it in the session picker
    /// would.
    ///
    /// Opens the session's newest autosave. Shells it was running that are still alive are
    /// taken over; agents whose process has ended resume from their recorded session.
    Open {
        /// The saved session's name, as `stored` lists it. Case does not matter.
        name: String,

        #[command(flatten)]
        instance: InstanceArg,
    },

    /// Replace a pane's shell from an older supervisor and resume its attached agent.
    ///
    /// This ends the original shell and any process still running in it. Use it only after
    /// deciding that the old process may be terminated.
    RestartInCurrentSupervisor {
        #[command(flatten)]
        terminal_view: TerminalViewArg,

        #[command(flatten)]
        instance: InstanceArg,
    },

    /// Replace a pane's shell with a new one and resume the agent it was running.
    ///
    /// Works on any pane, whichever supervisor holds its shell, and asks no confirmation: the
    /// original shell and everything still running in it end. Picks up a newly installed agent
    /// version, since the agent starts again from its recorded session.
    #[command(group(
        clap::ArgGroup::new("target").required(true).args(["terminal_view_id", "all_agents"])
    ))]
    Restart {
        /// The id of the terminal view to restart, as `panes` prints it.
        #[arg(long = "terminal-view", value_name = "ID")]
        terminal_view_id: Option<u64>,

        /// Restart every pane that is running an agent, one after another. A pane that has not
        /// attached yet is opened first, so its tab comes to the front.
        #[arg(long)]
        all_agents: bool,

        /// A terminal view to leave alone under `--all-agents`, such as the one running this
        /// command. Repeatable.
        #[arg(
            long = "except-view",
            value_name = "ID",
            conflicts_with = "terminal_view_id"
        )]
        except_views: Vec<u64>,

        #[command(flatten)]
        instance: InstanceArg,
    },

    /// End a session the supervisor is holding, killing its shell.
    ///
    /// Only reaches detached sessions. A session a window is using belongs to
    /// that window, and closing its pane is how it ends.
    Kill {
        #[command(flatten)]
        session: SessionArg,
    },

    /// Ask a running Warp instance to quit, exactly as the user quitting it
    /// would.
    ///
    /// The point of having this is the exit path: quitting runs the same
    /// teardown that decides which sessions outlive the app, so a test can
    /// trigger it and then look at what the supervisor kept.
    ///
    /// By default this closes every window, which is how a user quits on
    /// Windows and Linux -- the app has no separate quit there, and the event
    /// loop ends once the last window has gone.
    Quit {
        /// Ask the app to terminate instead of closing its windows.
        ///
        /// This is the macOS Cmd-Q path. It skips the per-window teardown
        /// entirely, so a session that only the window-close path preserves
        /// will not be preserved by it -- which is the point of being able to
        /// choose.
        #[arg(long)]
        terminate: bool,

        #[command(flatten)]
        instance: InstanceArg,
    },
}
