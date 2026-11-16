//! The server: accept, limit, route, answer, log, drain.

use crate::adapter;
use crate::config::HttpConfig;
use crate::failures::HttpFailure;
use crate::log_events::{HANDLER_FAILED, LISTENING, REQUEST};
use crate::proxy::{Trusted, client_address};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder as AutoBuilder;
use hyper_util::server::graceful::GracefulShutdown;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use timbre_kernel::http::{Body, Request, Response};
use timbre_kernel::{Ctx, Failure, Fallible, RequestId, RouteMatch, log};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

/// The largest header block accepted, in bytes. On HTTP/1.1 hyper refuses
/// a head that does not fit its read buffer (431, or the connection just
/// closes); `max_headers` separately bounds the header count, so both are
/// set from this one number. HTTP/2 is told the same `max_header_list_size`.
const MAX_HEADER_BYTES: usize = 16 * 1024;
/// HTTP/2 multiplexes streams on one connection; without this cap one client
/// could run 200 concurrent streams (hyper's default) past the per-connection
/// slot. HTTP/1.1 handles one request per connection at a time anyway.
const MAX_H2_STREAMS: u32 = 16;
/// How long a client may take to send its request headers. On h1 the same
/// timer also bounds the idle wait between keep-alive requests; hyper has no
/// separate h1 idle knob.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// How long in-flight responses may take to finish after the stop signal.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);
/// HTTP/2 idle: `PING` after 30 s of quiet and close 30 s later, i.e. the 60 s
/// idle timeout on the one protocol with keep-alive machinery in hyper.
const H2_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(30);
/// At most this many live connections at once.
const MAX_CONNECTIONS: usize = 1024;

/// Serve every address in `config` until `ctx` reports shutdown, then stop
/// accepting, drain in-flight responses for up to ten seconds, and return.
///
/// # Errors
///
/// `http.proxy_invalid` for a malformed trusted proxy, `http.bind_failed`
/// when an address cannot be parsed or bound.
pub fn serve(ctx: Ctx, config: HttpConfig) -> timbre_kernel::BoxFuture<Fallible> {
    Box::pin(run(ctx, config))
}

async fn run(ctx: Ctx, config: HttpConfig) -> Fallible {
    let mut trusted = Vec::new();
    for entry in &config.trusted_proxies {
        trusted.push(Trusted::parse(entry)?);
    }
    let limits = Arc::new(Limits {
        ctx: ctx.clone(),
        config,
        trusted,
        slots: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
    });
    let mut listeners = Vec::new();
    for entry in &limits.config.listen {
        listeners.push(bind(&ctx, entry).await?);
    }
    let graceful = Arc::new(GracefulShutdown::new());
    let mut accepting = tokio::task::JoinSet::new();
    for listener in listeners {
        let limits = Arc::clone(&limits);
        let graceful = Arc::clone(&graceful);
        accepting.spawn(async move {
            loop {
                // Slot before accept: with every slot taken the backlog waits
                // inside the kernel instead of burning spawned tasks.
                let slots = Arc::clone(&limits.slots);
                // The accept set is being shut down at the end of `serve` when
                // acquiring fails: leave the task then.
                let Ok(permit) = slots.acquire_owned().await else {
                    return;
                };
                let Ok((stream, peer)) = listener.accept().await else {
                    continue;
                };
                let limits = Arc::clone(&limits);
                // A fresh watcher per connection: `watcher()` subscribes to the
                // one stop signal held by `graceful`.
                let watcher = graceful.watcher();
                tokio::spawn(async move {
                    let _permit = permit;
                    drive(stream, peer, limits, watcher).await;
                });
            }
        });
    }
    ctx.shutdown().await;
    // Stop accepting; in-flight connections keep their permits and are waited
    // for by the drain below.
    accepting.shutdown().await;
    // Drain in-flight responses for a bounded moment; whatever is still open
    // after it is closed when the runtime goes away.
    let drain = match Arc::try_unwrap(graceful) {
        Ok(shutdown) => shutdown.shutdown(),
        // A leaked accept task can keep the Arc alive; watching is then over,
        // and the runtime teardown closes the connections.
        Err(_) => return Ok(()),
    };
    if tokio::time::timeout(DRAIN_TIMEOUT, drain).await.is_err() {
        // The drain expired with responses still open: hyper's per-connection
        // graceful shutdown already signalled them, so nothing dangles.
    }
    Ok(())
}

