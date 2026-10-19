use super::*;

/// Serialized block JSON with byte fields encoded as base64 strings, as produced by `to_json`.
const BASE64_JSON: &str = r#"{
    "id": "test-block-1",
    "stylized_command": "ZWNobyBoZWxsbw==",
    "stylized_output": "aGVsbG8gd29ybGQ=",
    "pwd": null,
    "git_head": null,
    "virtual_env": null,
    "conda_env": null,
    "node_version": null,
    "exit_code": 0,
    "did_execute": true,
    "completed_ts": null,
    "start_ts": null,
    "ps1": null,
    "rprompt": null,
    "honor_ps1": false,
    "is_background": false,
    "session_id": null,
    "shell_host": null,
    "prompt_snapshot": null,
    "ai_metadata": null
}"#;

/// Serialized block JSON with byte fields as integer arrays, as produced by plain `serde_json`.
const ARRAY_JSON: &str = r#"{
    "id": "test-block-1",
    "stylized_command": [101,99,104,111,32,104,101,108,108,111],
    "stylized_output": [104,101,108,108,111,32,119,111,114,108,100],
    "pwd": null,
    "git_head": null,
    "virtual_env": null,
    "conda_env": null,
    "node_version": null,
    "exit_code": 0,
    "did_execute": true,
    "completed_ts": null,
    "start_ts": null,
    "ps1": null,
    "rprompt": null,
    "honor_ps1": false,
    "is_background": false,
    "session_id": null,
    "shell_host": null,
    "prompt_snapshot": null,
    "ai_metadata": null
}"#;

#[test]
fn from_json_accepts_base64_encoded_bytes() {
    let block = SerializedBlock::from_json(BASE64_JSON.as_bytes()).unwrap();
    assert_eq!(block.stylized_command, b"echo hello");
    assert_eq!(block.stylized_output, b"hello world");
}

#[test]
fn from_json_accepts_integer_array_bytes() {
    let block = SerializedBlock::from_json(ARRAY_JSON.as_bytes()).unwrap();
    assert_eq!(block.stylized_command, b"echo hello");
    assert_eq!(block.stylized_output, b"hello world");
}

#[test]
fn output_within_budget_is_unchanged() {
    let bounded = bound_stylized_output(b"one\r\ntwo".to_vec(), 32);

    assert_eq!(bounded.bytes, b"one\r\ntwo");
    assert_eq!(bounded.original_len, 8);
    assert!(!bounded.truncated);
}

#[test]
fn oversized_output_retains_recent_complete_lines_with_reset_boundaries() {
    let bounded = bound_stylized_output(
        b"first line is deliberately old\nsecond line is deliberately old\nthird line\nfourth line"
            .to_vec(),
        64,
    );

    assert_eq!(bounded.original_len, 85);
    assert!(bounded.truncated);
    assert!(
        bounded
            .bytes
            .starts_with(b"\x1b[0m[older output truncated]\r\n")
    );
    assert!(bounded.bytes.ends_with(b"\x1b[0m"));
    assert!(
        bounded
            .bytes
            .windows(11)
            .any(|window| window == b"fourth line")
    );
    assert!(bounded.bytes.len() <= 64);
}

#[test]
fn tiny_output_budget_remains_bounded() {
    let bounded = bound_stylized_output(vec![b'x'; 100], 7);

    assert_eq!(bounded.bytes, TERMINAL_STYLE_RESET);
    assert_eq!(bounded.original_len, 100);
    assert!(bounded.truncated);
}

#[test]
fn minimum_visible_output_budget_retains_a_truncation_marker() {
    let bounded = bound_stylized_output(vec![b'x'; 100], MIN_VISIBLE_TRUNCATED_OUTPUT_BYTES);

    assert_eq!(bounded.bytes, visible_truncated_output());
    assert!(bounded.truncated);
}

#[test]
fn enforcing_a_budget_repairs_a_nonvisible_truncated_output() {
    let mut block = SerializedBlock::new_for_test(Vec::new(), TERMINAL_STYLE_RESET.to_vec());
    block.stylized_output_truncated = true;
    block.stylized_output_original_bytes = Some(100);

    block.enforce_output_budget(MIN_VISIBLE_TRUNCATED_OUTPUT_BYTES);

    assert_eq!(block.stylized_output, visible_truncated_output());
    assert!(block.stylized_output_truncated);
    assert_eq!(block.stylized_output_original_bytes, Some(100));
}

#[test]
fn deserializing_legacy_arrays_bounds_output_while_reading() {
    let original_len = MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES + 4096;
    let block = SerializedBlock::new_for_test(b"echo legacy".to_vec(), vec![b'x'; original_len]);
    let json = serde_json::to_vec(&block).unwrap();

    let restored = SerializedBlock::from_json(&json).unwrap();

    assert!(restored.stylized_output.len() <= MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES);
    assert!(
        restored
            .stylized_output
            .starts_with(TRUNCATED_OUTPUT_PREFIX)
    );
    assert!(restored.stylized_output.ends_with(TERMINAL_STYLE_RESET));
    assert!(restored.stylized_output_truncated);
    assert_eq!(restored.stylized_output_original_bytes, Some(original_len));
}

#[test]
fn invalid_base64_output_preserves_the_rest_of_the_block() {
    let json = BASE64_JSON.replace(
        "\"stylized_output\": \"aGVsbG8gd29ybGQ=\"",
        "\"stylized_output\": \"not valid base64\"",
    );

    let restored = SerializedBlock::from_json(json.as_bytes()).unwrap();

    assert_eq!(restored.stylized_command, b"echo hello");
    assert_eq!(restored.stylized_output, UNAVAILABLE_OUTPUT);
}

#[test]
fn invalid_legacy_array_output_preserves_the_rest_of_the_block() {
    let json = ARRAY_JSON.replace(
        "\"stylized_output\": [104,101,108,108,111,32,119,111,114,108,100]",
        "\"stylized_output\": [104,999,null,{\"invalid\":true}]",
    );

    let restored = SerializedBlock::from_json(json.as_bytes()).unwrap();

    assert_eq!(restored.stylized_command, b"echo hello");
    assert_eq!(restored.stylized_output, UNAVAILABLE_OUTPUT);
    assert!(restored.stylized_output_truncated);
}

#[test]
fn invalid_legacy_array_command_invalidates_the_block() {
    let json = ARRAY_JSON.replace(
        "\"stylized_command\": [101,99,104,111,32,104,101,108,108,111]",
        "\"stylized_command\": [101,999]",
    );

    assert!(SerializedBlock::from_json(json.as_bytes()).is_err());
}

#[test]
fn truncated_output_cancels_incomplete_terminal_control_sequences() {
    let mut output = vec![b'x'; 128];
    output.extend_from_slice(b"\x1b]8;;https://example.com");

    let bounded = bound_stylized_output(output, 64);

    assert!(bounded.bytes.ends_with(b"\x18\x1b[0m"));
}

#[test]
fn enforcing_a_budget_records_truncation_metadata() {
    let original_len = MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES + 1;
    let mut block = SerializedBlock::new_for_test(Vec::new(), vec![b'x'; original_len]);

    block.enforce_output_budget(MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES);

    assert!(block.stylized_output_truncated);
    assert_eq!(block.stylized_output_original_bytes, Some(original_len));
}
