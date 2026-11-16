//! What the wire shows: the kernel `Response` arriving whole (in-memory and
//! streamed bodies) and the 16 KiB HTTP/1.1 header cap refusing an oversized
//! head while a 4 KiB one is served.

mod support;

use support::{TestServer, request_with};

/// 4(c): the kernel `Response` reaches the wire whole: status, header,
/// in-memory body for `Body::text`, streamed chunks for `Body::Stream`.
#[test]
fn a_kernel_response_reaches_the_wire_whole() {
    let server = TestServer::start(|_| {});
    let response = server.roundtrip(&request_with("GET", "/echo", &[], ""));
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "the status arrives: {response}"
    );
    let head = response
        .split_once("\r\n\r\n")
        .map_or("", |(head, _)| head)
        .to_ascii_lowercase();
    assert!(
        head.contains("content-type: text/plain; charset=utf-8"),
        "the header arrives: {response}"
    );
    assert_eq!(
        server.body(&request_with("GET", "/echo", &[], "")),
        "hello",
        "the full in-memory body arrives"
    );
    assert_eq!(
        server.body(&request_with("GET", "/stream", &[], "")),
        "first-second",
        "the streamed body arrives chunk by chunk"
    );
    let lines = server.lines();
    let logged = |path: &str| {
        lines.iter().any(|line| {
            line.contains(r#""event":"http.request""#)
                && line.contains(r#""status":200"#)
                && line.contains(&format!(r#""path":"{path}""#))
        })
    };
    assert!(
        logged("/echo") && logged("/stream"),
        "every answered request leaves one 200 line: {lines:?}"
    );
    server.stop();
}

/// 2: the 16 KiB header cap holds on HTTP/1.1: a ~20 KiB head is refused
/// (431 or the connection closed), a ~4 KiB head is served normally.
#[test]
fn an_oversized_header_block_is_refused_on_http1() {
    let server = TestServer::start(|_| {});
    let huge = request_with(
        "GET",
        "/echo",
        &[format!("x-huge: {}", "h".repeat(20 * 1024))],
        "",
    );
    let answer = server.status_line(&huge);
    assert!(
        answer.is_empty() || answer.starts_with("HTTP/1.1 431"),
        "a 20 KiB head is refused with 431 or a closed connection, got: {answer}"
    );
    let small = request_with(
        "GET",
        "/echo",
        &[format!("x-small: {}", "s".repeat(4 * 1024))],
        "",
    );
    assert!(
        server.status_line(&small).starts_with("HTTP/1.1 200"),
        "a 4 KiB head stays under the cap and is served"
    );
    server.stop();
}
