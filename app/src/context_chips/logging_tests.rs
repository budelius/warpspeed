use warp_completer::completer::{CommandExitStatus, CommandOutput};

use super::*;

#[test]
fn test_prompt_chip_log_filename_uses_channel_logfile_stem() {
    assert_eq!(
        prompt_chip_log_filename("warp_dev.log", 1234),
        "warp_dev.prompt_chips.1234.log"
    );
    assert_eq!(
        prompt_chip_log_filename("warp_local", 5678),
        "warp_local.prompt_chips.5678.log"
    );
}

#[test]
fn prompt_chip_log_path_stays_beside_resolved_frontend_log() {
    for (active_log, expected_sidecar) in [
        (
            "/tmp/warp-logs/warp_dev.log",
            "/tmp/warp-logs/warp_dev.prompt_chips.log",
        ),
        (
            "/tmp/warp-logs/warp-cli/warp_preview.log",
            "/tmp/warp-logs/warp-cli/warp_preview.prompt_chips.log",
        ),
        (
            "/tmp/warp-logs/oz/warp.log",
            "/tmp/warp-logs/oz/warp.prompt_chips.log",
        ),
    ] {
        let process_id = std::process::id();
        assert_eq!(
            prompt_chip_log_file_path(std::path::Path::new(active_log)).unwrap(),
            std::path::PathBuf::from(expected_sidecar.replace(
                ".prompt_chips.log",
                &format!(".prompt_chips.{process_id}.log"),
            ))
        );
    }
}

#[test]
fn test_format_log_entry_uses_explicit_empty_and_missing_markers() {
    let entry = format_log_entry(&ChipCommandLogEntry {
        chip_kind: &ContextChipKind::GithubPullRequest,
        chip_title: "GitHub Pull Request",
        phase: PromptChipExecutionPhase::Value,
        shell_type: ShellType::Zsh,
        working_directory: None,
        command: "",
        output: None,
        timed_out: true,
    });

    assert!(entry.contains("status: timed_out"));
    assert!(entry.contains("working_directory: <none>"));
    assert!(entry.contains("exit_code: <none>"));
    assert!(entry.contains("command:\n<<<COMMAND\n<empty>\n>>>COMMAND"));
    assert!(entry.contains("stdout:\n<<<STDOUT\n<empty>\n>>>STDOUT"));
    assert!(entry.contains("stderr:\n<<<STDERR\n<empty>\n>>>STDERR"));
}

#[test]
fn test_format_log_entry_preserves_stdout_and_stderr_sections() {
    let output = CommandOutput {
        stdout: b"https://github.com/warpdotdev/warp-internal/pull/123\n".to_vec(),
        stderr: b"warning output\n".to_vec(),
        status: CommandExitStatus::Success,
        exit_code: Some(warp_core::command::ExitCode::from(0)),
    };

    let entry = format_log_entry(&ChipCommandLogEntry {
        chip_kind: &ContextChipKind::GithubPullRequest,
        chip_title: "GitHub Pull Request",
        phase: PromptChipExecutionPhase::OnClick,
        shell_type: ShellType::Zsh,
        working_directory: Some("/tmp/project"),
        command: "gh pr view --json url --jq .url",
        output: Some(&output),
        timed_out: false,
    });

    assert!(entry.contains("phase: on_click"));
    assert!(entry.contains("status: success"));
    assert!(entry.contains("working_directory: /tmp/project"));
    assert!(entry.contains("command:\n<<<COMMAND\ngh pr view --json url --jq .url\n>>>COMMAND"));
    assert!(entry.contains(
        "stdout:\n<<<STDOUT\nhttps://github.com/warpdotdev/warp-internal/pull/123\n>>>STDOUT"
    ));
    assert!(entry.contains("stderr:\n<<<STDERR\nwarning output\n>>>STDERR"));
}

#[test]
fn test_format_log_entry_bounds_command_output() {
    let output = CommandOutput {
        stdout: vec![b'x'; MAX_LOGGED_OUTPUT_BYTES + 10_000],
        stderr: vec![],
        status: CommandExitStatus::Success,
        exit_code: Some(warp_core::command::ExitCode::from(0)),
    };
    let command = "c".repeat(MAX_LOGGED_COMMAND_BYTES + 10_000);

    let entry = format_log_entry(&ChipCommandLogEntry {
        chip_kind: &ContextChipKind::GitDiffStats,
        chip_title: "Git Diff Stats",
        phase: PromptChipExecutionPhase::Value,
        shell_type: ShellType::Zsh,
        working_directory: Some("/tmp/project"),
        command: &command,
        output: Some(&output),
        timed_out: false,
    });

    assert!(entry.contains(&format!(
        "<truncated original_bytes={} retained_bytes={MAX_LOGGED_COMMAND_BYTES}>",
        command.len()
    )));
    assert!(entry.contains(&format!(
        "<truncated original_bytes={} retained_bytes={MAX_LOGGED_OUTPUT_BYTES}>",
        output.stdout.len()
    )));
    assert!(entry.len() < MAX_LOGGED_COMMAND_BYTES + MAX_LOGGED_OUTPUT_BYTES + 2_000);
}

