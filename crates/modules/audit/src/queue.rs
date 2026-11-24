//! The bounded queue between the signal handlers and the journal.
//!
//! A handler runs on whatever thread published the signal, including a
//! request thread, so it may not touch the database: it drops an entry here
//! and returns. The bound is what keeps a flood of failed logins from
//! turning into unbounded memory; what does not fit is counted and reported
//! instead of silently forgotten.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// One journal entry on its way to storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) time: i64,
    pub(crate) kind: &'static str,
    pub(crate) user: Option<String>,
    pub(crate) client: Option<String>,
    pub(crate) detail: String,
}

#[derive(Debug, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    pub(crate) kind: &'static str,
    pub(crate) user: Option<String>,
    pub(crate) client: Option<String>,
    pub(crate) detail: String,
}

pub(crate) struct Row {
    pub(crate) time: i64,
    pub(crate) key: Arc<Key>,
    pub(crate) count: i64,
}

#[derive(Default)]
pub(crate) struct Batch {
    rows: Vec<Row>,
    by_key: HashMap<Arc<Key>, usize>,
    auth_failed: usize,
}

impl Batch {
    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }

    fn merge(&mut self, key: &Key, count: i64) -> bool {
        let Some(row) = self
            .by_key
            .get(key)
            .and_then(|index| self.rows.get_mut(*index))
        else {
            return false;
        };
        row.count = row.count.saturating_add(count);
        true
    }

    fn has_room(&self, key: &Key, capacity: usize) -> bool {
        // One slot always stays available to failed logins, however small
        // the queue is: a reserve of the whole queue would drop them all.
        let quarter = (capacity / 4).max(1);
        let reserved = if quarter < capacity {
            quarter
        } else {
            capacity.saturating_sub(1)
        };
        self.rows.len() < capacity
            && (key.kind != "auth_failed" || self.auth_failed < capacity.saturating_sub(reserved))
    }

    fn insert(&mut self, row: Row) {
        self.auth_failed += usize::from(row.key.kind == "auth_failed");
        self.by_key.insert(Arc::clone(&row.key), self.rows.len());
        self.rows.push(row);
    }
}

#[derive(Default)]
struct Inner {
    batch: Batch,
    dropped: u64,
}

/// The queue itself.
#[derive(Default)]
pub(crate) struct Queue {
    inner: Mutex<Inner>,
    writer: Mutex<()>,
}

impl Queue {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn lock_writer(&self) -> MutexGuard<'_, ()> {
        self.writer.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keep the oldest distinct entries, with a quarter of capacity (at
    /// least one slot, never the last free one) reserved for kinds other
    /// than `auth_failed`.
    pub(crate) fn push(&self, entry: Entry, capacity: usize) {
        let Entry {
            time,
            kind,
            user,
            client,
            detail,
        } = entry;
        let key = Key {
            kind,
            user,
            client,
            detail,
        };
        let mut inner = self.lock();
        if inner.batch.merge(&key, 1) {
            return;
        }
        if inner.batch.has_room(&key, capacity) {
            inner.batch.insert(Row {
                time,
                key: Arc::new(key),
                count: 1,
            });
        } else {
            inner.dropped = inner.dropped.saturating_add(1);
        }
    }

    pub(crate) fn take(&self) -> (Batch, u64) {
        let mut inner = self.lock();
        (
            std::mem::take(&mut inner.batch),
            std::mem::take(&mut inner.dropped),
        )
    }

    /// The write failed, so the in-flight rows precede all newer entries.
    pub(crate) fn restore(&self, mut older: Batch, capacity: usize) {
        let mut inner = self.lock();
        for row in std::mem::take(&mut inner.batch).rows {
            if older.merge(&row.key, row.count) {
                continue;
            }
            if older.has_room(&row.key, capacity) {
                older.insert(row);
            } else {
                let count = u64::try_from(row.count).unwrap_or_default();
                inner.dropped = inner.dropped.saturating_add(count);
            }
        }
        inner.batch = older;
    }

    /// How many entries are waiting.
    #[cfg(test)]
    pub(crate) fn waiting(&self) -> usize {
        self.lock().batch.rows.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: &'static str) -> Entry {
        Entry {
            time: 1,
            kind,
            user: None,
            client: None,
            detail: String::new(),
        }
    }

    #[test]
    fn the_reserve_never_takes_the_last_free_slot() {
        // Settings keep the capacity at two or more, but the reserve must
        // not swallow a one-slot queue either: that would drop every
        // failed login, which is the record the journal exists for.
        let queue = Queue::default();
        queue.push(entry("auth_failed"), 1);
        assert_eq!(queue.waiting(), 1);
        let (batch, dropped) = queue.take();
        assert_eq!(dropped, 0);
        assert_eq!(batch.rows.len(), 1);
    }
}
