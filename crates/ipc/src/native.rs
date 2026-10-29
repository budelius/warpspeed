//! Native IPC transport over Unix domain sockets or Windows named pipes.
use async_compat::CompatExt as _;
use futures::{AsyncRead, AsyncWrite};

use crate::ConnectionAddress;

#[cfg(windows)]
#[path = "platform/windows_security.rs"]
mod windows_security;

#[cfg(not(windows))]
pub(crate) mod client {
    use interprocess::local_socket::tokio::LocalSocketStream;

    use super::*;
    use crate::client::{ClientError, InitializationError, Result};

    pub async fn connect_client(
        connection_address: ConnectionAddress,
    ) -> Result<(impl AsyncRead + Unpin, impl AsyncWrite + Unpin)> {
        let stream = LocalSocketStream::connect(connection_address.0.as_str())
            .compat()
            .await
            .map_err(|error| ClientError::Initialization(InitializationError::Io(error)))?;
        Ok(stream.into_split())
    }
}

#[cfg(windows)]
pub(crate) mod client {
    use futures::AsyncReadExt as _;
    use tokio::net::windows::named_pipe::ClientOptions;

    use super::*;
    use crate::client::{ClientError, InitializationError, Result};

    pub async fn connect_client(
        connection_address: ConnectionAddress,
    ) -> Result<(impl AsyncRead + Unpin, impl AsyncWrite + Unpin)> {
        let pipe_name = pipe_name(&connection_address);
        let stream = async move { ClientOptions::new().open(pipe_name) }
            .compat()
            .await
            .map_err(|error| ClientError::Initialization(InitializationError::Io(error)))?;
        super::windows_security::validate_server(&stream)
            .map_err(|error| ClientError::Initialization(InitializationError::Io(error)))?;
        Ok(stream.compat().split())
    }
}

#[cfg(not(windows))]
pub(crate) mod server {
    use interprocess::local_socket::tokio::{LocalSocketListener, LocalSocketStream};

    use super::*;
    use crate::server::{InitializationError, Result, ServerError};

    pub struct ConnectionImpl {
        stream: LocalSocketStream,
    }

    impl ConnectionImpl {
        pub fn into_split(self) -> (impl AsyncRead + Unpin, impl AsyncWrite + Unpin) {
            self.stream.into_split()
        }
    }

    pub struct ConnectionListenerImpl {
        listener: LocalSocketListener,
    }

    impl ConnectionListenerImpl {
        pub fn new(connection_address: ConnectionAddress) -> Result<Self> {
            let listener = warpui_core::r#async::block_on(
                async move { LocalSocketListener::bind(connection_address.to_string()) }.compat(),
            )
            .map_err(|error| ServerError::Initialization(InitializationError::Io(error)))?;
            Ok(Self { listener })
        }

        pub async fn accept_connection(&self) -> Result<ConnectionImpl> {
            self.listener
                .accept()
                .compat()
                .await
                .map(|stream| ConnectionImpl { stream })
                .map_err(ServerError::AcceptConnection)
        }
    }
}

#[cfg(windows)]
pub(crate) mod server {
    use std::ffi::c_void;

    use futures::AsyncReadExt as _;
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    use tokio::sync::Mutex;
    use windows::Win32::Security::SECURITY_ATTRIBUTES;

    use super::*;
    use crate::server::{InitializationError, Result, ServerError};

    pub struct ConnectionImpl {
        stream: NamedPipeServer,
    }

    impl ConnectionImpl {
        pub fn into_split(self) -> (impl AsyncRead + Unpin, impl AsyncWrite + Unpin) {
            self.stream.compat().split()
        }
    }

    pub struct ConnectionListenerImpl {
        pipe_name: String,
        listener: Mutex<NamedPipeServer>,
    }

    impl ConnectionListenerImpl {
        pub fn new(connection_address: ConnectionAddress) -> Result<Self> {
            let pipe_name = pipe_name(&connection_address);
            let listener =
                warpui_core::r#async::block_on(async { create_server(&pipe_name, true) }.compat())
                    .map_err(|error| ServerError::Initialization(InitializationError::Io(error)))?;
            Ok(Self {
                pipe_name,
                listener: Mutex::new(listener),
            })
        }

        pub async fn accept_connection(&self) -> Result<ConnectionImpl> {
            let mut listener = self.listener.lock().await;
            listener
                .connect()
                .await
                .map_err(ServerError::AcceptConnection)?;
            let replacement =
                create_server(&self.pipe_name, false).map_err(ServerError::AcceptConnection)?;
            let stream = std::mem::replace(&mut *listener, replacement);
            drop(listener);
            super::windows_security::validate_client(&stream)
                .map_err(ServerError::AcceptConnection)?;
            Ok(ConnectionImpl { stream })
        }
    }

    fn create_server(pipe_name: &str, first: bool) -> std::io::Result<NamedPipeServer> {
        let security = super::windows_security::PipeSecurity::current_user_only()?;
        let mut attributes: SECURITY_ATTRIBUTES = security.attributes();
        let mut options = ServerOptions::new();
        options
            .first_pipe_instance(first)
            .reject_remote_clients(true);
        unsafe {
            options.create_with_security_attributes_raw(
                pipe_name,
                (&mut attributes as *mut SECURITY_ATTRIBUTES).cast::<c_void>(),
            )
        }
    }
}

#[cfg(windows)]
fn pipe_name(connection_address: &ConnectionAddress) -> String {
    format!(r"\\.\pipe\{}", connection_address.0)
}
