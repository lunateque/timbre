//! Bounded, debounced filesystem notifications for library roots.
#![warn(missing_docs)]

pub mod failures;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TrySendError, sync_channel};
use std::time::{Duration, Instant};

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use timbre_kernel::{Failure, Fallible};

const CAPACITY: usize = 4096;

/// A hint from the filesystem. Neither variant guarantees a file is stable.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// Unique paths affected by create, modify, remove or rename events.
    Paths(Vec<PathBuf>),
    /// Notifications may be missing; reconcile the watched roots.
    Rescan,
}

/// Watches canonical roots until dropped.
pub struct Watcher {
    _backend: RecommendedWatcher,
    roots: BTreeSet<PathBuf>,
    input: Receiver<notify::Event>,
    lost: Arc<AtomicBool>,
    debounce: Duration,
    batch_started: Option<Instant>,
    paths: BTreeSet<PathBuf>,
}

fn watch_failure(error: impl std::fmt::Display) -> Box<dyn Failure> {
    Box::new(failures::WatchFailure(error.to_string()))
}

impl Watcher {
    /// Recursively watch root directories without following symlinks in the tree.
    /// An event starts a batch of at most `debounce` time, even during a sustained import.
    ///
    /// # Errors
    /// A root cannot be canonicalized, is not a directory, or cannot be watched.
    pub fn new(roots: &[PathBuf], debounce: Duration) -> Fallible<Self> {
        let roots: BTreeSet<_> = roots
            .iter()
            .map(|root| root.canonicalize().map_err(watch_failure))
            .collect::<Fallible<_>>()?;
        let (send, input) = sync_channel(CAPACITY);
        let lost = Arc::new(AtomicBool::new(false));
        let callback_lost = Arc::clone(&lost);
        let mut backend = RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| {
                let event = if let Ok(event) = result {
                    event
                } else {
                    callback_lost.store(true, Ordering::Release);
                    // An idle receiver must also wake up for a backend error.
                    notify::Event::new(EventKind::Other)
                };
                if let Err(TrySendError::Full(_)) = send.try_send(event) {
                    callback_lost.store(true, Ordering::Release);
                }
            },
            Config::default().with_follow_symlinks(false),
        )
        .map_err(watch_failure)?;
        for root in &roots {
            if !root.is_dir() {
                return Err(watch_failure("watch root must be a directory"));
            }
            backend
                .watch(root, RecursiveMode::Recursive)
                .map_err(watch_failure)?;
        }
        Ok(Self {
            _backend: backend,
            roots,
            input,
            lost,
            debounce,
            batch_started: None,
            paths: BTreeSet::new(),
        })
    }

    /// Wait up to `timeout` for a batch or a rescan hint.
    /// A timeout does not discard an unfinished batch.
    ///
    /// # Errors
    /// The backend's event channel has closed.
    pub fn recv_timeout(&mut self, timeout: Duration) -> Fallible<Option<Event>> {
        let call_started = Instant::now();
        let mut received = false;
        loop {
            if self.lost.swap(false, Ordering::AcqRel) {
                self.paths.clear();
                self.batch_started = None;
                return Ok(Some(Event::Rescan));
            }
            let batch_remaining = self
                .batch_started
                .map(|start| self.debounce.saturating_sub(start.elapsed()));
            if batch_remaining == Some(Duration::ZERO) {
                self.batch_started = None;
                return Ok(Some(Event::Paths(
                    std::mem::take(&mut self.paths).into_iter().collect(),
                )));
            }
            let remaining = timeout.saturating_sub(call_started.elapsed());
            if remaining.is_zero() && received {
                return Ok(None);
            }
            let event = match self
                .input
                .recv_timeout(batch_remaining.map_or(remaining, |time| time.min(remaining)))
            {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout)
                    if batch_remaining.is_some_and(|time| time <= remaining) =>
                {
                    continue;
                }
                Err(RecvTimeoutError::Timeout) => return Ok(None),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(watch_failure("notifier disconnected"));
                }
            };
            received = true;
            if event.need_rescan() {
                self.lost.store(true, Ordering::Release);
            } else if !matches!(event.kind, EventKind::Access(_) | EventKind::Other) {
                for path in event.paths {
                    if self.roots.iter().any(|root| path.starts_with(root)) {
                        self.batch_started.get_or_insert_with(Instant::now);
                        self.paths.insert(path);
                        if self.paths.len() >= CAPACITY {
                            self.lost.store(true, Ordering::Release);
                            break;
                        }
                    }
                }
            }
        }
    }
}
