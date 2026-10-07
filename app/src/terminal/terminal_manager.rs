use std::any::Any;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::FairMutex;
use pathfinder_geometry::vector::Vector2F;
use settings::Setting as _;
use warp_core::SessionId;
use warpui::{AppContext, ModelContext, SingletonEntity};

use super::event_listener::ChannelEventListener;
use super::model::block::BlockSize;
use super::safe_mode_settings::get_secret_obfuscation_mode;
use super::session_settings::SessionSettings;
use super::settings::TerminalSettings;
use super::view::{WARP_PROMPT_HEIGHT_LINES, create_size_info_for_blocklist};
use super::{BlockPadding, ShellLaunchData, ShellLaunchState, SizeInfo, TerminalModel, color};
use crate::PrivacySettings;
use crate::ai::blocklist::SerializedBlockListItem;
use crate::ai::blocklist::telemetry_banner::should_collect_ai_ugc_telemetry;
use crate::app_state::SessionHolder;
use crate::appearance::Appearance;
use crate::pane_group::pane::DetachType;
use crate::settings::{BlockVisibilitySettings, DebugSettings, InputModeSettings};
use crate::terminal::CLIAgent;

/// What a restored pane was going to run before the guard found its session held elsewhere.
#[derive(Clone, Debug, PartialEq)]
pub struct WithheldRestore {
    pub command: String,
    pub holder: SessionHolder,
    pub session_id: SessionId,
    pub agent: Option<HeldAgent>,
    pub verdict: HeldVerdict,
}

/// The agent a held session was running, identified the way its process registry records it.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldAgent {
    pub agent: CLIAgent,
    pub session_id: String,
}

/// Whether a window of the holding build is attached to the held session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldVerdict {
    Detached,
    Attached,
}

pub trait TerminalManager: Any {
    /// Returns the backing terminal model.
    fn model(&self) -> Arc<FairMutex<TerminalModel>>;

    /// Called when the terminal pane detaches from its pane group. This is a sensitive path -
    /// do not do anything with high latency here. Note that we cannot rely on events emitted
    /// here to be processed before the window closes.
    ///
    /// Implementations should preserve state on [`DetachType::HiddenForClose`] or
    /// [`DetachType::Moved`] and clean up only on [`DetachType::Closed`].
    fn on_view_detached(&self, _detach_type: DetachType, _app: &mut AppContext) {}

    /// Starts this manager's parked shell if it has one, and does nothing
    /// otherwise.
    ///
    /// Idempotent: implementations consume the parked start with
    /// `Option::take`, so any number of calls fork at most one shell. Managers
    /// that never defer keep the default no-op.
    fn ensure_shell_started(&mut self, _ctx: &mut ModelContext<Box<dyn TerminalManager>>) {}

    /// Whether a shell start is parked and has not run yet.
    fn has_deferred_shell(&self) -> bool {
        false
    }

    /// Returns the startup directory retained by a parked shell.
    fn deferred_shell_startup_directory(&self) -> Option<PathBuf> {
        None
    }

    /// Returns the stable identifier of the supervised PTY session, if any.
    fn persistent_session_id(&self) -> Option<SessionId> {
        None
    }

    /// Stops this manager's local session before its pane moves to another process.
    fn shutdown_for_process_handoff(&mut self) {}

    /// Supplies a supervised PTY session to reattach when this manager starts.
    fn restore_persistent_session(
        &mut self,
        _session_id: SessionId,
        _ctx: &mut ModelContext<Box<dyn TerminalManager>>,
    ) {
    }

    /// Supplies a detached supervisor session for this manager to take over.
    ///
    /// Unlike [`Self::restore_persistent_session`], the pane has no scrollback
    /// of its own, so the session's screen is repainted on attach rather than
    /// left to the blocks restored from sqlite.
    fn attach_detached_session(
        &mut self,
        session_id: SessionId,
        ctx: &mut ModelContext<Box<dyn TerminalManager>>,
    ) {
        self.restore_persistent_session(session_id, ctx);
    }

