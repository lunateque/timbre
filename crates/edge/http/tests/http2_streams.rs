//! The HTTP/2 concurrency bound on the raw wire: SETTINGS advertises 16
//! concurrent streams and the 17th stream of one connection is refused with
//! `REFUSED_STREAM` before any application logic runs, so the per-connection
//! slot cannot be multiplied into hundreds of concurrent handlers by one
//! client.

mod support;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use support::TestServer;

/// One HEADERS frame, HPACK literal-without-indexing, so the encoder state
/// never depends on what the server sent. `END_STREAM | END_HEADERS`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "test frame builder: the payload of one test header block is tiny"
)]
fn headers(stream: u32, path: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    for (name, value) in [
        (":method", "GET"),
        (":scheme", "http"),
        (":path", path),
        ("host", "timbre.test"),
    ] {
        payload.push(0x00u8);
        payload.push(name.len() as u8);
        payload.extend_from_slice(name.as_bytes());
        payload.push(value.len() as u8);
        payload.extend_from_slice(value.as_bytes());
    }
    let len = payload.len();
    let mut frame = vec![(len >> 16) as u8, (len >> 8) as u8, len as u8, 0x1, 0x5];
    frame.extend_from_slice(&stream.to_be_bytes());
    frame.extend_from_slice(&payload);
    frame
}

/// The SETTINGS parameters of one SETTINGS frame payload: pairs of
/// `(identifier, value)`, six bytes each.
fn settings(payload: &[u8]) -> BTreeMap<u16, u32> {
    let mut out = BTreeMap::new();
    let mut rest = payload;
    while rest.len() >= 6 {
        let (head, tail) = rest.split_at(6);
        let (id_bytes, value_bytes) = head.split_at(2);
        let id = u16::from_be_bytes(id_bytes.try_into().unwrap_or([0; 2]));
        let value = u32::from_be_bytes(value_bytes.try_into().unwrap_or([0; 4]));
        out.insert(id, value);
        rest = tail;
    }
    out
}

/// One frame: kind, stream id and body.
struct Frame {
    kind: u8,
    stream: u32,
    body: Vec<u8>,
}

/// Split complete frames off the front of `bytes`, leaving the partial tail.
#[expect(
    clippy::indexing_slicing,
    reason = "test wire parser: every index below is guarded by the length check"
)]
fn take_frames(bytes: &mut Vec<u8>) -> Vec<Frame> {
    let mut frames = Vec::new();
    // A frame needs its 9-byte header plus `len` payload bytes; a shorter
    // buffer stays until more arrives.
    while let Some(len) = bytes
        .get(..3)
        .map(|head| usize::from(head[0]) << 16 | usize::from(head[1]) << 8 | usize::from(head[2]))
    {
        if bytes.len() < 9 + len {
            break;
        }
        let kind = bytes[3];
        let stream = u32::from_be_bytes(bytes[5..9].try_into().unwrap_or([0; 4])) & 0x7F_FF_FF_FF;
        let body = bytes[9..9 + len].to_vec();
        bytes.drain(..9 + len);
        frames.push(Frame { kind, stream, body });
    }
    frames
}

/// Read frames until stream 33 was reset and all sixteen in-cap streams
/// produced a response head, or the socket goes silent.
#[expect(
    clippy::expect_used,
    reason = "test helper: a socket that refuses a read timeout is the failure to report"
)]
fn read_until_decision(socket: &mut std::net::TcpStream) -> Vec<Frame> {
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("a read timeout");
    let mut frames: Vec<Frame> = Vec::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    let decided = |frames: &[Frame]| {
        frames
            .iter()
            .any(|frame| frame.kind == 3 && frame.stream == 33)
            && (1..33)
                .step_by(2)
                .all(|stream| frames.iter().any(|f| f.kind == 1 && f.stream == stream))
    };
    while !decided(&frames) {
        frames.extend(take_frames(&mut pending));
        match socket.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(read) => pending.extend_from_slice(buf.get(..read).unwrap_or_default()),
        }
    }
    frames.extend(take_frames(&mut pending));
    frames
}

#[test]
fn the_seventeenth_concurrent_stream_is_refused() {
    let server = TestServer::start(|_| {});
    let mut socket = server.connect();
    socket
        .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
        .expect("the preface goes out");
    // `/slow` answers after two seconds: all sixteen accepted streams are
    // still open when the seventeenth HEADERS arrives on stream 33.
    for index in 0..17 {
        socket
            .write_all(&headers(1 + 2 * index, "/slow"))
            .expect("a HEADERS frame goes out");
    }
    socket.flush().expect("the frames go out");
    let frames = read_until_decision(&mut socket);
    // The SETTINGS the server advertises carry MAX_CONCURRENT_STREAMS (0x3) = 16.
    let advertised: BTreeMap<u16, u32> = frames
        .iter()
        .find(|frame| frame.kind == 4 && frame.stream == 0)
        .map_or_else(BTreeMap::new, |frame| settings(&frame.body));
    assert_eq!(
        advertised.get(&0x3),
        Some(&16),
        "SETTINGS advertise 16 concurrent streams: {:?}",
        frames
            .iter()
            .map(|frame| (frame.kind, frame.stream))
            .collect::<Vec<(u8, u32)>>()
    );
    // Stream 33, the seventeenth, is reset with REFUSED_STREAM (7) before any
    // handler runs: the RST_STREAM payload is the 32-bit error code.
    let refused = frames
        .iter()
        .find(|frame| frame.kind == 3 && frame.stream == 33)
        .unwrap_or_else(|| {
            panic!(
                "no RST_STREAM for the refused stream 33: {:?}",
                frames
                    .iter()
                    .map(|frame| (frame.kind, frame.stream))
                    .collect::<Vec<(u8, u32)>>()
            )
        });
    let code = refused
        .body
        .get(..4)
        .and_then(|head| <[u8; 4]>::try_from(head).ok())
        .map(u32::from_be_bytes);
    assert_eq!(
        code,
        Some(7),
        "the seventeenth stream is refused, not queued: {:?}",
        refused.body
    );
    // The sixteen inside the cap are answered with a HEADERS frame.
    let answered: Vec<u32> = frames
        .iter()
        .filter(|frame| frame.kind == 1 && frame.stream < 33)
        .map(|frame| frame.stream)
        .collect();
    assert_eq!(
        answered.len(),
        16,
        "every stream inside the cap gets a response head"
    );
    // The refusal reset one stream, not the connection: a fresh request is
    // served normally over HTTP/1.
    let answer = server.roundtrip(&support::request_with("GET", "/echo", &[], ""));
    assert!(
        answer.contains("hello"),
        "the server keeps serving after refusing a stream: {answer}"
    );
    server.stop();
}
