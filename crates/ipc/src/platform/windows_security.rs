use std::ffi::c_void;
use std::io;
use std::os::windows::io::AsRawHandle as _;

use tokio::net::windows::named_pipe::{NamedPipeClient, NamedPipeServer};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows::Win32::System::Pipes::{
    GetNamedPipeClientProcessId, GetNamedPipeClientSessionId, GetNamedPipeServerProcessId,
    GetNamedPipeServerSessionId,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcessToken};
use windows::core::PCWSTR;

pub(super) struct PipeSecurity {
    descriptor: PSECURITY_DESCRIPTOR,
}

impl PipeSecurity {
    pub(super) fn current_user_only() -> io::Result<Self> {
        let sid = current_user_sid_string()?;
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})\0").encode_utf16().collect();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .map_err(windows_error)?;
        Ok(Self { descriptor })
    }

    pub(super) fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0,
            bInheritHandle: false.into(),
        }
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        if !self.descriptor.is_invalid() {
            unsafe { LocalFree(Some(HLOCAL(self.descriptor.0))) };
        }
    }
}

pub(super) fn validate_client(server: &NamedPipeServer) -> io::Result<()> {
    let handle = HANDLE(server.as_raw_handle());
    let mut process_id = 0;
    let mut session_id = 0;
    unsafe {
        GetNamedPipeClientProcessId(handle, &mut process_id).map_err(windows_error)?;
        GetNamedPipeClientSessionId(handle, &mut session_id).map_err(windows_error)?;
    }
    validate_peer(process_id, session_id, "client")
}

pub(super) fn validate_server(client: &NamedPipeClient) -> io::Result<()> {
    let handle = HANDLE(client.as_raw_handle());
    let mut process_id = 0;
    let mut session_id = 0;
    unsafe {
        GetNamedPipeServerProcessId(handle, &mut process_id).map_err(windows_error)?;
        GetNamedPipeServerSessionId(handle, &mut session_id).map_err(windows_error)?;
    }
    validate_peer(process_id, session_id, "server")
}

fn validate_peer(process_id: u32, session_id: u32, role: &str) -> io::Result<()> {
    if process_id == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("named pipe {role} has no process identity"),
        ));
    }
    let mut current_session_id = 0;
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut current_session_id) }
        .map_err(windows_error)?;
    if session_id != current_session_id {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("named pipe {role} belongs to a different Windows session"),
        ));
    }
    Ok(())
}

fn current_user_sid_string() -> io::Result<String> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(windows_error)?;

    let result = (|| {
        let mut needed = 0;
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
        if needed == 0 {
            return Err(io::Error::last_os_error());
        }

        let mut buffer = vec![0; needed as usize];
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast::<c_void>()),
                needed,
                &mut needed,
            )
        }
        .map_err(windows_error)?;

        let token_user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let mut sid_string = windows::core::PWSTR::null();
        unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut sid_string) }
            .map_err(windows_error)?;
        let sid = unsafe { sid_string.to_string() }.map_err(io::Error::other);
        unsafe { LocalFree(Some(HLOCAL(sid_string.0.cast()))) };
        sid
    })();

    let _ = unsafe { CloseHandle(token) };
    result
}

fn windows_error(error: windows::core::Error) -> io::Error {
    io::Error::other(error)
}
