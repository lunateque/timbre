//! What the security fixes hold: parameter caps and their statuses, the
//! protocol-detection deadline, the handler deadline, framing headers that
//! hyper owns, multi-line `X-Forwarded-For`, and normalized paths.

mod support;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};
use support::TestServer;
use support::wire::{keepalive, read_until_end, wait_for_status};
use support::{body_of, request_with};

/// 1: more query pairs than `max_param_pairs` is a URL too long to answer.
#[test]
fn a_url_over_the_pair_cap_answers_414_and_logs() {
    let server = TestServer::start(|config| config.max_param_pairs = 5);
    let answer = server.roundtrip(&request_with(
        "GET",
        "/echo?a=1&b=2&c=3&d=4&e=5&f=6",
        &[],
        "",
    ));
    assert!(
        answer.starts_with("HTTP/1.1 414"),
        "six pairs over a cap of five answer 414: {answer}"
    );
    assert!(
        server.lines().iter().any(|line| {
            line.contains(r#""event":"http.params_refused""#)
                && line.contains(r#""source":"url""#)
                && line.contains(r#""reason":"too_many_pairs""#)
        }),
        "the refusal names the source and the reason: {:?}",
        server.lines()
    );
    assert!(
        server
            .status_line(&request_with("GET", "/echo?a=1&b=2", &[], ""))
            .starts_with("HTTP/1.1 200"),
        "a URL inside the cap is still served"
    );
    server.stop();
}

/// 1: a form over the pair cap answers 413.
#[test]
fn a_form_over_the_pair_cap_answers_413_and_logs() {
    let server = TestServer::start(|config| config.max_param_pairs = 5);
    let body = "a=1&b=2&c=3&d=4&e=5&f=6";
    let request = request_with(
        "POST",
        "/query",
        &[
            "content-type: application/x-www-form-urlencoded".to_owned(),
            format!("content-length: {}", body.len()),
        ],
        body,
    );
    assert!(
        server.status_line(&request).starts_with("HTTP/1.1 413"),
        "a six-pair form over a cap of five answers 413"
    );
    assert!(
        server.lines().iter().any(|line| {
            line.contains(r#""event":"http.params_refused""#)
                && line.contains(r#""source":"form""#)
                && line.contains(r#""reason":"too_many_pairs""#)
        }),
        "the refusal names the form as its source: {:?}",
        server.lines()
    );
    server.stop();
}

/// 1: a query key longer than 256 bytes is refused from the URL (431) and
/// from a form (413).
#[test]
fn an_overlong_key_is_refused() {
    let server = TestServer::start(|_| {});
    let long_key = "k".repeat(300);
    let answer = server.status_line(&request_with(
        "GET",
        &format!("/echo?{long_key}=1"),
        &[],
        "",
    ));
    assert!(
        answer.starts_with("HTTP/1.1 431"),
        "a 300-byte key from the URL answers 431: {answer}"
    );
    let body = format!("{long_key}=1");
    let form = request_with(
        "POST",
        "/query",
        &[
            "content-type: application/x-www-form-urlencoded".to_owned(),
            format!("content-length: {}", body.len()),
        ],
        &body,
    );
    assert!(
        server.status_line(&form).starts_with("HTTP/1.1 413"),
        "the same key from a form answers 413"
    );
    assert!(
        server.lines().iter().any(|line| {
            line.contains(r#""event":"http.params_refused""#)
                && line.contains(r#""reason":"key_too_long""#)
        }),
        "the refusal says the key was too long: {:?}",
        server.lines()
    );
    server.stop();
}

/// 1 and 7: a logged value is cut to 120 bytes with an ellipsis, secret
/// names redact by word, and the whole logged target stays bounded.
#[test]
fn logged_values_are_clipped_and_secret_names_redact() {
    let server = TestServer::start(|_| {});
    let long = "x".repeat(400);
    let target = format!("/clip?access_token={long}&note={long}");
    server.roundtrip(&request_with("GET", &target, &[], ""));
    let lines = server.lines();
    let logged = lines
        .iter()
        .find(|line| line.contains(r#""event":"http.request""#))
        .expect("the request is logged");
    let wanted = format!(
        r#""path":"/clip?access_token=***&note={}""#,
        "x".repeat(120) + "\u{2026}"
    );
    assert!(
        logged.contains(&wanted),
        "the secret value redacts, the plain one clips to 120 bytes plus an ellipsis: {logged}"
    );
    server.stop();
}

/// 2: a connection that sends nothing is closed within the header-read
/// budget, and the server keeps serving afterwards.
#[test]
fn a_silent_connection_releases_its_slot() {
    let server =
        TestServer::start(|config| config.header_read_timeout = Duration::from_millis(200));
    let mut idle = TcpStream::connect(server.address).expect("the server accepts");
    idle.set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a read timeout");
    let started = Instant::now();
    let mut one = [0u8; 1];
    let read = idle.read(&mut one);
    assert!(
        matches!(read, Ok(0)),
        "the silent connection is closed: {read:?}"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(150),
        "the deadline waited, not a snap-close"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "and closed within the 200 ms budget, not the read timeout"
    );
    assert!(
        server
            .status_line(&request_with("GET", "/echo", &[], ""))
            .starts_with("HTTP/1.1 200"),
        "a new request is served after a timed-out connection"
    );
    server.stop();
}

/// 2: a connection that sends one byte (still compatible with the h2
/// preface) is closed on the same deadline.
#[test]
fn a_dribbled_first_byte_is_closed_too() {
    let server =
        TestServer::start(|config| config.header_read_timeout = Duration::from_millis(200));
    let mut stuck = server.connect();
    stuck.write_all(b"P").expect("one byte goes out");
    let answer = read_until_end(&mut stuck);
    assert!(
        answer.is_empty(),
        "a half-preface connection gets no answer"
    );
    assert!(
        server
            .status_line(&request_with("GET", "/echo", &[], ""))
            .starts_with("HTTP/1.1 200"),
        "the server still serves after a closed dribble"
    );
    server.stop();
}

/// 3: a handler slower than `handler_timeout` is answered 504 and logged.
#[test]
fn a_handler_over_the_deadline_answers_504() {
    let server = TestServer::start(|config| config.handler_timeout = Duration::from_millis(300));
    let started = Instant::now();
    let answer = server.roundtrip(&request_with("GET", "/slow", &[], ""));
    assert!(
        answer.starts_with("HTTP/1.1 504"),
        "a 2 s handler under a 300 ms deadline answers 504: {answer}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(1800),
        "the answer comes at the deadline, not when the handler finishes"
    );
    assert!(
        server.lines().iter().any(|line| {
            line.contains(r#""event":"http.handler_timeout""#)
                && line.contains(r#""code":"http.handler_timeout""#)
        }),
        "the deadline is logged with its code: {:?}",
        server.lines()
    );
    server.stop();
}

/// 3: the deadline bounds the time to the head, never the body: a stream
/// whose first chunk is later than the deadline must reach the client whole.
#[test]
fn a_long_streaming_body_outlives_the_handler_deadline() {
    let server = TestServer::start(|config| config.handler_timeout = Duration::from_millis(100));
    let answer = server.roundtrip(&request_with("GET", "/slow-stream", &[], ""));
    assert!(
        answer.starts_with("HTTP/1.1 200"),
        "the head arrives before the deadline bites"
    );
    assert_eq!(
        body_of(&answer),
        "late",
        "a 400 ms body under a 100 ms handler deadline is not cut short"
    );
    server.stop();
}

/// 4: a kernel response carrying its own wrong framing headers is reframed
/// by hyper, and the connection stays usable for a second request.
#[test]
fn framing_headers_are_hyper_s_and_keep_alive_survives_a_wrong_length() {
    let server = TestServer::start(|_| {});
    let mut connection = server.connect();
    connection
        .write_all(keepalive("GET", "/wrong-length").as_bytes())
        .expect("the request goes out");
    let first = wait_for_status(&mut connection).expect("the first answer arrives");
    let head = first
        .split_once("\r\n\r\n")
        .map_or("", |(head, _)| head)
        .to_ascii_lowercase();
    assert!(
        first.starts_with("HTTP/1.1 200"),
        "the response is served: {first}"
    );
    assert!(
        !head.contains("connection: close"),
        "the handler's connection header is gone: {head}"
    );
    assert!(
        head.contains("content-length: 11") || head.contains("transfer-encoding: chunked"),
        "hyper derives framing for the 11-byte body: {head}"
    );
    assert_eq!(body_of(&first), "hello world", "the body arrives whole");
    connection
        .write_all(keepalive("GET", "/echo").as_bytes())
        .expect("the second request goes out");
    let second = wait_for_status(&mut connection).expect("the keep-alive answer arrives");
    assert!(
        second.starts_with("HTTP/1.1 200"),
        "the same connection serves the next request: {second}"
    );
    assert_eq!(body_of(&second), "hello");
}

/// 5: an `X-Forwarded-For` arriving as two header lines is the same list as
/// one comma-separated line; the rightmost-untrusted rule sees every hop.
#[test]
fn two_forwarded_for_lines_are_one_list() {
    let server = TestServer::start(|config| config.trusted_proxies = vec!["127.0.0.1".to_owned()]);
    let request = request_with(
        "GET",
        "/echo",
        &[
            "x-forwarded-for: 9.9.9.9".to_owned(),
            "x-forwarded-for: 203.0.113.9".to_owned(),
        ],
        "",
    );
    server.roundtrip(&request);
    let lines = server.lines();
    let logged = lines
        .iter()
        .find(|line| line.contains(r#""event":"http.request""#))
        .expect("the request is logged");
    assert!(
        logged.contains(r#""client":"203.0.113.9""#),
        "the second line is the proxy's own append and beats the forged first: {logged}"
    );
    server.stop();
}

/// 6: dot segments are normalized before routing; a path that cannot
/// normalize without climbing out of the root is 400.
#[test]
fn dot_segments_route_like_their_normalized_path() {
    let server = TestServer::start(|_| {});
    for (target, wanted) in [
        ("/rest/./x", "x"),
        ("/rest/y/../x", "x"),
        ("/rest//x", "x"),
        ("/rest/x/../y/../z", "z"),
    ] {
        assert_eq!(
            server.body(&request_with("GET", target, &[], "")),
            wanted,
            "{target} reaches the capture route normalized"
        );
    }
    // A dot segment is consumed before routing: it can never arrive at a
    // `{name}` capture.
    for target in ["/rest/.", "/rest/..", "/rest/%2e", "/rest/%2E%2E"] {
        let answer = server.status_line(&request_with("GET", target, &[], ""));
        assert!(
            answer.starts_with("HTTP/1.1 404") || answer.is_empty(),
            "{target} normalizes to a path the capture route does not match: {answer}"
        );
    }
    for target in ["/..", "/../x", "/a/../..", "/rest/../../.."] {
        let answer = server.status_line(&request_with("GET", target, &[], ""));
        assert!(
            answer.is_empty() || answer.starts_with("HTTP/1.1 400"),
            "{target} climbs above the root: {answer}"
        );
    }
    server.stop();
}

/// 8: a streaming body that fails mid-flight is logged, not swallowed.
#[test]
fn a_broken_stream_is_logged() {
    let server = TestServer::start(|_| {});
    let answer = server.roundtrip(&request_with("GET", "/broken", &[], ""));
    // Whatever hyper managed to flush, the failure after the head went out
    // has no channel left except the log.
    let lines = server.lines();
    assert!(
        lines.iter().any(|line| {
            line.contains(r#""event":"http.stream_failed""#)
                && line.contains(r#""code":"http.body_failed""#)
        }),
        "the mid-stream failure logs with its code (answer was {answer:?}): {lines:?}"
    );
    assert!(
        server
            .status_line(&request_with("GET", "/echo", &[], ""))
            .starts_with("HTTP/1.1 200"),
        "the server keeps serving after a broken stream"
    );
    server.stop();
}
