#[cfg(not(windows))]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail, ensure};
use uuid::Uuid;

use super::protocol::WorkerBootstrap;

const MAX_BOOTSTRAP_BYTES: u64 = 16 * 1024;
const BOOTSTRAP_DIRECTORY: &str = "window-process-workers";
const STALE_BOOTSTRAP_AGE: Duration = Duration::from_secs(10 * 60);

pub(crate) fn directory(state_dir: &Path) -> PathBuf {
    state_dir.join(BOOTSTRAP_DIRECTORY)
}

pub(crate) fn write(state_dir: &Path, bootstrap: &WorkerBootstrap) -> Result<PathBuf> {
    let directory = directory(state_dir);
    create_private_directory(&directory)?;
    let path = directory.join(format!("worker-{}.json", bootstrap.worker_id));
    let serialized = serde_json::to_vec(bootstrap).context("failed to encode worker bootstrap")?;
    ensure!(
        serialized.len() as u64 <= MAX_BOOTSTRAP_BYTES,
        "window worker bootstrap exceeds the byte limit"
    );

    let mut file = create_private_file(&path)?;
    file.write_all(&serialized)
        .context("failed to write worker bootstrap")?;
    file.sync_all()
        .context("failed to synchronize worker bootstrap")?;
    Ok(path)
}

pub(crate) fn consume(path: &Path) -> Result<WorkerBootstrap> {
    ensure_worker_bootstrap_name(path)?;
    let metadata = fs::symlink_metadata(path).context("failed to inspect worker bootstrap")?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "window worker bootstrap cannot be a symlink"
    );
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_BOOTSTRAP_BYTES,
        "invalid window worker bootstrap file"
    );

    let file = open_private_file(path)?;
    let mut serialized = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_BOOTSTRAP_BYTES + 1)
        .read_to_end(&mut serialized)
        .context("failed to read worker bootstrap")?;
    ensure!(
        serialized.len() as u64 <= MAX_BOOTSTRAP_BYTES,
        "window worker bootstrap exceeds the byte limit"
    );
    let bootstrap =
        serde_json::from_slice(&serialized).context("failed to decode worker bootstrap")?;
    fs::remove_file(path).context("failed to consume worker bootstrap")?;
    Ok(bootstrap)
}

pub(crate) fn clean_stale(state_dir: &Path, now: SystemTime) -> Result<()> {
    let directory = directory(state_dir);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("failed to inspect worker bootstrap directory"),
    };

    for entry in entries {
        let entry = entry.context("failed to inspect worker bootstrap entry")?;
        let path = entry.path();
        if ensure_worker_bootstrap_name(&path).is_err() {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        let modified = metadata
            .modified()
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if now.duration_since(modified).unwrap_or_default() >= STALE_BOOTSTRAP_AGE {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }
    }
    Ok(())
}

fn ensure_worker_bootstrap_name(path: &Path) -> Result<()> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        bail!("invalid window worker bootstrap filename");
    };
    let Some(id) = name
        .strip_prefix("worker-")
        .and_then(|name| name.strip_suffix(".json"))
    else {
        bail!("invalid window worker bootstrap filename");
    };
    Uuid::parse_str(id).context("invalid window worker bootstrap filename")?;
    Ok(())
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};

    if let Err(error) = fs::DirBuilder::new().mode(0o700).create(path)
        && error.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(error).context("failed to create worker bootstrap directory");
    }
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir(),
        "worker bootstrap path is not a directory"
    );
    ensure!(
        metadata.permissions().mode() & 0o077 == 0,
        "worker bootstrap directory permissions are too broad"
    );
    Ok(())
}

#[cfg(windows)]
fn create_private_directory(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    use windows::Win32::Foundation::ERROR_ALREADY_EXISTS;
    use windows::Win32::Storage::FileSystem::CreateDirectoryW;
    use windows::core::PCWSTR;

    let security =
        warp_terminal::local_tty::windows::pipe_security::PipeSecurity::current_user_only()?;
    let attributes = security.attributes();
    let path_utf16: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    if let Err(error) = unsafe { CreateDirectoryW(PCWSTR(path_utf16.as_ptr()), Some(&attributes)) }
        && error.code() != ERROR_ALREADY_EXISTS.to_hresult()
    {
        return Err(error).context("failed to create worker bootstrap directory");
    }
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "worker bootstrap path is not a private directory"
    );
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn create_private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).context("failed to create worker bootstrap directory")
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .context("failed to create worker bootstrap")
}

#[cfg(windows)]
fn create_private_file(path: &Path) -> Result<File> {
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::FromRawHandle as _;

    use windows::Win32::Foundation::GENERIC_WRITE;
    use windows::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_TEMPORARY, FILE_SHARE_MODE,
    };
    use windows::core::PCWSTR;

    let security =
        warp_terminal::local_tty::windows::pipe_security::PipeSecurity::current_user_only()?;
    let attributes = security.attributes();
    let path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_MODE(0),
            Some(&attributes),
            CREATE_NEW,
            FILE_ATTRIBUTE_TEMPORARY,
            None,
        )
    }
    .context("failed to create worker bootstrap")?;
    Ok(unsafe { File::from_raw_handle(handle.0) })
}

#[cfg(not(any(unix, windows)))]
fn create_private_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .context("failed to create worker bootstrap")
}

#[cfg(unix)]
fn open_private_file(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .context("failed to open worker bootstrap")
}

#[cfg(windows)]
fn open_private_file(path: &Path) -> Result<File> {
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::FromRawHandle as _;

    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_MODE,
        OPEN_EXISTING,
    };
    use windows::core::PCWSTR;

    let path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            GENERIC_READ.0,
            FILE_SHARE_MODE(0),
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
    }
    .context("failed to open worker bootstrap")?;
    Ok(unsafe { File::from_raw_handle(handle.0) })
}

#[cfg(not(any(unix, windows)))]
fn open_private_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .context("failed to open worker bootstrap")
}

#[cfg(test)]
#[path = "bootstrap_file_tests.rs"]
mod tests;