    /// Supplies a command to run only when a restored PTY cannot be reattached.
    fn restore_cli_agent_resume_command(&mut self, _command: String) {}

    /// Supplies the supervisor that held this pane's session when it was saved, and the agent the
    /// session was running, so a restore can tell whether that supervisor still holds it.
    fn restore_session_holder(&mut self, _holder: SessionHolder, _agent: Option<HeldAgent>) {}

    /// Ends the held session where it is and then runs the withheld command here.
    #[must_use]
    fn take_over_held_session(
        &mut self,
        _ctx: &mut ModelContext<Box<dyn TerminalManager>>,
    ) -> bool {
        false
    }

    /// Runs the withheld command without touching the holder.
    #[must_use]
    fn run_withheld_restore(&mut self, _ctx: &mut ModelContext<Box<dyn TerminalManager>>) -> bool {
        false
    }

    #[cfg(test)]
    fn set_withheld_restore_for_test(&mut self, _restore: WithheldRestore) {}

    /// Replaces this manager's shell session in place, keeping the pane, its
    /// blocklist, and its agent conversation.
    ///
    /// `target` is the shell to launch; `None` relaunches the one currently
    /// running. `cwd` is the replacement shell's startup directory; `None`
    /// leaves it to the shell.
    ///
    /// Managers with no local session to replace (the remote-TTY viewer, the
    /// mock) keep the default no-op, exactly as [`Self::ensure_shell_started`]
    /// does -- there is nothing for them to tear down or fork.
    ///
    /// Returns whether a session was actually replaced. Callers must not act on
    /// a restart they only *asked* for: the agent-resume continuation is parked
    /// in the pane's pending-command queue and fires on the next
    /// `BlockCompleted`, so enqueueing it against a manager that did nothing
    /// injects `claude --resume <uuid>` into a live, untouched shell. See
    /// [`Self::supports_session_restart`] for deciding before the fact.
    #[must_use = "a manager that cannot restart returns false; acting on it anyway \
                  injects a resume command into a shell that never restarted"]
    fn restart_session(
        &mut self,
        _target: Option<ShellLaunchData>,
        _cwd: Option<PathBuf>,
        _ctx: &mut ModelContext<Box<dyn TerminalManager>>,
    ) -> bool {
        false
    }

    /// Attaches this pane again, in place, to the supervised session it lost the connection to,
    /// keeping its scrollback. Returns whether a reattach was started; managers with no
    /// supervised session keep the default and return `false`.
    #[must_use]
    fn reattach_session(&mut self, _ctx: &mut ModelContext<Box<dyn TerminalManager>>) -> bool {
        false
    }

    /// Whether [`Self::restart_session`] can do anything for this manager.
    ///
    /// The restart/change-shell commands are registered once, process-wide, and
    /// the binding registry's `with_enabled` predicate takes no context -- so
    /// there is no way to hide them per pane. This lets the *caller* decline
    /// before it puts a confirmation modal or a shell picker in front of the
    /// user promising something that will not happen: an SSH pane, the
    /// shared-session viewer and the mock all answer `false`.
    fn supports_session_restart(&self) -> bool {
        false
    }

