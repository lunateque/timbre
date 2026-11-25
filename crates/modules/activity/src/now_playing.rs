//! Bounded in-memory table of active now-playing reports.
//!
//! Entries live only in RAM because a crash or restart invalidates transient
//! "currently playing" state anyway, and keeping them out of SQLite avoids
//! write traffic on every track start.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use timbre_kernel::activity::NowPlayingEntry;
use timbre_kernel::identity::UserName;

/// Maximum simultaneous active clients tracked per user.
const MAX_CLIENTS_PER_USER: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    user: UserName,
    client: String,
}

#[derive(Clone, Debug)]
struct Value {
    track: String,
    updated_at: SystemTime,
}

#[derive(Default)]
pub(crate) struct NowPlayingTable {
    entries: Mutex<HashMap<Key, Value>>,
}

impl NowPlayingTable {
    /// Record or refresh one `(user, client)` entry, evicting expired items
    /// and enforcing both the per-user and global entry caps. Returns the
    /// eviction reason when an active entry was dropped to make room.
    pub(crate) fn record(
        &self,
        user: UserName,
        client: String,
        track: String,
        now: SystemTime,
        ttl: Duration,
        max_entries: usize,
    ) -> Option<&'static str> {
        let mut map = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        prune_expired(&mut map, now, ttl);
        let key = Key { user, client };
        if let Some(existing) = map.get_mut(&key) {
            existing.track = track;
            existing.updated_at = now;
            return None;
        }
        let mut evicted = None;
        let user_count = map.keys().filter(|k| k.user == key.user).count();
        if user_count >= MAX_CLIENTS_PER_USER
            && let Some(oldest) = oldest_key(&map, Some(&key.user))
        {
            map.remove(&oldest);
            evicted = Some("user_cap");
        }
        while map.len() >= max_entries {
            let Some(oldest) = oldest_key(&map, None) else {
                break;
            };
            map.remove(&oldest);
            evicted = Some("table_cap");
        }
        if max_entries > 0 {
            map.insert(
                key,
                Value {
                    track,
                    updated_at: now,
                },
            );
        }
        evicted
    }

    /// Clear a matching `(user, client, track)` entry once its play submission
    /// completes so finished plays do not linger until TTL expiry.
    pub(crate) fn clear_completed(&self, user: &UserName, client: &str, track: &str) {
        let mut map = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = Key {
            user: user.clone(),
            client: client.to_owned(),
        };
        if map.get(&key).is_some_and(|v| v.track == track) {
            map.remove(&key);
        }
    }

    /// Return non-expired entries (optionally filtered to `only_user`) ordered
    /// by `updated_at DESC, user ASC, client ASC`. Releases the mutex before
    /// sorting so readers do not block concurrent updates.
    pub(crate) fn snapshot(
        &self,
        only_user: Option<&UserName>,
        now: SystemTime,
        ttl: Duration,
    ) -> Vec<NowPlayingEntry> {
        let mut list: Vec<NowPlayingEntry> = {
            let mut map = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            prune_expired(&mut map, now, ttl);
            map.iter()
                .filter(|(k, _)| only_user.is_none_or(|u| k.user == *u))
                .map(|(k, v)| NowPlayingEntry {
                    user: k.user.clone(),
                    track: v.track.clone(),
                    client: k.client.clone(),
                    updated_at: v.updated_at,
                })
                .collect()
        };
        list.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.user.as_str().cmp(b.user.as_str()))
                .then_with(|| a.client.cmp(&b.client))
        });
        list
    }

    /// Drop every active entry belonging to `user`.
    pub(crate) fn remove_user(&self, user: &UserName) -> usize {
        let mut map = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = map.len();
        map.retain(|k, _| k.user != *user);
        before.saturating_sub(map.len())
    }
}

fn prune_expired(map: &mut HashMap<Key, Value>, now: SystemTime, ttl: Duration) {
    map.retain(|_, v| {
        now.duration_since(v.updated_at)
            .map_or(true, |age| age <= ttl)
    });
}

fn oldest_key(map: &HashMap<Key, Value>, only_user: Option<&UserName>) -> Option<Key> {
    map.iter()
        .filter(|(k, _)| only_user.is_none_or(|u| k.user == *u))
        .min_by(|(ka, va), (kb, vb)| {
            va.updated_at
                .cmp(&vb.updated_at)
                .then_with(|| ka.user.as_str().cmp(kb.user.as_str()))
                .then_with(|| ka.client.cmp(&kb.client))
        })
        .map(|(k, _)| k.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn user(name: &str) -> UserName {
        UserName::normalize(name).unwrap()
    }

    fn ts(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn expires_entries_past_ttl_and_enforces_per_user_and_global_caps() {
        let table = NowPlayingTable::default();
        let ttl = Duration::from_secs(60);
        let alice = user("alice");
        let bob = user("bob");

        let mut last_eviction = None;
        for i in 0..=MAX_CLIENTS_PER_USER {
            last_eviction = table.record(
                alice.clone(),
                format!("client-{i}"),
                format!("t-{i}"),
                ts(100 + u64::try_from(i).unwrap()),
                ttl,
                100,
            );
        }
        assert_eq!(last_eviction, Some("user_cap"));
        let snap = table.snapshot(None, ts(120), ttl);
        assert_eq!(snap.len(), MAX_CLIENTS_PER_USER);
        assert!(snap.iter().all(|e| e.client != "client-0"));

        assert_eq!(table.snapshot(None, ts(200), ttl), []);

        assert_eq!(
            table.record(
                alice.clone(),
                "a".to_owned(),
                "t1".to_owned(),
                ts(300),
                ttl,
                2
            ),
            None
        );
        assert_eq!(
            table.record(
                bob.clone(),
                "b".to_owned(),
                "t2".to_owned(),
                ts(301),
                ttl,
                2
            ),
            None
        );
        assert_eq!(
            table.record(
                bob.clone(),
                "b2".to_owned(),
                "t3".to_owned(),
                ts(302),
                ttl,
                2
            ),
            Some("table_cap")
        );
        let capped = table.snapshot(None, ts(305), ttl);
        assert_eq!(capped.len(), 2);
        assert!(capped.iter().all(|e| e.user == bob));

        table.clear_completed(&bob, "b", "t2");
        assert_eq!(table.snapshot(Some(&bob), ts(305), ttl).len(), 1);

        assert_eq!(table.remove_user(&bob), 1);
        assert_eq!(table.snapshot(None, ts(305), ttl), []);
    }
}
