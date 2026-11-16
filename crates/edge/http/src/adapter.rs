//! Turning a hyper request into the kernel `Request`, and the kernel
//! `Response` back into a hyper one.

use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Bytes, Frame, Incoming};
use percent_encoding::percent_decode_str;
use timbre_kernel::Failure;
use timbre_kernel::http::{Body, Method};

/// The hyper body type the server sends: empty, in-memory, or streamed.
pub(crate) enum Reply {
    /// No body.
    Empty(Empty<Bytes>),
    /// A body held whole.
    Full(Full<Bytes>),
    /// A body produced chunk by chunk from a kernel `Chunks` stream; the
    /// stream's errors close the body.
    Streamed(Streamed),
}

/// The kernel `Chunks` stream seen as a hyper `Body`.
pub(crate) struct Streamed {
    inner: std::pin::Pin<Box<dyn timbre_kernel::http::Chunks + Send>>,
}

impl hyper::body::Body for Reply {
    type Data = Bytes;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        // Reply is an enum of Unpin bodies; the boxes inside are Unpin too.
        match std::pin::Pin::into_inner(self) {
            Self::Empty(body) => std::pin::Pin::new(body)
                .poll_frame(cx)
                .map(|frame| frame.map(|each| each.map_err(Into::into))),
            Self::Full(body) => std::pin::Pin::new(body)
                .poll_frame(cx)
                .map(|frame| frame.map(|each| each.map_err(Into::into))),
            Self::Streamed(body) => std::pin::Pin::new(body).poll_frame(cx),
        }
    }

    fn size_hint(&self) -> http_body::SizeHint {
        // The log's `bytes` field reads this; a stream stays unknown.
        match self {
            Self::Empty(body) => http_body::Body::size_hint(body),
            Self::Full(body) => http_body::Body::size_hint(body),
            Self::Streamed(_) => http_body::SizeHint::new(),
        }
    }
}

impl hyper::body::Body for Streamed {
    type Data = Bytes;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = std::pin::Pin::into_inner(self);
        match this.inner.as_mut().poll_chunk(cx) {
            std::task::Poll::Pending => std::task::Poll::Pending,
            std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
            std::task::Poll::Ready(Some(Ok(chunk))) => {
                std::task::Poll::Ready(Some(Ok(Frame::data(Bytes::from(chunk)))))
            }
            std::task::Poll::Ready(Some(Err(failure))) => {
                std::task::Poll::Ready(Some(Err(format!("{failure:?}").into())))
            }
        }
    }
}

impl From<Body> for Reply {
    fn from(body: Body) -> Self {
        match body {
            Body::Empty => Reply::Empty(Empty::new()),
            Body::Bytes(bytes) => Reply::Full(Full::new(Bytes::from(bytes))),
            Body::Stream(chunks) => Reply::Streamed(Streamed { inner: chunks }),
        }
    }
}

/// The method behind `text`, or `None` for methods the kernel has no shape
/// for (they are answered 405 before routing).
#[must_use]
pub(crate) fn to_method(text: &str) -> Option<Method> {
    Some(match text {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        "PATCH" => Method::Patch,
        "OPTIONS" => Method::Options,
        _ => return None,
    })
}

/// The decoded path and the parsed query of a request-target.
///
/// # Errors
///
/// `400` when the path holds a broken percent-escape or does not decode to
/// UTF-8.
pub(crate) fn path_and_query(target: &str) -> Result<(String, Vec<(String, String)>), u16> {
    let (raw_path, raw_query) = match target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (target, None),
    };
    let decoded = percent_decode_str(raw_path)
        .decode_utf8()
        .map_err(|_| 400u16)?;
    if decoded.contains('\0') || !decoded.starts_with('/') {
        return Err(400);
    }
    let query = raw_query.map_or_else(Vec::new, |raw| {
        form_urlencoded::parse(raw.as_bytes())
            .into_owned()
            .collect::<Vec<(String, String)>>()
    });
    Ok((decoded.into_owned(), query))
}

