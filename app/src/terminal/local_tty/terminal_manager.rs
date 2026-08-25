use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{SendError, SyncSender};
use std::thread::JoinHandle;

use ai::api_keys::ApiKeyManager;
use anyhow::Context as _;
use async_broadcast::InactiveReceiver;
#[cfg(unix)]
use nix::sys::termios::LocalFlags;
use parking_lot::{FairMutex, Mutex};
use pathfinder_geometry::vector::Vector2F;
use settings::Setting as _;
use typed_path::WindowsPath;
use warp_core::SessionId;
use warp_errors::report_error;
use warp_util::path::{
    convert_windows_host_to_wsl_path, convert_windows_native_to_msys2_path, msys2_exe_to_root,
};
use warpui::r#async::SpawnedFutureHandle;
use warpui::r#async::executor::Background;
use warpui::{AppContext, Entity, ModelContext, ModelHandle, SingletonEntity, ViewHandle};

use super::event_loop::EventLoop;
use super::shell::{ShellStarter, ShellStarterSource, ShellStarterSourceOrWslName};
#[cfg(unix)]
use super::terminal_attributes::TerminalAttributesPoller;
use super::{mio_channel, recorder};
use crate::ai::aws_credentials::AwsCredentialRefresher as _;
use crate::ai::blocklist::SerializedBlockListItem;
use crate::auth::AuthStateProvider;
use crate::auth::auth_state::AuthState;
use crate::banner::BannerState;
use crate::context_chips::ContextChipKind;
use crate::context_chips::prompt::Prompt;
use crate::features::FeatureFlag;
use crate::persistence::ModelEvent;
use crate::send_telemetry_on_executor;
use crate::server::telemetry::TelemetryEvent;
use crate::settings::{DebugSettings, PrivacySettings, SshSettings};
use crate::terminal::available_shells::{AvailableShell, AvailableShells};
use crate::terminal::color::List as ColorList;
use crate::terminal::event_listener::ChannelEventListener;
#[cfg(unix)]
use crate::terminal::local_tty::terminal_attributes::Event as TerminalAttributesPollerEvent;
use crate::terminal::local_tty::{Pty, PtyOptions};
use crate::terminal::model::session::Sessions;
#[cfg(unix)]
use crate::terminal::model::terminal_model::BlockIndex;
use crate::terminal::model::terminal_model::ExitReason;
#[cfg(unix)]
use crate::terminal::model_events::ModelEvent as TerminalModelEvent;
use crate::terminal::model_events::{ModelEventDispatcher, SshRemoteServerSupport};
use crate::terminal::session_settings::{SessionSettings, ToolbarChipSelection};
use crate::terminal::shared_session::sharer::network::Network;
use crate::terminal::shared_session::{IsSharedSessionCreator, SharedSessionStatus};
use crate::terminal::shell::ShellName;
use crate::terminal::shell_start_policy::ShellStartPolicy;
use crate::terminal::terminal_manager::BlockSpacing;
use crate::terminal::warpify::settings::WarpifySettings;
use crate::terminal::writeable_pty::pty_controller::{EventLoopSendError, EventLoopSender};
use crate::terminal::writeable_pty::terminal_manager_util::{
    init_pty_controller_model, init_remote_server_controller, wire_up_pty_controller_with_surface,
};
use crate::terminal::writeable_pty::{self, Message, PtyIntentEvent, TerminalSurface};
use crate::terminal::{
    PTY_READS_BROADCAST_CHANNEL_SIZE, ShellLaunchData, ShellLaunchState, SizeInfo,
    TerminalManager as TerminalManagerTrait, TerminalModel, terminal_manager,
};

type PtyController = writeable_pty::PtyController<mio_channel::Sender<Message>>;
type RemoteServerController =
    writeable_pty::remote_server_controller::RemoteServerController<mio_channel::Sender<Message>>;

/// Owns a local terminal session: the terminal model, PTY event loop, PTY
/// controller, and a terminal surface.
///
/// Holds onto data that needs to live as long as the session does (e.g. the
/// event loop join handle).
pub struct TerminalManager<S> {
    event_loop_tx: Arc<Mutex<mio_channel::Sender<Message>>>,
    /// This is an `Option` so that we can take ownership of the inner
    /// `JoinHandle` in `TerminalManager::drop`.
    event_loop_handle: Option<JoinHandle<()>>,
    pub(super) model: Arc<FairMutex<TerminalModel>>,
    pub(super) view: ViewHandle<S>,

    /// The manager is responsible for managing the lifetime
    /// of the terminal attributes poller. None if the event loop has not yet started.
    #[cfg(unix)]
    #[allow(dead_code)]
    terminal_attributes_poller: Option<ModelHandle<TerminalAttributesPoller>>,

    /// The manager is responsible for managing the lifetime
    /// of the PTY controller.
    pty_controller: ModelHandle<PtyController>,

    /// The manager is responsible for managing the lifetime of the remote server controller.
    remote_server_controller: ModelHandle<RemoteServerController>,

    /// The process ID of the PTY. Purely used for integration tests. None if the PTY has not yet
    /// been started.
    #[cfg(feature = "integration_tests")]
    pub(super) pid: Option<u32>,

    /// An inactive receiver for PTY reads that we can upgrade to an active
    /// receiver as needed. We prefer to not create active receivers eagerly
    /// to avoid unnecessary allocations of data coming from the PTY (high throughput).
    /// Note that we need to hold onto the inactive receiver so that the channel isn't closed prematurely.
    inactive_pty_reads_rx: InactiveReceiver<Arc<Vec<u8>>>,

    /// The sharer side of the session sharing protocol. [`Some`] only when a
    /// shared session connection is ongoing.
    pub(super) session_sharer: Rc<RefCell<Option<ModelHandle<Network>>>>,

    /// Inputs for a shell that has not been forked yet. `None` once the shell
    /// has started, or immediately for [`ShellStartPolicy::Eager`] managers.
    ///
    /// `ensure_shell_started` consumes this with `Option::take`, which is what
    /// makes starting idempotent — there is no separate flag or state machine
    /// that could disagree with it, and no way to fork twice.
    pub(super) deferred_shell: Option<DeferredShellStart>,

    /// Everything a *second* session needs that the first one's
    /// [`DeferredShellStart`] consumed. See [`SessionTemplate`].
    session_template: SessionTemplate,

    /// Which session this manager is currently forking or running.
    ///
    /// [`Self::spawn_shell_start`] captures this, and its callback refuses to
    /// run `on_shell_determined` against any other value. A restart bumps it,
    /// which is what makes an already-in-flight start inert.
    ///
    /// Aborting the pending future is not sufficient on its own:
    /// `ModelContext::spawn` honours an abort only at the future's next poll,
    /// so a start whose future has already resolved still delivers its
    /// callback. Without this counter, a restart racing an in-flight start
    /// creates a second PTY whose `event_loop_handle` *overwrites* the first --
    /// detaching that thread, orphaning its shell process for the app's
    /// lifetime, and interleaving two shells into one blocklist. The window is
    /// at least one tick for every eager pane and seconds for a WSL shell,
    /// whose starter resolution shells out to `wsl ... printenv`.
    session_generation: u64,

    /// The shell start that has been spawned but has not reached
    /// `on_shell_determined` yet, retained so a restart can cancel its
    /// background work. Best effort only -- see `session_generation`.
    pending_shell_start: Option<SpawnedFutureHandle>,

