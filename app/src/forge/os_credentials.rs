//! Reading a forge token out of the OS credential store.
//!
//! This is the last link in the chain and the one that needs no configuration:
//! it reuses whatever the user already logged into. Reading the store is an API
//! call, not a subprocess, which is the whole point — `gh auth token` would work
//! too, and would be a process spawn.
//!
//! Reading `gh`'s config file is not enough on its own. Modern `gh` keeps the
//! token in the OS keyring and leaves only the username behind in `hosts.yml`,
//! so the file supplies the username that completes the keyring target name.

use std::path::PathBuf;

use super::credentials::{Credential, CredentialStore, os_store_targets};

#[cfg(test)]
#[path = "os_credentials_tests.rs"]
mod tests;

/// The username `gh` recorded for `host` in its `hosts.yml`.
///
/// The file's `users:` block lists accounts and `user:` names the active one.
/// Only the latter is wanted, and `users:` must not be mistaken for it.
pub fn username_from_gh_hosts(yaml: &str, host: &str) -> Option<String> {
    let mut in_host_block = false;
    for line in yaml.lines() {
        let is_top_level = !line.starts_with([' ', '\t']) && !line.trim().is_empty();
        if is_top_level {
            in_host_block = line.trim_end().strip_suffix(':') == Some(host);
            continue;
        }
        if !in_host_block {
            continue;
        }
        if let Some(value) = line.trim().strip_prefix("user:") {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

/// Where `gh` keeps `hosts.yml`.
///
/// `GH_CONFIG_DIR` wins when set, as it does for `gh` itself. Otherwise Windows
/// uses `%AppData%\GitHub CLI\`, and everything else the XDG path — the same
/// location `git_credentials.rs` writes for cloud agents.
pub fn gh_hosts_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("GH_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("hosts.yml"));
    }
    #[cfg(windows)]
    {
        let appdata = std::env::var_os("AppData")?;
        Some(PathBuf::from(appdata).join("GitHub CLI").join("hosts.yml"))
    }
    #[cfg(not(windows))]
    {
        if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
            return Some(PathBuf::from(dir).join("gh").join("hosts.yml"));
        }
        Some(
            dirs::home_dir()?
                .join(".config")
                .join("gh")
                .join("hosts.yml"),
        )
    }
}

/// Decodes a credential blob, which stores may hold as UTF-8 or as UTF-16LE.
///
/// Windows credential blobs are opaque bytes and the writer chooses the
/// encoding; `gh` and Git Credential Manager do not agree. A UTF-16LE token is
/// ASCII-heavy, so every second byte is zero, which tells the two apart
/// reliably for the token alphabets in use.
pub fn decode_credential_blob(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let looks_utf16 =
        bytes.len().is_multiple_of(2) && bytes.iter().skip(1).step_by(2).all(|byte| *byte == 0);
    let decoded = if looks_utf16 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&units).ok()?
    } else {
        String::from_utf8(bytes.to_vec()).ok()?
    };
    let trimmed = decoded.trim_end_matches('\0').trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Reads the platform credential store.
pub struct OsCredentialStore;

impl CredentialStore for OsCredentialStore {
    fn credential(&self, host: &str) -> Option<Credential> {
        let gh_username = gh_hosts_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|yaml| username_from_gh_hosts(&yaml, host));

        os_store_targets(host, gh_username.as_deref())
            .into_iter()
            .find_map(|target| read_target(&target))
    }

    fn name(&self) -> &'static str {
        "OS credential store"
    }
}

#[cfg(windows)]
fn read_target(target: &str) -> Option<Credential> {
    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{
        CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW,
    };
    use windows::core::PCWSTR;

    let target_wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let mut raw: *mut CREDENTIALW = std::ptr::null_mut();

    // SAFETY: `target_wide` is a NUL-terminated UTF-16 buffer that outlives the
    // call. On success Windows allocates `raw`, which is freed with `CredFree`
    // below; every value read out of it is copied first, so nothing borrowed
    // from the allocation escapes.
    let result = unsafe {
        CredReadW(
            PCWSTR(target_wide.as_ptr()),
            CRED_TYPE_GENERIC,
            None,
            &mut raw,
        )
    };
    if let Err(error) = result {
        // A missing entry is the normal case for a host the user never signed
        // into, and must stay quiet.
        if error.code() != ERROR_NOT_FOUND.to_hresult() {
            log::debug!("forge: reading credential {target} failed: {error}");
        }
        return None;
    }
    if raw.is_null() {
        return None;
    }

    // SAFETY: `raw` is non-null and owned here until `CredFree`.
    unsafe {
        let blob =
            std::slice::from_raw_parts((*raw).CredentialBlob, (*raw).CredentialBlobSize as usize);
        let token = decode_credential_blob(blob);
        let username = if (*raw).UserName.is_null() {
            None
        } else {
            (*raw)
                .UserName
                .to_string()
                .ok()
                .filter(|name| !name.is_empty())
        };
        let credential = token.map(|token| Credential { username, token });
        CredFree(raw as *const std::ffi::c_void);
        credential
    }
}

#[cfg(not(windows))]
fn read_target(_target: &str) -> Option<Credential> {
    // macOS Keychain and Linux libsecret are their own implementations. Until
    // they land, this platform has no OS-store credential, which the chain
    // treats as unauthenticated rather than as a failure.
    None
}
