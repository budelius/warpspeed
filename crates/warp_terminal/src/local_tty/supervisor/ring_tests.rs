use super::*;

/// The ordinary case: everything written since the client left comes back.
#[test]
fn output_after_a_sequence_is_returned_whole() {
    let mut ring = OutputRing::new(1024);
    ring.push(b"before");
    let checkpoint = ring.sequence();
    ring.push(b"after");

    let (bytes, dropped) = ring.read_after(Some(checkpoint));

    assert_eq!(bytes, b"after");
    assert!(!dropped, "nothing should have been dropped");
}

/// Overrun must be reported, not hidden.
#[test]
fn overrunning_the_ring_reports_dropped_output() {
    let mut ring = OutputRing::new(8);
    let checkpoint = ring.sequence();
    ring.push(b"0123456789abcdef");

    let (bytes, dropped) = ring.read_after(Some(checkpoint));

    assert!(dropped, "overrun must be reported");
    assert_eq!(
        bytes, b"89abcdef",
        "the most recent bytes are the ones worth keeping"
    );
}

/// A client with no sequence number -- a fresh attach -- gets whatever the ring
/// still holds.
#[test]
fn attaching_without_a_sequence_returns_the_whole_ring() {
    let mut ring = OutputRing::new(1024);
    ring.push(b"everything");

    let (bytes, dropped) = ring.read_after(None);

    assert_eq!(bytes, b"everything");
    assert!(!dropped);
}

/// Asking for output after the newest byte is the common case for a client that
/// never fell behind, and must return nothing rather than replaying the buffer.
#[test]
fn reading_after_the_newest_byte_returns_nothing() {
    let mut ring = OutputRing::new(1024);
    ring.push(b"already seen");

    let (bytes, dropped) = ring.read_after(Some(ring.sequence()));

    assert!(
        bytes.is_empty(),
        "a caught-up client must not be sent output twice, got {:?}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(!dropped);
}

/// Pushes are cumulative: a client that missed several writes gets all of them,
/// concatenated in order, not just the most recent push.
#[test]
fn output_from_several_pushes_is_returned_in_order() {
    let mut ring = OutputRing::new(1024);
    let checkpoint = ring.sequence();
    ring.push(b"one ");
    ring.push(b"two ");
    ring.push(b"three");

    let (bytes, dropped) = ring.read_after(Some(checkpoint));

    assert_eq!(bytes, b"one two three");
    assert!(!dropped);
}

/// The boundary that decides whether a client is told it lost output. Reading
/// from exactly the oldest byte held is complete; one before it is not.
#[test]
fn the_dropped_flag_turns_on_exactly_at_the_oldest_held_byte() {
    let mut ring = OutputRing::new(4);
    ring.push(b"abcdef");

    // Six bytes pushed, four retained, so the oldest held byte is sequence 2.
    let (_, dropped_at_oldest) = ring.read_after(Some(2));
    let (_, dropped_before_oldest) = ring.read_after(Some(1));

    assert!(
        !dropped_at_oldest,
        "reading from the oldest held byte loses nothing"
    );
    assert!(
        dropped_before_oldest,
        "reading from before the oldest held byte must report the loss"
    );
}