    /// Test-only: how many times a PTY event loop has been installed on this
    /// manager.
    ///
    /// This is the quantity [`Self::session_generation`] exists to protect. A
    /// superseded start that is not discarded installs a *second* event loop,
    /// overwriting `event_loop_handle` and detaching the first thread along
    /// with its shell process. Nothing in production observes that -- there is
    /// no log line and no error, just a stray shell for the app's lifetime --
    /// so the guard needs a hook to be testable at all.
    #[cfg(test)]
    established_session_count: usize,

    /// Test-only: the startup directory most recently handed to
    /// `create_pty`, in whatever final form `on_shell_determined` settled on
    /// (native form, the WSL home-directory fallback, or `None`). Nothing in
    /// production observes this once it crosses into `PtyOptions` -- it ends
    /// up as an environment variable read by the shell's own bootstrap --
    /// so, like `established_session_count`, the guard needs a hook to be
    /// testable at all.
    #[cfg(test)]
    last_resolved_startup_directory: Option<PathBuf>,
}

/// The inputs a replacement session needs, retained for the lifetime of the
/// manager because the first session's [`DeferredShellStart`] is moved into
/// `spawn_shell_start` and gone by the time a restart runs.
///
/// The event proxy and (on unix) the dispatcher are the manager's long-lived
/// async plumbing rather than per-session state -- see
/// [`TerminalManager::build_session_resources`], which takes clones of exactly
/// these two. `env_vars` and the banner handle are the pane's own launch
/// inputs, kept so a restart relaunches *this* pane -- its session focus env
/// vars included -- rather than a default one.
struct SessionTemplate {
    channel_event_proxy: ChannelEventListener,
    #[cfg(unix)]
    model_events: ModelHandle<ModelEventDispatcher>,
    env_vars: HashMap<OsString, OsString>,
    user_default_shell_unsupported_banner_model_handle: ModelHandle<BannerState>,
}

/// Shared inputs needed to construct a terminal surface for a local PTY.
pub struct TerminalSurfaceInit {
    pub wakeups_rx: async_channel::Receiver<()>,
    pub model_events: ModelHandle<ModelEventDispatcher>,
    pub model: Arc<FairMutex<TerminalModel>>,
    pub sessions: ModelHandle<Sessions>,
    pub size_info: SizeInfo,
    pub colors: ColorList,
    pub inactive_pty_reads_rx: InactiveReceiver<Arc<Vec<u8>>>,
}

#[cfg(any(test, all(feature = "tui", feature = "test-util")))]
impl TerminalSurfaceInit {
    /// Creates mock terminal surface inputs without spawning a PTY.
    pub fn new_for_test(ctx: &mut AppContext) -> Self {
        let (_wakeups_tx, wakeups_rx) = async_channel::unbounded();
        let (_events_tx, events_rx) = async_channel::unbounded();
        let (pty_reads_tx, pty_reads_rx) =
            async_broadcast::broadcast(PTY_READS_BROADCAST_CHANNEL_SIZE);
        drop(pty_reads_tx);
        let sessions = ctx.add_model(|_| Sessions::new_for_test());
        let model_events =
            ctx.add_model(|ctx| ModelEventDispatcher::new(events_rx, sessions.clone(), ctx));
        let model = Arc::new(FairMutex::new(TerminalModel::mock(None, None)));
        let colors = model.lock().colors();
        let size_info = model.lock().block_list().size().to_owned();
        Self {
            wakeups_rx,
            model_events,
            model,
            sessions,
            size_info,
            colors,
            inactive_pty_reads_rx: pty_reads_rx.deactivate(),
        }
    }
}

/// A newly constructed terminal surface and its manager post-wiring callback.
pub struct TerminalSurfaceResult<S, PostWire> {
    pub surface: ViewHandle<S>,
    pub post_wire: PostWire,
}

/// One-shot resources consumed when the shell is determined and the PTY starts.
struct ShellStartupResources {
    event_loop_rx: mio_channel::Receiver<Message>,
    channel_event_proxy: ChannelEventListener,
    #[cfg(unix)]
    model_events: ModelHandle<ModelEventDispatcher>,
}

/// The complete set of inputs [`on_shell_determined`] needs, parked so the
/// shell can be forked later.
///
/// Holds exactly what the eager path's `ctx.spawn` closure captures. Nothing
/// else in `create_model_with_manager` depends on the shell existing, which is
/// why deferral needs no other plumbing.
///
/// The shell is *selected* eagerly regardless of policy: `ShellStarter::init`
/// runs at construction (see the `preferred_shell` resolution above), and it
/// is that already-chosen `wsl_name_or_shell_starter` which gets parked here.
/// Only the *fork* -- resolving the starter source and running
/// `on_shell_determined` -- is deferred. This is what makes an eager and a
/// deferred pane start the same shell.
pub(super) struct DeferredShellStart {
    startup_directory: Option<PathBuf>,
    env_vars: HashMap<OsString, OsString>,
    user_default_shell_unsupported_banner_model_handle: ModelHandle<BannerState>,
    shell_startup_resources: ShellStartupResources,
    /// The async block's input: either a resolved starter source or a WSL
    /// distro name that still needs a WSL instance to resolve.
    wsl_name_or_shell_starter: Option<ShellStarterSourceOrWslName>,
}

/// Handles created for a local terminal manager and its surface.
pub struct TerminalManagerInit<S> {
    pub manager: ModelHandle<Box<dyn TerminalManagerTrait>>,
    pub surface: ViewHandle<S>,
}
/// Adapts a TUI-owned surface manager to Warp's type-erased manager contract.
struct TuiTerminalManager<S>(TerminalManager<S>);

impl<S: TerminalSurface> TerminalManagerTrait for TuiTerminalManager<S>
where
    <S as Entity>::Event: PtyIntentEvent,
{
    fn model(&self) -> Arc<FairMutex<TerminalModel>> {
        self.0.model()
    }

    fn ensure_shell_started(&mut self, ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>) {
        self.0.start_deferred_shell(ctx)
    }

    fn has_deferred_shell(&self) -> bool {
        self.0.is_shell_deferred()
    }

    fn restart_session(
        &mut self,
        target: Option<ShellLaunchData>,
        cwd: Option<PathBuf>,
        ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>,
    ) {
        self.0.restart_session_inner(target, cwd, ctx)
    }

    fn as_any(&self) -> &dyn Any {
        &self.0
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        &mut self.0
    }
}

impl<S> Drop for TerminalManager<S> {
    fn drop(&mut self) {
        self.shutdown_event_loop();
    }
}

