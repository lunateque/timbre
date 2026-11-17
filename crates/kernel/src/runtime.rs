//! The runtime services: where async work runs, and the std implementation
//! used by CLI commands and tests.

use crate::future::BoxFuture;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

/// Where the kernel's async work runs. The binary wires one implementation
/// into the [`Kernel`](crate::Kernel) at `Registry::finish`; the HTTP edge
/// brings a real executor, while CLI commands and tests use
/// [`ThreadRuntime`], which needs no external crate.
pub trait Runtime: Send + Sync {
    /// Run `task` until it completes, without waiting for it.
    fn spawn(&self, task: BoxFuture<()>);

    /// Run the blocking closure `work` off the async path. The returned
    /// future resolves after `work` has finished, also when it panics, so
    /// the awaiter never waits forever; anything `work` did is visible to it.
    fn run_blocking(&self, work: Box<dyn FnOnce() + Send>) -> BoxFuture<()>;

    /// A future that resolves when the server is asked to stop, resolved by
    /// every caller at once.
    fn shutdown(&self) -> BoxFuture<()>;
}

/// One-shot hand-off between the thread doing work and the future awaiting it.
pub(crate) struct Slot<T> {
    shared: Arc<Mutex<SlotState<T>>>,
}

pub(crate) struct SlotState<T> {
    value: Option<T>,
    wakers: Vec<Waker>,
}

