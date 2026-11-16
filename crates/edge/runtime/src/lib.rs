//! The tokio-backed runtime the serving binary runs on.
//!
//! The kernel's `ThreadRuntime` gives every task its own thread, which is
//! right for a CLI command and wrong for a server holding many idle
//! connections. This crate supplies the multi-threaded tokio executor as the
//! same `Runtime` contract, plus the OS shutdown signal, so everything else
//! in the tree stays executor-agnostic.

#![warn(missing_docs)]

use std::sync::Arc;
use timbre_kernel::{BoxFuture, Runtime};
use tokio::sync::watch;

/// The kernel `Runtime` on tokio's multi-threaded executor.
///
/// One instance backs the whole process: `spawn` schedules tasks on it,
/// `run_blocking` moves closures to tokio's blocking pool, and `shutdown`
/// resolves on the first Ctrl-C / SIGINT / SIGTERM or on an explicit
/// [`TokioRuntime::trigger_shutdown`]. A waiter that arrives after the
/// signal still resolves immediately.
pub struct TokioRuntime {
    handle: tokio::runtime::Handle,
    stopping: watch::Receiver<bool>,
    // Held so the channel never closes while the runtime lives.
    stop: watch::Sender<bool>,
}

impl TokioRuntime {
    /// Build the runtime and start the OS signal listeners. Must be called
    /// from inside a tokio runtime context, e.g. from within
    /// [`TokioRuntime::block_on`].
    #[must_use]
    pub fn current() -> Arc<Self> {
        let (stop, stopping) = watch::channel(false);
        let listening = stop.clone();
        tokio::spawn(async move {
            // `ctrl_c` covers Ctrl-C on Windows and SIGINT on Unix; SIGTERM
            // arrives through the unix-only listener below.
            let interrupted = tokio::signal::ctrl_c();
            #[cfg(unix)]
            let terminated = sigterm();
            #[cfg(not(unix))]
            let terminated = std::future::pending::<()>();
            tokio::select! {
                _ = interrupted => {}
                () = terminated => {}
            }
            // A failed send means `trigger_shutdown` already stopped us.
            let _ = listening.send(true);
        });
        Arc::new(TokioRuntime {
            handle: tokio::runtime::Handle::current(),
            stopping,
            stop,
        })
    }

    /// Resolve every outstanding (and every later) `shutdown` wait now.
    pub fn trigger_shutdown(&self) {
        // The sender lives in `self`, so its receiving half cannot be gone.
        let _ = self.stop.send(true);
    }

    /// Run `future` on a fresh multi-threaded tokio runtime. The binary's
    /// `main` uses this as its only entry into async code.
    ///
    /// # Errors
    ///
    /// When tokio cannot build the runtime, e.g. the process is out of
    /// threads.
    pub fn block_on<T>(future: impl Future<Output = T>) -> Result<T, std::io::Error> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        Ok(runtime.block_on(future))
    }
}

#[cfg(unix)]
async fn sigterm() {
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(mut stream) => {
            stream.recv().await;
        }
        // No listener for SIGTERM: Ctrl-C remains the stop path.
        Err(_) => std::future::pending::<()>().await,
    }
}

impl Runtime for TokioRuntime {
    fn spawn(&self, task: BoxFuture<()>) {
        self.handle.spawn(task);
    }

    fn run_blocking(&self, work: Box<dyn FnOnce() + Send>) -> BoxFuture<()> {
        let sent = self.handle.spawn_blocking(work);
        Box::pin(async move {
            // A panicked or cancelled closure still completes the awaiter:
            // the kernel contract is "resolved after the work ran", and the
            // panic itself is reported on the thread that caused it.
            let _ = sent.await;
        })
    }

    fn shutdown(&self) -> BoxFuture<()> {
        let mut stopping = self.stopping.clone();
        Box::pin(async move {
            // Check before awaiting: a signal that fired earlier must not be
            // missed by a waiter that starts after it.
            while !*stopping.borrow_and_update() {
                // The sender lives until the runtime does; an error means it
                // is gone, which is as good as stopped.
                if stopping.changed().await.is_err() {
                    break;
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    // The kernel `Runtime` contract, proven against this executor: blocking
    // work resolves its awaiter with its effect visible, spawn runs, and
    // `shutdown` resolves on the trigger and again for a late waiter.
    #[test]
    fn the_tokio_runtime_honours_the_kernel_contract() {
        TokioRuntime::block_on(async {
            let runtime = TokioRuntime::current();
            let ran = Arc::new(AtomicBool::new(false));
            let marking = Arc::clone(&ran);
            runtime
                .run_blocking(Box::new(move || marking.store(true, Ordering::SeqCst)))
                .await;
            assert!(ran.load(Ordering::SeqCst), "the blocking closure ran");

            let spawned = Arc::new(AtomicBool::new(false));
            let marking = Arc::clone(&spawned);
            runtime.spawn(Box::pin(
                async move { marking.store(true, Ordering::SeqCst) },
            ));
            runtime.trigger_shutdown();
            runtime.shutdown().await;
            runtime.shutdown().await;
            // The spawned task was scheduled, not run inline: give the worker
            // a bounded number of yields before calling the contract broken.
            for _ in 0..1_000 {
                if spawned.load(Ordering::SeqCst) {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(spawned.load(Ordering::SeqCst), "the spawned task ran");
        })
        .expect("the tokio runtime starts");
    }
}
