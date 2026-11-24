mod errors;
mod import;
mod paths;
mod progress;
mod read;
mod relink;
mod scope;
mod sidecars;
mod tree;
mod walk;
mod warnings;

pub(crate) use paths::{observe, watchable};
pub(crate) use read::Source;

use crate::LibraryState;
use crate::config::{Settings, shared_db};
use crate::failures::LibraryFailure;
use crate::log_events::{SCAN_FAILED, SCAN_STARTED};
use crate::store::{exec, one, path_to_bytes, time, unsigned};
use progress::Progress;
use std::collections::HashSet;
use std::future::poll_fn;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;
use std::time::{Duration, Instant};
use timbre_edge_sqlite::rusqlite::params;
use timbre_kernel::catalog::ScanStatus;
use timbre_kernel::{Ctx, Fallible, log};

pub(crate) const STABLE: Duration = Duration::from_secs(2);
/// How many times a watch pass re-walks before it applies what it has.
const STABLE_ROUNDS: usize = 3;

pub(crate) enum WatchScope {
    Paths(HashSet<PathBuf>),
    Reconcile,
}

pub(crate) struct Running(Arc<dyn LibraryState>);

impl Running {
    pub(crate) fn begin(library: Arc<dyn LibraryState>) -> Fallible<Self> {
        {
            let mut scan = library
                .scan()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if scan.scanning {
                return Err(LibraryFailure::ScanRunning.boxed());
            }
            *scan = ScanStatus {
                scanning: true,
                ..ScanStatus::default()
            };
        }
        Ok(Self(library))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.0
            .scan()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .scanning = false;
    }
}

pub(crate) fn status(library: &dyn LibraryState, ctx: &Ctx) -> Fallible<ScanStatus> {
    let current = *library
        .scan()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if current.scanning {
        return Ok(current);
    }
    shared_db(ctx)?.get(ctx)?.read(|sql| one(sql, "SELECT (SELECT count(*) FROM library_tracks), scanned_folders,tag_reads,unreadable,finished FROM library_state WHERE id=1", [], |row| Ok(ScanStatus {
        scanning: false, tracks: unsigned(row, 0)?, folders: unsigned(row, 1)?, tag_reads: unsigned(row, 2)?, unreadable: unsigned(row, 3)?, finished: row.get::<_, Option<i64>>(4)?.and_then(|value| u64::try_from(value).ok()).map(time),
    })))
}

pub(crate) fn start(ctx: &Ctx, library: &dyn LibraryState, full: bool) -> Fallible<ScanStatus> {
    let settings = Settings::get(ctx)?;
    settings.canonical_roots(shared_db(ctx)?.path())?;
    let current = status(library, ctx)?;
    let running = Running::begin(crate::state(ctx)?)?;
    let ctx = ctx.clone();
    ctx.clone().spawn(Box::pin(async move {
        if let Err(error) = stoppable(&ctx, move |stop, ctx| {
            run(&ctx, &running, &settings, full, &stop)
        })
        .await
        {
            log!(
                ctx,
                error,
                SCAN_FAILED,
                code = error.code().as_str(),
                reason = format!("{error:?}").as_str()
            );
        }
    }));
    Ok(ScanStatus {
        scanning: true,
        ..current
    })
}