impl<S> TerminalManager<S> {
    /// Creates a local terminal manager model and terminal surface.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_model<PostWire>(
        startup_directory: Option<PathBuf>,
        env_vars: HashMap<OsString, OsString>,
        is_shared_session_creator: IsSharedSessionCreator,
        all_restored_blocks: Option<&Vec<SerializedBlockListItem>>,
        user_default_shell_unsupported_banner_model_handle: ModelHandle<BannerState>,
        initial_size: Vector2F,
        model_event_sender: Option<SyncSender<ModelEvent>>,
        chosen_shell: Option<AvailableShell>,
        shell_start: ShellStartPolicy,
        ctx: &mut AppContext,
        create_surface: impl FnOnce(
            TerminalSurfaceInit,
            &mut AppContext,
        ) -> TerminalSurfaceResult<S, PostWire>,
    ) -> TerminalManagerInit<S>
    where
        S: TerminalSurface,
        <S as Entity>::Event: PtyIntentEvent,
        Self: TerminalManagerTrait,
        PostWire: FnOnce(&mut Self, &ViewHandle<S>, &mut AppContext),
    {
        Self::create_model_with_manager(
            startup_directory,
            env_vars,
            is_shared_session_creator,
            all_restored_blocks,
            user_default_shell_unsupported_banner_model_handle,
            initial_size,
            model_event_sender,
            chosen_shell,
            shell_start,
            BlockSpacing::for_gui(ctx),
            SshRemoteServerSupport::Enabled,
            ctx,
            create_surface,
            |manager| Box::new(manager),
        )
    }

    /// Creates a local terminal manager for a TUI-owned terminal surface.
    /// `block_spacing` is the TUI frontend's spacing baked into block heights.
    #[allow(clippy::too_many_arguments)]
    pub fn create_tui_model<PostWire>(
        startup_directory: Option<PathBuf>,
        env_vars: HashMap<OsString, OsString>,
        is_shared_session_creator: IsSharedSessionCreator,
        all_restored_blocks: Option<&Vec<SerializedBlockListItem>>,
        user_default_shell_unsupported_banner_model_handle: ModelHandle<BannerState>,
        initial_size: Vector2F,
        model_event_sender: Option<SyncSender<ModelEvent>>,
        chosen_shell: Option<AvailableShell>,
        shell_start: ShellStartPolicy,
        block_spacing: BlockSpacing,
        ctx: &mut AppContext,
        create_surface: impl FnOnce(
            TerminalSurfaceInit,
            &mut AppContext,
        ) -> TerminalSurfaceResult<S, PostWire>,
    ) -> TerminalManagerInit<S>
    where
        S: TerminalSurface,
        <S as Entity>::Event: PtyIntentEvent,
        PostWire: FnOnce(&mut Self, &ViewHandle<S>, &mut AppContext),
    {
        Self::create_model_with_manager(
            startup_directory,
            env_vars,
            is_shared_session_creator,
            all_restored_blocks,
            user_default_shell_unsupported_banner_model_handle,
            initial_size,
            model_event_sender,
            chosen_shell,
            shell_start,
            block_spacing,
            SshRemoteServerSupport::Disabled,
            ctx,
            create_surface,
            |manager| Box::new(TuiTerminalManager(manager)),
        )
    }

    /// Builds one session's worth of event-loop plumbing.
    ///
    /// Extracted from `create_model_with_manager` so a restart can build a second
    /// set. `ShellStartupResources` is consumed when the PTY starts, so a manager
    /// that has already run a shell cannot reuse its first set.
    ///
    /// `channel_event_proxy` and (on unix) `model_events` are long-lived handles
    /// tied to the manager's async event channels, not to any one shell session --
    /// callers pass a clone and keep using their own copy afterwards. Only the
    /// `mio_channel` pair is freshly constructed here, because its `Receiver` is
    /// moved into the PTY event loop and consumed exactly once per session.
    fn build_session_resources(
        channel_event_proxy: ChannelEventListener,
        #[cfg(unix)] model_events: ModelHandle<ModelEventDispatcher>,
    ) -> (ShellStartupResources, mio_channel::Sender<Message>) {
        let (event_loop_tx, event_loop_rx) = mio_channel::channel();
        (
            ShellStartupResources {
                event_loop_rx,
                channel_event_proxy,
                #[cfg(unix)]
                model_events,
            },
            event_loop_tx,
        )
    }

    /// Creates a manager using the supplied type-erasure adapter.
    #[allow(clippy::too_many_arguments)]
    fn create_model_with_manager<PostWire, BoxManager>(
        startup_directory: Option<PathBuf>,
        env_vars: HashMap<OsString, OsString>,
        is_shared_session_creator: IsSharedSessionCreator,
        all_restored_blocks: Option<&Vec<SerializedBlockListItem>>,
        user_default_shell_unsupported_banner_model_handle: ModelHandle<BannerState>,
        initial_size: Vector2F,
        model_event_sender: Option<SyncSender<ModelEvent>>,
        chosen_shell: Option<AvailableShell>,
        shell_start: ShellStartPolicy,
        block_spacing: BlockSpacing,
        ssh_remote_server_support: SshRemoteServerSupport,
        ctx: &mut AppContext,
        create_surface: impl FnOnce(
            TerminalSurfaceInit,
            &mut AppContext,
        ) -> TerminalSurfaceResult<S, PostWire>,
        box_manager: BoxManager,
    ) -> TerminalManagerInit<S>
    where
        S: TerminalSurface,
        <S as Entity>::Event: PtyIntentEvent,
        PostWire: FnOnce(&mut Self, &ViewHandle<S>, &mut AppContext),
        BoxManager: FnOnce(Self) -> Box<dyn TerminalManagerTrait> + 'static,
    {
        let (wakeups_tx, wakeups_rx) = async_channel::unbounded();
        let (events_tx, events_rx) = async_channel::unbounded();
        let (executor_command_tx, executor_command_rx) = async_channel::unbounded();

        // Create the broadcast channel to receive data from the PTY, but deactivate it immediately.
        // We only want to create active receivers as necessary.
        let (pty_reads_tx, pty_reads_rx) =
            async_broadcast::broadcast(PTY_READS_BROADCAST_CHANNEL_SIZE);
        let inactive_pty_reads_rx = pty_reads_rx.deactivate();

        let channel_event_proxy = ChannelEventListener::new(wakeups_tx, events_tx, pty_reads_tx);

        // Initialize the sessions model.
        let sessions = ctx.add_model(|ctx| Sessions::new(executor_command_tx.clone(), ctx));

        let model_events = ctx.add_model(|ctx| {
            ModelEventDispatcher::new_with_ssh_remote_server_support(
                events_rx,
                sessions.clone(),
                ssh_remote_server_support,
                ctx,
            )
        });

        let (shell_startup_resources, event_loop_tx) = Self::build_session_resources(
            channel_event_proxy.clone(),
            #[cfg(unix)]
            model_events.clone(),
        );

        let preferred_shell = chosen_shell.unwrap_or_else(|| {
            AvailableShells::handle(ctx)
                .read(ctx, |shells, ctx| shells.get_user_preferred_shell(ctx))
        });
        let wsl_name_or_shell_starter = ShellStarter::init(preferred_shell.clone());

        // Create the terminal model with all restored blocks
        log::info!(
            "Creating terminal model with {} restored blocks",
            all_restored_blocks
                .as_ref()
                .map(|blocks| blocks.len())
                .unwrap_or(0)
        );
        let model = terminal_manager::create_terminal_model(
            startup_directory.clone(),
            all_restored_blocks,
            initial_size,
            channel_event_proxy.clone(),
            ShellLaunchState::DeterminingShell {
                available_shell: Some(preferred_shell),
                display_name: wsl_name_or_shell_starter
                    .as_ref()
                    .map(|wsl_name_or_shell_starter| wsl_name_or_shell_starter.name())
                    .unwrap_or(ShellName::LessDescriptive("Shell".to_owned())),
            },
            block_spacing,
            ctx,
        );
        let colors = model.colors();
        let model = Arc::new(FairMutex::new(model));

        // Have ApiKeyManager subscribe to block completion events for AWS credential refresh.
        // This must happen after `model` is created, since the subscription needs it to resolve
        // lazily-computed `UserBlockCompleted` fields.
        ApiKeyManager::handle(ctx).update(ctx, |manager, ctx| {
            manager.register_model_event_dispatcher(&model_events, model.clone(), ctx);
        });

        // This is purely for measuring throughput on WarpDev.
        if FeatureFlag::RecordPtyThroughput.is_enabled() {
            let auth_state = AuthStateProvider::as_ref(ctx).get();
            recorder::record_pty_throughput(
                inactive_pty_reads_rx.clone().activate(),
                model.clone(),
                auth_state.clone(),
                ctx.background_executor().to_owned(),
            );
        }

        // If this session should be a shared-session creator, configure its initial
        // shared-session state before the surface is constructed, so that bootstrap
        // events can observe the correct pending status and source type.
        match is_shared_session_creator {
            IsSharedSessionCreator::Yes { source }
                if FeatureFlag::CreatingSharedSessions.is_enabled() =>
            {
                model.lock().set_shared_session_status(
                    SharedSessionStatus::SharePendingPreBootstrap { source },
                );
                log::info!("Configured terminal to start sharing after bootstrap");
            }
            IsSharedSessionCreator::Yes { .. } => {
                log::warn!(
                    "Session sharing was requested, but CreatingSharedSessions is disabled; \
                     skipping shared-session startup"
                );
            }
            IsSharedSessionCreator::No => {}
        }

        // Initialize the PtyController.
        let pty_controller = init_pty_controller_model(
            event_loop_tx.clone(),
            executor_command_rx,
            model_events.clone(),
            sessions.clone(),
            model.clone(),
            ctx,
        );

        // Initialize the RemoteServerController.
        let remote_server_controller =
            init_remote_server_controller(&pty_controller, &model_events, ctx);
        let size_info = model.lock().block_list().size().to_owned();
        let TerminalSurfaceResult { surface, post_wire } = create_surface(
            TerminalSurfaceInit {
                wakeups_rx,
                model_events: model_events.clone(),
                model: model.clone(),
                sessions: sessions.clone(),
                size_info,
                colors,
                inactive_pty_reads_rx: inactive_pty_reads_rx.clone(),
            },
            ctx,
        );
        wire_up_pty_controller_with_surface(
            &pty_controller,
            &surface,
            model.clone(),
            sessions.clone(),
            model_event_sender,
            ctx,
        );
        let mut terminal_manager = Self {
            event_loop_tx: Arc::new(Mutex::new(event_loop_tx)),
            model,
            event_loop_handle: None,
            view: surface.clone(),
            #[cfg(unix)]
            terminal_attributes_poller: None,
            pty_controller,
            remote_server_controller,
            #[cfg(feature = "integration_tests")]
            pid: None,
            inactive_pty_reads_rx,
            session_sharer: Rc::new(RefCell::new(None)),
            deferred_shell: None,
            session_generation: 0,
            pending_shell_start: None,
            #[cfg(test)]
            established_session_count: 0,
            #[cfg(test)]
            last_resolved_startup_directory: None,
            session_template: SessionTemplate {
                channel_event_proxy,
                #[cfg(unix)]
                model_events,
                env_vars: env_vars.clone(),
                user_default_shell_unsupported_banner_model_handle:
                    user_default_shell_unsupported_banner_model_handle.clone(),
            },
        };

        // Run surface-specific wiring after the manager exists, because this
        // step may need manager-owned controllers and retained handles.
        post_wire(&mut terminal_manager, &surface, ctx);

        let terminal_surface = surface.clone();

        let terminal_manager_model = ctx.add_model(|ctx| match shell_start {
            ShellStartPolicy::Deferred => {
                log::info!("Deferring shell startup for this pane until activation");
                terminal_manager.deferred_shell = Some(DeferredShellStart {
                    startup_directory,
                    env_vars,
                    user_default_shell_unsupported_banner_model_handle,
                    shell_startup_resources,
                    wsl_name_or_shell_starter,
                });
                box_manager(terminal_manager)
            }
            ShellStartPolicy::Eager => {
                terminal_manager.spawn_shell_start(
                    DeferredShellStart {
                        startup_directory,
                        env_vars,
                        user_default_shell_unsupported_banner_model_handle,
                        shell_startup_resources,
                        wsl_name_or_shell_starter,
                    },
                    ctx,
                );
                box_manager(terminal_manager)
            }
        });

        TerminalManagerInit {
            manager: terminal_manager_model,
            surface: terminal_surface,
        }
    }

    /// Resolves the shell starter off the main thread and hands the result to
    /// [`on_shell_determined`]. This is the body the eager path runs at
    /// construction and the deferred path runs from `ensure_shell_started`.
    ///
    /// The spawned start is stamped with the current
    /// [`Self::session_generation`], and its callback discards itself if the
    /// manager has moved on to another session by the time the shell starter
    /// resolves.
    pub(super) fn spawn_shell_start(
        &mut self,
        parked: DeferredShellStart,
        ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>,
    ) where
        S: TerminalSurface,
        <S as Entity>::Event: PtyIntentEvent,
    {
        let DeferredShellStart {
            startup_directory,
            env_vars,
            user_default_shell_unsupported_banner_model_handle,
            shell_startup_resources,
            wsl_name_or_shell_starter,
        } = parked;

        let generation = self.session_generation;
        let handle = ctx.spawn(
            async move {
                match wsl_name_or_shell_starter {
                    Some(starter_source) => starter_source.to_shell_starter_source().await,
                    None => None,
                }
            },
            move |terminal_manager: &mut Box<dyn TerminalManagerTrait>,
                  shell_starter_source,
                  ctx| {
                let Some(terminal_manager) =
                    TerminalManagerTrait::as_any_mut(terminal_manager.as_mut())
                        .downcast_mut::<Self>()
                else {
                    return;
                };

                if terminal_manager.session_generation != generation {
                    // A restart superseded this start while its shell starter
                    // was resolving. Running on from here would fork a PTY for
                    // a session that no longer exists and overwrite the live
                    // session's event loop handle, detaching that thread.
                    log::info!(
                        "Discarding a superseded shell start: it belongs to session {generation}, \
                         but this pane is now on session {}",
                        terminal_manager.session_generation
                    );
                    return;
                }
                terminal_manager.pending_shell_start = None;

                on_shell_determined(
                    terminal_manager,
                    startup_directory,
                    env_vars,
                    user_default_shell_unsupported_banner_model_handle,
                    shell_startup_resources,
                    shell_starter_source,
                    ctx,
                )
            },
        );

        // Retained so a restart can cancel the background work (for WSL, a
        // `wsl ... printenv SHELL` subprocess) where it still can.
        self.pending_shell_start = Some(handle);
    }

    /// The sole body of `TerminalManagerTrait::ensure_shell_started`. Both the
    /// concrete `impl TerminalManagerTrait for TerminalManager<TerminalView>`
    /// (terminal_view_adaptor.rs) and `TuiTerminalManager`'s forwarding impl
    /// below call this rather than duplicating its body, so there is exactly
    /// one `Option::take` on `deferred_shell` in the whole crate: deleting this
    /// method is a compile error at both call sites, not silent recursion.
    pub(super) fn start_deferred_shell(
        &mut self,
        ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>,
    ) where
        S: TerminalSurface,
        <S as Entity>::Event: PtyIntentEvent,
    {
        let Some(parked) = self.deferred_shell.take() else {
            return;
        };
        self.spawn_shell_start(parked, ctx);
    }

    /// The sole body of `TerminalManagerTrait::has_deferred_shell`. See
    /// [`Self::start_deferred_shell`] for why this is shared rather than
    /// duplicated.
    pub(super) fn is_shell_deferred(&self) -> bool {
        self.deferred_shell.is_some()
    }

    /// Returns the terminal model owned by this manager.
    pub(crate) fn model(&self) -> Arc<FairMutex<TerminalModel>> {
        self.model.clone()
    }

    /// Returns the remote server controller owned by this manager.
    pub(super) fn remote_server_controller(&self) -> ModelHandle<RemoteServerController> {
        self.remote_server_controller.clone()
    }

    /// Ends this manager's session for good: stops the event loop and closes
    /// the PTY reads broadcast channel.
    ///
    /// Only correct when nothing will use this manager again. Closing the
    /// broadcast channel is not reversible and a replacement session publishes
    /// to that same channel, so a restart calls [`Self::stop_event_loop`]
    /// instead.
    pub(super) fn shutdown_event_loop(&mut self) {
        self.stop_event_loop();
        self.inactive_pty_reads_rx.close();
    }

    /// Sends a shutdown message to the PTY event loop and waits for it to
    /// process that event.
    ///
    /// Blocking on the join is what makes a restart orderable: the event
    /// loop's last act before its thread ends is `TerminalModel::exit`, so once
    /// this returns the replaced session can no longer queue a terminal event,
    /// and a barrier posted afterwards is necessarily the last one.
    fn stop_event_loop(&mut self) {
        let shutdown_res = self.event_loop_tx.lock().send(Message::Shutdown);
        // Happens normally if the event loop has already been terminated (so the channel is now gone).
        if let Err(e) = shutdown_res {
            log::info!("Failed to send Shutdown {e:?}");
        }

        match self.event_loop_handle.take() {
            Some(join_handle) => {
                if let Err(e) = join_handle.join() {
                    report_error!(
                        "Failed to join event loop handle",
                        extra: { "error" => ?e }
                    );
                }
            }
            _ => {
                if self.deferred_shell.is_some() || self.pending_shell_start.is_some() {
                    // Routine: this pane's shell was deferred and never started
                    // (e.g. a background launch-config tab the user never
                    // visited), or its start is still resolving, so no event
                    // loop was ever spawned and there is nothing to join. A
                    // 141-pane launch config can close 135 such panes at once,
                    // and a user can restart a pane repeatedly; logging either
                    // as a warning would be spurious noise.
                    log::debug!(
                        "No event loop handle to join when stopping terminal session: no shell has started yet."
                    );
                } else {
                    // Anomalous: a shell should have been started and produced
                    // an event loop handle.
                    log::warn!("No event loop handle to join when stopping terminal session.");
                }
            }
        }
    }

    /// The sole body of `TerminalManagerTrait::restart_session`. Shared by the
    /// concrete `impl TerminalManagerTrait for TerminalManager<TerminalView>`
    /// (terminal_view_adaptor.rs) and `TuiTerminalManager`'s forwarding impl
    /// above for the same reason [`Self::start_deferred_shell`] is: the
    /// sequence below is ordering-sensitive, and a duplicated copy is a copy
    /// that can drift out of that order.
    ///
    /// `target` is the shell to launch, or `None` to relaunch the one this
    /// pane is already running. `cwd` is the replacement's startup directory.
    pub(super) fn restart_session_inner(
        &mut self,
        target: Option<ShellLaunchData>,
        cwd: Option<PathBuf>,
        ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>,
    ) where
        S: TerminalSurface,
        <S as Entity>::Event: PtyIntentEvent,
    {
        // 1. Mark the restart on the surface *before* anything can exit, so no
        //    exit produced below is mistaken for a shell that died. Until the
        //    barrier posted in step 5 comes back round, the view suppresses the
        //    whole teardown -- including `Event::Exited`, which `TerminalPane`
        //    turns into `close_pane`.
        //
        //    Deliberately not `TerminalView::shutdown_pty`, the other way to
        //    stop a PTY: that sets `manual_pty_shutdown_requested`, and that
        //    flag's own doc says it stays true for the view's entire lifetime,
        //    which would permanently suppress `AgentExitedShellProcess`
        //    telemetry for every genuine agent-caused exit in the pane
        //    afterwards. A restart tears the event loop down directly instead
        //    and leaves that flag alone.
        self.view
            .update(ctx, |surface, ctx| surface.on_session_restart_started(ctx));

        // 2. Tear the replaced session down. This joins the event loop thread,
        //    whose last act is `TerminalModel::exit`, so once it returns that
        //    session can no longer touch the model: whatever `Exit` it queued
        //    is already in the event channel, and nothing more can be queued
        //    behind it. Unlike the drop path this leaves the PTY reads
        //    broadcast channel open, because the replacement publishes to it.
        //
        //    Runs before step 3 so `stop_event_loop` can still see that this
        //    pane's shell was parked or still resolving, and log a missing
        //    event loop handle as routine rather than anomalous.
        self.stop_event_loop();

        // 3. Retire the replaced session's identity. Any shell start still
        //    resolving belongs to it and is now inert: the callback compares
        //    generations and discards itself rather than forking a PTY whose
        //    event loop handle would overwrite -- and thereby detach -- the one
        //    this restart is about to install. Aborting is only a best-effort
        //    way to stop the background work early; the counter is what makes
        //    it correct. Nothing can run in between: `join` blocks the main
        //    thread, so no spawned callback is dispatched during step 2.
        self.session_generation = self.session_generation.wrapping_add(1);
        if let Some(pending) = self.pending_shell_start.take() {
            pending.abort();
        }

        // A parked start is superseded too -- the shell is being forked here
        // and now. Leaving it parked would fork a second one the next time the
        // pane is activated.
        self.deferred_shell = None;

        // 4. Stop the replaced session's terminal-attributes poller, which
        //    holds its PTY fd on a repeating timer. `on_shell_determined` would
        //    otherwise simply overwrite the handle once the replacement is up,
        //    leaving the old poller to `tcgetattr` a closed -- and possibly
        //    already recycled -- descriptor in between.
        #[cfg(unix)]
        if let Some(poller) = self.terminal_attributes_poller.take() {
            poller.update(ctx, |poller, _| poller.stop_polling());
        }

        // 5. Post the restart barrier. It travels the same FIFO channel as the
        //    replaced shell's own exit, and step 2 guarantees nothing can slip
        //    in behind it, so the view observing it is proof that the old exit
        //    has already been delivered and suppressed. That is what lets
        //    `TerminalView::finish_restart` run at a provably safe point rather
        //    than a hopeful one. See `TerminalModel::exit_for_restart`.
        self.model.lock().exit_for_restart();

        // 6. Build a second set of event-loop plumbing. The first set's
        //    `ShellStartupResources` were consumed when the first shell forked.
        let (shell_startup_resources, event_loop_tx) = Self::build_session_resources(
            self.session_template.channel_event_proxy.clone(),
            #[cfg(unix)]
            self.session_template.model_events.clone(),
        );
        // The PTY controller holds its own clone of the sender, and the one it
        // was built with now addresses the event loop joined in step 2; every
        // write it makes would fail until it is repointed.
        self.pty_controller.update(ctx, |controller, _| {
            controller.set_event_loop_sender(event_loop_tx.clone());
        });
        *self.event_loop_tx.lock() = event_loop_tx;

        // 7. Resolve the shell to launch: the requested target, else the one
        //    this pane is already running, else the user's preferred shell.
        let shells = AvailableShells::handle(ctx);
        let requested = target.or_else(|| self.model.lock().active_shell_launch_data().cloned());
        let shell = requested
            .and_then(|launch_data| {
                shells.read(ctx, |shells, _| {
                    shells.get_from_shell_launch_data(&launch_data)
                })
            })
            .or_else(|| self.model.lock().get_shell())
            .unwrap_or_else(|| {
                shells.read(ctx, |shells, ctx| shells.get_user_preferred_shell(ctx))
            });
        // `cwd` stays in host form here: which shell will actually launch
        // isn't settled until `ShellStarter::init`'s async resolution
        // completes -- a `SystemDefault` setting, the out-of-the-box case
        // for any user who hasn't customized their shell, still has to fall
        // through `compute_fallback_shell()` before the real shell is
        // known. `on_shell_determined` converts `cwd` once the real
        // `ShellStarter` -- and thus the real `ShellLaunchData` -- is
        // resolved; see `native_startup_directory`.
        let wsl_name_or_shell_starter = ShellStarter::init(shell.clone());
        // Label the pane with the shell it is launching, not the one it just
        // replaced -- a restart may be a switch to a different shell.
        self.model
            .lock()
            .set_determining_shell(ShellLaunchState::DeterminingShell {
                available_shell: Some(shell),
                display_name: wsl_name_or_shell_starter
                    .as_ref()
                    .map(|wsl_name_or_shell_starter| wsl_name_or_shell_starter.name())
                    .unwrap_or(ShellName::LessDescriptive("Shell".to_owned())),
            });

        // 8. Fork it through exactly the path `create_model_with_manager` takes
        //    for an eager pane. `DeferredShellStart` is built fresh here rather
        //    than reusing a parked one: its `ShellStartupResources` are
        //    one-shot, and step 6 is where this session's replacements come
        //    from.
        self.spawn_shell_start(
            DeferredShellStart {
                startup_directory: cwd,
                env_vars: self.session_template.env_vars.clone(),
                user_default_shell_unsupported_banner_model_handle: self
                    .session_template
                    .user_default_shell_unsupported_banner_model_handle
                    .clone(),
                shell_startup_resources,
                wsl_name_or_shell_starter,
            },
            ctx,
        );
    }

    /// Test-only: whether a PTY event loop is currently running for this
    /// manager's session.
    #[cfg(test)]
    pub(crate) fn has_running_event_loop(&self) -> bool {
        self.event_loop_handle.is_some()
    }

    /// Test-only: how many PTY event loops this manager has installed over its
    /// lifetime. See [`Self::established_session_count`].
    #[cfg(test)]
    pub(crate) fn established_session_count(&self) -> usize {
        self.established_session_count
    }

    /// Test-only: the startup directory most recently handed to
    /// `create_pty`. See the field doc for why this needs a hook.
    #[cfg(test)]
    pub(crate) fn last_resolved_startup_directory_for_test(&self) -> Option<PathBuf> {
        self.last_resolved_startup_directory.clone()
    }
}

