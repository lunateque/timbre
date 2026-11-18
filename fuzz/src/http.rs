//! The request-target parsers: normalization, query and form decoding,
//! header pair conversion, path parameter extraction, caps and redaction.

use std::sync::{Arc, LazyLock};
use timbre_kernel::http::{Method, Request, Response};
use timbre_kernel::{
    BoxFuture, Ctx, Fallible, Guard, Kernel, LogFilter, Logger, Registry, RouteMatch, ThreadRuntime,
};

/// The pair cap a fuzzed request is checked against; the shipped default.
const MAX_PARAM_PAIRS: usize = 1024;

/// Mirrors of the private caps in `params.rs` the invariants quote; a drift
/// shows up as a corpus replay failure, not as a silently weaker check.
const MAX_LOGGED_VALUE_BYTES: usize = 120;
const MAX_LOGGED_TARGET_BYTES: usize = 2048;
/// The ellipsis `clip` appends; three bytes in UTF-8.
const ELLIPSIS: char = '\u{2026}';
const ELLIPSIS_BYTES: usize = 3;

/// A kernel with the two route shapes the harness needs: a `{name}` capture
/// and a pure-literal path. One instance is enough for every input; the
/// table is read-only after registration.
static KERNEL: LazyLock<Arc<Kernel>> = LazyLock::new(|| {
    let mut registry = Registry::new();
    registry
        .route(Method::Get, "/rest/{id}", Guard::Public, echo)
        .expect("the pattern is well-formed");
    registry
        .route(Method::Get, "/a/b", Guard::Public, echo)
        .expect("the pattern is well-formed");
    registry.finish(
        Logger::new(
            LogFilter::default(),
            timbre_kernel::Format::Json,
            Box::new(Vec::new()),
            || std::time::SystemTime::UNIX_EPOCH,
        ),
        Arc::new(ThreadRuntime::new()),
    )
});

/// A handler that never runs here; the table only needs something to store.
fn echo(_: Ctx, _: Request) -> BoxFuture<Fallible<Response>> {
    Box::pin(async { Ok(Response::new(200)) })
}

/// Feed the whole input to every request-target parser: path
/// normalization, query decoding, header pair conversion, path parameter
/// extraction, and the caps and redaction that gate them.
///
/// # Panics
///
/// When a parser misbehaves: a refusal that is not `400`, a normalized path
/// the contract rules out, a logged target past its cap, a refusal whose
/// reason changes with the source, a captured parameter that is not the
/// path's last segment, a header name left un-lowercased, or any panic
/// inside the parsers themselves.
pub fn http_head(data: &[u8]) {
    let target = String::from_utf8_lossy(data);
    let (path, query) = match timbre_edge_http::fuzzing::path_and_query(&target) {
        Ok(parsed) => parsed,
        Err(status) => {
            assert_eq!(status, 400, "the parser refuses a target only with 400");
            (String::new(), Vec::new())
        }
    };
    if !path.is_empty() {
        assert!(
            path.starts_with('/') && !path.contains("//"),
            "a normalized path starts at the root and has no empty segment: {path}"
        );
        for segment in path.split('/').filter(|segment| !segment.is_empty()) {
            assert_ne!(segment, ".", "normalization consumes `.` segments");
            assert_ne!(segment, "..", "normalization consumes `..` segments");
            // A `{name}` capture reaches a handler only through this check,
            // so a normalized segment must always pass it.
            assert!(
                timbre_edge_http::fuzzing::param_usable(segment),
                "a normalized segment is a usable parameter"
            );
        }
        // Normalization is idempotent - for paths a second decode reads
        // unchanged: a `%` or `?` inside a decoded path is re-read as an
        // escape or a query divider, which is the decoder doing its
        // documented job again, not the normalizer losing its result.
        if !path.contains('%') && !path.contains('?') {
            let again = timbre_edge_http::fuzzing::path_and_query(&path)
                .unwrap_or_else(|_| panic!("`{path}` is a normalized path"))
                .0;
            assert_eq!(again, path, "normalization must be idempotent");
        }
    }
    // Path parameter extraction through the real route table: whatever the
    // capture says must be the path's last segment, and the literal route
    // must never capture.
    match KERNEL.find_route(Method::Get, &path) {
        RouteMatch::Found(matched) => {
            let last = path.rsplit('/').next().unwrap_or_default();
            match matched.param() {
                Some(captured) => assert_eq!(
                    captured, last,
                    "the capture must be the path's last segment: {captured} vs {last}"
                ),
                None => assert_eq!(path, "/a/b", "only the capture route yields no parameter"),
            }
        }
        RouteMatch::MethodNotAllowed | RouteMatch::NotFound => {}
    }
    // The logged shapes stay inside their caps no matter what arrived.
    let logged = timbre_edge_http::fuzzing::logged_target(&path, &query);
    assert!(
        logged.len() <= MAX_LOGGED_TARGET_BYTES + ELLIPSIS_BYTES,
        "a logged target stays inside its cap"
    );
    let redacted_target = timbre_edge_http::fuzzing::redacted_target(&target);
    assert!(
        redacted_target.len() <= MAX_LOGGED_TARGET_BYTES + ELLIPSIS_BYTES,
        "a redacted raw target stays inside its cap"
    );
    // The caps: url and form refusals may differ only in the status.
    let url = timbre_edge_http::fuzzing::refusal(&query, MAX_PARAM_PAIRS, true);
    let form = timbre_edge_http::fuzzing::refusal(&query, MAX_PARAM_PAIRS, false);
    match (url, form) {
        (None, None) => {}
        (Some((431 | 414, reason)), Some((413, same))) => {
            assert_eq!(
                reason, same,
                "the refusal reason does not change with the source"
            );
        }
        _ => panic!("url and form refusals must differ only in status: {url:?} {form:?}"),
    }
    headers_and_form_gate(data, &query);
}

