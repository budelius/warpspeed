use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ManagedEnvironment {
    Staging,
}

#[derive(Debug, Clone, Args)]
pub struct ManagedSessionsArgs {
    #[command(subcommand)]
    pub command: ManagedSessionsCommand,

    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ManagedSessionsCommand {
    /// List locally bookmarked AgentCtrl sessions.
    List,

    /// Start one configured operator-enabled agent, bound to a task or to a repository.
    Start {
        /// Agent kind (claude, pi, codex) or configured AgentCtrl profile name.
        #[arg(long)]
        agent: String,

        /// Canonical STOZN task id. Required unless the run starts on a repository.
        #[arg(long, required_unless_present = "repo", conflicts_with = "repo")]
        task: Option<String>,

        /// Configured repository URL; starts an interactive run in a fresh worktree.
        #[arg(long)]
        repo: Option<String>,

        /// Base branch for the run's worktree; defaults to the repository's configured branch.
        #[arg(long, requires = "repo", conflicts_with = "task")]
        branch: Option<String>,

        #[arg(long)]
        prompt: Option<String>,

        /// A run on a repository is always interactive.
        #[arg(long, conflicts_with = "repo")]
        interactive: bool,

        #[arg(long, value_enum, default_value_t = ManagedEnvironment::Staging)]
        environment: ManagedEnvironment,
    },

    /// List the repositories the AgentCtrl node can start runs on.
    Repos {
        #[arg(long, value_enum, default_value_t = ManagedEnvironment::Staging)]
        environment: ManagedEnvironment,
    },

    /// Move every local agent session under a directory to AgentCtrl.
    MoveAll {
        #[arg(long)]
        under: PathBuf,

        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,

        #[arg(long, value_enum, default_value_t = ManagedEnvironment::Staging)]
        environment: ManagedEnvironment,
    },

    /// Open the managed-session viewer for a dashboard-launched run.
    Open {
        job_id: String,

        #[arg(long, value_enum, default_value_t = ManagedEnvironment::Staging)]
        environment: ManagedEnvironment,
    },

    /// Watch or resume a managed session by controller job id.
    Attach {
        job_id: String,

        #[arg(long, value_enum, default_value_t = ManagedEnvironment::Staging)]
        environment: ManagedEnvironment,

        /// Fetch currently available events and return instead of following.
        #[arg(long)]
        once: bool,
    },

    /// Send one audited follow-up turn to a live interactive run.
    Input {
        job_id: String,
        text: String,

        #[arg(long, value_enum, default_value_t = ManagedEnvironment::Staging)]
        environment: ManagedEnvironment,
    },

    /// Gracefully close a live interactive run's input.
    Close {
        job_id: String,

        #[arg(long, value_enum, default_value_t = ManagedEnvironment::Staging)]
        environment: ManagedEnvironment,
    },

    /// Stop a managed run.
    Stop {
        job_id: String,

        #[arg(long)]
        force: bool,

        #[arg(long, value_enum, default_value_t = ManagedEnvironment::Staging)]
        environment: ManagedEnvironment,
    },

    /// Pause a live interactive run: its agent exits, its work is pushed to the run's branch, and
    /// it frees its node slot until resumed.
    Pause {
        job_id: String,

        #[arg(long, value_enum, default_value_t = ManagedEnvironment::Staging)]
        environment: ManagedEnvironment,
    },

    /// Resume a paused run, with its conversation, on a node with a free slot.
    Resume {
        job_id: String,

        #[arg(long, value_enum, default_value_t = ManagedEnvironment::Staging)]
        environment: ManagedEnvironment,
    },
}