impl<T> Slot<T> {
    pub(crate) fn new() -> Self {
        Slot {
            shared: Arc::new(Mutex::new(SlotState {
                value: None,
                wakers: Vec::new(),
            })),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SlotState<T>> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Store `value` and wake everyone awaiting it.
    pub(crate) fn fill(&self, value: T) {
        let wakers = {
            let mut state = self.lock();
            state.value = Some(value);
            std::mem::take(&mut state.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    /// A future resolving with the value once [`Slot::fill`] stores it. A
    /// value taken by an earlier awaiter leaves this future unresolved.
    pub(crate) fn wait(&self) -> Waiting<T> {
        Waiting {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> Clone for Slot<T> {
    fn clone(&self) -> Self {
        Slot {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> std::fmt::Debug for Slot<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Slot")
    }
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// The awaiting half of a [`Slot`].
pub(crate) struct Waiting<T> {
    shared: Arc<Mutex<SlotState<T>>>,
}

impl<T: Send> Future for Waiting<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut state = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(value) = state.value.take() {
            return Poll::Ready(value);
        }
        // Checking and registering under the same lock `fill` takes makes a
        // lost wake impossible: either the value is already here or `fill`
        // finds this waker registered.
        state.wakers.push(cx.waker().clone());
        Poll::Pending
    }
}

/// Fills its slot when dropped, so an awaiter resolves even when the work
/// unwinds.
struct FillOnDrop(Slot<()>);

impl Drop for FillOnDrop {
    fn drop(&mut self) {
        self.0.fill(());
    }
}

/// The std runtime: every task gets its own thread, so CLI commands and tests need no
/// executor crate. Each `spawn` and each `run_blocking` starts one OS thread and nothing
/// bounds how many run at once, so this is for CLI commands and tests only; the server
/// runs on the executor in `edge/runtime`.
pub struct ThreadRuntime {
    shutdown: Arc<ShutdownSignal>,
}

impl Default for ThreadRuntime {
    fn default() -> Self {
        Self {
            shutdown: Arc::new(ShutdownSignal::default()),
        }
    }
}

#[derive(Default)]
struct ShutdownSignal {
    fired: std::sync::atomic::AtomicBool,
    wakers: Mutex<Vec<Waker>>,
}

impl ShutdownSignal {
    fn fire(&self) {
        self.fired.store(true, std::sync::atomic::Ordering::SeqCst);
        let wakers =
            std::mem::take(&mut *self.wakers.lock().unwrap_or_else(PoisonError::into_inner));
        for waker in wakers {
            waker.wake();
        }
    }
}

/// The future returned by [`Runtime::shutdown`]; resolves once the signal
/// fired, so waiting after the fact still completes immediately.
pub(crate) struct Shutdown {
    signal: Arc<ShutdownSignal>,
}

impl Future for Shutdown {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.signal.fired.load(std::sync::atomic::Ordering::SeqCst) {
            return Poll::Ready(());
        }
        let mut wakers = self
            .signal
            .wakers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Re-check under the lock `fire` takes, so a concurrent fire cannot
        // miss this registration.
        if self.signal.fired.load(std::sync::atomic::Ordering::SeqCst) {
            Poll::Ready(())
        } else {
            wakers.push(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl ThreadRuntime {
    /// A runtime whose shutdown signal has not fired yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve every outstanding (and every future) [`Runtime::shutdown`] wait.
    pub fn trigger_shutdown(&self) {
        self.shutdown.fire();
    }

    /// Start `body` on a named thread; when the OS refuses to give one, run
    /// it inline so a future awaiting this call still resolves.
    fn named_thread(name: &str, body: impl FnOnce() + Send + 'static) {
        let cell = Arc::new(Mutex::new(Some(Box::new(body) as Box<dyn FnOnce() + Send>)));
        let runner = Arc::clone(&cell);
        let spawned = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let work = runner.lock().unwrap_or_else(PoisonError::into_inner).take();
                if let Some(work) = work {
                    work();
                }
            })
            .is_ok();
        if !spawned {
            let work = cell.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(work) = work {
                work();
            }
        }
    }
}

impl Runtime for ThreadRuntime {
    fn spawn(&self, task: BoxFuture<()>) {
        ThreadRuntime::named_thread("timbre-task", move || {
            crate::future::block_on(task);
        });
    }

    fn run_blocking(&self, work: Box<dyn FnOnce() + Send>) -> BoxFuture<()> {
        let slot = Slot::<()>::new();
        let completion = FillOnDrop(slot.clone());
        ThreadRuntime::named_thread("timbre-blocking", move || {
            // Dropping the guard fills the slot, so the awaiter resolves even
            // when `work` unwinds; the panic still reaches the worker thread.
            let _completion = completion;
            work();
        });
        Box::pin(slot.wait())
    }

    fn shutdown(&self) -> BoxFuture<()> {
        Box::pin(Shutdown {
            signal: Arc::clone(&self.shutdown),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Ctx, LogFilter, Logger, Registry};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn kernel(runtime: Arc<ThreadRuntime>) -> Arc<crate::Kernel> {
        Registry::new().finish(
            Logger::stdout(LogFilter::default()),
            runtime as Arc<dyn Runtime>,
        )
    }

    #[test]
    fn blocking_runs_the_closure_off_thread_and_returns_its_value() {
        let runtime = Arc::new(ThreadRuntime::new());
        let ctx = Ctx::root(&kernel(runtime));
        let from = std::thread::current().id();
        let (where_it_ran, value) =
            crate::block_on(ctx.blocking(move || (std::thread::current().id(), 7u64 * 6)))
                .expect("the closure does not panic");
        assert_eq!(value, 42, "the closure's value arrives");
        assert_ne!(where_it_ran, from, "the work ran on another thread");
    }

    #[test]
    fn shutdown_resolves_on_trigger_and_for_late_waiters() {
        let runtime = Arc::new(ThreadRuntime::new());
        let waiting = Ctx::root(&kernel(Arc::clone(&runtime))).shutdown();
        let fired = Arc::new(AtomicUsize::new(0));
        let waker_fired = Arc::clone(&fired);
        // `block_on` parks this thread, so the trigger must come from another.
        let trigger = {
            let runtime = Arc::clone(&runtime);
            move || {
                std::thread::sleep(std::time::Duration::from_millis(10));
                waker_fired.fetch_add(1, Ordering::SeqCst);
                runtime.trigger_shutdown();
            }
        };
        let sender = std::thread::spawn(trigger);
        crate::block_on(waiting);
        sender.join().expect("the trigger thread ran");
        assert_eq!(fired.load(Ordering::SeqCst), 1);
        // A later wait sees the fired signal immediately.
        let late = Ctx::root(&kernel(runtime)).shutdown();
        crate::block_on(late);
    }

    #[test]
    fn spawn_runs_a_task_without_the_caller_waiting() {
        let runtime = Arc::new(ThreadRuntime::new());
        let done = Arc::new(AtomicUsize::new(0));
        let task_flag = Arc::clone(&done);
        Ctx::root(&kernel(runtime)).spawn(Box::pin(async move {
            task_flag.fetch_add(1, Ordering::SeqCst);
        }));
        for _ in 0..400 {
            if done.load(Ordering::SeqCst) == 1 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the spawned task never ran");
    }

    #[test]
    fn a_panicking_closure_still_resolves_its_awaiter() {
        let runtime = Arc::new(ThreadRuntime::new());
        let ctx = Ctx::root(&kernel(Arc::clone(&runtime)));
        let (sent, received) = std::sync::mpsc::channel();
        // The awaiters run on a helper thread, so a hang surfaces as a
        // timeout instead of stalling the whole test run.
        std::thread::spawn(move || {
            crate::block_on(runtime.run_blocking(Box::new(|| panic!("the closure fails"))));
            let outcome = crate::block_on(ctx.blocking(|| -> u64 { panic!("the closure fails") }));
            sent.send(outcome.is_err())
                .expect("the test is still waiting");
        });
        let failed = received
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("both awaiters resolve after the panic");
        assert!(failed, "a panicked closure is reported as a failure");
    }
}
