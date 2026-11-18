//! A streamed body's declared length: `content-length` is the one framing
//! header a `Body::Stream` response keeps when the handler's value is
//! well-formed, it reaches the client on both protocols, the request log
//! counts it, and a stream that breaks the promise costs the connection
//! instead of desynchronizing it. A value the edge cannot trust is dropped
//! and logged.

mod support;

use std::io::Write;
use support::wire::{keepalive, wait_for_status};
use support::{TestServer, request_with};

/// The lowercased head of a raw HTTP/1 answer, without the body.
fn head_of(response: &str) -> String {
    response
        .split_once("\r\n\r\n")
        .map_or_else(String::new, |(head, _)| head.to_ascii_lowercase())
}

/// A streamed response with a correct `content-length` arrives with exactly
/// that header over HTTP/1.1, framed by length rather than chunked, and the
/// `http.request` line reports the declared size.
#[test]
fn a_declared_stream_length_reaches_http1_and_the_log() {
    let server = TestServer::start(|_| {});
    let answer = server.roundtrip(&request_with("GET", "/declared-stream", &[], ""));
    assert!(
        answer.starts_with("HTTP/1.1 200"),
        "the stream is served: {answer}"
    );
    let head = head_of(&answer);
    assert!(
        head.contains("content-length: 12"),
        "the declared length reaches the wire: {head}"
    );
    assert!(
        !head.contains("transfer-encoding"),
        "a declared length means no chunked framing: {head}"
    );
    assert_eq!(
        support::body_of(&answer),
        "first-second",
        "the streamed body arrives whole"
    );
    let lines = server.lines();
    let logged = lines.iter().any(|line| {
        line.contains(r#""event":"http.request""#)
            && line.contains(r#""path":"/declared-stream""#)
            && line.contains(r#""bytes":12"#)
    });
    assert!(
        logged,
        "the request log counts the declared size: {lines:?}"
    );
    server.stop();
}

/// The same stream over HTTP/2 (h2c, decided by the preface probe): the
/// length rides the wire as a `content-length` header field.
#[test]
fn a_declared_stream_length_reaches_http2() {
    let server = TestServer::start(|_| {});
    let answer = server.roundtrip_h2("/declared-stream");
    let (head, body) = answer.split_once("\n\n").unwrap_or((answer.as_str(), ""));
    assert!(
        head.starts_with("200\n"),
        "the stream is served over h2: {answer}"
    );
    let lowered = head.to_ascii_lowercase();
    assert!(
        lowered.contains("content-length: 12"),
        "the declared length reaches the h2 client: {head}"
    );
    assert_eq!(body, "first-second", "the body arrives whole: {answer}");
    server.stop();
}

/// A `HEAD` request against a handler that answers with the declared stream
/// keeps the header (Subsonic clients probe sizes this way) while the body
/// stays suppressed.
#[test]
fn a_head_stream_keeps_the_declared_length() {
    let server = TestServer::start(|_| {});
    let answer = server.roundtrip(&request_with("HEAD", "/declared-stream", &[], ""));
    assert!(
        answer.starts_with("HTTP/1.1 200"),
        "the HEAD answer arrives: {answer}"
    );
    let head = head_of(&answer);
    assert!(
        head.contains("content-length: 12"),
        "the declared length survives a HEAD: {head}"
    );
    let (_, body) = answer
        .split_once("\r\n\r\n")
        .unwrap_or((answer.as_str(), ""));
    assert!(
        body.is_empty(),
        "a HEAD answer carries no body bytes: {answer}"
    );
    server.stop();
}

/// A stream that ends before its declared length breaks the promise the
/// wire was framed by. What hyper actually does (1.12.0, verified against
/// `hyper::proto::h1`): on the body's end-of-stream its length encoder still
/// owes bytes, `end_body` records `body write aborted: stream indicated EOF
/// was reached too early` and hard-closes the connection instead of letting
/// the next keep-alive read start off the wrong byte. The head and the few
/// body bytes may or may not have reached the socket before that close -
/// hyper errors without a guaranteed flush - so the client either sees a
/// truncated answer or nothing; what it must never see is the lie carried
/// on into the next response.
#[test]
fn a_short_stream_closes_the_connection() {
    let server = TestServer::start(|_| {});
    let mut connection = server.connect();
    connection
        .write_all(keepalive("GET", "/short-stream").as_bytes())
        .expect("the request goes out");
    // A complete answer cannot exist: the body is shorter than promised, so
    // `wait_for_status` never sees `content-length` worth of bytes. It
    // returns `None` at the close (whatever arrived, if anything, died with
    // it) - or, if the promise were silently forgiven, a full answer.
    let first = wait_for_status(&mut connection);
    assert!(
        first.is_none(),
        "the promised body never completed; the connection died instead: {first:?}"
    );
    // On the dead connection a second request buys nothing: no answer was
    // framed on a stream that lied about its length.
    let second = keepalive("GET", "/echo");
    if connection.write_all(second.as_bytes()).is_ok() {
        let rest = support::wire::read_until_end(&mut connection);
        assert!(
            !rest.contains("HTTP/1.1"),
            "no second answer on a connection that lied: {rest:?}"
        );
    }
    // And the server itself is unharmed.
    assert!(
        server
            .status_line(&request_with("GET", "/echo", &[], ""))
            .starts_with("HTTP/1.1 200"),
        "the server keeps serving after a short stream"
    );
    server.stop();
}

/// The same promise tested against a client that pipelines blind: whatever
/// bytes the connection ever carries must start at an answer's first byte.
/// A desynchronized keep-alive would show the second answer beginning
/// mid-status-line.
#[test]
fn a_pipelined_request_after_a_short_stream_never_reads_mid_answer() {
    let server = TestServer::start(|_| {});
    let mut connection = server.connect();
    connection
        .write_all(
            format!(
                "{}{}",
                keepalive("GET", "/short-stream"),
                keepalive("GET", "/echo")
            )
            .as_bytes(),
        )
        .expect("both requests go out at once");
    let mut answers = Vec::new();
    while answers.len() < 4 {
        match wait_for_status(&mut connection) {
            Some(answer) => answers.push(answer),
            None => break,
        }
    }
    for answer in &answers {
        assert!(
            answer.starts_with("HTTP/1.1 200"),
            "an answer started mid-byte: {answer}"
        );
    }
    // Correct behavior ends the connection with the lie: at most one answer
    // can complete, and the short stream's own never does (its body is
    // short), so a clean server closes the books with zero answers.
    assert!(
        answers.is_empty(),
        "the short stream's answer never completed and nothing followed it: {answers:?}"
    );
    server.stop();
}

/// A stream longer than its declared length: the bytes past the promise are
/// cut, the framing stays exact and the connection keeps serving - the
/// decoder of the next request never sees a leftover byte.
#[test]
fn a_long_stream_stops_at_the_declared_length_and_the_connection_survives() {
    let server = TestServer::start(|_| {});
    let mut connection = server.connect();
    connection
        .write_all(keepalive("GET", "/long-stream").as_bytes())
        .expect("the request goes out");
    let first = wait_for_status(&mut connection).expect("the first answer arrives");
    assert!(
        first.starts_with("HTTP/1.1 200"),
        "the stream is served: {first}"
    );
    assert_eq!(
        support::body_of(&first),
        "first",
        "only the declared bytes are on the wire"
    );
    connection
        .write_all(keepalive("GET", "/echo").as_bytes())
        .expect("the second request goes out");
    let second = wait_for_status(&mut connection).expect("the keep-alive answer arrives");
    assert!(
        second.starts_with("HTTP/1.1 200"),
        "the overshoot did not poison the connection: {second}"
    );
    assert_eq!(support::body_of(&second), "hello");
    server.stop();
}

/// A declared length that is not a number never reaches the wire: the
/// header is dropped, hyper frames the stream its own way, the body still
/// arrives and the refusal is logged with the offending value.
#[test]
fn a_malformed_declared_length_is_dropped_and_logged() {
    let server = TestServer::start(|_| {});
    let answer = server.roundtrip(&request_with("GET", "/bad-declared-stream", &[], ""));
    assert!(
        answer.starts_with("HTTP/1.1 200"),
        "the stream is still served: {answer}"
    );
    let head = head_of(&answer);
    assert!(
        !head.contains("content-length"),
        "the value the edge cannot trust stays off the wire: {head}"
    );
    assert_eq!(
        support::body_of(&answer),
        "first-second",
        "hyper's own framing carries the body whole"
    );
    let lines = server.lines();
    let logged = lines.iter().any(|line| {
        line.contains(r#""event":"http.content_length_invalid""#)
            && line.contains(r#""value":"twelve""#)
    });
    assert!(
        logged,
        "the dropped declaration is logged with its value: {lines:?}"
    );
    server.stop();
}

/// Two `content-length` values, even agreeing ones, are one header too
/// many: the edge cannot vouch for the pair, so it drops both, frames by
/// hyper's rules and logs the refusal.
#[test]
fn a_duplicated_declared_length_is_dropped_and_logged() {
    let server = TestServer::start(|_| {});
    let answer = server.roundtrip(&request_with("GET", "/double-declared-stream", &[], ""));
    assert!(
        answer.starts_with("HTTP/1.1 200"),
        "the stream is still served: {answer}"
    );
    let head = head_of(&answer);
    assert!(
        !head.contains("content-length"),
        "a duplicated value stays off the wire: {head}"
    );
    assert_eq!(support::body_of(&answer), "first-second");
    let lines = server.lines();
    let logged = lines.iter().any(|line| {
        line.contains(r#""event":"http.content_length_invalid""#)
            && line.contains(r#""value":"12, 12""#)
    });
    assert!(logged, "the duplicated declaration is logged: {lines:?}");
    server.stop();
}
