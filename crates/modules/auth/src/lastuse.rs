//! When a device was last seen, recorded off the request path.
//!
//! A request that presents a valid secret must not write to the database:
//! that would turn every read into a write, and a burst of requests from one
//! player into a burst of transactions. Instead the check leaves a note in
//! memory, at most one per device per minute, and a job writes the notes out
//! in one transaction.

use crate::config::shared_db;
use crate::devices;
use crate::log_events::{LAST_USE_FAILED, LAST_USE_WRITTEN};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};
use timbre_kernel::clock::unix_millis;
use timbre_kernel::{BoxFuture, Ctx, Fallible, log, run_periodic};

/// How often one device's activity is written at most.
const RECORD_EVERY: Duration = Duration::from_secs(60);
/// How often the job writes out what has accumulated.
const DRAIN_EVERY: Duration = Duration::from_secs(10);

/// One device's state: when its use was last taken note of, and the note
/// that has not been written yet.
struct Entry {
    noted: Instant,
    pending: Option<Activity>,
}

#[derive(Clone, Copy)]
struct Activity {
    when: i64,
    client: Option<IpAddr>,
}

pub(crate) struct Note {
    prefix: String,
    noted: Instant,
    activity: Activity,
}

/// The device activity waiting to be written.
#[derive(Default)]
pub(crate) struct Pending {
    entries: Mutex<HashMap<String, Entry>>,
}

impl Pending {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Rate-limit notes by elapsed time, not by the timestamp we persist.
    pub(crate) fn note(&self, prefix: String, wall: SystemTime, client: Option<IpAddr>) {
        let mut entries = self.lock();
        Self::record(&mut entries, prefix, Instant::now(), wall, client);
    }

    fn record(
        entries: &mut HashMap<String, Entry>,
        prefix: String,
        now: Instant,
        wall: SystemTime,
        client: Option<IpAddr>,
    ) {
        let pending = Some(Activity {
            when: unix_millis(wall),
            client,
        });
        entries
            .entry(prefix)
            .and_modify(|entry| {
                if now.saturating_duration_since(entry.noted) >= RECORD_EVERY {
                    *entry = Entry {
                        noted: now,
                        pending,
                    };
                }
            })
            .or_insert(Entry {
                noted: now,
                pending,
            });
    }

    /// Keep rate-limit entries only while recent; a pending note is drained
    /// even when the entry is no longer recent.
    pub(crate) fn take(&self, now: Instant) -> Vec<Note> {
        let mut entries = self.lock();
        let mut writes = Vec::new();
        entries.retain(|prefix, entry| {
            if let Some(activity) = entry.pending.take() {
                writes.push(Note {
                    prefix: prefix.clone(),
                    noted: entry.noted,
                    activity,
                });
            }
            now.saturating_duration_since(entry.noted) < RECORD_EVERY
        });
        writes
    }

    fn restore(&self, writes: Vec<Note>) {
        let mut entries = self.lock();
        for Note {
            prefix,
            noted,
            activity,
        } in writes
        {
            entries
                .entry(prefix)
                .and_modify(|entry| {
                    if entry.noted == noted && entry.pending.is_none() {
                        entry.pending = Some(activity);
                    }
                })
                .or_insert(Entry {
                    noted,
                    pending: Some(activity),
                });
        }
    }

    /// How many devices are remembered; the once-a-minute rule is what this
    /// bounds, and the test of that rule reads it.
    #[cfg(test)]
    pub(crate) fn tracked(&self) -> usize {
        self.lock().len()
    }
}

/// Write out what has accumulated. Doing nothing when nothing is pending is
/// what lets this run on a host that has no database at all.
pub(crate) fn flush(ctx: &Ctx) -> Fallible {
    let state = crate::state(ctx)?;
    let activity = state.activity();
    let writes = activity.take(Instant::now());
    if writes.is_empty() {
        return Ok(());
    }
    let result = (|| {
        let shared = shared_db(ctx)?;
        let database = shared.get(ctx)?;
        database.write(|sql| {
            for note in &writes {
                let client = note.activity.client.map(|ip| ip.to_string());
                devices::record_use(sql, &note.prefix, note.activity.when, client.as_deref())?;
            }
            Ok(())
        })
    })();
    if let Err(failure) = result {
        activity.restore(writes);
        return Err(failure);
    }
    log!(ctx, debug, LAST_USE_WRITTEN, devices = writes.len());
    Ok(())
}

/// The job that writes device activity out every few seconds.
pub(crate) fn job(ctx: Ctx) -> BoxFuture<Fallible> {
    Box::pin(
        async move { run_periodic(&ctx, DRAIN_EVERY, DRAIN_EVERY, LAST_USE_FAILED, flush).await },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn moving_the_wall_clock_back_does_not_extend_the_rate_limit() {
        let pending = Pending::default();
        let start = Instant::now();
        let first_wall = UNIX_EPOCH + Duration::from_secs(3600);
        let earlier_wall = UNIX_EPOCH + Duration::from_mins(30);
        Pending::record(
            &mut pending.lock(),
            "prefix".into(),
            start,
            first_wall,
            None,
        );
        assert_eq!(
            pending.take(start)[0].activity.when,
            unix_millis(first_wall)
        );
        let later = start + RECORD_EVERY;
        Pending::record(
            &mut pending.lock(),
            "prefix".into(),
            later,
            earlier_wall,
            None,
        );
        assert_eq!(
            pending.take(later)[0].activity.when,
            unix_millis(earlier_wall)
        );
    }
}