/// Whether this is a form POST whose body must be merged into the query.
#[must_use]
pub(crate) fn is_form_post(method: &str, content_type: Option<&[u8]>) -> bool {
    method == "POST"
        && content_type.is_some_and(|value| {
            let head = value
                .iter()
                .position(|byte| *byte == b';')
                .unwrap_or(value.len());
            let Some(head) = value.get(..head) else {
                return false;
            };
            String::from_utf8_lossy(head)
                .trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
        })
}

/// The pairs of an `application/x-www-form-urlencoded` body, in wire order.
#[must_use]
pub(crate) fn form_pairs(body: &[u8]) -> Vec<(String, String)> {
    form_urlencoded::parse(body)
        .into_owned()
        .collect::<Vec<(String, String)>>()
}

/// Query and form keys whose values are credentials: the logger never shows
/// them. The list is wire spellings, matched case-insensitively: `apiKey`,
/// Subsonic's `p`/`t`/`s`, and the OAuth words.
pub(crate) const SECRET_QUERY_KEYS: &[&str] = &["apikey", "p", "t", "s", "token", "password"];

/// The header names whose values must never reach a log line: the edge logs
/// no header value at all, and every future logger honors this list.
pub const SECRET_HEADER_NAMES: &[&str] = &["authorization", "cookie", "proxy-authorization"];

/// Whether `name` (already lowercased) names a header whose value stays out of
/// the log.
#[must_use]
pub fn is_secret_header(name: &str) -> bool {
    SECRET_HEADER_NAMES.contains(&name)
}

/// Whether `key` names a secret query or form value.
#[must_use]
pub(crate) fn is_secret_key(key: &str) -> bool {
    SECRET_QUERY_KEYS
        .iter()
        .any(|secret| key.eq_ignore_ascii_case(secret))
}

/// The query as it may be logged: secret keys keep their name, their value
/// becomes `***`, order and the rest stay as they arrived.
#[must_use]
pub(crate) fn redacted_query(pairs: &[(String, String)]) -> String {
    let mut text = String::new();
    for (key, value) in pairs {
        if !text.is_empty() {
            text.push('&');
        }
        text.push_str(key);
        text.push('=');
        text.push_str(if is_secret_key(key) { "***" } else { value });
    }
    text
}

/// The path and query as they may be logged: the query part, if any, is
/// decoded and its secret values redacted.
#[must_use]
pub(crate) fn redacted_target(target: &str) -> String {
    let Some((path, query)) = target.split_once('?') else {
        return target.to_owned();
    };
    format!("{path}?{}", redacted_query(&form_pairs(query.as_bytes())))
}

/// The value of one header, as text, from the kernel-shaped list.
#[must_use]
pub(crate) fn header_value(headers: &[(String, Vec<u8>)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(stored, _)| stored == name)
        .map(|(_, value)| String::from_utf8_lossy(value).into_owned())
}

/// The value of one header as raw bytes, from the kernel-shaped list.
#[must_use]
pub(crate) fn header_bytes(headers: &[(String, Vec<u8>)], name: &str) -> Option<Vec<u8>> {
    headers
        .iter()
        .find(|(stored, _)| stored == name)
        .map(|(_, value)| value.clone())
}

/// Read the request body whole, refusing more than `limit` bytes.
///
/// # Errors
///
/// `413` when the body exceeds the limit, `400` when the stream fails.
pub(crate) async fn collect_body(mut incoming: Incoming, limit: usize) -> Result<Vec<u8>, u16> {
    let mut collected: Vec<u8> = Vec::new();
    loop {
        let Some(result) = incoming.frame().await else {
            return Ok(collected);
        };
        let frame = result.map_err(|_| 400u16)?;
        // Only data frames carry body bytes; trailers cannot overflow the cap
        // meaningfully and are accepted and dropped like hyper does for now.
        let Ok(data) = frame.into_data() else {
            continue;
        };
        let room = limit.saturating_sub(collected.len());
        if data.len() > room {
            return Err(413);
        }
        collected.extend_from_slice(data.as_ref());
    }
}