/// Converts a host-form directory to the form the target shell expects.
///
/// Returns `None` when conversion fails, which starts the shell with no
/// startup directory -- it then uses its own default, the documented
/// fallback for a directory that no longer resolves.
pub(crate) fn native_startup_directory(cwd: &Path, shell: &ShellLaunchData) -> Option<PathBuf> {
    match shell {
        ShellLaunchData::Executable { .. } => Some(cwd.to_path_buf()),
        ShellLaunchData::WSL { distro } => convert_windows_host_to_wsl_path(cwd, distro)
            .ok()
            .map(PathBuf::from),
        ShellLaunchData::MSYS2 {
            executable_path, ..
        } => {
            let root = msys2_exe_to_root(WindowsPath::new(
                executable_path.as_os_str().as_encoded_bytes(),
            ));
            // `WindowsPathBuf` has no `AsRef<OsStr>` impl on this platform,
            // so go through `std::path::PathBuf`'s UTF-8 based `TryFrom`
            // instead of `Path::new(&root)`.
            let root = PathBuf::try_from(root).ok()?;
            convert_windows_native_to_msys2_path(cwd, &root).map(PathBuf::from)
        }
        // Paths inside a Docker sandbox container are not host paths at all
        // -- there is no meaningful translation, so the sandbox starts in
        // its own default directory.
        ShellLaunchData::DockerSandbox { .. } => None,
    }
}

