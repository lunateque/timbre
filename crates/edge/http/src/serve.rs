//! The server: accept, limit, drive connections, drain.
//!
//! One request's path lives in [`crate::request`].

use crate::config::HttpConfig;
use crate::detect;
use crate::failures::HttpFailure;
use crate::log_events::LISTENING;
use crate::proxy::Trusted;
use crate::request::handle;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder as AutoBuilder;
use hyper_util::server::graceful::GracefulShutdown;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use timbre_kernel::{Ctx, Failure, Fallible, log};
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
        Box::new(HttpFailure::BindFailed {
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
pub(crate) struct Limits {
    pub(crate) ctx: Ctx,
    pub(crate) config: HttpConfig,
    pub(crate) trusted: Vec<Trusted>,
    slots: Arc<Semaphore>,
}

pub(crate) type HyperReply = http::Response<crate::body::Reply>;

/// Drive one connection until it ends or the process stops.
async fn drive(
    mut stream: tokio::net::TcpStream,
    peer: SocketAddr,
    limits: Arc<Limits>,
    graceful: hyper_util::server::graceful::Watcher,
) {
    if stream.set_nodelay(true).is_err() {
        // Nagle stays on; a socket that refuses the option still serves.
    }
    // hyper's own protocol probe has no timer of its own, so the first bytes
    // are waited for under the header-read budget here: a client that
    // connects and says nothing then releases its slot instead of holding it
    // until the process stops. The bytes stay in the socket (the wait peeks),
    // so hyper makes the same decision from them without waiting again.
    if detect::detect(&mut stream, limits.config.header_read_timeout)
        .await
        .is_none()
    {
        return;
    }
    let io = TokioIo::new(stream);
    let ctx = limits.ctx.clone();
    // The builder reads this after the service closure has taken `limits`.
    let header_read_timeout = limits.config.header_read_timeout;
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
        .header_read_timeout(header_read_timeout)
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

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);
/// The per-process part of every request id, taken from std's OS-seeded hash
/// keys: processes that start in the same nanosecond still differ.
static REQUEST_SEED: std::sync::LazyLock<u64> =
    std::sync::LazyLock::new(|| RandomState::new().hash_one("timbre request id"));

/// The next request id. Request ids correlate the log lines of one request;
/// they are not cryptographic and not meant to be unguessable.
pub(crate) fn next_request_id() -> u64 {
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    // Multiply-shift of seed xor counter: distinct per increment, scattered
    // across the u64 space.
    *REQUEST_SEED ^ counter.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

#[cfg(test)]
mod tests {
    use super::next_request_id;

    #[test]
    fn consecutive_request_ids_never_repeat() {
        let mut seen: Vec<u64> = (0..1000).map(|_| next_request_id()).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), before, "no two requests share one id");
    }
}
