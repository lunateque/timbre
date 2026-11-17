//! A timed gate in front of the writer connection: like the reader lease,
//! a caller that cannot get the writer within [`GATE_TIMEOUT`] is turned
//! away with `sqlite.busy` instead of waiting forever behind one long
//! transaction. `std::sync::Mutex` has no timed lock, so the gate is a
//! `Mutex<bool>` plus a `Condvar`, exactly the shape `pool.rs` uses.

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long a writer-gate wait lasts before `sqlite.busy`.
const GATE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct Gate {
    held: Mutex<bool>,
    free: Condvar,
}

impl Gate {
    pub(crate) fn new() -> Self {
        Gate {
            held: Mutex::new(false),
            free: Condvar::new(),
        }
    }

    /// Take the gate, waiting at most [`GATE_TIMEOUT`]; `None` when it
    /// stayed held for that whole time.
    pub(crate) fn acquire(&self) -> Option<Guard<'_>> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let until = Instant::now() + GATE_TIMEOUT;
        loop {
            if !*held {
                *held = true;
                return Some(Guard { gate: self });
            }
            let now = Instant::now();
            if now >= until {
                return None;
            }
            // `wait_timeout` may wake early (spurious, or a release another
            // waiter took); the loop re-checks the deadline.
            let (next, wait) = self
                .free
                .wait_timeout(held, until - now)
                .unwrap_or_else(PoisonError::into_inner);
            held = next;
            if wait.timed_out() {
                // One final look: the gate may have been released during the
                // last wake before giving up.
                if !*held {
                    *held = true;
                    return Some(Guard { gate: self });
                }
                return None;
            }
        }
    }
}

/// One held gate; releases it on drop.
pub(crate) struct Guard<'a> {
    gate: &'a Gate,
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        *self
            .gate
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = false;
        self.gate.free.notify_one();
    }
}