/// Callback invoked upon determining the shell to be spawned when starting the event loop.
#[allow(clippy::too_many_arguments)]
fn on_shell_determined<S: TerminalSurface>(
    manager: &mut TerminalManager<S>,
    startup_directory: Option<PathBuf>,
    env_vars: HashMap<OsString, OsString>,
    user_default_shell_unsupported_banner_model_handle: ModelHandle<BannerState>,
    shell_startup_resources: ShellStartupResources,
    shell_starter_source: Option<ShellStarterSource>,
    ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>,
) where
    <S as Entity>::Event: PtyIntentEvent,
{
    // This is executed as a callback and the window could be closed in the interim.
    if !ctx.is_window_open(manager.view.window_id(ctx)) {
        log::warn!("Window was closed before shell was determined, aborting shell startup.");
        return;
    }

    log::debug!("Using shell starter source {shell_starter_source:?}");
    let bg_executor = ctx.background_executor();
    let auth_state = AuthStateProvider::as_ref(ctx).get();

    let is_fallback_shell = matches!(
        shell_starter_source,
        Some(ShellStarterSource::Fallback { .. })
    );
    let shell_starter = shell_starter_source
        .map(|source| get_shell_starter_internal(source, bg_executor, auth_state));
    let shell_starter = match shell_starter {
        Some(shell_starter) => shell_starter,
        None => {
            report_error!("Could not compute fallback shell");
            manager.view.update(ctx, |surface, ctx| {
                surface.on_pty_spawn_failed(
                    anyhow::Error::msg("Could not find a fallback shell. If you have PowerShell or WSL installed, please file an issue."),
                    ctx,
                );
            });
            manager.model().lock().exit(ExitReason::ShellNotFound);
            return;
        }
    };

    // The real shell is now resolved -- compute its `ShellLaunchData` here,
    // ahead of its other uses below, so `native_startup_directory` can
    // convert `startup_directory` against the shell that is actually about
    // to launch rather than a setting (e.g. `Config::SystemDefault`) that
    // may still resolve to something else entirely.
    let shell_launch_data = match &shell_starter {
        ShellStarter::Direct(shell_starter) => ShellLaunchData::Executable {
            executable_path: shell_starter.logical_shell_path().to_owned(),
            shell_type: shell_starter.shell_type(),
        },
        ShellStarter::DockerSandbox(docker_starter) => ShellLaunchData::Executable {
            executable_path: docker_starter.logical_shell_path().to_owned(),
            shell_type: docker_starter.shell_type(),
        },
        ShellStarter::Wsl(shell_starter) => ShellLaunchData::WSL {
            distro: shell_starter.distribution().to_owned(),
        },
        ShellStarter::MSYS2(shell_starter) => ShellLaunchData::MSYS2 {
            executable_path: shell_starter.logical_shell_path().to_owned(),
            shell_type: shell_starter.shell_type(),
        },
    };

    // `startup_directory` is in host form when it came from a caller (e.g.
    // a restart's requested `cwd`); translate it to the form the resolved
    // shell expects. A conversion failure, or a directory that no longer
    // resolves, falls through to `None` -- the shell then starts in its own
    // default directory below rather than the restart failing.
    let startup_directory = startup_directory
        .and_then(|host_dir| native_startup_directory(&host_dir, &shell_launch_data));

    // In WSL, default to the WSL home directory, not the native Windows home directory.
    let startup_directory = if let (ShellStarter::Wsl(wsl_shell_starter), None) =
        (&shell_starter, &startup_directory)
    {
        wsl_shell_starter.home_directory()
    } else {
        startup_directory
    };

    // Record the final, native-form directory for tests -- see the field doc
    // on `TerminalManager::last_resolved_startup_directory`.
    #[cfg(test)]
    {
        manager.last_resolved_startup_directory = startup_directory.clone();
    }

    // Show a "shell unsupported" banner, if applicable.
    if is_fallback_shell
        && user_default_shell_unsupported_banner_model_handle.as_ref(ctx)
            == &BannerState::NotDismissed
    {
        user_default_shell_unsupported_banner_model_handle.update(ctx, |model, ctx| {
            *model = BannerState::Open;
            ctx.notify();
        })
    }

    manager
        .model()
        .lock()
        .set_login_shell_spawned(shell_starter.shell_type());

    // This needs to be done before bootstrapping starts (i.e. before spawning the event loop below).
    manager
        .model()
        .lock()
        .set_pending_shell_launch_data(shell_launch_data.clone());

    // Register the session ID that was generated during shell starter construction.
    // For bash, fish, and PowerShell, the session ID is already baked into the command
    // args. For zsh and MSYS2, enqueue_init_script injects this same ID.
    let generated_session_id = match &shell_starter {
        ShellStarter::Direct(starter) | ShellStarter::MSYS2(starter) => starter.session_id(),
        ShellStarter::DockerSandbox(starter) => starter.session_id(),
        ShellStarter::Wsl(starter) => starter.session_id(),
    };
    manager
        .model()
        .lock()
        .register_session_id(generated_session_id);

    // Enqueue the init shell script (for shells that need it), then create
    // the PTY and start its corresponding event loop.
    let ShellStartupResources {
        event_loop_rx,
        channel_event_proxy,
        #[cfg(unix)]
        model_events,
    } = shell_startup_resources;
    let model = manager.model();
    #[cfg(windows)]
    let event_loop_tx = manager.event_loop_tx.lock().clone();
    let pty = match manager
        .enqueue_init_script(&shell_starter, generated_session_id)
        .context("Failed to write shell init script to the pty")
        .and_then(|_| {
            TerminalManager::<S>::create_pty(
                startup_directory,
                shell_starter,
                env_vars,
                model.clone(),
                #[cfg(windows)]
                event_loop_tx,
                ctx,
            )
        }) {
        Ok(pty) => pty,
        Err(err) => {
            report_error!(&err);
            manager.view.update(ctx, |surface, ctx| {
                surface.on_pty_spawn_failed(err, ctx);
            });
            manager.model().lock().exit(ExitReason::PtySpawnFailed);
            return;
        }
    };

    #[cfg(feature = "integration_tests")]
    let pid = pty.get_pid();
    #[cfg(unix)]
    let fd = pty.get_fd();

    // Create the channel above and pass the receving side to the event loop.
    let event_loop_handle = TerminalManager::<S>::start_pty_event_loop(
        pty,
        event_loop_rx,
        model.clone(),
        channel_event_proxy,
    );

    manager.event_loop_handle = Some(event_loop_handle);
    #[cfg(test)]
    {
        manager.established_session_count += 1;
    }
    #[cfg(feature = "integration_tests")]
    {
        manager.pid = Some(pid);
    }

    manager.view.update(ctx, |surface, ctx| {
        surface.on_shell_determined(ctx);
        surface.on_active_shell_launch_data_updated(Some(shell_launch_data), ctx);
    });

    // Initialize the terminal attributes poller.
    // TODO(CORE-2297): Implement TerminalPoller on Windows.
    #[cfg(unix)]
    {
        let terminal_attributes_poller = ctx.add_model(|_| TerminalAttributesPoller::new(fd));
        wire_up_terminal_attribute_poller_with_surface(
            &terminal_attributes_poller,
            &manager.view,
            &model_events,
            model.clone(),
            ctx,
        );

        manager.terminal_attributes_poller = Some(terminal_attributes_poller);
    }
}

