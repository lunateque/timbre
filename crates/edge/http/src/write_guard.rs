//! The write-side stall guard around one connection's socket.
//!
//! A streaming response deliberately has no deadline (a long download, or a
//! paused player that stops reading, is normal), but a peer that vanished
//! without closing can leave writes queued in the socket forever and pin the
//! connection slot until the process stops. This wrapper fails a connection
//! whose writes make no progress at all for `send_stall_timeout`: the clock
//! starts at the first blocked write and resets whenever any byte lands or
//! an error surfaces, so a slow-but-moving client is never punished while a
//! dead one dies within the bound. TCP keepalive covers the idle case; this
//! covers the case where the server still has bytes to give. Both protocols
//! are guarded because the wrapper sits under hyper's IO abstraction, below
//! the HTTP/1 and HTTP/2 state machines alike.

use crate::log_events::SEND_STALL;
use hyper::rt::{Read, ReadBufCursor, Write};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use timbre_kernel::{Ctx, log};
use tokio::time::{Instant as TokioInstant, Sleep, sleep_until};

/// A connection's IO with a bound on how long one blocked write may last.
pub(crate) struct WriteGuard<I> {
    inner: I,
    stall: Duration,
    ctx: Ctx,
    /// The peer, for the line that says the guard tripped.
    peer: String,
    /// When the current blocked write started, set at its first `Pending`.
    blocked_since: Option<TokioInstant>,
    /// The timer that wakes the task at the deadline.
    wake_at: Option<Pin<Box<Sleep>>>,
    /// Set once the guard tripped: the connection is dying, later polls must
    /// answer the same without logging the same stall twice.
    failed: bool,
}

impl<I> WriteGuard<I> {
    pub(crate) fn new(inner: I, stall: Duration, ctx: Ctx, peer: String) -> Self {
        Self {
            inner,
            stall,
            ctx,
            peer,
            blocked_since: None,
            wake_at: None,
            failed: false,
        }
    }

    /// The IO error that fails the connection: hyper treats it like any
    /// socket death and tears the connection down.
    fn stalled() -> io::Error {
        io::Error::new(io::ErrorKind::TimedOut, "the client stopped reading")
    }

    /// Called on every `Pending` write or flush: `Pending` while the stall
    /// bound still has time left (the task is woken at the deadline),
    /// `Ready` with the fatal error once it has passed. Bytes that landed in
    /// between reset the clock through [`Self::progress`].
    fn on_blocked(&mut self, cx: &mut Context<'_>) -> Poll<io::Error> {
        let now = TokioInstant::now();
        let since = *self.blocked_since.get_or_insert(now);
        // Elapsed-vs-bound compares by subtraction, never by addition: a
        // configured `send_stall_timeout_ms` of `u64::MAX` is a duration no
        // instant can be reached at, and `since + stall` would panic.
        if self.failed || now.saturating_duration_since(since) >= self.stall {
            if !self.failed {
                self.failed = true;
                log!(
                    self.ctx,
                    warn,
                    SEND_STALL,
                    peer = self.peer.as_str(),
                    ms = u64::try_from(self.stall.as_millis()).unwrap_or(u64::MAX)
                );
            }
            return Poll::Ready(Self::stalled());
        }
        // Park the task until either the socket moves again (the inner poll
        // wakes us) or the stall bound expires. An unrepresentable deadline
        // means the bound is "as good as never": there is nothing to arm,
        // and no wake is owed for it.
        if let Some(deadline) = since.checked_add(self.stall) {
            let wake = self
                .wake_at
                .get_or_insert_with(|| Box::pin(sleep_until(deadline)));
            let _ = wake.as_mut().poll(cx);
        }
        Poll::Pending
    }

    /// Any byte that reached the socket (or any surfaced error) proves the
    /// peer is still taking data; forget the blocked-write clock.
    fn progress(&mut self) {
        self.blocked_since = None;
        self.wake_at = None;
    }
}

impl<I> Read for WriteGuard<I>
where
    I: Read + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl<I> Write for WriteGuard<I>
where
    I: Write + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.failed {
            return Poll::Ready(Err(Self::stalled()));
        }
        match Pin::new(&mut this.inner).poll_write(cx, buf) {
            Poll::Ready(Ok(written)) => {
                this.progress();
                Poll::Ready(Ok(written))
            }
            Poll::Ready(Err(error)) => {
                this.progress();
                Poll::Ready(Err(error))
            }
            Poll::Pending => this.on_blocked(cx).map(Err),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.failed {
            return Poll::Ready(Err(Self::stalled()));
        }
        match Pin::new(&mut this.inner).poll_flush(cx) {
            Poll::Ready(result) => {
                this.progress();
                Poll::Ready(result)
            }
            // A flush stuck on a full socket is the same stall as a write:
            // on HTTP/2, where h2 keeps its own send buffer, the blocked
            // poll is the one hyper actually repeats.
            Poll::Pending => this.on_blocked(cx).map(Err),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::Waker;
    use timbre_kernel::{Format, LogFilter, Logger, Registry, ThreadRuntime};

    /// A sink for the test kernel's logger: log lines are not what the
    /// stall clock is tested against, but building a `Ctx` needs a `Kernel`.
    #[derive(Clone, Default)]
    struct Discard;

    impl io::Write for Discard {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    // These polls never await; a noop waker only has to exist so the stall
    // timer can be armed against it.

    /// A hyper `Write` that always reports the socket full.
    struct Full;

    impl hyper::rt::Read for Full {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: hyper::rt::ReadBufCursor<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl hyper::rt::Write for Full {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Pending
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    fn ctx() -> Ctx {
        let runtime = std::sync::Arc::new(ThreadRuntime::new());
        let kernel = Registry::new().finish(
            Logger::new(
                LogFilter::default(),
                Format::Json,
                Box::new(Discard),
                std::time::SystemTime::now,
            ),
            runtime,
        );
        Ctx::root(&kernel)
    }

    #[test]
    fn an_unreachable_stall_bound_needs_no_timer_and_never_panics() {
        // `u64::MAX` milliseconds is a duration no instant can be reached at;
        // the guard must read it as "never", not panic the connection task
        // while arming its wake-up. The poll runs inside a runtime because
        // tokio timers require one.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("a timer-enabled runtime");
        runtime.block_on(async {
            let mut guard = WriteGuard::new(
                Full,
                Duration::from_millis(u64::MAX),
                ctx(),
                "test".to_owned(),
            );
            let waker = Waker::noop();
            let mut cx = Context::from_waker(waker);
            for _ in 0..3 {
                assert!(
                    Pin::new(&mut guard).poll_write(&mut cx, b"x").is_pending(),
                    "an unreachable bound is exactly 'never stall'"
                );
                tokio::task::yield_now().await;
            }
        });
    }

    #[test]
    fn an_expired_stall_bound_fails_the_write_at_once() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("a timer-enabled runtime");
        runtime.block_on(async {
            let mut guard = WriteGuard::new(Full, Duration::ZERO, ctx(), "test".to_owned());
            let waker = Waker::noop();
            let mut cx = Context::from_waker(waker);
            // The first blocked poll arms nothing that can fire later than
            // `now` itself: the second poll must already report the failure.
            let _ = Pin::new(&mut guard).poll_write(&mut cx, b"x");
            match Pin::new(&mut guard).poll_write(&mut cx, b"x") {
                Poll::Ready(Err(error)) => {
                    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                }
                other => panic!(
                    "a zero bound must fail the write, got ready={}",
                    other.is_ready()
                ),
            }
        });
    }
}
