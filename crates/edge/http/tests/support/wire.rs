//! Raw-wire helpers for the HTTP/1.1 tests: request builders and readers
//! that know enough about framing to stop at the end of one answer.

use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;

/// Read until the peer closes; a reset or timeout yields what arrived.
#[expect(
    clippy::panic,
    reason = "test helper: a failed read on the wire is the failure to report"
)]
pub(crate) fn read_until_end(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => bytes.extend_from_slice(chunk.get(..read).unwrap_or(&[])),
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::UnexpectedEof
                        | ErrorKind::ConnectionReset
                        | ErrorKind::BrokenPipe
                        | ErrorKind::WouldBlock
                        | ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(error) => panic!("reading the answer failed: {error}"),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The body of a raw HTTP/1 answer, chunk-decoded when chunked.
#[expect(
    clippy::expect_used,
    reason = "test helper: a headless answer is the failure to report"
)]
pub(crate) fn body_of(response: &str) -> String {
    let (head, body) = response
        .split_once("\r\n\r\n")
        .expect("headers, a blank line, then a body");
    if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        dechunk(body)
    } else {
        body.to_owned()
    }
}

/// Concatenate the payloads of a `chunked` transfer body.
fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size, tail)) = rest.split_once("\r\n") {
        let Ok(len) = usize::from_str_radix(size.trim_end(), 16) else {
            break;
        };
        let Some(chunk) = tail.get(..len) else { break };
        out.push_str(chunk);
        rest = tail.get(len + 2..).unwrap_or_default();
    }
    out
}

/// `METHOD target HTTP/1.1` with `connection: close` and the given extras.
pub(crate) fn request_with(method: &str, target: &str, headers: &[String], body: &str) -> String {
    let mut head =
        format!("{method} {target} HTTP/1.1\r\nhost: timbre.test\r\nconnection: close\r\n");
    for header in headers {
        head.push_str(header);
        head.push_str("\r\n");
    }
    format!("{head}\r\n{body}")
}

/// `METHOD target HTTP/1.1` without `connection: close`, so the connection
/// stays on the wire for the next request.
pub(crate) fn keepalive(method: &str, target: &str) -> String {
    format!("{method} {target} HTTP/1.1\r\nhost: timbre.test\r\n\r\n")
}

/// Read exactly one HTTP/1.1 answer (head plus its `content-length` or
/// chunked body) from an open connection; `None` when the peer closed it
/// before an answer arrived. Keep-alive tests send several requests on one
/// stream and must not swallow the start of the next answer.
#[expect(
    clippy::panic,
    reason = "test helper: a failed read on the wire is the failure to report"
)]
pub(crate) fn wait_for_status(stream: &mut TcpStream) -> Option<String> {
    let mut bytes: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return None,
            Ok(read) => {
                bytes.extend_from_slice(chunk.get(..read).unwrap_or(&[]));
                let text = String::from_utf8_lossy(&bytes).into_owned();
                if complete(&text) {
                    return Some(text);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::UnexpectedEof
                        | ErrorKind::ConnectionReset
                        | ErrorKind::BrokenPipe
                        | ErrorKind::WouldBlock
                        | ErrorKind::TimedOut
                ) =>
            {
                return None;
            }
            Err(error) => panic!("reading the answer failed: {error}"),
        }
    }
}

/// Whether `text` holds the whole head and body of one HTTP/1.1 response.
fn complete(text: &str) -> bool {
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        return false;
    };
    let lower = head.to_ascii_lowercase();
    if lower.contains("transfer-encoding: chunked") {
        // hyper ends a chunked body with the zero-size chunk; the reader
        // waits for exactly that instead of guessing from arrival order.
        return body.ends_with("0\r\n\r\n");
    }
    lower
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .is_some_and(|len| body.len() >= len)
}

/// Send `request` on `stream` and collect until the peer closes it; `TestServer`
/// opens the stream so a refused head can still show up as 431 or nothing.
pub(crate) fn roundtrip_on(mut stream: TcpStream, request: &str) -> String {
    if stream.write_all(request.as_bytes()).is_err() {
        return String::new(); // refused the head before the write landed
    }
    read_until_end(&mut stream)
}