/// Bind one configured address and log the port actually bound (`:0` gets an
/// ephemeral port the operator must see to connect).
async fn bind(ctx: &Ctx, entry: &str) -> Fallible<TcpListener> {
    let bind_failure = |reason: String| {
        Box::new(HttpFailure::Bind {
            address: entry.to_owned(),
            reason,
        }) as Box<dyn Failure>
    };
    let address: SocketAddr = entry
        .parse()
        .map_err(|error: std::net::AddrParseError| bind_failure(error.to_string()))?;
    let listener = TcpListener::bind(address)
        .await
        .map_err(|error| bind_failure(error.to_string()))?;
    let bound = listener
        .local_addr()
        .map_err(|error| bind_failure(error.to_string()))?;
    log!(ctx, info, LISTENING, addr = bound.to_string());
    Ok(listener)
}

/// Everything a connection needs that does not change between requests.
struct Limits {
    ctx: Ctx,
    config: HttpConfig,
    trusted: Vec<Trusted>,
    slots: Arc<Semaphore>,
}

type HyperReply = http::Response<adapter::Reply>;

/// Drive one connection until it ends or the process stops.
async fn drive(
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    limits: Arc<Limits>,
    graceful: hyper_util::server::graceful::Watcher,
) {
    if stream.set_nodelay(true).is_err() {
        // Nagle stays on; a socket that refuses the option still serves.
    }
    let io = TokioIo::new(stream);
    let ctx = limits.ctx.clone();
    let service = service_fn(move |request| {
        let ctx = Ctx::clone(&ctx);
        let limits = Arc::clone(&limits);
        async move { Ok::<_, std::convert::Infallible>(handle(ctx, limits, peer, request).await) }
    });
    let mut builder = AutoBuilder::new(TokioExecutor::new());
    builder
        .http1()
        .max_headers(MAX_HEADER_BYTES / 64)
        // The hyper minimum is 8192, comfortably below MAX_HEADER_BYTES.
        .max_buf_size(MAX_HEADER_BYTES)
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .timer(TokioTimer::new());
    builder
        .http2()
        .max_concurrent_streams(MAX_H2_STREAMS)
        .max_header_list_size(u32::try_from(MAX_HEADER_BYTES).unwrap_or(u32::MAX))
        .keep_alive_interval(H2_KEEP_ALIVE_INTERVAL)
        .keep_alive_timeout(H2_KEEP_ALIVE_INTERVAL)
        .timer(TokioTimer::new());
    let connection = builder.serve_connection_with_upgrades(io, service);
    // A connection error (a client that hung up) is normal; the request log
    // recorded whatever completed.
    let _ = graceful.watch(connection).await;
}

/// Answer one request and log it.
async fn handle(
    ctx: Ctx,
    limits: Arc<Limits>,
    peer: SocketAddr,
    incoming: http::Request<Incoming>,
) -> HyperReply {
    let started = Instant::now();
    let id = RequestId::new(rand_u64());
    let request_ctx = ctx.with_request(id);
    let answered = process(&request_ctx, &limits, peer, incoming).await;
    log!(
        answered.ctx,
        info,
        REQUEST,
        method = answered.method.as_str(),
        path = answered.path.as_str(),
        status = u64::from(answered.reply.status().as_u16()),
        bytes = body_bytes(&answered.reply),
        ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        client = answered.client.to_string()
    );
    answered.reply
}

/// What `process` produced for one request.
struct Answered {
    ctx: Ctx,
    reply: HyperReply,
    method: String,
    /// The path with its query as it may be logged: secret values redacted.
    path: String,
    client: IpAddr,
}

