//! Reaching a running Warp instance from a `sessions` process.
//!
//! Blocking, on purpose. This runs in a short-lived command-line process whose
//! only job is one request and one answer, and an async surface there would be
//! machinery around a straight line.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ipc::{Client, ConnectionAddress, service_caller};
use warpui_core::r#async::block_on;
use warpui_core::r#async::executor::Background;

use super::address::{self, InstanceMarker};
use super::protocol::{PROTOCOL_VERSION, Request, Response};
use super::server::RemoteControlService;

/// How long to wait for one answer.
///
/// Generous because the far end answers on its main thread, which may be busy
/// laying out a window or blocked in a round trip to the supervisor of its own.
/// Short enough that an instance wedged behind a modal dialog reports a timeout
/// rather than hanging a script forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// A connection to one running instance.
pub(crate) struct Instance {
    marker: InstanceMarker,
    client: Arc<Client>,
    /// Never read. It owns the threads running the client's socket tasks, so
    /// dropping it would disconnect the very connection `client` is using.
    _executor: Arc<Background>,
}

impl Instance {
    /// Connects to the instance a marker describes.
    pub(crate) fn connect(marker: InstanceMarker) -> Result<Self> {
        // Two threads: one to drive the socket's reads, one its writes. The
        // default would size a pool by CPU count for a process that makes a
        // single request.
        let executor = Arc::new(Background::new(2, |i| format!("sessions-cli-{i}")));
        let client = block_on(Client::connect(
            ConnectionAddress::from(marker.address.clone()),
            executor.clone(),
        ))
        .map_err(|error| anyhow::anyhow!("{error}"))
        .with_context(|| format!("Failed to reach the Warp instance with pid {}", marker.pid))?;

        Ok(Self {
            marker,
            client: Arc::new(client),
            _executor: executor,
        })
    }

    /// The process id this instance runs as.
    pub(crate) fn pid(&self) -> u32 {
        self.marker.pid
    }

    /// The build it is running.
    pub(crate) fn version(&self) -> &str {
        &self.marker.version
    }

    /// Sends one request and waits for the answer.
    pub(crate) fn call(&self, request: Request) -> Result<Response> {
        self.call_within(request, REQUEST_TIMEOUT)
    }

    /// Sends one request and waits at most `timeout` for the answer.
    pub(crate) fn call_within(&self, request: Request, timeout: Duration) -> Result<Response> {
        let caller = service_caller::<RemoteControlService>(self.client.clone());
        let answer = block_on(futures::future::select(
            Box::pin(caller.call(request)),
            Box::pin(warpui_core::r#async::Timer::after(timeout)),
        ));

        match answer {
            futures::future::Either::Left((result, _)) => {
                result.map_err(|error| anyhow::anyhow!("{error}"))
            }
            futures::future::Either::Right(_) => bail!(
                "The Warp instance with pid {} did not answer within {} seconds",
                self.marker.pid,
                timeout.as_secs()
            ),
        }
    }
}

/// Every instance that answers, with the markers of those that do not removed.
///
/// A marker outliving its process is ordinary -- a crash, a kill -- so a
/// listing is also the moment those get cleaned up. Nothing else sweeps them.
pub(crate) fn live_instances() -> Vec<Instance> {
    let mut instances = Vec::new();
    for marker in address::current_markers() {
        let pid = marker.pid;
        match Instance::connect(marker) {
            Ok(instance) => match instance.call(Request::Ping) {
                Ok(Response::Pong { .. }) => instances.push(instance),
                _ => address::forget(pid),
            },
            Err(_) => address::forget(pid),
        }
    }
    instances
}

/// Whether the instance with this pid still answers.
///
/// A marker that is present proves nothing and a marker that is absent proves
/// nothing either; only a reply does. Used to tell a quit that has finished
/// from one that has not.
pub(crate) fn is_listening(pid: u32) -> bool {
    let Some(marker) = address::current_markers()
        .into_iter()
        .find(|marker| marker.pid == pid)
    else {
        return false;
    };
    Instance::connect(marker)
        .and_then(|instance| instance.call(Request::Ping))
        .is_ok()
}

/// The one instance a command is addressed to.
///
/// With a pid given, that instance or an error naming why it could not be
/// reached. Without one, the only instance running -- and an error listing the
/// candidates when there is more than one, because picking for the caller here
/// means detaching a session in a window they were not looking at.
pub(crate) fn select_instance(pid: Option<u32>) -> Result<Instance> {
    if let Some(pid) = pid {
        let Some(marker) = address::current_markers()
            .into_iter()
            .find(|marker| marker.pid == pid)
        else {
            let other = address::running_markers_of_other_protocols()
                .into_iter()
                .find(|found| found.marker.pid == pid);
            match other {
                Some(found) => bail!(
                    "The Warp instance with pid {pid} runs build {}, which speaks {}; this \
                     command speaks protocol {PROTOCOL_VERSION}. Run that build's own executable, \
                     or reopen the window on this build.",
                    found.marker.version,
                    describe_protocol(found.protocol_version)
                ),
                None => bail!(
                    "No Warp instance with pid {pid} is listening. Try `{} sessions instances`.",
                    super::program_name()
                ),
            }
        };
        let instance = Instance::connect(marker)?;
        instance.call(Request::Ping)?;
        return Ok(instance);
    }

    let mut instances = live_instances();
    match instances.len() {
        0 => bail!(
            "No running Warp instance is listening for the `sessions` command. Start Warp, or check that \
             it is the same profile: this command talks to the instances of the state directory \
             at {:?}.{}",
            warp_core::paths::state_dir(),
            other_protocols_note()
                .map(|note| format!("\n{note}"))
                .unwrap_or_default()
        ),
        1 => Ok(instances.remove(0)),
        _ => {
            let pids = instances
                .iter()
                .map(|instance| instance.pid().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "Several Warp instances are running ({pids}). Name one with --instance <PID>, \
                 or list them with `{} sessions instances`.",
                super::program_name()
            )
        }
    }
}

/// Names the running instances this build cannot talk to, or `None` when there are none.
pub(crate) fn other_protocols_note() -> Option<String> {
    let others = address::running_markers_of_other_protocols();
    if others.is_empty() {
        return None;
    }
    let listed = others
        .iter()
        .map(|found| {
            format!(
                "  pid {}, build {}, {}",
                found.marker.pid,
                found.marker.version,
                describe_protocol(found.protocol_version)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "Running, but on another protocol than this build's {PROTOCOL_VERSION}, so out of its \
         reach:\n{listed}\nUse that build's own executable, or reopen those windows on this build."
    ))
}

fn describe_protocol(version: Option<u32>) -> String {
    match version {
        Some(version) => format!("protocol {version}"),
        None => "an unrecognised protocol".to_owned(),
    }
}
