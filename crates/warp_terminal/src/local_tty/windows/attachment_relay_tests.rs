use std::io::{self, Read, Write};
use std::os::windows::io::FromRawHandle;
use std::time::Duration;

use instant::Instant;
use mio::windows::NamedPipe;
use mio::{Events, Interest, Poll, Token};

use super::*;

fn write_all(handle: HANDLE, event: HANDLE, bytes: &[u8]) -> Result<()> {
    write_unless_stopped(handle, event, bytes, &AtomicBool::new(false))
}

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

    write_all(pty.client, write_event.0, b"output").unwrap();
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

#[test]
fn relay_moves_output_and_input_in_both_directions() {
    let pty = pipes::create_async_anonymous_pipe().unwrap();
    let (relay, client) = AttachmentRelay::start(pty.server).unwrap();
    let write_event = Event::new().unwrap();
    let read_event = Event::new().unwrap();
    let mut buffer = [0u8; 32];

    write_all(pty.client, write_event.0, b"output").unwrap();
    let read = read_with_timeout(client, read_event.0, &mut buffer, 500).unwrap();
    assert_eq!(&buffer[..read], b"output");

    write_all(client, write_event.0, b"input").unwrap();
    let read = read_with_timeout(pty.client, read_event.0, &mut buffer, 500).unwrap();
    assert_eq!(&buffer[..read], b"input");

    let _ = unsafe { CloseHandle(client) };
    drop(relay);
    let _ = unsafe { CloseHandle(pty.server) };
    let _ = unsafe { CloseHandle(pty.client) };
}

#[test]
fn relay_survives_a_client_that_stalls_during_an_output_burst() {
    const BURST: usize = 1024 * 1024;
    let pty = pipes::create_async_anonymous_pipe().unwrap();
    let (relay, client) = AttachmentRelay::start(pty.server).unwrap();

    let shell = pty.client.0 as usize;
    let producer = thread::spawn(move || {
        let shell = HANDLE(shell as *mut _);
        let event = Event::new().unwrap();
        let chunk = [b'x'; 16 * 1024];
        for _ in 0..BURST / chunk.len() {
            write_all(shell, event.0, &chunk).unwrap();
        }
    });

    // Longer than any single relay write may take; a GUI client stalls like this while it
    // reflows a large scrollback on resize.
    thread::sleep(Duration::from_millis(500));

    let read_event = Event::new().unwrap();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut received = 0;
    while received < BURST {
        let read = read_with_timeout(client, read_event.0, &mut buffer, 2_000).unwrap();
        assert!(
            read > 0,
            "relay stopped delivering output after {received} bytes"
        );
        received += read;
    }
    producer.join().unwrap();
    assert_eq!(received, BURST);

    let write_event = Event::new().unwrap();
    write_all(client, write_event.0, b"input").unwrap();
    let mut input = [0u8; 32];
    let read = read_with_timeout(pty.client, read_event.0, &mut input, 500).unwrap();
    assert_eq!(&input[..read], b"input");

    let _ = unsafe { CloseHandle(client) };
    drop(relay);
    let _ = unsafe { CloseHandle(pty.server) };
    let _ = unsafe { CloseHandle(pty.client) };
}

#[test]
fn dropping_the_relay_ends_a_write_the_client_never_drains() {
    let pty = pipes::create_async_anonymous_pipe().unwrap();
    let (relay, client) = AttachmentRelay::start(pty.server).unwrap();
    let event = Event::new().unwrap();
    let chunk = [b'x'; 16 * 1024];
    // Fill both pipes so the relay is parked in a write to the client.
    for _ in 0..32 {
        if write_with_deadline(pty.client, event.0, &chunk, 200).is_err() {
            break;
        }
    }

    let started = Instant::now();
    drop(relay);
    assert!(started.elapsed() < Duration::from_secs(1));

    let _ = unsafe { CloseHandle(client) };
    let _ = unsafe { CloseHandle(pty.server) };
    let _ = unsafe { CloseHandle(pty.client) };
}

fn write_with_deadline(handle: HANDLE, event: HANDLE, bytes: &[u8], timeout_ms: u64) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let timer_stop = stop.clone();
    let timer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(timeout_ms));
        timer_stop.store(true, Ordering::Release);
    });
    let result = write_unless_stopped(handle, event, bytes, &stop);
    timer.join().unwrap();
    result
}
