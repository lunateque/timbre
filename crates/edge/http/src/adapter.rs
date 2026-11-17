//! Turning a hyper request into the kernel `Request`: the method, the
//! normalized path, the parsed query, the collected body.

use crate::failures::HttpFailure;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use percent_encoding::percent_decode_str;
use timbre_kernel::Failure;
use timbre_kernel::http::Method;

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

/// The decoded, dot-normalized path and the parsed query of a request-target.
///
/// The path is normalized per RFC 3986 `remove_dot_segments` and empty
/// segments are collapsed, so routing sees one spelling per resource
/// (`/a//b/../c`, `/a/c` and `/a/b/./../c` all arrive as `/a/c`). A path
/// whose `..` climb goes above the root is refused: it names no resource.
///
/// # Errors
///
/// `400` when the path holds a broken percent-escape, does not decode to
/// UTF-8, does not start with `/`, or climbs above the root.
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
    let path = normalize_path(&decoded).ok_or(400u16)?;
    let query = raw_query.map_or_else(Vec::new, |raw| {
        form_urlencoded::parse(raw.as_bytes())
            .into_owned()
            .collect::<Vec<(String, String)>>()
    });
    Ok((path, query))
}

/// RFC 3986 `remove_dot_segments` with empty interior segments collapsed:
/// `.` and `..` are whole segments only, never parts of a name (`a.b` and
/// `...` stay as they are). `None` when the climb leaves the root.
fn normalize_path(path: &str) -> Option<String> {
    let mut kept: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                kept.pop()?;
            }
            other => kept.push(other),
        }
    }
    let mut out = String::with_capacity(path.len());
    for segment in kept {
        out.push('/');
        out.push_str(segment);
    }
    if out.is_empty() {
        out.push('/');
    }
    Some(out)
}

/// Whether a route's `{name}` capture can be handed to a handler. After
/// normalization a segment is never `.` or `..` (the normalization consumes
/// those), but the capture check keeps that guarantee local to this crate
/// instead of depending on the call order in `serve`.
#[must_use]
pub(crate) fn param_usable(param: &str) -> bool {
    param != "." && param != ".."
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

/// The value of one header as raw bytes, from the kernel-shaped list.
#[must_use]
pub(crate) fn header_bytes(headers: &[(String, Vec<u8>)], name: &str) -> Option<Vec<u8>> {
    headers
        .iter()
        .find(|(stored, _)| stored == name)
        .map(|(_, value)| value.clone())
}

/// Every value of header `name`, in wire order, from the kernel-shaped list.
pub(crate) fn header_values<'a>(
    headers: &'a [(String, Vec<u8>)],
    name: &str,
) -> impl Iterator<Item = &'a [u8]> {
    headers
        .iter()
        .filter(move |(stored, _)| stored == name)
        .map(|(_, value)| value.as_slice())
}

/// Read the request body whole, refusing more than `limit` bytes.
///
/// # Errors
///
/// `Refused::TooLarge` (413) when the body exceeds the limit,
/// `http.body_failed` when the stream itself fails.
pub(crate) async fn collect_body(mut incoming: Incoming, limit: usize) -> Result<Vec<u8>, Refused> {
    let mut collected: Vec<u8> = Vec::new();
    loop {
        let Some(result) = incoming.frame().await else {
            return Ok(collected);
        };
        let frame = result.map_err(|_| Refused::Failure(HttpFailure::BodyFailed))?;
        // Only data frames carry body bytes; trailers cannot overflow the cap
        // meaningfully and are accepted and dropped like hyper does for now.
        let Ok(data) = frame.into_data() else {
            continue;
        };
        let room = limit.saturating_sub(collected.len());
        if data.len() > room {
            return Err(Refused::TooLarge);
        }
        collected.extend_from_slice(data.as_ref());
    }
}

/// Why a request body was refused before the handler ran.
pub(crate) enum Refused {
    /// The body stream failed or stalled; the failure carries the status and
    /// the public message to answer with.
    Failure(HttpFailure),
    /// The body crossed `max_body_bytes`: 413, answered with an empty body.
    TooLarge,
}

impl Refused {
    /// The status the client is answered.
    pub(crate) fn status(&self) -> u16 {
        match self {
            Self::Failure(failure) => failure.status(),
            Self::TooLarge => 413,
        }
    }

    /// The body of the answer, `None` when it stays empty.
    pub(crate) fn message(&self) -> Option<&str> {
        match self {
            Self::Failure(failure) => Some(failure.public_message()),
            Self::TooLarge => None,
        }
    }
}