pub(crate) async fn stoppable<T: Send + 'static>(
    ctx: &Ctx,
    work: impl FnOnce(Arc<AtomicBool>, Ctx) -> Fallible<T> + Send + 'static,
) -> Fallible<T> {
    let stop = Arc::new(AtomicBool::new(false));
    let in_worker = Arc::clone(&stop);
    let worker_ctx = ctx.clone();
    let mut blocking = ctx.blocking(move || work(in_worker, worker_ctx));
    let mut shutdown = Some(ctx.shutdown());
    poll_fn(move |cx| {
        if shutdown
            .as_mut()
            .is_some_and(|signal| signal.as_mut().poll(cx).is_ready())
        {
            stop.store(true, Ordering::Release);
            shutdown = None;
        }
        match blocking.as_mut().poll(cx) {
            Poll::Ready(result) => Poll::Ready(result.and_then(std::convert::identity)),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

pub(crate) fn run(
    ctx: &Ctx,
    running: &Running,
    settings: &Settings,
    full: bool,
    stop: &AtomicBool,
) -> Fallible {
    run_inner(ctx, running, settings, full, stop, None)
}

pub(crate) fn run_watched(
    ctx: &Ctx,
    running: &Running,
    settings: &Settings,
    stop: &AtomicBool,
    scope: &WatchScope,
) -> Fallible {
    run_inner(ctx, running, settings, false, stop, Some(scope))
}

fn run_inner(
    ctx: &Ctx,
    running: &Running,
    settings: &Settings,
    full: bool,
    stop: &AtomicBool,
    watch: Option<&WatchScope>,
) -> Fallible {
    let shared = shared_db(ctx)?;
    let roots = settings.canonical_roots(shared.path())?;
    let db = shared.get(ctx)?;
    log!(
        ctx,
        info,
        SCAN_STARTED,
        roots = roots.len(),
        full = full,
        workers = settings.scan_workers
    );
    let progress = Progress::new(ctx, &running.0)?;
    let stopped = || stop.load(Ordering::Acquire);
    let policy = settings.policy_key()?;
    let full = full || needs_full_scan(db, &policy)?;
    let (indexed, previous) = tree::snapshot(db)?;
    let roots_changed = roots.len() != indexed.len()
        || roots.iter().enumerate().any(|(order, path)| {
            indexed
                .get(path)
                .is_none_or(|(_, old)| i64::try_from(order).ok() != Some(*old))
        });
    let scope = match watch {
        Some(WatchScope::Paths(paths)) if !full && !roots_changed => Some(paths),
        _ => None,
    };
    let inventory = || walk::inventory(ctx, &roots, &indexed, &previous, scope, &stopped);
    let (observed, unstable) = if watch.is_some() {
        stable_inventory(inventory, &stopped)?
    } else {
        (inventory()?, Vec::new())
    };
    let forest = &observed.forest;
    progress.folders(forest.len().saturating_sub(observed.retained.len()));
    if stopped() {
        return progress.finish(db, None);
    }
    let changed = full || roots_changed || forest_changed(forest, &indexed, &previous);
    prepare_reconcile(db, changed)?;
    if changed {
        let tree = tree::Tree::apply(
            db,
            &roots,
            &observed,
            &indexed,
            &previous,
            full || roots_changed,
        )?;
        let known = import::known(db)?;
        let locations = import::audio_locations(forest);
        let retained = import::retained_paths(db, &observed.retained)?;
        let previous_errors = errors::Previous::load(db, full)?;
        let mut unreadable = errors::Unreadable::new();
        let requests = import::requests(
            &locations,
            &roots,
            &known,
            full,
            &previous_errors,
            &mut unreadable,
        );
        let (mut imports, new_errors) = import::read_tags(
            ctx,
            &requests,
            &locations,
            settings.scan_workers,
            &stopped,
            |failed| progress.tag(failed),
        )?;
        unreadable.extend(new_errors);
        if stopped() {
            return progress.finish(db, None);
        }
        let present = locations
            .keys()
            .copied()
            .chain(retained.keys().map(PathBuf::as_path))
            .collect::<HashSet<_>>();
        relink::assign(ctx, &known, &present, &mut imports);
        let tracks = import::apply(ctx, db, imports, &known, &mut unreadable)?;
        let paths = import::PathUpdates {
            tracks: &tracks,
            errors: &unreadable,
            retained: &retained,
        };
        import::update_folders(ctx, db, &tree, &observed, &roots, &locations, &paths)?;
        tree.erase_missing(db)?;
        let scope = scope::regroup_scope(db, &tree, !full && !roots_changed)?;
        if !crate::albums::regroup(db, settings, scope.as_ref(), &stopped)? {
            return progress.finish(db, None);
        }
        let tree_changed =
            !tree.changed.is_empty() || !tree.removed.is_empty() || !tree.removed_roots.is_empty();
        db.write(|sql| { exec(sql, "UPDATE library_state SET tree_version=tree_version+?1,policy=?2,dirty=0 WHERE id=1", params![tree_changed, policy])?; Ok(()) })?;
    }
    mark_unstable(db, &unstable)?;
    progress.finish(db, Some(observed.unreadable))
}

fn prepare_reconcile(db: &timbre_edge_sqlite::Database, changed: bool) -> Fallible {
    db.write(|sql| {
        exec(
            sql,
            "DELETE FROM library_diagnostics WHERE kind='relink_ambiguous'",
            [],
        )?;
        if changed {
            exec(sql, "UPDATE library_state SET dirty=1 WHERE id=1", [])?;
        }
        Ok(())
    })
}

fn needs_full_scan(db: &timbre_edge_sqlite::Database, policy: &str) -> Fallible<bool> {
    db.read(|sql| {
        one(
            sql,
            "SELECT policy!=?1 OR dirty FROM library_state WHERE id=1",
            [policy],
            |row| row.get(0),
        )
    })
}

fn forest_changed(
    forest: &walk::Forest,
    indexed: &tree::Roots,
    previous: &[tree::KnownFolder],
) -> bool {
    if forest.len() != previous.len() {
        return true;
    }
    let orders: std::collections::HashMap<_, _> = indexed
        .values()
        .filter_map(|(id, ordinal)| Some((*id, usize::try_from(*ordinal).ok()?)))
        .collect();
    previous.iter().any(|folder| {
        orders
            .get(&folder.root)
            .and_then(|index| forest.get(&(*index, folder.relative.clone())))
            .is_none_or(|now| now.hash != folder.hash)
    })
}

/// Re-walk until two rounds see the same folders, at most [`STABLE_ROUNDS`]
/// times. A folder still changing in the last round is applied as it was
/// last seen and reported, instead of holding the whole pass open while one
/// directory is being written into.
fn stable_inventory(
    inventory: impl Fn() -> Fallible<walk::Inventory>,
    stopped: &impl Fn() -> bool,
) -> Fallible<(walk::Inventory, Vec<PathBuf>)> {
    let mut observed = inventory()?;
    let mut unstable = Vec::new();
    for _ in 0..STABLE_ROUNDS {
        if stopped() {
            break;
        }
        let since = Instant::now();
        while since.elapsed() < STABLE && !stopped() {
            std::thread::sleep(
                Duration::from_millis(100).min(STABLE.saturating_sub(since.elapsed())),
            );
        }
        if stopped() {
            break;
        }
        let next = inventory()?;
        unstable = next
            .forest
            .iter()
            .filter(|(key, folder)| {
                observed
                    .forest
                    .get(*key)
                    .is_none_or(|before| before.hash != folder.hash)
            })
            .map(|(_, folder)| folder.absolute.clone())
            .collect();
        observed = next;
        if unstable.is_empty() {
            break;
        }
    }
    Ok((observed, unstable))
}

/// Make the next pass read these folders again: an empty hash can never
/// equal a computed digest, so the walk sees them as changed.
fn mark_unstable(db: &timbre_edge_sqlite::Database, folders: &[PathBuf]) -> Fallible {
    if folders.is_empty() {
        return Ok(());
    }
    db.write(|sql| {
        for path in folders {
            exec(
                sql,
                "UPDATE library_folders SET folder_hash='' WHERE absolute=?1",
                [path_to_bytes(path)],
            )?;
        }
        Ok(())
    })
}
