#[cfg(not(test))]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
#[cfg(not(target_family = "wasm"))]
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write as _},
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{Local, SecondsFormat};
#[cfg(test)]
use parking_lot::Mutex;
use warp_completer::completer::{CommandExitStatus, CommandOutput};
#[cfg(not(target_family = "wasm"))]
use warp_errors::report_error;

use super::ContextChipKind;
use crate::terminal::shell::ShellType;

const EMPTY_VALUE: &str = "<empty>";
const MISSING_VALUE: &str = "<none>";
/// Maximum bytes captured from one diagnostic command field.
const MAX_LOGGED_COMMAND_BYTES: usize = 4 * 1024;
/// Maximum bytes captured from each diagnostic output stream.
const MAX_LOGGED_OUTPUT_BYTES: usize = 4 * 1024;
#[cfg(not(target_family = "wasm"))]
/// Maximum pending records before diagnostics are dropped rather than blocking prompt work.
const LOG_QUEUE_CAPACITY: usize = 128;
#[cfg(not(target_family = "wasm"))]
/// Maximum bytes retained in each active or rotated diagnostic file.
const MAX_LOG_FILE_BYTES: u64 = 10 * 1024 * 1024;
#[cfg(not(target_family = "wasm"))]
/// Number of rotated generations retained for the active diagnostic process.
const RETAINED_LOG_FILES: usize = 2;
#[cfg(not(target_family = "wasm"))]
const FLUSH_INTERVAL: Duration = Duration::from_millis(250);
#[cfg(not(target_family = "wasm"))]
const ENTRIES_PER_FLUSH: usize = 16;

pub(crate) struct ChipCommandLogEntry<'a> {
    pub chip_kind: &'a ContextChipKind,
    pub chip_title: &'a str,
    pub phase: PromptChipExecutionPhase,
    pub shell_type: ShellType,
    pub working_directory: Option<&'a str>,
    pub command: &'a str,
    pub output: Option<&'a CommandOutput>,
    pub timed_out: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum PromptChipExecutionPhase {
    Value,
    OnClick,
}

impl PromptChipExecutionPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Value => "value",
            Self::OnClick => "on_click",
        }
    }
}

#[derive(Clone)]
pub(crate) enum PromptChipLogger {
    Disabled,
    Runtime {
        sender: mpsc::SyncSender<String>,
        dropped_entries: Arc<AtomicU64>,
    },
    #[cfg(test)]
    TestBuffer {
        entries: Arc<Mutex<Vec<String>>>,
    },
}

impl Default for PromptChipLogger {
    fn default() -> Self {
        Self::shared()
    }
}

