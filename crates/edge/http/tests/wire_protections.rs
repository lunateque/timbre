//! The protections we rely on from hyper/httparse, pinned on the raw wire
//! rather than trusted from their changelogs: if a future upgrade starts
//! accepting any of these, the test says so.

mod support;

use support::TestServer;
use support::request_with;

/// Two differing `Content-Length` values describe one body two ways: hyper
/// refuses the request outright (the CVE-2020-11981 class) instead of
/// guessing, so no reader downstream can disagree about the body's end.
#[test]
fn duplicate_differing_content_lengths_are_400() {
    let server = TestServer::start(|_| {});
    let request = request_with(
        "GET",
        "/echo",
        &[
            "content-length: 5".to_owned(),
            "content-length: 6".to_owned(),
        ],
        "hello",
    );
    assert!(
        server.status_line(&request).starts_with("HTTP/1.1 400"),
        "a self-contradictory framing is refused"
    );
    // Two agreeing values are one value stated twice, which RFC 9110 allows.
    let same = request_with(
        "GET",
        "/echo",
        &[
            "content-length: 5".to_owned(),
            "content-length: 5".to_owned(),
        ],
        "hello",
    );
    assert!(
        server.status_line(&same).starts_with("HTTP/1.1 200"),
        "agreeing duplicates stay acceptable"
    );
    server.stop();
}

/// A `Transfer-Encoding` whose last coding is not `chunked` (here the
/// list form `chunked, gzip`) cannot frame the body: hyper answers 400
/// rather than trusting a decoding it never implements.
#[test]
fn a_transfer_encoding_that_is_not_chunked_is_400() {
    let server = TestServer::start(|_| {});
    let request = request_with(
        "GET",
        "/echo",
        &["transfer-encoding: chunked, gzip".to_owned()],
        "0\r\n\r\n",
    );
    let answer = server.status_line(&request);
    assert!(
        answer.is_empty() || answer.starts_with("HTTP/1.1 400"),
        "a `TE: chunked, gzip` head is refused or closed, never framed: {answer}"
    );
    server.stop();
}

/// An obsolete line fold (a header continued on a SP/HT-starting line, the
/// CVE-2021-40438 class) is refused in requests: httparse's
/// `allow_obsolete_multiline_headers_in_responses` cannot smuggle a folded
/// header past a front end that would not fold it back.
#[test]
fn an_obsolete_line_fold_is_400() {
    let server = TestServer::start(|_| {});
    let answer = server.roundtrip(
        "GET /echo HTTP/1.1\r\nhost: timbre.test\r\n x-folded\r\nconnection: close\r\n\r\n",
    );
    let first = answer.lines().next().unwrap_or_default();
    assert!(
        answer.is_empty() || first.starts_with("HTTP/1.1 400"),
        "a continuation line is refused: {answer:?}"
    );
    server.stop();
}

/// A percent-decoded NUL byte in the path is refused (the CVE-2010-2263
/// class): a string with an embedded terminator is not a resource name.
#[test]
fn a_nul_byte_in_the_path_is_400() {
    let server = TestServer::start(|_| {});
    for target in ["/a%00b", "/rest/%00", "/echo%00.txt"] {
        let answer = server.status_line(&request_with("GET", target, &[], ""));
        assert!(
            answer.starts_with("HTTP/1.1 400"),
            "{target} decodes to a path with a NUL in it: {answer}"
        );
    }
    server.stop();
}

/// An encoded slash: this edge percent-decodes the path once, before
/// splitting it into segments, so `%2F` is a `/` for routing and cannot
/// hide a segment boundary from a `{name}` capture. `/rest/a%2Fb` names the
/// two-segment path `/rest/a/b`, which no route owns: 404, and the capture
/// never sees `a/b` (nor `a%2Fb`). The behavior is pinned whatever it is, so
/// a change in the decode-then-split order shows up here.
#[test]
fn an_encoded_slash_decodes_before_routing() {
    let server = TestServer::start(|_| {});
    // `/rest/a%2Fb` -> `/rest/a/b`: one segment short of the capture route.
    assert!(
        server
            .status_line(&request_with("GET", "/rest/a%2Fb", &[], ""))
            .starts_with("HTTP/1.1 404"),
        "the decoded slash splits the path before routing"
    );
    // `/%2Fecho` -> `//echo`, which collapses to `/echo` and is served: the
    // slash was decoded to structure, not kept as segment text.
    assert!(
        server
            .status_line(&request_with("GET", "/%2Fecho", &[], ""))
            .starts_with("HTTP/1.1 200"),
        "a decoded slash becomes a real separator"
    );
    // A capture keeps everything else intact: `%252F` decodes once to the
    // literal text `%2F` (double encoding stays one safe layer deep).
    assert_eq!(
        server.body(&request_with("GET", "/rest/a%252Fb", &[], "")),
        "a%2Fb",
        "double encoding decodes exactly once"
    );
    server.stop();
}