    /// Returns this [`TerminalManager`] as an [`Any`], to support downcasting.
    fn as_any(&self) -> &dyn Any;

    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl warpui::Entity for Box<dyn TerminalManager> {
    type Event = ();
}

/// Spacing baked into block heights: the per-block padding, the height
/// reserved for the rendered Warp prompt (in lines), and whether blocks
/// reserve a footer row for the debug memory-stats overlay.
///
/// [`Self::for_gui`] derives the GUI blocklist's spacing from the user's
/// settings; frontends whose rendering differs (e.g. the row-based TUI
/// transcript) define their own spacing and pass it when creating the
/// terminal model.
pub struct BlockSpacing {
    pub block_padding: BlockPadding,
    pub warp_prompt_height_lines: f32,
    pub show_memory_stats: bool,
}

impl BlockSpacing {
    /// The GUI blocklist's spacing, derived from the user's settings: padding
    /// from the terminal-spacing style, the rendered Warp prompt's height, and
    /// the debug memory-stats footer toggle.
    pub(super) fn for_gui(ctx: &AppContext) -> Self {
        let appearance = Appearance::as_ref(ctx);
        let terminal_spacing =
            TerminalSettings::as_ref(ctx).terminal_spacing(appearance.line_height_ratio(), ctx);
        Self {
            block_padding: terminal_spacing.block_padding,
            warp_prompt_height_lines: WARP_PROMPT_HEIGHT_LINES,
            show_memory_stats: DebugSettings::as_ref(ctx).should_show_memory_stats(),
        }
    }
}

pub(super) fn compute_block_size(
    initial_size: Vector2F,
    block_spacing: &BlockSpacing,
    ctx: &mut AppContext,
) -> BlockSize {
    let appearance = Appearance::as_ref(ctx);
    let size_info = if ctx.is_headless() {
        // In headless mode, we don't actually have a font since we aren't rendering anything.
        // We skip the font-based size computation and hardcode a terminal size, so that
        // viewers of the shared session see a reasonable terminal width.
        SizeInfo::new_without_font_metrics(24, 120)
    } else {
        let font_cache = ctx.font_cache();
        create_size_info_for_blocklist(
            initial_size,
            font_cache,
            appearance.monospace_font_family(),
            appearance.monospace_font_size(),
            appearance.ui_builder().line_height_ratio(),
        )
    };
    let maximum_grid_size = *TerminalSettings::as_ref(ctx).maximum_grid_size.value();
    BlockSize {
        block_padding: block_spacing.block_padding,
        size: size_info,
        max_block_scroll_limit: maximum_grid_size,
        warp_prompt_height_lines: block_spacing.warp_prompt_height_lines,
    }
}

/// Creates a [`TerminalModel`], the source of truth for the session's state.
///
/// `block_spacing` is the frontend's spacing baked into block heights;
/// the GUI's settings-driven frontends derive it via [`BlockSpacing::for_gui`].
#[allow(clippy::too_many_arguments)]
pub(super) fn create_terminal_model(
    startup_directory: Option<PathBuf>,
    restored_blocks: Option<&Vec<SerializedBlockListItem>>,
    initial_size: Vector2F,
    channel_event_proxy: ChannelEventListener,
    shell_state: ShellLaunchState,
    block_spacing: BlockSpacing,
    ctx: &mut AppContext,
) -> TerminalModel {
    let (should_show_bootstrap_block, should_show_in_band_command_blocks) = {
        let settings = BlockVisibilitySettings::as_ref(ctx);
        (
            *settings.should_show_bootstrap_block.value(),
            *settings.should_show_in_band_command_blocks.value(),
        )
    };
    let honor_ps1 = *SessionSettings::as_ref(ctx).honor_ps1;
    let input_mode = *InputModeSettings::as_ref(ctx).input_mode.value();
    let is_inverted = input_mode.is_inverted_blocklist();

    let sizes = compute_block_size(initial_size, &block_spacing, ctx);

    let obfuscate_secrets = get_secret_obfuscation_mode(ctx);
    let is_ai_ugc_telemetry_enabled =
        should_collect_ai_ugc_telemetry(ctx, PrivacySettings::as_ref(ctx).is_telemetry_enabled);

    TerminalModel::new(
        restored_blocks.map(|v| v.as_slice()),
        sizes,
        terminal_colors_list(ctx),
        channel_event_proxy,
        ctx.background_executor().clone(),
        should_show_bootstrap_block,
        should_show_in_band_command_blocks,
        block_spacing.show_memory_stats,
        honor_ps1,
        is_inverted,
        obfuscate_secrets,
        is_ai_ugc_telemetry_enabled,
        startup_directory,
        shell_state,
    )
}

pub(super) fn terminal_colors_list(ctx: &AppContext) -> color::List {
    let appearance = Appearance::as_ref(ctx);
    let theme = appearance.theme();
    color::List::from(&theme.to_owned().into())
}
