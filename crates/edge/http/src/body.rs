//! The response side of the wire: the kernel `Response` as a hyper body and
//! `http::Response`, and the framing rules that keep a connection usable.

use http_body_util::{Empty, Full};
use hyper::body::{Bytes, Frame};
use timbre_kernel::Ctx;
use timbre_kernel::http::{Body, Response};

/// The headers that describe the transfer framing rather than the payload:
/// hyper derives all of them from the body it is handing out, so a kernel
/// `Response` carrying its own (possibly wrong) value must not reach the wire
/// with it. A stale `content-length` shorter than the body desynchronizes a
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
}

/// The kernel body as the hyper body the server sends.
pub(crate) fn reply(body: Body, ctx: &Ctx) -> Reply {
    match body {
        Body::Empty => Reply::Empty(Empty::new()),
        Body::Bytes(bytes) => Reply::Full(Full::new(Bytes::from(bytes))),
        Body::Stream(chunks) => Reply::Streamed(Streamed {
            inner: chunks,
            ctx: ctx.clone(),
        }),
    }
}

/// Convert a kernel response; a refused builder means a header name the wire
/// rejects, which is the server's fault: 500.
pub(crate) fn to_hyper(response: Response, ctx: &Ctx) -> http::Response<Reply> {
    let mut builder = http::Response::builder().status(response.status);
    for (name, value) in &response.headers {
        if is_framing_header(name) {
            continue;
        }
        builder = builder.header(name.as_str(), value.clone());
    }
    match builder.body(reply(response.body, ctx)) {
        Ok(built) => built,
        Err(_) => plain(500, Some("The server failed to answer.".to_owned())),
    }
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

/// Bytes counted for the log line: known for in-memory bodies, `0` for a
/// stream whose size hyper does not know up front.
pub(crate) fn body_bytes(reply: &http::Response<Reply>) -> u64 {
    use http_body::Body as _;
    // `SizeHint::upper` is already a `u64`.
    reply.body().size_hint().upper().unwrap_or_default()
}