impl<S> TerminalManager<S> {
    /// Sends bindkey to notify shell process to switch to PS1 logic for prompt
    /// with the combined prompt/command grid (we restore the saved PS1 value).
    pub fn send_switch_to_ps1_bindkey(&self, app_ctx: &mut AppContext) {
        self.pty_controller.update(app_ctx, |pty_controller, ctx| {
            pty_controller.send_switch_to_ps1_bindkey(ctx);
        });
    }

    /// Sends bindkey to notify shell process to switch to Warp prompt logic for prompt
    /// with the combined prompt/command grid (we unset the PS1, but save the value for potential
    /// future restoration).
    pub fn send_switch_to_warp_prompt_bindkey(&self, app_ctx: &mut AppContext) {
        self.pty_controller.update(app_ctx, |pty_controller, ctx| {
            pty_controller.send_switch_to_warp_prompt_bindkey(ctx);
        });
    }

    fn enqueue_init_script(
        &self,
        shell_starter: &ShellStarter,
        session_id: SessionId,
    ) -> Result<(), SendError<Message>> {
        let shell_type = shell_starter.shell_type();
        if shell_type == crate::terminal::shell::ShellType::Zsh
            // For more on why this is necessary on Git Bash, see https://linear.app/warpdotdev/issue/CORE-3202.
            || shell_starter.is_msys2()
        {
            let init_shell_script = crate::terminal::bootstrap::init_shell_script_for_shell(
                shell_type,
                &crate::ASSETS,
                session_id,
            );
            let tx = self.event_loop_tx.lock();
            tx.send(Message::Input(init_shell_script.into_bytes().into()))?;
            tx.send(Message::Input(shell_type.execute_command_bytes().into()))
        } else {
            Ok(())
        }
    }

