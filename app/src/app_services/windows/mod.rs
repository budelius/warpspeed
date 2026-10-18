use registry::register_uri_handler;
use service_impl::forward_uri_to_sole_running_instance;
use single_instance_manager::SingleInstanceManager;
use thiserror::Error;
use url::Url;
use warp_core::channel::ChannelState;
use warp_errors::report_error;
use warpui::AppContext;

pub(crate) use super::enforces_profile_ownership;

mod registry;
mod service_impl;
mod single_instance_manager;

const FORWARD_ATTEMPTS: usize = 100;
const FORWARD_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

#[derive(Error, Debug)]
pub enum StartupArgsForwardingError {
    #[error("should not forward arguments after an auto-update")]
    IgnoredAfterAutoUpdate,
    #[error("there is no other instance of Warp")]
    NoExistingInstance,
    #[error("failed to construct url")]
    CouldNotCreateUrl(#[from] url::ParseError),
    #[error("IPC Client failed to send message")]
    IpcError(#[from] ipc::ClientError),
    #[error("Win32 error")]
    WindowsError(#[from] windows::core::Error),
}

fn single_instance_enabled() -> bool {
    cfg!(feature = "release_bundle") || enforces_profile_ownership()
}

pub fn pass_startup_args_to_existing_instance(
    args: &warp_cli::AppArgs,
) -> Result<(), StartupArgsForwardingError> {
    if !single_instance_enabled() {
        return Err(StartupArgsForwardingError::NoExistingInstance);
    }
    if args.finish_update && !enforces_profile_ownership() {
        return Err(StartupArgsForwardingError::IgnoredAfterAutoUpdate);
    }
    if SingleInstanceManager::is_sole_running_instance()? {
        return Err(StartupArgsForwardingError::NoExistingInstance);
    }

    let mut urls = if args.urls.is_empty() {
        let mut open_new_url = format!("{}://action/new_window", ChannelState::url_scheme());
        if let Ok(current_dir) = std::env::current_dir() {
            match current_dir.into_os_string().into_string() {
                Ok(current_dir) => open_new_url.push_str(&format!("?path={current_dir}")),
                Err(os_string) => {
                    report_error!(
                        "Failed to convert OsString to String",
                        extra: { "os_string" => ?os_string }
                    );
                }
            }
        }
        vec![Url::parse(&open_new_url)?]
    } else {
        args.urls.clone()
    };

    super::add_forwarded_source_version(&mut urls);
    log::info!(
        "Forwarding launch to Warp profile owner source_version={}",
        crate::build_info::version_identity()
    );
    warpui::r#async::block_on(async {
        let mut last_error = None;
        for attempt in 0..FORWARD_ATTEMPTS {
            match forward_uri_to_sole_running_instance(urls.clone()).await {
                Ok(()) => return Ok(()),
                Err(err) => last_error = Some(err),
            }
            if attempt + 1 < FORWARD_ATTEMPTS {
                async_io::Timer::after(FORWARD_RETRY_DELAY).await;
            }
        }
        Err(StartupArgsForwardingError::IpcError(
            last_error.expect("at least one forwarding attempt"),
        ))
    })
}

pub(super) fn init(ctx: &mut AppContext) {
    if single_instance_enabled() {
        ctx.add_singleton_model(SingleInstanceManager::new);
    }
    register_uri_handler();
}