/// Read the request body with one deadline over the whole read: a client
/// that sends headers and then stalls mid-body would otherwise hold the
/// connection slot forever. An expiry logs `http.body_timeout` on its own
/// before answering.
///
/// The `Err` is the status to answer: 408 past the deadline, 413 over the
/// byte cap, 400 for a body stream that failed.
pub(crate) async fn read_body(
    request_ctx: &timbre_kernel::Ctx,
    incoming: Incoming,
    limit: usize,
    timeout: std::time::Duration,
) -> Result<Vec<u8>, u16> {
    match tokio::time::timeout(timeout, collect_body(incoming, limit)).await {
        Err(_) => {
            let failure = crate::failures::HttpFailure::BodyTimeout;
            timbre_kernel::log!(
                request_ctx,
                warn,
                crate::log_events::BODY_TIMEOUT,
                code = failure.code().as_str()
            );
            Err(failure.status())
        }
        Ok(Ok(body)) => Ok(body),
        Ok(Err(status)) => Err(status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_query_keys_keep_their_order() {
        let (path, query) = path_and_query("/rest/x?id=3&id=1&id=2").expect("a valid target");
        assert_eq!(path, "/rest/x");
        let ids: Vec<&str> = query.iter().map(|(_, value)| value.as_str()).collect();
        assert_eq!(ids, ["3", "1", "2"], "wire order is not deduplicated order");
        assert!(query.iter().all(|(key, _)| key == "id"));
    }

    #[test]
    fn percent_escapes_decode_in_the_path_and_the_query() {
        let (path, query) = path_and_query("/a%20b/c%C3%A9d?x=%D0%BF").expect("a valid target");
        assert_eq!(path, "/a b/céd");
        assert_eq!(query, [("x".to_owned(), "п".to_owned())]);
    }

    #[test]
    fn a_broken_escape_stays_literal_and_invalid_utf8_is_400() {
        // `percent-encoding` is lenient: `%zz` is not an escape and stays text.
        assert_eq!(
            path_and_query("/bad%zz").expect("a lenient path").0,
            "/bad%zz"
        );
        assert_eq!(path_and_query("/bad%FF").err(), Some(400));
        assert_eq!(path_and_query("relative").err(), Some(400));
    }

    #[test]
    fn form_pairs_keep_order_and_decode() {
        let body = b"q=%D0%BF%D0%BE%D0%B8%C1%81k&n=1&q=two";
        let pairs = form_pairs(body);
        assert_eq!(pairs.len(), 3, "repeated keys survive");
        assert_eq!(pairs[2], ("q".to_owned(), "two".to_owned()));
    }

    #[test]
    fn secret_query_keys_redact_and_others_pass_through() {
        let pairs = form_pairs(b"u=anna&p=secret&apiKey=3f&t=deadbeef&s=cafe&x=1");
        assert_eq!(
            redacted_query(&pairs),
            "u=anna&p=***&apiKey=***&t=***&s=***&x=1"
        );
        assert!(is_secret_header("authorization"));
        assert!(is_secret_header("cookie"));
        assert!(is_secret_header("proxy-authorization"));
        assert!(!is_secret_header("content-type"));
    }

    #[test]
    fn the_form_check_looks_only_at_the_mime_head() {
        assert!(is_form_post(
            "POST",
            Some(b"application/x-www-form-urlencoded; charset=utf-8")
        ));
        assert!(is_form_post(
            "POST",
            Some(b"APPLICATION/X-WWW-FORM-URLENCODED")
        ));
        assert!(!is_form_post(
            "GET",
            Some(b"application/x-www-form-urlencoded")
        ));
        assert!(!is_form_post("POST", Some(b"application/json")));
        assert!(!is_form_post("POST", None));
    }
}
