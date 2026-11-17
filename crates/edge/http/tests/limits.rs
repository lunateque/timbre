//! What the limits refuse: a body over `max_body_bytes` (413), a body that
//! stalls past `body_read_timeout` (408, logged), and what a form POST
//! merges into the query.

mod support;

use std::io::Write;
use std::time::Duration;
use support::wire::read_until_end;
use support::{TestServer, body_of, request_with};

/// 4(a): a body over `max_body_bytes` is refused with 413 and the refusal is
/// logged like every other answered request.
#[test]
fn a_body_over_the_cap_answers_413() {
    let server = TestServer::start(|config| config.max_body_bytes = 64);
    let body = "x".repeat(100);
    let request = request_with(
        "POST",
        "/echo",
        &[format!("content-length: {}", body.len())],
        &body,
    );
    assert!(
        server.status_line(&request).starts_with("HTTP/1.1 413"),
        "an oversized body answers 413"
    );
    assert!(
        server.lines().iter().any(|line| {
            line.contains(r#""event":"http.request""#)
                && line.contains(r#""status":413"#)
                && line.contains(r#""path":"/echo""#)
        }),
        "the 413 is logged with its path and status: {:?}",
        server.lines()
    );
    server.stop();
}

/// 1: a body that stops arriving inside `body_read_timeout` is answered 408
/// and logged as `http.body_timeout`.
#[test]
fn a_stalled_body_is_answered_408() {
    let server = TestServer::start(|config| config.body_read_timeout = Duration::from_millis(200));
    let request = request_with(
        "POST",
        "/echo",
        &["content-length: 100".to_owned()],
        "xxxxxxxxxx",
    );
    let answer = server.roundtrip(&request);
    assert!(
        answer.starts_with("HTTP/1.1 408"),
        "a stalled body is answered 408: {answer}"
    );
    assert!(
        server
            .lines()
            .iter()
            .any(|line| line.contains(r#""event":"http.body_timeout""#)
                && line.contains(r#""code":"http.body_timeout""#)),
        "the timeout is logged with its code: {:?}",
        server.lines()
    );
    server.stop();
}

/// 4(c): a request body that ends in error (the connection dies mid-body)
/// is answered 400 through `http.body_failed`, its public message on the
/// wire and its code in the log.
#[test]
fn a_failed_body_stream_answers_400_and_logs_its_code() {
    let server = TestServer::start(|_| {});
    let request = request_with(
        "POST",
        "/echo",
        &["content-length: 100".to_owned()],
        "xxxxxxxxxx",
    );
    let mut stream = server.connect();
    stream
        .write_all(request.as_bytes())
        .expect("the head and a tenth of the body go out");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("the client half-closes: the body will end in error");
    let answer = read_until_end(&mut stream);
    assert!(
        answer.starts_with("HTTP/1.1 400"),
        "a body stream that failed is answered 400: {answer}"
    );
    assert_eq!(
        body_of(&answer),
        "The request body could not be read.",
        "the answer carries the failure's public message"
    );
    assert!(
        server.lines().iter().any(|line| {
            line.contains(r#""event":"http.body_failed""#)
                && line.contains(r#""code":"http.body_failed""#)
        }),
        "the failed body is logged with its code: {:?}",
        server.lines()
    );
    server.stop();
}

/// 4(b): the form fields of a form POST are merged into the query after the
/// URL query, in their own wire order, repeating keys rather than replacing.
#[test]
fn form_fields_merge_after_the_url_query() {
    let server = TestServer::start(|_| {});
    let request = request_with(
        "POST",
        "/query?a=1&b=2",
        &[
            "content-type: application/x-www-form-urlencoded".to_owned(),
            format!("content-length: {}", "c=3&a=9".len()),
        ],
        "c=3&a=9",
    );
    assert_eq!(
        server.body(&request),
        "a=1;b=2;c=3;a=9",
        "the URL query comes first, the form fields after it in wire order"
    );
    server.stop();
}
