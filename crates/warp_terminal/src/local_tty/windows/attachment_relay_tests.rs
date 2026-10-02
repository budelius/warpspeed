use std::io::{self, Read, Write};
use std::os::windows::io::FromRawHandle;
use std::time::Duration;

use instant::Instant;
use mio::windows::NamedPipe;
use mio::{Events, Interest, Poll, Token};

use super::*;

fn read_mio_with_timeout(
    pipe: &mut NamedPipe,
    poll: &mut Poll,
    buffer: &mut [u8],
) -> io::Result<usize> {
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut events = Events::with_capacity(4);

    loop {
        match pipe.read(buffer) {
            Ok(read) => return Ok(read),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }

        let Some(timeout) = deadline.checked_duration_since(Instant::now()) else {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out reading the attachment relay",
            ));
        };
        poll.poll(&mut events, Some(timeout))?;
        events.clear();
    }
}

fn exchange_with_mio_client(pty: &pipes::DuplexPipe) {
    let (relay, client) = AttachmentRelay::start(pty.server).unwrap();
    let mut client = unsafe { NamedPipe::from_raw_handle(client.0) };
    let mut poll = Poll::new().unwrap();
    poll.registry()
        .register(
            &mut client,
            Token(0),
            Interest::READABLE | Interest::WRITABLE,
        )
        .unwrap();
    let read_event = Event::new().unwrap();
    let write_event = Event::new().unwrap();
    let mut buffer = [0u8; 32];

    client.write_all(b"input").unwrap();
    let read = read_with_timeout(pty.client, read_event.0, &mut buffer, 500).unwrap();
    assert_eq!(&buffer[..read], b"input");

    write_with_timeout(pty.client, write_event.0, b"output", 500).unwrap();
    let read = read_mio_with_timeout(&mut client, &mut poll, &mut buffer).unwrap();
    assert_eq!(&buffer[..read], b"output");

    drop(client);
    drop(relay);
}

#[test]
fn repeated_mio_clients_can_type_and_receive_output_through_relay() {
    let pty = pipes::create_async_anonymous_pipe().unwrap();

    exchange_with_mio_client(&pty);
    exchange_with_mio_client(&pty);
    exchange_with_mio_client(&pty);

    let _ = unsafe { CloseHandle(pty.server) };
    let _ = unsafe { CloseHandle(pty.client) };
}