impl PromptChipLogger {
    pub(crate) fn shared() -> Self {
        cfg_if::cfg_if! {
            if #[cfg(test)] {
                Self::Disabled
            } else {
                static SHARED_LOGGER: OnceLock<PromptChipLogger> = OnceLock::new();
                SHARED_LOGGER.get_or_init(Self::init_runtime).clone()
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_buffer(entries: Arc<Mutex<Vec<String>>>) -> Self {
        Self::TestBuffer { entries }
    }

    pub(crate) fn log_shell_command(&self, entry: &ChipCommandLogEntry<'_>) {
        if matches!(self, Self::Disabled) {
            return;
        }
        let formatted = format_log_entry(entry);

        match self {
            Self::Disabled => {}
            Self::Runtime {
                sender,
                dropped_entries,
            } => match sender.try_send(formatted) {
                Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => {}
                Err(mpsc::TrySendError::Full(_)) => {
                    dropped_entries.fetch_add(1, Ordering::Relaxed);
                }
            },
            #[cfg(test)]
            Self::TestBuffer { entries } => {
                entries.lock().push(formatted);
            }
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn init_runtime() -> Self {
        if !warp_core::channel::ChannelState::enable_debug_features() {
            return Self::Disabled;
        }

        let log_path = match log_file_path() {
            Ok(log_path) => log_path,
            Err(err) => {
                log::warn!("Failed to determine prompt chip log file path: {err:#}");
                return Self::Disabled;
            }
        };

        match spawn_log_writer(log_path.clone()) {
            Ok((sender, dropped_entries)) => Self::Runtime {
                sender,
                dropped_entries,
            },
            Err(err) => {
                log::warn!(
                    "Failed to initialize prompt chip log writer at {}: {err:#}",
                    log_path.display()
                );
                Self::Disabled
            }
        }
    }

    #[cfg(target_family = "wasm")]
    fn init_runtime() -> Self {
        Self::Disabled
    }
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn log_file_path() -> anyhow::Result<PathBuf> {
    let log_path = warp_logging::log_file_path()?;
    prompt_chip_log_file_path(&log_path)
}

#[cfg(not(target_family = "wasm"))]
fn prompt_chip_log_file_path(log_path: &Path) -> anyhow::Result<PathBuf> {
    let channel_logfile_name = log_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("Resolved log path has no filename"))?;
    Ok(log_path.with_file_name(prompt_chip_log_filename(
        channel_logfile_name,
        std::process::id(),
    )))
}

#[cfg(not(target_family = "wasm"))]
fn spawn_log_writer(log_path: PathBuf) -> io::Result<(mpsc::SyncSender<String>, Arc<AtomicU64>)> {
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    cleanup_stale_prompt_logs(&log_path)?;
    rotate_if_oversized(&log_path)?;
    let file = open_log_file(&log_path)?;
    let (tx, rx) = mpsc::sync_channel(LOG_QUEUE_CAPACITY);
    let dropped_entries = Arc::new(AtomicU64::new(0));
    let writer_dropped_entries = dropped_entries.clone();

    std::thread::Builder::new()
        .name("prompt-chip-log-writer".to_string())
        .spawn(move || write_log_entries(file, rx, log_path, writer_dropped_entries))
        .map_err(io::Error::other)?;

    Ok((tx, dropped_entries))
}

#[cfg(not(target_family = "wasm"))]
fn cleanup_stale_prompt_logs(current_log_path: &Path) -> io::Result<()> {
    let Some(parent) = current_log_path.parent() else {
        return Ok(());
    };
    let Some(current_name) = current_log_path.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    let current_suffix = format!("{}.log", std::process::id());
    let Some(prefix) = current_name.strip_suffix(&current_suffix) else {
        return Ok(());
    };

    let candidates = fs::read_dir(parent)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            let process_id = name
                .strip_prefix(prefix)?
                .split_once(".log")?
                .0
                .parse::<u32>()
                .ok()?;
            Some((process_id, entry.path()))
        })
        .collect::<Vec<_>>();
    let process_ids = candidates
        .iter()
        .map(|(process_id, _)| sysinfo::Pid::from_u32(*process_id))
        .collect::<Vec<_>>();
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&process_ids),
        false,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    for (process_id, path) in candidates {
        if process_id != std::process::id()
            && system.process(sysinfo::Pid::from_u32(process_id)).is_none()
        {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

#[cfg(not(target_family = "wasm"))]
fn open_log_file(log_path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(log_path)
}

#[cfg(not(target_family = "wasm"))]
fn rotate_if_oversized(log_path: &Path) -> io::Result<()> {
    if fs::metadata(log_path).is_ok_and(|metadata| metadata.len() >= MAX_LOG_FILE_BYTES) {
        rotate_log_files(log_path)?;
    }
    Ok(())
}

#[cfg(not(target_family = "wasm"))]
fn rotate_log_files(log_path: &Path) -> io::Result<()> {
    for generation in (1..=RETAINED_LOG_FILES).rev() {
        let destination = rotated_log_path(log_path, generation);
        if destination.exists() {
            fs::remove_file(&destination)?;
        }
        let source = if generation == 1 {
            log_path.to_path_buf()
        } else {
            rotated_log_path(log_path, generation - 1)
        };
        if source.exists() {
            fs::rename(source, destination)?;
        }
    }
    Ok(())
}

#[cfg(not(target_family = "wasm"))]
fn rotated_log_path(log_path: &Path, generation: usize) -> PathBuf {
    let mut path = log_path.as_os_str().to_os_string();
    path.push(format!(".{generation}"));
    path.into()
}

#[cfg(not(target_family = "wasm"))]
fn write_log_entries(
    file: File,
    rx: mpsc::Receiver<String>,
    log_path: PathBuf,
    dropped_entries: Arc<AtomicU64>,
) {
    let mut writer = BufWriter::new(file);
    let mut current_size = fs::metadata(&log_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let mut unflushed_entries = 0;

    loop {
        let (entry, flush_after_write, disconnected) = match rx.recv_timeout(FLUSH_INTERVAL) {
            Ok(entry) => (Some(entry), false, false),
            Err(mpsc::RecvTimeoutError::Timeout) => (None, true, false),
            Err(mpsc::RecvTimeoutError::Disconnected) => (None, true, true),
        };
        let dropped = dropped_entries.swap(0, Ordering::Relaxed);
        let dropped_notice =
            (dropped > 0).then(|| format!("prompt_chip_log_entries_dropped: {dropped}\n"));
        let bytes_to_write =
            entry.as_ref().map_or(0, String::len) + dropped_notice.as_ref().map_or(0, String::len);

        if bytes_to_write > 0 {
            if current_size > 0
                && current_size.saturating_add(bytes_to_write as u64) > MAX_LOG_FILE_BYTES
            {
                if let Err(err) = writer.flush() {
                    report_log_write_error(err, &log_path);
                    return;
                }
                drop(writer);
                if let Err(err) = rotate_log_files(&log_path) {
                    report_log_write_error(err, &log_path);
                    return;
                }
                match open_log_file(&log_path) {
                    Ok(file) => {
                        writer = BufWriter::new(file);
                        current_size = 0;
                    }
                    Err(err) => {
                        report_log_write_error(err, &log_path);
                        return;
                    }
                }
            }

            let write_result = dropped_notice
                .as_ref()
                .map_or(Ok(()), |notice| writer.write_all(notice.as_bytes()))
                .and_then(|_| {
                    entry
                        .as_ref()
                        .map_or(Ok(()), |entry| writer.write_all(entry.as_bytes()))
                });
            if let Err(err) = write_result {
                report_log_write_error(err, &log_path);
                return;
            }
            current_size = current_size.saturating_add(bytes_to_write as u64);
            unflushed_entries += 1;
        }

        if (flush_after_write || unflushed_entries >= ENTRIES_PER_FLUSH) && unflushed_entries > 0 {
            if let Err(err) = writer.flush() {
                report_log_write_error(err, &log_path);
                return;
            }
            unflushed_entries = 0;
        }
        if disconnected {
            return;
        }
    }
}

#[cfg(not(target_family = "wasm"))]
fn report_log_write_error(err: io::Error, log_path: &Path) {
    report_error!(
        anyhow::Error::new(err).context("Failed to write prompt chip log entry"),
        extra: { "log_path" => %log_path.display() }
    );
}

fn prompt_chip_log_filename(channel_logfile_name: &str, process_id: u32) -> String {
    let channel_logfile_stem = channel_logfile_name
        .strip_suffix(".log")
        .unwrap_or(channel_logfile_name);
    format!("{channel_logfile_stem}.prompt_chips.{process_id}.log")
}

fn format_log_entry(entry: &ChipCommandLogEntry<'_>) -> String {
    let timestamp = Local::now().to_rfc3339_opts(SecondsFormat::Millis, false);
    let status = if entry.timed_out {
        "timed_out"
    } else if entry
        .output
        .is_some_and(|output| output.status == CommandExitStatus::Success)
    {
        "success"
    } else {
        "failure"
    };
    let exit_code = entry
        .output
        .and_then(CommandOutput::exit_code)
        .map(|exit_code| exit_code.to_string())
        .unwrap_or_else(|| MISSING_VALUE.to_string());
    let stdout = entry
        .output
        .map_or(&[][..], |output| output.stdout.as_slice());
    let stderr = entry
        .output
        .map_or(&[][..], |output| output.stderr.as_slice());

    format!(
        "\
===== PROMPT CHIP EXECUTION BEGIN =====
timestamp: {timestamp}
chip_kind: {:?}
chip_title: {}
phase: {}
shell_type: {:?}
working_directory: {}
status: {status}
timed_out: {}
exit_code: {exit_code}
{}
{}
{}
===== PROMPT CHIP EXECUTION END =====

",
        entry.chip_kind,
        entry.chip_title,
        entry.phase.as_str(),
        entry.shell_type,
        format_scalar_field(entry.working_directory),
        entry.timed_out,
        format_text_block(
            "command",
            "COMMAND",
            entry.command,
            MAX_LOGGED_COMMAND_BYTES,
        ),
        format_bytes_block("stdout", "STDOUT", stdout, MAX_LOGGED_OUTPUT_BYTES),
        format_bytes_block("stderr", "STDERR", stderr, MAX_LOGGED_OUTPUT_BYTES),
    )
}

fn format_scalar_field(value: Option<&str>) -> String {
    let Some(value) = value else {
        return MISSING_VALUE.to_owned();
    };
    let original_bytes = value.len();
    let retained = &value.as_bytes()[..original_bytes.min(MAX_LOGGED_COMMAND_BYTES)];
    let mut formatted = String::from_utf8_lossy(retained).into_owned();
    if original_bytes > retained.len() {
        formatted.push_str(&format!(
            " <truncated original_bytes={original_bytes} retained_bytes={}>",
            retained.len()
        ));
    }
    formatted
}

fn format_text_block(label: &str, marker: &str, content: &str, max_bytes: usize) -> String {
    format_bytes_block(label, marker, content.as_bytes(), max_bytes)
}

fn format_bytes_block(label: &str, marker: &str, content: &[u8], max_bytes: usize) -> String {
    let original_bytes = content.len();
    let retained = &content[..content.len().min(max_bytes)];
    let mut content = if retained.is_empty() {
        EMPTY_VALUE.to_string()
    } else {
        String::from_utf8_lossy(retained).into_owned()
    };
    if original_bytes > retained.len() {
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&format!(
            "<truncated original_bytes={original_bytes} retained_bytes={}>\n",
            retained.len()
        ));
    }

    format_block(label, marker, &content)
}

fn format_block(label: &str, marker: &str, content: &str) -> String {
    let content = if content.is_empty() {
        EMPTY_VALUE
    } else {
        content
    };
    let mut output = String::new();
    output.push_str(label);
    output.push_str(":\n<<<");
    output.push_str(marker);
    output.push('\n');
    output.push_str(content);
    if !content.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(">>>");
    output.push_str(marker);
    output.push('\n');
    output
}

#[cfg(test)]
#[path = "logging_tests.rs"]
mod tests;