    fn create_pty(
        startup_directory: Option<PathBuf>,
        shell_starter: ShellStarter,
        env_vars: HashMap<OsString, OsString>,
        model: Arc<FairMutex<TerminalModel>>,
        #[cfg(windows)] event_loop_tx: mio_channel::Sender<Message>,
        ctx: &mut AppContext,
    ) -> anyhow::Result<Pty> {
        let is_shell_debug_mode_enabled = *DebugSettings::as_ref(ctx)
            .is_shell_debug_mode_enabled
            .value();
        let is_honor_ps1_enabled = *SessionSettings::as_ref(ctx).honor_ps1;
        let is_crash_reporting_enabled = PrivacySettings::as_ref(ctx).is_crash_reporting_enabled;

        // Determine whether the Node.js Version chip is enabled anywhere it could be
        // shown (the Warp prompt, the agent footer, or the CLI agent footer). When it
        // is not, the shell bootstrap skips the expensive per-prompt `node --version`
        // detection. The chip value is fed by the same precmd payload regardless of
        // where it is displayed, so we must check all three locations.
        let node_version_chip_enabled = {
            let in_prompt = !is_honor_ps1_enabled
                && Prompt::as_ref(ctx)
                    .chip_kinds()
                    .contains(&ContextChipKind::NodeVersion);
            let settings = SessionSettings::as_ref(ctx);
            in_prompt
                || settings
                    .agent_footer_chip_selection
                    .all_chips()
                    .contains(&ContextChipKind::NodeVersion)
                || settings
                    .cli_agent_footer_chip_selection
                    .all_chips()
                    .contains(&ContextChipKind::NodeVersion)
        };

        // `enable_ssh_warpification` is the single source of truth for whether the SSH
        // wrapper is active. The bootstrap scripts check `WARP_USE_SSH_WRAPPER` (derived
        // from this value) before invoking `warp_ssh_helper`, which spawns the ControlMaster
        // and opens agent-protocol channels.
        let enable_ssh_wrapper = *WarpifySettings::as_ref(ctx)
            .enable_ssh_warpification
            .value();

        // Only meaningful when the legacy ControlMaster wrapper is active.
        let reuse_ssh_control_master = enable_ssh_wrapper
            && *SshSettings::as_ref(ctx)
                .reuse_existing_control_master
                .value();

        let size: SizeInfo = model.lock().block_list().size().to_owned();
        let options = PtyOptions {
            size,
            window_id: None,
            shell_starter,
            start_dir: startup_directory,
            env_vars,
            enable_ssh_wrapper,
            reuse_ssh_control_master,
            shell_debug_mode: is_shell_debug_mode_enabled,
            honor_ps1: is_honor_ps1_enabled,
            node_version_chip_enabled,
            close_fds: true,
        };

        Pty::new(
            options,
            is_crash_reporting_enabled,
            #[cfg(windows)]
            event_loop_tx,
            ctx,
        )
    }

