//! A bounded buffer of a detached session's output.
//!
//! Sequence numbers let a reattaching client ask for "everything after what I
//! already saw", which is also exactly what a dropped network connection needs
//! in order to resume without a gap or a duplicate.
//!
//! Overrun is reported rather than hidden. A pane that silently misses output
//! is worse than one that says it missed some: the user cannot otherwise tell
//! "the build printed nothing" from "we lost it".

use std::collections::VecDeque;

/// Output held for a detached session, oldest bytes discarded first.
pub struct OutputRing {
    buffer: VecDeque<u8>,
    capacity: usize,

    /// Total bytes ever pushed, which is the sequence number just past the
    /// newest byte.
    sequence: u64,

    /// The sequence number of the oldest byte still held.
    oldest_sequence: u64,
}

impl OutputRing {
    /// Creates a ring holding at most `capacity_bytes`.
    pub fn new(capacity_bytes: usize) -> Self {
        Self {
            buffer: VecDeque::new(),
            capacity: capacity_bytes,
            sequence: 0,
            oldest_sequence: 0,
        }
    }

    /// Appends output, discarding the oldest bytes if that overruns capacity.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend(bytes.iter().copied());
        self.sequence += bytes.len() as u64;

        while self.buffer.len() > self.capacity {
            self.buffer.pop_front();
            self.oldest_sequence += 1;
        }
    }

    /// The sequence number just past the newest byte held.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Everything after `sequence`, and whether any of that span had already
    /// been discarded.
    ///
    /// `None` asks for whatever is still held, which is what a client attaching
    /// without prior state wants.
    pub fn read_after(&self, sequence: Option<u64>) -> (Vec<u8>, bool) {
        let requested_from = sequence.unwrap_or(self.oldest_sequence);
        let dropped = requested_from < self.oldest_sequence;
        let start = requested_from.max(self.oldest_sequence) - self.oldest_sequence;

        let bytes = self
            .buffer
            .iter()
            .skip(usize::try_from(start).unwrap_or(usize::MAX))
            .copied()
            .collect();

        (bytes, dropped)
    }
}

#[cfg(test)]
#[path = "ring_tests.rs"]
mod tests;
