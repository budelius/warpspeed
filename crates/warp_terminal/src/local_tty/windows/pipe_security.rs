//! The security descriptor guarding the supervisor's named pipe.
//!
//! A client that reaches the supervisor can attach to any session it holds,
//! which means driving a live shell. So the pipe is restricted to the account
//! that created it rather than left on the system default: on a shared or
//! domain-joined machine the default is broader than "only me", and this is not
//! a channel where that is acceptable.
//!
//! The Unix side gets the same property from a socket at mode `0600` under the
//! user's own uid.

use std::ffi::c_void;

use anyhow::{Context, Result};
use windows::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PCWSTR;

/// A security descriptor allocated by Windows, freed when dropped.
pub struct PipeSecurity {
    descriptor: PSECURITY_DESCRIPTOR,
}

impl PipeSecurity {
    /// Builds a descriptor granting full control to the current user and to
    /// nobody else.
    ///
    /// `D:P` makes the DACL protected, so it does not inherit permissive
    /// entries from anywhere; the single ACE grants `GA` (all access) to the
    /// account that owns this process.
    pub fn current_user_only() -> Result<Self> {
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
        .context("Failed to build the supervisor pipe's security descriptor")?;

        Ok(Self { descriptor })
    }

    /// The `SECURITY_ATTRIBUTES` to pass to `CreateNamedPipeW`.
    ///
    /// Borrows from `self`, so the returned value must not outlive it -- the
    /// descriptor is freed on drop.
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
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

/// The current user's SID in string form, for embedding in an SDDL ACE.
fn current_user_sid_string() -> Result<String> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .context("Failed to open this process's token")?;

    // Ask for the size first: TOKEN_USER is variable length, because the SID it
    // carries is.
    let mut needed = 0u32;
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };

    let mut buffer = vec![0u8; needed as usize];
    let result = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr() as *mut c_void),
            needed,
            &mut needed,
        )
    };

    let _ = unsafe { windows::Win32::Foundation::CloseHandle(token) };
    result.context("Failed to read this process's user from its token")?;

    let token_user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
    let mut sid_string = windows::core::PWSTR::null();
    unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut sid_string) }
        .context("Failed to convert this process's user SID to a string")?;

    let owned =
        unsafe { sid_string.to_string() }.context("The user SID string was not valid UTF-16")?;
    unsafe { LocalFree(Some(HLOCAL(sid_string.0 as *mut c_void))) };

    Ok(owned)
}

#[cfg(test)]
#[path = "pipe_security_tests.rs"]
mod tests;
