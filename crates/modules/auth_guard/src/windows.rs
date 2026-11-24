//! The sliding windows themselves: how many failures a key has had inside
//! the window, with a hard cap on how many keys are remembered.
//!
//! The cap is the point of the whole structure. An attacker who can send
//! failures from a new address every time would otherwise make the server
//! remember one window per address until it runs out of memory, and a
//! limiter that kills the server it protects is worse than none.

use crate::config::Settings;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Key {
    V4(u32),
    V6(u64),
    User(Arc<str>),
}

impl Key {
    /// The window of a client. IPv6 is grouped by its /64: a single host is
    /// routinely handed every address in one, so counting per address would
    /// make the limit free to evade.
    pub(crate) fn client(address: IpAddr) -> Self {
        match address.to_canonical() {
            IpAddr::V4(address) => Self::V4(address.to_bits()),
            IpAddr::V6(address) => {
                Self::V6(u64::try_from(address.to_bits() >> 64).unwrap_or_default())
            }
        }
    }

    /// The window of a named user, whatever address the attempts come from.
    pub(crate) fn user(name: &str) -> Self {
        Self::User(name.into())
    }

    /// What the log calls this window: never the address itself, which the
    /// kernel's own line already carries.
    pub(crate) fn scope(&self) -> &'static str {
        match self {
            Self::User(_) => "user",
            Self::V4(_) | Self::V6(_) => "address",
        }
    }

    fn limit(&self, settings: &Settings) -> u32 {
        match self {
            Self::User(_) => settings.user_failures,
            Self::V4(_) | Self::V6(_) => settings.address_failures,
        }
    }
}

#[derive(Default)]
struct Entry {
    failures: VecDeque<Instant>,
    in_flight: u32,
    touched: u64,
    protected_until: Option<Instant>,
}

impl Entry {
    fn prune(&mut self, now: Instant, window: Duration) {
        while self
            .failures
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) > window)
        {
            self.failures.pop_front();
        }
    }

    fn full(&self, limit: u32) -> bool {
        self.failures.len() >= usize::try_from(limit - self.in_flight).unwrap_or(usize::MAX)
    }
}

/// Keep protected entries separately so a full table can refuse unfamiliar
/// keys without scanning every entry. In-flight entries belong to neither
/// index: their eventual settlement makes them eligible or starts cooldown.
#[derive(Default)]
struct Eviction {
    eligible: BTreeSet<(u64, Key)>,
    cooling: BTreeSet<(Instant, Key)>,
    next: u64,
}

impl Eviction {
    fn remove(&mut self, key: &Key, entry: &Entry) {
        if entry.in_flight == 0 {
            if let Some(until) = entry.protected_until {
                self.cooling.remove(&(until, key.clone()));
            } else {
                self.eligible.remove(&(entry.touched, key.clone()));
            }
        }
    }

    fn insert(&mut self, key: Key, entry: &Entry) {
        if entry.in_flight == 0 {
            if let Some(until) = entry.protected_until {
                self.cooling.insert((until, key));
            } else {
                self.eligible.insert((entry.touched, key));
            }
        }
    }

    fn touch(&mut self, entry: &mut Entry) {
        entry.touched = self.next;
        self.next = self.next.saturating_add(1);
    }
}

/// Every tracked window.
#[derive(Default)]
pub(crate) struct Windows {
    entries: HashMap<Key, Entry>,
    eviction: Eviction,
}

impl Windows {
    /// Refuse full windows; reserve as many other required slots as the cap allows.
    pub(crate) fn reserve(
        &mut self,
        keys: [Option<Key>; 2],
        now: Instant,
        settings: &Settings,
    ) -> Option<([bool; 2], u64)> {
        let window = settings.window();
        let mut has_slots = true;
        for key in keys.iter().flatten() {
            if let Some(entry) = self.entries.get_mut(key) {
                entry.prune(now, window);
                has_slots &= !entry.full(key.limit(settings));
            }
        }
        if !has_slots {
            for key in keys.into_iter().flatten() {
                if let Some(entry) = self.entries.get_mut(&key) {
                    self.eviction.remove(&key, entry);
                    self.eviction.touch(entry);
                    self.eviction.insert(key, entry);
                }
            }
            return None;
        }
        let mut room = self.make_room(&keys, now, settings.max_tracked);
        let mut untracked = 0;
        let reserved = keys.map(|key| {
            let Some(key) = key else {
                return false;
            };
            if let Some(entry) = self.entries.get_mut(&key) {
                self.eviction.remove(&key, entry);
                entry.in_flight += 1;
                self.eviction.touch(entry);
            } else if room != 0 {
                room -= 1;
                let mut entry = Entry {
                    in_flight: 1,
                    ..Entry::default()
                };
                self.eviction.touch(&mut entry);
                self.entries.insert(key, entry);
            } else {
                untracked += 1;
                return false;
            }
            true
        });
        Some((reserved, untracked))
    }

    /// Finish a reserved check and report a window that has just filled with
    /// failures, rather than one that only temporarily ran out of in-flight slots.
    pub(crate) fn finish(
        &mut self,
        key: Key,
        now: Instant,
        settings: &Settings,
        failed: bool,
    ) -> Option<u32> {
        let entry = self.entries.get_mut(&key)?;
        let window = settings.window();
        entry.in_flight -= 1;
        entry.prune(now, window);
        let limit = key.limit(settings);
        let mut filled = None;
        if failed {
            entry.failures.push_back(now);
            if entry.in_flight == 0 && entry.full(limit) {
                entry.protected_until = Some(now + window);
                filled = Some(limit);
            }
        }
        if entry.in_flight == 0 && entry.failures.is_empty() {
            self.entries.remove(&key);
        } else {
            self.eviction.touch(entry);
            self.eviction.insert(key, entry);
        }
        filled
    }

    /// How many keys are remembered; the cap is what the tests watch.
    #[cfg(test)]
    pub(crate) fn tracked(&self) -> usize {
        self.entries.len()
    }

    fn make_room(&mut self, keys: &[Option<Key>; 2], now: Instant, cap: usize) -> usize {
        let missing = keys
            .iter()
            .flatten()
            .filter(|key| !self.entries.contains_key(*key))
            .count();
        let needed = (self.entries.len() + missing).saturating_sub(cap);
        if needed == 0 {
            return missing;
        }
        while self
            .eviction
            .cooling
            .first()
            .is_some_and(|(until, _)| *until < now)
        {
            if let Some((_, key)) = self.eviction.cooling.pop_first()
                && let Some(entry) = self.entries.get_mut(&key)
            {
                entry.protected_until = None;
                self.eviction.insert(key, entry);
            }
        }
        let available = |key: &Key| !keys.iter().flatten().any(|selected| selected == key);
        let mut evicted = 0;
        for _ in 0..needed {
            if let Some(oldest) = self
                .eviction
                .eligible
                .iter()
                .find(|(_, key)| available(key))
                .cloned()
            {
                self.entries.remove(&oldest.1);
                self.eviction.eligible.remove(&oldest);
                evicted += 1;
            } else {
                break;
            }
        }
        missing - needed + evicted
    }
}
