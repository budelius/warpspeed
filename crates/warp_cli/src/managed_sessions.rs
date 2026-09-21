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

    /// Start one task-bound configured operator-enabled agent.
    Start {
        /// Configured agent profile name.
        #[arg(long)]
        agent: String,

        /// Canonical STOZN task id. Every managed session requires one.
        #[arg(long)]
        task: String,

        #[arg(long)]
        prompt: Option<String>,

        #[arg(long)]
        interactive: bool,

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
}
