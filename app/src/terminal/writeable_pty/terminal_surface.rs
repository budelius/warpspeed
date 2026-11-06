use std::borrow::Cow;

use async_channel::Sender;
#[cfg(any(unix, all(windows, feature = "local_tty")))]
use warpui::AppContext;
use warpui::{Entity, ViewContext};

use crate::ai::agent::AIAgentPtyWriteMode;
#[cfg(unix)]
use crate::terminal::event::AfterBlockCompletedEvent;
use crate::terminal::model::completions::ShellCompletion;
#[cfg(unix)]
use crate::terminal::model::terminal_model::BlockIndex;
#[cfg(feature = "local_tty")]
use crate::terminal::terminal_manager::WithheldRestore;
use crate::terminal::view::ExecuteCommandEvent;
use crate::terminal::{ShellLaunchData, SizeUpdate};

/// A normalized request from a terminal UI surface to the PTY controller.
///
/// This is the narrow vocabulary that `TerminalManager` uses to drive the PTY
/// without knowing the concrete UI implementation. It only contains actions
/// meaningful to the PTY/session boundary: process control, byte writes,
/// resizing, command execution, and native shell completions.
pub enum PtyIntent {
    CtrlD,
    #[cfg(not(target_family = "wasm"))]
    Interrupt,
    ShutdownPty,
    WriteBytes(Cow<'static, [u8]>),
    WriteAgentInput {
        bytes: Cow<'static, [u8]>,
        mode: AIAgentPtyWriteMode,
    },
    Resize(SizeUpdate),
    ExecuteCommand(ExecuteCommandEvent),
    RunNativeShellCompletions {
        buffer_text: String,
        results_tx: Sender<Vec<ShellCompletion>>,
    },
}

/// Event types that can be projected into an [`Option<PtyIntent>`].
pub trait PtyIntentEvent {
    /// Projects this event into a PTY/session intent, or `None` if it is not a
    /// PTY-driving event.
    fn pty_intent(&self) -> Option<PtyIntent>;
}

/// A terminal frontend surface driven by `TerminalManager`.
///
/// Each surface defines how its own event type collapses into a PTY/session intent.
/// This is bounded by [`Entity`] instead of [`View`](warpui::View) so the same
/// manager can drive both GUI views and TUI views.
pub trait TerminalSurface: Entity + 'static
where
    <Self as Entity>::Event: PtyIntentEvent,
{
    /// Whether the local manager should start polling termios for a password prompt
    /// after the given block starts.
    #[cfg(unix)]
    fn should_start_password_prompt_polling(&self, _command: &str, _ctx: &AppContext) -> bool {
        false
    }

    /// Whether the local manager should stop password-prompt polling for this completed block.
    #[cfg(unix)]
    fn should_stop_password_prompt_polling(&self, _completed: &AfterBlockCompletedEvent) -> bool {
        false
    }

    /// Called with the process ID of the shell once its PTY has started, and again whenever a
    /// restarted session replaces it.
    #[cfg(feature = "local_tty")]
    fn on_shell_pid_determined(&mut self, _pid: u32, _ctx: &mut ViewContext<Self>) {}

    /// Called once the shell starter has been determined and the PTY event loop
    /// has started, so the surface can react to shell launch metadata.
    #[cfg(feature = "local_tty")]
    fn on_shell_determined(&mut self, _ctx: &mut ViewContext<Self>) {}

    /// Queues a command that resumes state after a restored PTY could not be reattached.
    #[cfg(feature = "local_tty")]
    fn set_pending_restore_command(&mut self, _command: String, _ctx: &mut ViewContext<Self>) {}

    /// Called with the token the shell's agent hooks address this pane by, once the session the
    /// pane runs (or adopted) is known.
    #[cfg(feature = "local_tty")]
    fn on_agent_event_token_determined(&mut self, _token: String, _ctx: &mut ViewContext<Self>) {}

    /// Holds a restore command back because another build's supervisor still holds the session.
    #[cfg(feature = "local_tty")]
    fn withhold_restore_command(
        &mut self,
        _restore: WithheldRestore,
        _ctx: &mut ViewContext<Self>,
    ) {
    }

    #[cfg(all(feature = "local_tty", windows))]
    fn can_replace_shell_for_held_attach(&self, ctx: &AppContext) -> bool {
        let _ = ctx;
        false
    }

    #[cfg(all(feature = "local_tty", windows))]
    fn on_held_attach_started(&mut self, ctx: &mut ViewContext<Self>) {
        let _ = ctx;
    }

    #[cfg(feature = "local_tty")]
    fn on_held_attach_failed(&mut self, error: String, ctx: &mut ViewContext<Self>) {
        let _ = (error, ctx);
    }

    /// Called when a take-over has started ending the held session.
    #[cfg(feature = "local_tty")]
    fn on_take_over_started(&mut self, _ctx: &mut ViewContext<Self>) {}

    /// Called when taking over a held session failed, with the reason.
    #[cfg(feature = "local_tty")]
    fn on_take_over_failed(&mut self, _error: String, _ctx: &mut ViewContext<Self>) {}

    /// Called just before a withheld restore command runs here.
    #[cfg(feature = "local_tty")]
    fn on_withheld_restore_resolved(&mut self, _ctx: &mut ViewContext<Self>) {}

    /// Identifies the supervisor that owns the connected shell.
    #[cfg(feature = "local_tty")]
    fn on_supervisor_changed(
        &mut self,
        holder: Option<crate::app_state::SessionHolder>,
        ctx: &mut ViewContext<Self>,
    ) {
        let _ = (holder, ctx);
    }

    /// Called when the active shell launch data is updated (e.g. shell indicator metadata).
    fn on_active_shell_launch_data_updated(
        &mut self,
        _shell_launch_data: Option<ShellLaunchData>,
        _ctx: &mut ViewContext<Self>,
    ) {
    }

    /// Called when the PTY fails to spawn so the surface can surface the error.
    #[cfg(feature = "local_tty")]
    fn on_pty_spawn_failed(&mut self, error: anyhow::Error, ctx: &mut ViewContext<Self>);

    /// Called immediately before the manager tears the current session down to
    /// replace it, so the surface can treat the resulting shell exit as a
    /// deliberate restart rather than a death.
    ///
    /// The surface stays marked until it observes the restart barrier the
    /// manager posts once the old event loop is gone
    /// (`ExitReason::Restarting`); surfaces that do not distinguish the two
    /// keep the default no-op.
    #[cfg(feature = "local_tty")]
    fn on_session_restart_started(&mut self, _ctx: &mut ViewContext<Self>) {}

    /// Called when termios indicates a likely password prompt is blocking the active block.
    #[cfg(unix)]
    fn on_possible_password_prompt(
        &mut self,
        _block_index: Option<BlockIndex>,
        _ctx: &mut ViewContext<Self>,
    ) {
    }

    /// Called when the block the poller was tracking completes.
    #[cfg(unix)]
    fn on_polled_block_completed(
        &mut self,
        _completed: &AfterBlockCompletedEvent,
        _ctx: &mut ViewContext<Self>,
    ) {
    }
}
