//! Length-prefixed message framing over any byte stream.
//!
//! Deliberately ignorant of sockets and file descriptors: the Unix transport
//! passes descriptors alongside these bytes, the Windows one duplicates
//! handles, and a network transport does neither. Framing is the part they all
//! share.
//!
//! The encoding matches `local_tty::server::protocol` -- bincode behind an
//! eight-byte big-endian length -- so the codebase carries one supervisor
//! serialization format rather than two.

use std::io::{Read, Write};

use anyhow::{Context, Result, bail};

use super::compatibility;
use super::message::{Message, PROTOCOL_VERSION};

/// Refuses a length prefix larger than this, so a corrupt or hostile header
/// cannot make the reader allocate without bound.
const MAX_MESSAGE_BYTES: u64 = 64 * 1024 * 1024;

/// The size of the length prefix.
const LENGTH_PREFIX_BYTES: usize = 8;

/// Writes `message` behind its length prefix.
pub fn write_message<W: Write>(writer: &mut W, message: &Message) -> Result<()> {
    write_message_for_protocol(writer, message, PROTOCOL_VERSION)
}

pub(crate) fn write_message_for_protocol<W: Write>(
    writer: &mut W,
    message: &Message,
    version: u32,
) -> Result<()> {
    let payload = compatibility::encode(message, version)
        .context("Failed to serialize supervisor message")?;
    let length = payload.len() as u64;
    if length > MAX_MESSAGE_BYTES {
        bail!("Supervisor message of {length} bytes exceeds the framing limit");
    }

    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

/// Reads one message, or `None` when the stream does not yet hold a whole one.
///
/// A partial message is not an error. Transports deliver bytes when they
/// please, and treating a half-arrived snapshot as complete would repaint a
/// pane with a truncated screen.
pub fn read_message<R: Read>(reader: &mut R) -> Result<Option<Message>> {
    read_message_for_protocol(reader, PROTOCOL_VERSION)
}

pub(crate) fn read_message_for_protocol<R: Read>(
    reader: &mut R,
    version: u32,
) -> Result<Option<Message>> {
    let mut header = [0u8; LENGTH_PREFIX_BYTES];
    if !read_fully_or_none(reader, &mut header)? {
        return Ok(None);
    }

    let length = u64::from_be_bytes(header);
    if length > MAX_MESSAGE_BYTES {
        bail!("Supervisor message header claims {length} bytes, beyond the framing limit");
    }

    let mut payload = vec![0u8; length as usize];
    if !read_fully_or_none(reader, &mut payload)? {
        return Ok(None);
    }

    let message = compatibility::decode(&payload, version)
        .context("Failed to deserialize supervisor message")?;
    Ok(Some(message))
}

/// Fills `buffer`, returning `false` if the stream ran out first.
fn read_fully_or_none<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Result<bool> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => return Ok(false),
            Ok(read) => filled += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(error).context("Failed to read a supervisor message"),
        }
    }
    Ok(true)
}

#[cfg(test)]
#[path = "framing_tests.rs"]
mod tests;