/// Read the request body with one deadline over the whole read: a client
/// that sends headers and then stalls mid-body would otherwise hold the
/// connection slot forever. Every refusal logs its own event with the
/// failure's code before being returned.
pub(crate) async fn read_body(
    request_ctx: &timbre_kernel::Ctx,
    incoming: Incoming,
    limit: usize,
    timeout: std::time::Duration,
) -> Result<Vec<u8>, Refused> {
    match tokio::time::timeout(timeout, collect_body(incoming, limit)).await {
        Err(_) => Err(refuse(
            request_ctx,
            HttpFailure::BodyTimeout,
            crate::log_events::BODY_TIMEOUT,
        )),
        Ok(Ok(body)) => Ok(body),
        Ok(Err(refused @ Refused::TooLarge)) => Err(refused),
        Ok(Err(Refused::Failure(failure))) => {
            Err(refuse(request_ctx, failure, crate::log_events::BODY_FAILED))
        }
    }
}

/// One warn line naming the failure's code, then the refusal to answer with.
fn refuse(
    request_ctx: &timbre_kernel::Ctx,
    failure: HttpFailure,
    event: timbre_kernel::LogEvent,
) -> Refused {
    timbre_kernel::log!(request_ctx, warn, event, code = failure.code().as_str());
    Refused::Failure(failure)
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
    fn dot_segments_normalize_and_empty_segments_collapse() {
        for (target, wanted) in [
            ("/a/./b", "/a/b"),
            ("/a/b/..", "/a"),
            ("/a/b/../c", "/a/c"),
            ("/a//b", "/a/b"),
            ("/.//a", "/a"),
            ("/.", "/"),
            ("/a/b/./../c", "/a/c"),
            // `...` and dotted names are not dot segments; a decoded `..`
            // pops like a raw one.
            ("/a/.../../b", "/a/b"),
            ("/a.b/c..", "/a.b/c.."),
            // A decoded `..` inside a longer name is ordinary text.
            ("/a/b%2e%2e/c", "/a/b../c"),
            ("/a/%2e%2e/c", "/c"),
        ] {
            let (path, _) = path_and_query(target).unwrap_or_else(|_| panic!("{target} is valid"));
            assert_eq!(path, wanted, "{target} normalizes");
        }
    }

    #[test]
    fn a_path_climbing_above_the_root_is_400() {
        for target in [
            "/..",
            "/../x",
            "/a/../..",
            "/a/../../..",
            "/a/../../b",
            "/%2E%2E/%2E%2E",
            "/x/%2E%2E/..",
        ] {
            assert_eq!(
                path_and_query(target).err(),
                Some(400),
                "{target} climbs out"
            );
        }
    }

    #[test]
    fn a_capture_is_never_a_dot_segment() {
        assert!(param_usable("album-42"));
        assert!(param_usable("a.b"));
        assert!(!param_usable("."));
        assert!(!param_usable(".."));
        // Normalization consumed every dot segment before routing: a path
        // that ends in `.` routes as its parent.
        assert_eq!(
            path_and_query("/rest/.").expect("dots normalize").0,
            "/rest"
        );
        assert_eq!(
            path_and_query("/rest/..").expect("dots normalize").0,
            "/",
            "`..` empties the stack; that is not a climb above the root"
        );
    }

    #[test]
    fn form_pairs_keep_order_and_decode() {
        let body = b"q=%D0%BF%D0%BE%D0%B8%C1%81k&n=1&q=two";
        let pairs = form_pairs(body);
        assert_eq!(pairs.len(), 3, "repeated keys survive");
        assert_eq!(pairs[2], ("q".to_owned(), "two".to_owned()));
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

    #[test]
    fn every_header_value_of_a_name_arrives_in_wire_order() {
        let headers = vec![
            ("host".to_owned(), b"x".to_vec()),
            ("x-forwarded-for".to_owned(), b"1.1.1.1".to_vec()),
            ("x-forwarded-for".to_owned(), b"2.2.2.2".to_vec()),
        ];
        let joined = header_values(&headers, "x-forwarded-for")
            .map(|value| String::from_utf8_lossy(value).into_owned())
            .collect::<Vec<String>>()
            .join(", ");
        assert_eq!(joined, "1.1.1.1, 2.2.2.2");
        assert_eq!(header_values(&headers, "missing").count(), 0);
    }
}