/// The header half of the input: pair conversion, one-name lookups, the
/// form-POST gate and the parameter caps measured on the decoded query.
///
/// # Panics
///
/// As [`http_head`]: a contract break inside `adapter::header_*`,
/// `params::refusal` or `params::is_secret_key`.
fn headers_and_form_gate(data: &[u8], query: &[(String, String)]) {
    // Header pair conversion: names arrive lowercased and every stored name
    // reads back through the kernel shape.
    let pairs = timbre_edge_http::fuzzing::header_pairs_from(&raw_headers(data));
    for (name, _) in &pairs {
        assert_eq!(
            *name,
            name.to_ascii_lowercase(),
            "header names are lowercased"
        );
        assert!(
            timbre_edge_http::fuzzing::header_bytes(&pairs, name).is_some(),
            "a stored name reads back"
        );
        assert_eq!(
            timbre_edge_http::fuzzing::header_values(&pairs, name).count(),
            pairs.iter().filter(|(stored, _)| stored == name).count(),
            "every value of a name is reachable in the list"
        );
    }
    // The form-POST gate on the request's own content type, the way
    // `request::process` applies it, and on the constant spellings.
    let content_type = timbre_edge_http::fuzzing::header_bytes(&pairs, "content-type");
    std::hint::black_box(timbre_edge_http::fuzzing::is_form_post(
        "POST",
        content_type.as_deref(),
    ));
    assert!(timbre_edge_http::fuzzing::is_form_post(
        "POST",
        Some(b"application/x-www-form-urlencoded")
    ));
    assert!(!timbre_edge_http::fuzzing::is_form_post(
        "GET",
        Some(b"application/x-www-form-urlencoded")
    ));
    // Query keys may be dot segments (unlike path parameters); the secret
    // classification must simply not panic and must ignore ASCII case.
    for (key, _) in query {
        std::hint::black_box(timbre_edge_http::fuzzing::is_secret_key(key));
        assert_eq!(
            timbre_edge_http::fuzzing::is_secret_key(key),
            timbre_edge_http::fuzzing::is_secret_key(&key.to_ascii_lowercase()),
            "the secret check reads the lowercased name"
        );
    }
    // The longest key the caps accept must not be refused at the boundary.
    let edge = vec![(
        "x".repeat(timbre_edge_http::fuzzing::MAX_PARAM_KEY_BYTES),
        "v".to_owned(),
    )];
    assert_eq!(
        timbre_edge_http::fuzzing::refusal(&edge, MAX_PARAM_PAIRS, true),
        None,
        "a key exactly at the cap fits"
    );
}