#[test]
fn scalar_fields_are_bounded() {
    let working_directory = "d".repeat(MAX_LOGGED_COMMAND_BYTES + 100);
    let entry = format_log_entry(&ChipCommandLogEntry {
        chip_kind: &ContextChipKind::GitDiffStats,
        chip_title: "Git Diff Stats",
        phase: PromptChipExecutionPhase::Value,
        shell_type: ShellType::Zsh,
        working_directory: Some(&working_directory),
        command: "git diff",
        output: None,
        timed_out: false,
    });

    assert!(entry.contains(&format!(
        "working_directory: {} <truncated original_bytes={} retained_bytes={MAX_LOGGED_COMMAND_BYTES}>",
        "d".repeat(MAX_LOGGED_COMMAND_BYTES),
        working_directory.len()
    )));
}

#[test]
fn custom_chip_names_and_titles_are_bounded() {
    let custom_name = "k".repeat(MAX_LOGGED_COMMAND_BYTES + 10_000);
    let chip_title = "t".repeat(MAX_LOGGED_COMMAND_BYTES + 10_000);
    let chip_kind = ContextChipKind::Custom { title: custom_name };
    let entry = format_log_entry(&ChipCommandLogEntry {
        chip_kind: &chip_kind,
        chip_title: &chip_title,
        phase: PromptChipExecutionPhase::Value,
        shell_type: ShellType::Zsh,
        working_directory: None,
        command: "custom command",
        output: None,
        timed_out: false,
    });

    assert!(entry.contains("chip_kind: Custom"));
    assert!(entry.contains(&format!(
        "chip_title: {} <truncated original_bytes={} retained_bytes={MAX_LOGGED_COMMAND_BYTES}>",
        "t".repeat(MAX_LOGGED_COMMAND_BYTES),
        chip_title.len()
    )));
    assert!(entry.len() < MAX_LOGGED_COMMAND_BYTES + 2_000);
}

#[test]
fn runtime_logger_drops_entries_when_its_queue_is_full() {
    let (sender, _receiver) = std::sync::mpsc::sync_channel(1);
    let dropped_entries = std::sync::Arc::new(AtomicU64::new(0));
    let logger = PromptChipLogger::Runtime {
        sender,
        dropped_entries: dropped_entries.clone(),
    };
    let log_entry = ChipCommandLogEntry {
        chip_kind: &ContextChipKind::GitDiffStats,
        chip_title: "Git Diff Stats",
        phase: PromptChipExecutionPhase::Value,
        shell_type: ShellType::Zsh,
        working_directory: None,
        command: "git diff",
        output: None,
        timed_out: false,
    };

    logger.log_shell_command(&log_entry);
    logger.log_shell_command(&log_entry);

    assert_eq!(dropped_entries.load(Ordering::Relaxed), 1);
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn writer_records_dropped_entries_even_without_a_later_command() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let log_path = temp_dir.path().join("prompt.log");
    let file = open_log_file(&log_path).unwrap();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let dropped_entries = std::sync::Arc::new(AtomicU64::new(3));
    drop(sender);

    write_log_entries(file, receiver, log_path.clone(), dropped_entries);

    assert_eq!(
        std::fs::read_to_string(log_path).unwrap(),
        "prompt_chip_log_entries_dropped: 3\n"
    );
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn concurrent_process_writers_create_dense_independent_files() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let paths = [
        temp_dir.path().join("prompt_chips.100.log"),
        temp_dir.path().join("prompt_chips.200.log"),
    ];
    let mut senders = Vec::new();
    let mut writers = Vec::new();
    for path in &paths {
        let file = open_log_file(path).unwrap();
        let (sender, receiver) = std::sync::mpsc::sync_channel(128);
        let dropped = std::sync::Arc::new(AtomicU64::new(0));
        let writer_path = path.clone();
        writers.push(std::thread::spawn(move || {
            write_log_entries(file, receiver, writer_path, dropped)
        }));
        senders.push(sender);
    }
    for (index, sender) in senders.iter().enumerate() {
        for record in 0..64 {
            sender
                .send(format!("writer={index} record={record}\n"))
                .unwrap();
        }
    }
    drop(senders);
    for writer in writers {
        writer.join().unwrap();
    }

    for path in paths {
        let contents = std::fs::read(path).unwrap();
        assert!(!contents.contains(&0));
        assert_eq!(contents.iter().filter(|byte| **byte == b'\n').count(), 64);
    }
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn stale_process_logs_are_removed_without_touching_the_current_process() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let current = temp_dir.path().join(prompt_chip_log_filename(
        "warpspeed.log",
        std::process::id(),
    ));
    let stale = temp_dir
        .path()
        .join(prompt_chip_log_filename("warpspeed.log", 4_000_000));
    let stale_rotated = rotated_log_path(&stale, 1);
    std::fs::write(&current, "current").unwrap();
    std::fs::write(&stale, "stale").unwrap();
    std::fs::write(&stale_rotated, "stale rotated").unwrap();

    cleanup_stale_prompt_logs(&current).unwrap();

    assert!(current.is_file());
    assert!(!stale.exists());
    assert!(!stale_rotated.exists());
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn rotation_retains_only_bounded_generations() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let log_path = temp_dir.path().join("prompt.log");
    std::fs::write(&log_path, "current").unwrap();
    std::fs::write(rotated_log_path(&log_path, 1), "previous").unwrap();
    std::fs::write(rotated_log_path(&log_path, 2), "oldest").unwrap();

    rotate_log_files(&log_path).unwrap();

    assert!(!log_path.exists());
    assert_eq!(
        std::fs::read_to_string(rotated_log_path(&log_path, 1)).unwrap(),
        "current"
    );
    assert_eq!(
        std::fs::read_to_string(rotated_log_path(&log_path, 2)).unwrap(),
        "previous"
    );
    assert!(!rotated_log_path(&log_path, 3).exists());
}
