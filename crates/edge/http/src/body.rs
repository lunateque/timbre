//! The response side of the wire: the kernel `Response` as a hyper body and
//! `http::Response`, and the framing rules that keep a connection usable.

use crate::log_events::CONTENT_LENGTH_INVALID;
use crate::params::{MAX_LOGGED_VALUE_BYTES, clip};
use http_body_util::{Empty, Full};
use hyper::body::{Bytes, Frame};
use timbre_kernel::Ctx;
use timbre_kernel::http::{Body, Response};

/// The headers that describe the transfer framing rather than the payload:
/// hyper derives them from the body it is handing out, so a kernel
/// `Response` carrying its own (possibly wrong) value must not reach the wire
/// with it. The one exception is `content-length` on a stream, which
/// [`to_hyper`] passes through when the handler's value is well-formed: hyper
/// cannot know a stream's size and an audio client cannot seek without the
/// header. A stale `content-length` shorter than the body desynchronizes a
/// keep-alive connection; a `connection` or `upgrade` value the edge did not
/// negotiate does the same.
const FRAMING_HEADERS: &[&str] = &[
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "upgrade",
];

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
    /// The byte count a handler declared for this stream, once trusted and
    /// on the wire as `content-length`; `None` when the handler declared
    /// nothing or its declaration was refused. Reporting it as the body's
    /// exact size is what lets hyper frame the response by length and the
    /// request log count a stream whose size was known up front.
    declared: Option<u64>,
    /// A stream that fails mid-body cannot fail the response any more: the
    /// head is already written. The context is what remains, so the failure
    /// reaches the log with its code.
    ctx: Ctx,
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
        // The log's `bytes` field reads this; an undeclared stream stays
        // unknown, a declared one reports the promised length.
        match self {
            Self::Empty(body) => http_body::Body::size_hint(body),
            Self::Full(body) => http_body::Body::size_hint(body),
            Self::Streamed(body) => http_body::Body::size_hint(body),
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
                timbre_kernel::log!(
                    this.ctx,
                    warn,
                    crate::log_events::STREAM_FAILED,
                    code = failure.code().as_str()
                );
                std::task::Poll::Ready(Some(Err(format!("{failure:?}").into())))
            }
        }
    }

    fn size_hint(&self) -> http_body::SizeHint {
        // The declared length is a promise the edge keeps to hyper: with it
        // set exact, hyper frames the response by length instead of chunked
        // (so `HEAD` carries the header with no body) and a stream that ends
        // before it errors the connection rather than desynchronizing it.
        self.declared
            .map_or_else(http_body::SizeHint::new, http_body::SizeHint::with_exact)
    }
}

/// The kernel body as the hyper body the server sends. `declared` is the
/// byte count a stream handler promised with a trusted `content-length`.
pub(crate) fn reply(body: Body, ctx: &Ctx, declared: Option<u64>) -> Reply {
    match body {
        Body::Empty => Reply::Empty(Empty::new()),
        Body::Bytes(bytes) => Reply::Full(Full::new(Bytes::from(bytes))),
        Body::Stream(chunks) => Reply::Streamed(Streamed {
            inner: chunks,
            declared,
            ctx: ctx.clone(),
        }),
    }
}

/// Convert a kernel response; a refused builder means a header name the wire
/// rejects, which is the server's fault: 500.
pub(crate) fn to_hyper(response: Response, ctx: &Ctx) -> http::Response<Reply> {
    // A stream is the one body hyper cannot size itself, so its handler's
    // `content-length` is the only framing header the edge may pass through,
    // and only when the value is one the edge trusts (see
    // [`declared_length`]). A `HEAD` handler that answers with the stream it
    // would have sent keeps the header too: hyper suppresses the body bytes
    // for `HEAD` but never a framing header it was given.
    let declared = match &response.body {
        Body::Stream(_) => declared_length(&response.headers, ctx),
        Body::Empty | Body::Bytes(_) => None,
    };
    let mut builder = http::Response::builder().status(response.status);
    for (name, value) in &response.headers {
        let pass_through = declared.is_some() && name.eq_ignore_ascii_case("content-length");
        if is_framing_header(name) && !pass_through {
            continue;
        }
        builder = builder.header(name.as_str(), value.clone());
    }
    match builder.body(reply(response.body, ctx, declared)) {
        Ok(built) => built,
        Err(_) => plain(500, Some("The server failed to answer.".to_owned())),
    }
}

/// The size a handler declared for a streamed body: exactly one
/// `content-length` header whose value is non-empty ASCII digits a `u64`
/// holds. Anything else - a malformed value, several values (even agreeing
/// ones, which hyper would fold into one line the edge cannot vouch for) -
/// is dropped from the wire and logged, because hyper frames the response by
/// this number: a length the edge does not trust must not reach the client.
fn declared_length(headers: &[(String, Vec<u8>)], ctx: &Ctx) -> Option<u64> {
    let values: Vec<&[u8]> = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.as_slice())
        .collect();
    match values.as_slice() {
        // No declaration is normal: the stream stays hyper's to chunk.
        [] => None,
        [only] => parse_length(only).or_else(|| {
            refused_length(ctx, &values);
            None
        }),
        many => {
            refused_length(ctx, many);
            None
        }
    }
}

/// A length value: non-empty ASCII digits, within `u64`. Leading zeros are
/// digits too and hold their value; `+12` or `12abc` do not parse.
fn parse_length(value: &[u8]) -> Option<u64> {
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return None;
    }
    // ASCII digits are always valid UTF-8, but the conversion says no
    // explicitly rather than trusting the loop above to stay in step.
    std::str::from_utf8(value).ok()?.parse::<u64>().ok()
}

/// One line saying the handler's `content-length` was not trusted; the value
/// is what arrived, clipped like every other logged one, so the fix is
/// readable from the log.
fn refused_length(ctx: &Ctx, values: &[&[u8]]) {
    let joined = values
        .iter()
        .map(|value| String::from_utf8_lossy(value).into_owned())
        .collect::<Vec<String>>()
        .join(", ");
    timbre_kernel::log!(
        ctx,
        warn,
        CONTENT_LENGTH_INVALID,
        value = clip(&joined, MAX_LOGGED_VALUE_BYTES)
    );
}

/// Whether `name` is one of the framing headers hyper owns.
#[must_use]
pub(crate) fn is_framing_header(name: &str) -> bool {
    FRAMING_HEADERS
        .iter()
        .any(|header| name.eq_ignore_ascii_case(header))
}

/// A plain-text (or empty) response the server produces itself.
pub(crate) fn plain(status: u16, message: Option<String>) -> http::Response<Reply> {
    let body = message.map_or_else(
        || Reply::Empty(Empty::new()),
        |text| Reply::Full(Full::new(Bytes::from(text))),
    );
    match http::Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(body)
    {
        Ok(built) => built,
        // The one infallible reply: no headers, empty body.
        Err(_) => http::Response::new(Reply::Empty(Empty::new())),
    }
}

/// Bytes counted for the log line: known for in-memory bodies and for a
/// stream whose handler declared a trusted `content-length`, `0` for a
/// stream whose size nobody knows up front.
pub(crate) fn body_bytes(reply: &http::Response<Reply>) -> u64 {
    use http_body::Body as _;
    // `SizeHint::upper` is already a `u64`.
    reply.body().size_hint().upper().unwrap_or_default()
}