    /// Start's the PTY event loop, returning a sender for the event loop and the event loop's join handle.
    fn start_pty_event_loop(
        pty: Pty,
        rx: mio_channel::Receiver<Message>,
        model: Arc<FairMutex<TerminalModel>>,
        channel_event_proxy: ChannelEventListener,
    ) -> JoinHandle<()> {
        // Create the event loop and get a handle to the injector.
        let event_loop = EventLoop::new(model, channel_event_proxy, pty, rx);

        // Spawn the event loop on a separate thread to interact with the PTY and write the data back
        // to the terminal.
        event_loop.spawn()
    }
}

/// Wires the Unix terminal-attributes poller to a terminal surface.
///
/// The poller is driven off `ModelEventDispatcher` block start/complete events
/// rather than surface-specific view events, and termios results are routed back
/// through the [`TerminalSurface`] Unix hooks so the manager stays surface-agnostic.
///
/// NOTE: we cannot simply use the strong references (the handle arguments to this
/// wire_up fn) in the subscription callbacks because that will create a reference
/// cycle. Instead, we use weak handles and upgrade them lazily.
#[cfg(unix)]
fn wire_up_terminal_attribute_poller_with_surface<S: TerminalSurface>(
    terminal_attributes_poller: &ModelHandle<TerminalAttributesPoller>,
    surface: &ViewHandle<S>,
    model_events: &ModelHandle<ModelEventDispatcher>,
    model: Arc<FairMutex<TerminalModel>>,
    ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>,
) where
    <S as Entity>::Event: PtyIntentEvent,
{
    let poller_weak_handle = terminal_attributes_poller.downgrade();
    let poller_weak_handle_for_termios = poller_weak_handle.clone();
    let surface_weak_handle = surface.downgrade();
    let surface_weak_handle_for_termios = surface_weak_handle.clone();

    // Tracks the index of the started block across the model-event <-> poller interactions.
    let block_index: Rc<RefCell<Option<BlockIndex>>> = Rc::new(RefCell::new(None));
    let block_index_for_termios = block_index.clone();

    // On block start, record the active block index and ask the surface whether
    // password-prompt polling is useful before starting the poller. On block
    // completion, stop polling and let the surface react to the completed block.
    ctx.subscribe_to_model(model_events, move |_manager, _dispatcher, event, ctx| {
        let Some(poller) = poller_weak_handle.upgrade(ctx) else {
            return;
        };

        match event {
            TerminalModelEvent::AfterBlockStarted {
                command,
                is_for_in_band_command: false,
                ..
            } => {
                let should_poll = surface_weak_handle.upgrade(ctx).is_some_and(|surface| {
                    surface.read(ctx, |surface, ctx| {
                        surface.should_start_password_prompt_polling(command, ctx)
                    })
                });
                if should_poll {
                    *block_index.borrow_mut() =
                        Some(model.lock().block_list().active_block_index());
                    poller.update(ctx, |poller, ctx| {
                        poller.start_polling(ctx);
                    });
                } else {
                    *block_index.borrow_mut() = None;
                }
            }
            TerminalModelEvent::AfterBlockCompleted(completed) => {
                if let Some(surface) = surface_weak_handle.upgrade(ctx) {
                    let should_stop = surface.read(ctx, |surface, _ctx| {
                        surface.should_stop_password_prompt_polling(completed)
                    });
                    if should_stop {
                        poller.update(ctx, |poller, _ctx| {
                            poller.stop_polling();
                        });
                        surface.update(ctx, |surface, ctx| {
                            surface.on_polled_block_completed(completed, ctx);
                        });
                    }
                }
            }
            _ => {}
        }
    });

    // When the poller detects termios consistent with a password prompt, route the
    // result through the surface and stop after the first detection to preserve
    // today's one-notification-per-command behavior.
    ctx.subscribe_to_model(
        terminal_attributes_poller,
        move |_manager, _poller, event, ctx| {
            let TerminalAttributesPollerEvent::TermiosQueryFinished { termios } = event;

            // A PTY likely has a password prompt if ECHO is disabled but ICANON is
            // still enabled. Apps like neovim disable both in raw mode, so requiring
            // ICANON avoids false positives.
            let might_be_password_prompt = !termios.local_flags.contains(LocalFlags::ECHO)
                && termios.local_flags.contains(LocalFlags::ICANON);
            if !might_be_password_prompt {
                return;
            }

            let Some(surface) = surface_weak_handle_for_termios.upgrade(ctx) else {
                return;
            };
            let block_index = block_index_for_termios.borrow_mut().take();
            surface.update(ctx, |surface, ctx| {
                surface.on_possible_password_prompt(block_index, ctx);
            });

            if let Some(poller) = poller_weak_handle_for_termios.upgrade(ctx) {
                poller.update(ctx, |poller, _ctx| {
                    poller.stop_polling();
                });
            }
        },
    );
}

pub fn get_shell_starter(
    chosen_shell: Option<AvailableShell>,
    auth_state: &AuthState,
    ctx: &mut AppContext,
) -> Option<ShellStarter> {
    let preferred_shell = chosen_shell.unwrap_or_else(|| {
        AvailableShells::handle(ctx).read(ctx, |shells, ctx| shells.get_user_preferred_shell(ctx))
    });
    let shell_starter_or_wsl_name = ShellStarter::init(preferred_shell);

    // TODO(alokedesai): Further refactor this function to make it clear that it's expensive.
    shell_starter_or_wsl_name
        .and_then(|starter| {
            warpui::r#async::block_on(async { starter.to_shell_starter_source().await })
        })
        .map(|starter_source| {
            get_shell_starter_internal(
                starter_source,
                ctx.background_executor().clone(),
                auth_state,
            )
        })
}

fn get_shell_starter_internal(
    shell_starter_source: ShellStarterSource,
    background_executor: Arc<Background>,
    auth_state: &AuthState,
) -> ShellStarter {
    match shell_starter_source {
        ShellStarterSource::Override(shell_starter) => shell_starter,
        ShellStarterSource::Environment(starter) | ShellStarterSource::UserDefault(starter) => {
            ShellStarter::Direct(starter)
        }
        ShellStarterSource::Fallback {
            unsupported_shell,
            starter,
        } => {
            if let Some(unsupported_shell) = unsupported_shell {
                send_telemetry_on_executor!(
                    auth_state,
                    TelemetryEvent::UnsupportedShell {
                        shell: unsupported_shell
                    },
                    background_executor
                );
            }

            ShellStarter::Direct(starter)
        }
    }
}

impl EventLoopSender for mio_channel::Sender<Message> {
    fn send(&self, message: Message) -> Result<(), EventLoopSendError> {
        self.send(message).map_err(|error| match error {
            SendError(_) => EventLoopSendError::Disconnected,
        })
    }
}
