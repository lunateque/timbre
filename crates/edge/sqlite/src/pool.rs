//! The idle reader connections; a caller that takes one puts it back on drop.

use rusqlite::Connection;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long a reader-lease wait lasts before `sqlite.busy`.
const LEASE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct Pool {
    idle: Mutex<Vec<Connection>>,
    available: Condvar,
}

impl Pool {
    pub(crate) fn new(idle: Vec<Connection>) -> Self {
        Pool {
            idle: Mutex::new(idle),
            available: Condvar::new(),
        }
    }

    /// Borrow an idle reader, waiting at most [`LEASE_TIMEOUT`] for one;
    /// `None` when every connection stayed busy for that whole time.
    pub(crate) fn take(&self) -> Option<Lease<'_>> {
        let mut idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner);
        let until = Instant::now() + LEASE_TIMEOUT;
        loop {
            if let Some(connection) = idle.pop() {
                return Some(Lease {
                    connection: Some(connection),
                    pool: self,
                });
            }
            let now = Instant::now();
            if now >= until {
                return None;
            }
            // `wait_timeout` may wake early (spurious, or a notified lease
            // taken by another waiter); the loop re-checks the deadline.
            let (next, wait) = self
                .available
                .wait_timeout(idle, until - now)
                .unwrap_or_else(PoisonError::into_inner);
            idle = next;
            if wait.timed_out() {
                // One final look: a lease may have been returned during the
                // last wake before giving up.
                return idle.pop().map(|connection| Lease {
                    connection: Some(connection),
                    pool: self,
                });
            }
        }
    }
}

/// One borrowed reader; returns its connection to the pool.
pub(crate) struct Lease<'a> {
    connection: Option<Connection>,
    pool: &'a Pool,
}

impl std::ops::Deref for Lease<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        // Only `drop` takes the connection out, and `drop` ends the borrow.
        self.connection
            .as_ref()
            .unwrap_or_else(|| unreachable_with("the lease holds its connection until dropped"))
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            self.pool
                .idle
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(connection);
            self.pool.available.notify_one();
        }
    }
}

pub(crate) fn unreachable_with(why: &str) -> ! {
    // A broken invariant here is a logic error in the pool, not bad data:
    // abort is the only way out that neither panics nor prints in a library.
    let _ = why;
    std::process::abort()
}