/// The pairs of a urlencoded body must survive a re-encode/decode round
/// trip, and the redacted query must still show every nameable non-secret
/// pair in full (the differential that caught the bench's planted
/// wrong-result bug).
///
/// # Panics
///
/// When the round trip or the redaction differential fails, or a parser
/// inside them panics.
pub fn form_roundtrip(data: &[u8]) {
    let pairs = timbre_edge_http::fuzzing::form_pairs(data);
    let encoded: String = {
        let mut serializer = form_urlencoded::Serializer::new(String::new());
        for (key, value) in &pairs {
            serializer.append_pair(key, value);
        }
        serializer.finish()
    };
    assert_eq!(
        timbre_edge_http::fuzzing::form_pairs(encoded.as_bytes()),
        pairs,
        "the encoded pairs must decode back to the same pairs"
    );
    let logged = timbre_edge_http::fuzzing::redacted_query(&pairs);
    // The whole expected log line, re-derived from the contract: secret
    // keys show `key=***`, others show `key=` plus the value clipped at
    // 120 bytes on a character boundary with the ellipsis appended when
    // bytes fell off, joined by `&` in pair order. Exact string equality -
    // no substring checks: a urlencoded *value* may itself contain `&` or
    // `=` and would read as a phantom segment either way.
    let want = want_query(&pairs);
    assert_eq!(logged, want, "redacted_query must be the contract exactly");
    // The target-shaped log of the same body: `redacted_target("/?<data>")`
    // re-parses the query with `form_pairs` and caps the whole line at
    // 2 KiB. The pairs it sees are parsed from the lossy-converted text,
    // not the originals (a non-UTF-8 byte inflates to the replacement
    // character), so the mirror re-parses the same way and rebuilds the
    // line from the contract - checking the target cap's boundary, which
    // the length bound in `http_head` cannot see.
    let raw_target = format!("/?{}", String::from_utf8_lossy(data));
    let (path_part, query_part) = raw_target
        .split_once('?')
        .unwrap_or((raw_target.as_str(), ""));
    let target_pairs = timbre_edge_http::fuzzing::form_pairs(query_part.as_bytes());
    let want_target = if target_pairs.is_empty() {
        path_part.to_owned()
    } else {
        format!("{path_part}?{}", want_query(&target_pairs))
    };
    assert_eq!(
        timbre_edge_http::fuzzing::redacted_target(&raw_target),
        clip_target_mirror(&want_target),
        "redacted_target must be the contract exactly"
    );
    for (key, value) in &pairs {
        let nameable = !key.is_empty()
            && !key.contains(['&', '=', '?', '#'])
            && !value.contains(['&', '=', '#'])
            && value.len() <= MAX_LOGGED_VALUE_BYTES
            && !value.contains(ELLIPSIS);
        if nameable && !timbre_edge_http::fuzzing::is_secret_key(key) {
            let expected = format!("{key}={value}");
            assert!(
                logged.contains(&expected),
                "the logged query lost `{expected}`: {logged}"
            );
        }
    }
}

/// The spelling `redacted_query` shows in place of a secret value; a value
/// spelled exactly this way is not a leak, just indistinguishable from the
/// marker.
const REDACTED_MARK: &str = "***";

/// The contract's own redacted query for `pairs`, built independently of
/// the parser: join each pair as `key=***` (secret key) or `key=` plus the
/// clipped value (any other key) with `&`.
fn want_query(pairs: &[(String, String)]) -> String {
    let mut text = String::new();
    for (key, value) in pairs {
        if !text.is_empty() {
            text.push('&');
        }
        text.push_str(key);
        text.push('=');
        if timbre_edge_http::fuzzing::is_secret_key(key) {
            text.push_str(REDACTED_MARK);
        } else {
            text.push_str(&clip_mirror(value));
        }
    }
    text
}

/// The contract's own clip: `text` whole, or cut to at most `budget` bytes
/// on a character boundary plus the ellipsis. Mirrors the parser's private
/// `clip` independently so a redaction that silently changes shape fails
/// the differential above.
fn clip(text: &str, budget: usize) -> String {
    if text.len() <= budget {
        return text.to_owned();
    }
    let mut end = budget;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::new();
    out.push_str(&text[..end]);
    out.push(ELLIPSIS);
    out
}

/// The per-value clip at `MAX_LOGGED_VALUE_BYTES`.
fn clip_mirror(text: &str) -> String {
    clip(text, MAX_LOGGED_VALUE_BYTES)
}

/// The whole-target clip at `MAX_LOGGED_TARGET_BYTES`.
fn clip_target_mirror(text: &str) -> String {
    clip(text, MAX_LOGGED_TARGET_BYTES)
}

/// The input read as `(name, value)` header pairs: split lines on `\n` and
/// the first `:` on each, so any byte string describes headers.
pub(crate) fn raw_headers(data: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut pairs = Vec::new();
    for line in data.split(|byte| *byte == b'\n') {
        if let Some(position) = line.iter().position(|byte| *byte == b':') {
            let (name, value) = line.split_at(position);
            pairs.push((name.to_vec(), value.get(1..).unwrap_or_default().to_vec()));
        }
    }
    pairs
}
