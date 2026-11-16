//! Boxed futures and a minimal std executor for blocking contexts.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

/// The future shape every async kernel contract returns.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Drive one future to completion on the current thread by parking between
/// polls: the waker unparks this thread, so no wake is lost between a
/// `Pending` poll and the park. Used by CLI commands and tests, which have no
/// async runtime of their own; server request handling runs on a real
/// [`Runtime`](crate::Runtime) instead.
pub fn block_on<T>(future: impl Future<Output = T>) -> T {
    let waker = Waker::from(Arc::new(Parked(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
        // `park` returns immediately when a token was left by an earlier
        // `unpark`, and spurious wakeups only cost one more poll.
        std::thread::park();
    }
}

struct Parked(std::thread::Thread);

impl Wake for Parked {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, PoisonError};

    #[derive(Default)]
    struct Gate {
        done: bool,
        waker: Option<Waker>,
    }

    struct Waiter(Arc<Mutex<Gate>>);

    impl Future for Waiter {
        type Output = u32;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u32> {
            let mut gate = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            if gate.done {
                Poll::Ready(42)
            } else {
                gate.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }

    #[test]
    fn block_on_returns_when_another_thread_wakes_the_future() {
        let gate = Arc::new(Mutex::new(Gate::default()));
        let remote = Arc::clone(&gate);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let mut gate = remote.lock().unwrap_or_else(PoisonError::into_inner);
            gate.done = true;
            if let Some(waker) = gate.waker.take() {
                waker.wake();
            }
        });
        // If the flag lands before the first poll, the poll returns directly;
        // if after, the wake unparks the parked thread.
        assert_eq!(block_on(Waiter(gate)), 42);
    }
}
