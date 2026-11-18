//! The send-stall guard: a response write that makes no progress for
//! `send_stall_timeout` fails the connection and logs `http.send_stall`, so
//! a peer that took its bytes away without closing cannot pin a connection
//! slot for the life of the process.

mod support;

use std::io::{Read, Write};
use std::time::{Duration, Instant};
use support::TestServer;

#[test]
fn a_peer_that_stops_reading_dies_on_the_send_stall_bound() {
    let server = TestServer::start(|config| config.send_stall_timeout = Duration::from_millis(300));
    let mut stream = server.connect();
    stream
        .write_all(b"GET /endless HTTP/1.1\r\nhost: timbre.test\r\nconnection: close\r\n\r\n")
        .expect("the request goes out");
    stream.flush().expect("the request is out");
    // Drink the head plus a little, then go silent for good: the endless
    // stream outruns both socket buffers within milliseconds and the
    // server's writes block.
    let mut buf = [0u8; 4096];
    let mut total = 0usize;
    while total < 16 * 1024 {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(read) => total += read,
            Err(error) => panic!("the stream died before the client went quiet: {error}"),
        }
    }
    // Long past the 300 ms bound: the kernel receive buffer is full, the
    // server's write is parked, the guard must have fired and closed the
    // connection by now.
    std::thread::sleep(Duration::from_millis(1_500));
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("a read timeout");
    // Draining what the kernel still holds must end at EOF quickly: the
    // close is already there, only the buffer lies between us and it.
    let started = Instant::now();
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(read) => total += read,
            Err(error) => panic!("the stalled connection never reported EOF: {error}"),
        }
    }
    let drained_in = started.elapsed();
    assert!(
        drained_in < Duration::from_secs(5),
        "EOF arrived soon after the guard's bound, not after a full drain race: {drained_in:?}"
    );
    assert!(
        total > 16 * 1024,
        "the client really did receive bytes before going quiet: {total}"
    );
    let lines = server.lines();
    assert!(
        lines.iter().any(|line| {
            line.contains(r#""event":"http.send_stall""#) && line.contains(r#""ms":300"#)
        }),
        "the stalled write is logged with its bound: {lines:?}"
    );
    // The server goes on serving everyone else.
    let answer = server.roundtrip(&support::request_with("GET", "/echo", &[], ""));
    assert!(
        answer.contains("hello"),
        "the edge keeps working after the guard fired: {answer}"
    );
    server.stop();
}
