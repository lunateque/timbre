//! The kernel's HTTP shapes: what a route handler receives and returns.

use crate::failure::Fallible;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::task::{Context, Poll};

/// The HTTP methods a route can bind to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    /// `GET`.
    Get,
    /// `HEAD`.
    Head,
    /// `POST`.
    Post,
    /// `PUT`.
    Put,
    /// `DELETE`.
    Delete,
    /// `PATCH`.
    Patch,
    /// `OPTIONS`.
    Options,
}

impl Method {
    /// The uppercase wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Patch => "PATCH",
            Self::Options => "OPTIONS",
        }
    }
}

/// A stream of body chunks; the kernel's stand-in for async iteration, which
/// std has not stabilized. The HTTP edge drives it; handlers pin one inside
/// [`Body::Stream`].
pub trait Chunks: Send {
    /// Poll the next chunk. `None` means the end; an `Err` aborts the body.
    fn poll_chunk(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Fallible<Vec<u8>>>>;
}

/// The body of a request or a response.
pub enum Body {
    /// No body.
    Empty,
    /// A body held whole in memory.
    Bytes(Vec<u8>),
    /// A body produced chunk by chunk; large outputs stream this way so no
    /// full result set is held in memory. `Send + Sync`: a request reference
    /// lives across the guard's authentication await inside a future the
    /// HTTP edge spawns, so the borrowed body must cross threads by shared
    /// reference, not only by move.
    Stream(Pin<Box<dyn Chunks + Send + Sync>>),
}

impl Body {
    /// A UTF-8 body from a string.
    #[must_use]
    pub fn text(text: &str) -> Self {
        Self::Bytes(text.as_bytes().to_vec())
    }

    /// The whole body when it is in memory, `None` for a stream or empty.
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(bytes) => Some(bytes),
            Self::Empty | Self::Stream(_) => None,
        }
    }
}

/// One incoming request, already parsed by the HTTP edge: the path is
/// percent-decoded and valid UTF-8, form fields of a form POST are merged into
/// `query`, and `client` is the peer address or, behind trusted proxies, the
/// rightmost `X-Forwarded-For` address that is not itself a trusted proxy.
pub struct Request {
    /// The request method.
    pub method: Method,
    /// The decoded request path.
    pub path: String,
    /// The value captured by a `{name}` route segment, when the route has one.
    pub param: Option<String>,
    /// Query parameters in wire order; keys may repeat.
    pub query: Vec<(String, String)>,
    /// Headers as `(name, value)` pairs: names are lowercased by the edge,
    /// the values of one name keep their receipt order, but the order across
    /// different names is not guaranteed.
    pub headers: Vec<(String, Vec<u8>)>,
    /// The address the connection came from.
    pub peer: SocketAddr,
    /// The address of the client behind any trusted proxies.
    pub client: IpAddr,
    /// The request body.
    pub body: Body,
}

impl Request {
    /// The first value of query key `key`, in wire order.
    #[must_use]
    pub fn query_first(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// Every value of query key `key`, in wire order.
    pub fn query_all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> {
        self.query
            .iter()
            .filter(move |(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// The value of header `name`, ASCII-case-insensitively.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&[u8]> {
        self.headers
            .iter()
            .find(|(stored, _)| stored.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_slice())
    }
}

/// One outgoing response.
pub struct Response {
    /// The HTTP status.
    pub status: u16,
    /// Response headers in insertion order.
    pub headers: Vec<(String, Vec<u8>)>,
    /// The response body.
    pub body: Body,
}

impl Response {
    /// A response with `status`, no headers and no body.
    #[must_use]
    pub fn new(status: u16) -> Self {
        Response {
            status,
            headers: Vec::new(),
            body: Body::Empty,
        }
    }

    /// Add one header.
    #[must_use]
    pub fn with_header(mut self, name: &str, value: &[u8]) -> Self {
        self.headers.push((name.to_owned(), value.to_vec()));
        self
    }

    /// Set the body.
    #[must_use]
    pub fn with_body(mut self, body: Body) -> Self {
        self.body = body;
        self
    }

    /// The value of header `name`, ASCII-case-insensitively.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&[u8]> {
        self.headers
            .iter()
            .find(|(stored, _)| stored.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_slice())
    }
}
