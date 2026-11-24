use crate::config::{Settings, shared_db};
use crate::failures::SCAN_RUNNING;
use crate::log_events::WATCH_OVERFLOW;
use crate::scan::{self, Running, STABLE, WatchScope};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};
use timbre_edge_fswatch::{Event, Watcher};
use timbre_kernel::{BoxFuture, Ctx, Fallible, log};

const POLL: Duration = Duration::from_millis(200);
const MAX_PENDING: usize = 16_384;

pub(crate) fn scan(ctx: Ctx) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let library = crate::state(&ctx)?;
        let settings = Arc::clone(crate::settings(library.as_ref())?);
        if !settings.scan_on_start || settings.roots.is_empty() {
            return Ok(());
        }
        let running = match Running::begin(library) {
            Ok(running) => running,
            Err(error) if error.code() == SCAN_RUNNING => return Ok(()),
            Err(error) => return Err(error),
        };
        scan::stoppable(&ctx, move |stop, ctx| {
            scan::run(&ctx, &running, &settings, false, &stop)
        })
        .await?;
        Ok(())
    })
}

pub(crate) fn watch(ctx: Ctx) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let library = crate::state(&ctx)?;
        let settings = Arc::clone(crate::settings(library.as_ref())?);
        if !settings.watch || settings.roots.is_empty() {
            return Ok(());
        }
        scan::stoppable(&ctx, move |stop, ctx| watching(&ctx, &settings, &stop)).await
    })
}

fn watching(ctx: &Ctx, settings: &Settings, stop: &Arc<AtomicBool>) -> Fallible {
    let roots = settings.canonical_roots(shared_db(ctx)?.path())?;
    let mut watcher = Watcher::new(&roots, POLL)?;
    let library = crate::state(ctx)?;
    let mut pending = Pending::default();
    while !stop.load(Ordering::Acquire) {
        if let Some(event) = watcher.recv_timeout(POLL)?
            && pending.add(event)
        {
            log!(ctx, warn, WATCH_OVERFLOW);
        }
        if let Some(scope) = pending.ready() {
            match Running::begin(Arc::clone(&library)) {
                Ok(run) => {
                    scan::run_watched(ctx, &run, settings, stop, &scope)?;
                }
                // The paths stay queued until a pass can take them.
                Err(error) if error.code() == SCAN_RUNNING => pending.restore(scope),
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct Pending {
    paths: BTreeMap<PathBuf, (Option<(u64, SystemTime)>, Instant)>,
    rescan: Option<Instant>,
}

impl Pending {
    fn add(&mut self, event: Event) -> bool {
        let now = Instant::now();
        if let Event::Paths(paths) = event {
            for path in paths {
                if !scan::watchable(&path) {
                    continue;
                }
                let stamp = scan::observe(&path);
                self.paths
                    .entry(path)
                    .and_modify(|old| {
                        if old.0 != stamp {
                            *old = (stamp, now);
                        }
                    })
                    .or_insert((stamp, now));
            }
            if self.paths.len() < MAX_PENDING {
                return false;
            }
        }
        self.paths.clear();
        self.rescan = Some(now);
        true
    }

    /// The paths whose size and modification time have held still across
    /// two observations [`STABLE`] apart. One slow copy no longer keeps
    /// every other queued path waiting.
    fn ready(&mut self) -> Option<WatchScope> {
        let now = Instant::now();
        if self
            .rescan
            .is_some_and(|since| now.duration_since(since) >= STABLE)
        {
            self.rescan = None;
            self.paths.clear();
            return Some(WatchScope::Reconcile);
        }
        let mut ready = HashSet::new();
        self.paths.retain(|path, (stamp, since)| {
            if now.duration_since(*since) < STABLE {
                return true;
            }
            let current = scan::observe(path);
            if current == *stamp {
                ready.insert(path.clone());
                return false;
            }
            *stamp = current;
            *since = now;
            true
        });
        (!ready.is_empty()).then_some(WatchScope::Paths(ready))
    }

    /// Put a scope back after a scan refused to start.
    fn restore(&mut self, scope: WatchScope) {
        let ready = Instant::now()
            .checked_sub(STABLE)
            .unwrap_or_else(Instant::now);
        match scope {
            WatchScope::Reconcile => self.rescan = Some(ready),
            WatchScope::Paths(paths) => {
                for path in paths {
                    let stamp = scan::observe(&path);
                    self.paths.entry(path).or_insert((stamp, ready));
                }
            }
        }
    }
}
