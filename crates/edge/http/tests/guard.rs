//! The guard in the request path: a guarded route answers the protocol's
//! own deny without running its handler, runs it with the principal when
//! the credential passes, and the `http.request` line names the user only
//! on the authenticated path.

mod support;

use std::time::{Duration, Instant};
use support::{TestServer, request_with};

/// Headers of the fake protocol's credential: a name (raw, to be
/// canonicalized by the kernel) and the provider's secret.
fn creds(user: &str, secret: &str) -> Vec<String> {
    vec![
        format!("x-test-user: {user}"),
        format!("x-test-key: {secret}"),
    ]
}

fn request(user: &str, secret: &str) -> String {
    request_with("GET", "/guarded", &creds(user, secret), "")
}

#[test]
fn valid_credentials_reach_the_handler_with_the_principal() {
    let server = TestServer::start(|_| {});
    let response = server.roundtrip(&request("  ALICE ", "good-secret"));
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "the accepted request is served: {response}"
    );
    // The handler saw the guard-minted principal with the kernel-canonical
    // name: the raw wire text was trimmed and lowercased on the way in.
    assert_eq!(
        support::body_of(&response),
        "ran:alice",
        "the handler runs as the authenticated user"
    );
    assert_eq!(server.guarded_calls(), 1, "the handler ran exactly once");
    server.stop();
}

#[test]
fn a_missing_credential_gets_the_route_deny_and_skips_the_handler() {
    let server = TestServer::start(|_| {});
    let response = server.roundtrip(&request_with("GET", "/guarded", &[], ""));
    assert!(
        response.starts_with("HTTP/1.1 401"),
        "the protocol's own refusal answers: {response}"
    );
    let head = response
        .split_once("\r\n\r\n")
        .map_or("", |(head, _)| head)
        .to_ascii_lowercase();
    // The deny response is a handler-shaped `Response`: its own
    // `content-length: 999` was stripped as framing, hyper derived the real
    // value, and the body reached the wire whole.
    assert!(
        !head.contains("999"),
        "the deny's framing header is stripped: {head}"
    );
    assert_eq!(support::body_of(&response), "deny:missing");
    assert_eq!(
        server.guarded_calls(),
        0,
        "a denied request never reaches the handler"
    );
    server.stop();
}

#[test]
fn a_rejected_credential_gets_the_route_deny_and_skips_the_handler() {
    let server = TestServer::start(|_| {});
    let response = server.roundtrip(&request("alice", "wrong-secret"));
    assert!(
        response.starts_with("HTTP/1.1 401"),
        "the provider's refusal answers in the protocol's shape: {response}"
    );
    assert_eq!(support::body_of(&response), "deny:rejected");
    assert_eq!(
        server.guarded_calls(),
        0,
        "a rejected credential never reaches the handler"
    );
    server.stop();
}

#[test]
fn a_forbidden_principal_gets_the_route_deny_and_skips_the_handler() {
    let server = TestServer::start(|_| {});
    // Valid credential, but the route asks a permission the identity lacks.
    let response = server.roundtrip(&request_with(
        "GET",
        "/admin",
        &creds("alice", "good-secret"),
        "",
    ));
    assert!(
        response.starts_with("HTTP/1.1 403"),
        "the permission check answers in the protocol's shape: {response}"
    );
    assert_eq!(support::body_of(&response), "deny:forbidden:test.admin");
    assert_eq!(
        server.guarded_calls(),
        0,
        "a forbidden request never reaches the handler"
    );
    server.stop();
}

