use super::{MAX_MESSAGE_BYTES, validate_message_len};

#[test]
fn message_length_limit_accepts_its_boundary() {
    validate_message_len(MAX_MESSAGE_BYTES).unwrap();
}

#[test]
fn message_length_limit_rejects_the_first_oversized_length() {
    let error = validate_message_len(MAX_MESSAGE_BYTES + 1).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Unknown error occurred: IPC message exceeds the 67108864-byte limit"
    );
}