/// The request path: decode, enforce limits, route, run the handler.
async fn process(
    request_ctx: &Ctx,
    limits: &Limits,
    peer: SocketAddr,
    incoming: http::Request<Incoming>,
) -> Answered {
    let (parts, body) = incoming.into_parts();
    let method = parts.method.as_str().to_owned();
    let target = parts
        .uri
        .path_and_query()
        .map_or("/", |each| each.as_str())
        .to_owned();
    // What the log may carry: the target minus every secret query value.
    let logged_target = adapter::redacted_target(&target);
    let fail = |status: u16, client: IpAddr, path: String| Answered {
        ctx: request_ctx.clone(),
        reply: plain(status, None),
        method: method.clone(),
        path,
        client,
    };
    let Some(kernel_method) = adapter::to_method(&method) else {
        return fail(405, peer.ip(), logged_target);
    };
    let Ok((path, mut query)) = adapter::path_and_query(&target) else {
        return fail(400, peer.ip(), logged_target);
    };
    // Once parsed, the path and its (form-merged) query are logged through
    // the redaction, so a secret POST field stays out of the line as well.
    let logged = |path: &str, query: &[(String, String)]| {
        if query.is_empty() {
            path.to_owned()
        } else {
            format!("{path}?{}", adapter::redacted_query(query))
        }
    };
    let headers = header_pairs(&parts.headers);
    let forwarded = adapter::header_value(&headers, "x-forwarded-for");
    let client = client_address(peer.ip(), forwarded.as_deref(), &limits.trusted);
    let content_type = adapter::header_bytes(&headers, "content-type");
    let body = match adapter::read_body(
        request_ctx,
        body,
        limits.config.max_body_bytes,
        limits.config.body_read_timeout,
    )
    .await
    {
        Ok(body) => body,
        Err(status) => return fail(status, client, logged(&path, &query)),
    };
    if adapter::is_form_post(&method, content_type.as_deref()) {
        query.extend(adapter::form_pairs(&body));
    }
    let logged = logged(&path, &query);
    let request_ctx = request_ctx.with_client(client);
    let (handler, param) = match request_ctx.kernel().find_route(kernel_method, &path) {
        RouteMatch::Found { handler, param } => (handler, param),
        RouteMatch::MethodNotAllowed => return fail(405, client, logged.clone()),
        RouteMatch::NotFound => return fail(404, client, logged.clone()),
    };
    let request = Request {
        method: kernel_method,
        path: path.clone(),
        param,
        query,
        headers,
        peer,
        client,
        body: Body::Bytes(body),
    };
    let answered_ctx = Ctx::clone(&request_ctx);
    match handler(request_ctx, request).await {
        Ok(response) => Answered {
            ctx: answered_ctx,
            reply: to_hyper(response),
            method,
            path: logged,
            client,
        },
        Err(failure) => {
            let status = failure.status();
            log!(
                answered_ctx,
                warn,
                HANDLER_FAILED,
                code = failure.code().as_str(),
                status = u64::from(status)
            );
            Answered {
                ctx: answered_ctx,
                reply: plain(status, Some(failure.public_message().to_owned())),
                method,
                path: logged,
                client,
            }
        }
    }
}

/// The request headers as the kernel shape sees them: names lowercased,
/// values raw bytes, in wire order. No logger prints these values; secret
/// ones (`adapter::SECRET_HEADER_NAMES`) would never reach the log anyway.
fn header_pairs(map: &http::HeaderMap) -> Vec<(String, Vec<u8>)> {
    map.iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                value.as_bytes().to_vec(),
            )
        })
        .collect()
}

/// Convert a kernel response; a refused builder means a header name the wire
/// rejects, which is the server's fault: 500.
fn to_hyper(response: Response) -> HyperReply {
    let mut builder = http::Response::builder().status(response.status);
    for (name, value) in &response.headers {
        builder = builder.header(name.as_str(), value.clone());
    }
    let reply = adapter::Reply::from(response.body);
    match builder.body(reply) {
        Ok(built) => built,
        Err(_) => plain(500, Some("The server failed to answer.".to_owned())),
    }
}

/// A plain-text (or empty) response the server produces itself.
fn plain(status: u16, message: Option<String>) -> HyperReply {
    let body = message.map_or_else(
        || adapter::Reply::Empty(http_body_util::Empty::new()),
        |text| adapter::Reply::Full(http_body_util::Full::new(bytes::Bytes::from(text))),
    );
    match http::Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(body)
    {
        Ok(built) => built,
        // The one infallible reply: no headers, empty body.
        Err(_) => http::Response::new(adapter::Reply::Empty(http_body_util::Empty::new())),
    }
}

/// Bytes counted for the log line: known for in-memory bodies, `0` for a
/// stream whose size hyper does not know up front.
fn body_bytes(reply: &HyperReply) -> u64 {
    use http_body::Body as _;
    // `SizeHint::upper` is already a `u64`.
    reply.body().size_hint().upper().unwrap_or_default()
}

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

/// A per-process identifier for log correlation; not cryptographic, only
/// required not to repeat across concurrent requests.
fn rand_u64() -> u64 {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let nanos = u64::try_from(since.as_nanos()).unwrap_or(u64::MAX);
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    // Multiply-shift of clock xor counter: distinct per increment, scattered
    // across the u64 space.
    nanos
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(counter.wrapping_mul(0xD6E8_FEB8_6659_FD93))
}