/// The `http.request` line carries `user` only for an authenticated
/// request: the deny line and the public line must not name anyone.
#[test]
fn the_request_log_line_gains_the_user_name_only_when_authenticated() {
    let server = TestServer::start(|_| {});
    server.roundtrip(&request("Alice", "good-secret"));
    server.roundtrip(&request_with("GET", "/guarded", &[], ""));
    server.roundtrip(&request_with("GET", "/echo", &[], ""));
    let lines = server.lines();
    let request_lines = |path: &str| -> Vec<String> {
        lines
            .iter()
            .filter(|line| {
                line.contains(r#""event":"http.request""#)
                    && line.contains(&format!(r#""path":"{path}""#))
            })
            .cloned()
            .collect()
    };
    let guarded_lines = request_lines("/guarded");
    assert_eq!(guarded_lines.len(), 2, "both guarded requests logged");
    let ok = guarded_lines
        .iter()
        .find(|line| line.contains(r#""status":200"#))
        .unwrap_or_else(|| panic!("the accepted request logged its status: {guarded_lines:?}"));
    assert!(
        ok.contains(r#""user":"alice""#),
        "the accepted line names the canonical user: {ok}"
    );
    let denied = guarded_lines
        .iter()
        .find(|line| line.contains(r#""status":401"#))
        .unwrap_or_else(|| panic!("the deny logged the protocol's status: {guarded_lines:?}"));
    assert!(
        !denied.contains(r#""user":"#),
        "a denied request has no principal to name: {denied}"
    );
    let public = request_lines("/echo")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the public request logged"));
    assert!(
        !public.contains(r#""user":"#),
        "a public request has no principal to name: {public}"
    );
    server.stop();
}

/// A guarded non-form route is checked before a single body byte is
/// buffered: a bogus credential answers with the deny while the body is
/// still on the wire, so an over-cap body never reaches the reader and a
/// 404/405 path is refused without the body being read at all.
#[test]
fn a_denied_guarded_request_is_refused_before_the_body_is_read() {
    let server = TestServer::start(|config| config.max_body_bytes = 64);
    let big = "x".repeat(100);
    let denied = server.roundtrip(&request_with(
        "GET",
        "/guarded",
        &[
            format!("content-length: {}", big.len()),
            "x-test-user: alice".to_owned(),
            "x-test-key: wrong-secret".to_owned(),
        ],
        &big,
    ));
    assert!(
        denied.starts_with("HTTP/1.1 401"),
        "the guard denial answers, not the body cap: {denied}"
    );
    assert_eq!(support::body_of(&denied), "deny:rejected");
    // The same oversized body on a route nobody owns: 404, no body read.
    let missing = server.roundtrip(&request_with(
        "POST",
        "/missing",
        &[format!("content-length: {}", big.len())],
        &big,
    ));
    assert!(
        missing.starts_with("HTTP/1.1 404"),
        "the route lookup refuses before the body: {missing}"
    );
    // And a wrong method on a path that exists: 405 before the body too.
    let wrong_method = server.roundtrip(&request_with(
        "PUT",
        "/echo",
        &[format!("content-length: {}", big.len())],
        &big,
    ));
    assert!(
        wrong_method.starts_with("HTTP/1.1 405"),
        "the method refusal arrives before any body read: {wrong_method}"
    );
    server.stop();
}

/// Passing the guard does not lift the body limits: an authorized request
/// whose body is over the cap is still refused 413, and one inside it
/// reaches the handler with the whole body buffered after the check.
#[test]
fn an_authorized_guarded_request_still_gets_its_body_checked_and_delivered() {
    let server = TestServer::start(|config| config.max_body_bytes = 64);
    let big = "x".repeat(100);
    let over = server.roundtrip(&request_with(
        "GET",
        "/guarded",
        &[
            format!("content-length: {}", big.len()),
            "x-test-user: alice".to_owned(),
            "x-test-key: good-secret".to_owned(),
        ],
        &big,
    ));
    assert!(
        over.starts_with("HTTP/1.1 413"),
        "the cap still bites once the guard passed: {over}"
    );
    let small = "y".repeat(20);
    let accepted = server.roundtrip(&request_with(
        "GET",
        "/guarded-body",
        &[
            format!("content-length: {}", small.len()),
            "x-test-user: alice".to_owned(),
            "x-test-key: good-secret".to_owned(),
        ],
        &small,
    ));
    assert!(
        accepted.starts_with("HTTP/1.1 200"),
        "the authorized handler runs: {accepted}"
    );
    assert_eq!(
        support::body_of(&accepted),
        "ran:alice:20",
        "the body buffered after the check reaches the handler whole"
    );
    server.stop();
}

/// A form POST's credential may travel in the body, so the edge must read
/// and merge the body before its guard runs: the form authenticates.
#[test]
fn a_form_credential_authenticates_from_the_body() {
    let server = TestServer::start(|_| {});
    let body = "user=alice&key=good-secret";
    let accepted = server.roundtrip(&request_with(
        "POST",
        "/guarded-form",
        &[
            "content-type: application/x-www-form-urlencoded".to_owned(),
            format!("content-length: {}", body.len()),
        ],
        body,
    ));
    assert!(
        accepted.starts_with("HTTP/1.1 200"),
        "the guard saw the credential inside the body: {accepted}"
    );
    assert_eq!(support::body_of(&accepted), "ran:alice");
    let bad = "user=alice&key=wrong-secret";
    let denied = server.roundtrip(&request_with(
        "POST",
        "/guarded-form",
        &[
            "content-type: application/x-www-form-urlencoded".to_owned(),
            format!("content-length: {}", bad.len()),
        ],
        bad,
    ));
    assert!(
        denied.starts_with("HTTP/1.1 401"),
        "a wrong body credential is still denied: {denied}"
    );
    assert_eq!(support::body_of(&denied), "deny:rejected");
    server.stop();
}

/// A guard check that outlives `guard_timeout` is answered 504 and logged
/// with `http.guard_timeout`; the provider's late verdict is dropped and
/// the handler never runs.
#[test]
fn a_hanging_guard_is_answered_504_at_the_guard_deadline() {
    let server = TestServer::start(|config| {
        config.guard_timeout = Duration::from_millis(200);
        config.handler_timeout = Duration::from_secs(30);
    });
    let started = Instant::now();
    let answer = server.roundtrip(&request("alice", "hang-secret"));
    assert!(
        answer.starts_with("HTTP/1.1 504"),
        "an unanswered guard check answers 504: {answer}"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(150),
        "the deadline waited, not a snap-close"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "and answered at the 200 ms guard deadline, not the handler one"
    );
    assert_eq!(
        server.guarded_calls(),
        0,
        "a timed-out guard never releases the handler"
    );
    assert!(
        server.lines().iter().any(|line| {
            line.contains(r#""event":"http.guard_timeout""#)
                && line.contains(r#""code":"http.guard_timeout""#)
        }),
        "the guard deadline is logged with its code: {:?}",
        server.lines()
    );
    server.stop();
}
