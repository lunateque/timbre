use crate::Database;
use crate::failures::SqliteFailure;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};
use timbre_kernel::{Ctx, Failure, Fallible};

/// How long a caller waits for another caller's open before `sqlite.busy`.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// One database per host, opened and migrated on first use.
///
/// A failed initialization is not remembered: the next caller tries again, so
/// a data directory that was not writable yet (or a migration that failed on
/// a full disk) recovers without restarting the process. Only one caller
/// opens at a time; the others wait at most five seconds.
pub struct SharedDatabase {
    path: PathBuf,
    prepare: fn(&Ctx, &Path) -> Fallible,
    database: OnceLock<Database>,
    opening: Mutex<bool>,
    finished: Condvar,
}

impl SharedDatabase {
    /// Access the shared database resource without opening it.
    ///
    /// # Errors
    /// Returns the caller's not-configured failure if the host installed
    /// no database resource.
    pub fn require(ctx: &Ctx, missing: impl Failure) -> Fallible<Arc<Self>> {
        ctx.resource::<Self>()
            .ok_or_else(|| Box::new(missing) as Box<dyn Failure>)
    }

    /// Set the database path and the host's filesystem preparation function.
    pub const fn new(path: PathBuf, prepare: fn(&Ctx, &Path) -> Fallible) -> Self {
        Self {
            path,
            prepare,
            database: OnceLock::new(),
            opening: Mutex::new(false),
            finished: Condvar::new(),
        }
    }

    /// The configured database path, without initializing the database.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Access the database, performing initialization once across all callers.
    /// A failed attempt is reported to its own caller only; the next call
    /// retries the preparation, open and migration.
    ///
    /// # Errors
    /// The preparation, open or migration failure of this attempt, with its
    /// original code, or `sqlite.busy` when another caller's open did not
    /// finish within five seconds.
    pub fn get(&self, ctx: &Ctx) -> Fallible<&Database> {
        if let Some(database) = self.database.get() {
            return Ok(database);
        }
        let Some(_attempt) = self.claim()? else {
            // Another caller finished the open while this one waited.
            return self.database.get().ok_or_else(busy);
        };
        let database = self.open(ctx)?;
        Ok(self.database.get_or_init(|| database))
    }

    /// Become the opening caller, or wait (bounded) for the one that already
    /// is. `None` means the database was opened while waiting.
    fn claim(&self) -> Fallible<Option<Attempt<'_>>> {
        let mut opening = self.opening.lock().unwrap_or_else(PoisonError::into_inner);
        let deadline = Instant::now() + OPEN_TIMEOUT;
        while *opening {
            if self.database.get().is_some() {
                return Ok(None);
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(busy());
            };
            let (guard, wait) = self
                .finished
                .wait_timeout(opening, left)
                .unwrap_or_else(PoisonError::into_inner);
            opening = guard;
            if wait.timed_out() && *opening {
                return Err(busy());
            }
        }
        if self.database.get().is_some() {
            return Ok(None);
        }
        *opening = true;
        Ok(Some(Attempt(self)))
    }

    fn open(&self, ctx: &Ctx) -> Fallible<Database> {
        (self.prepare)(ctx, &self.path)?;
        let database = Database::open(&self.path)?;
        database.migrate(ctx, ctx.kernel().migrations())?;
        Ok(database)
    }
}

/// The right to run one open; released even if the open panics, so a failed
/// attempt never leaves every later caller waiting for an opener that is gone.
struct Attempt<'a>(&'a SharedDatabase);

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        *self
            .0
            .opening
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = false;
        self.0.finished.notify_all();
    }
}

fn busy() -> Box<dyn timbre_kernel::Failure> {
    Box::new(SqliteFailure::Busy)
}

#[cfg(test)]
mod tests {
    use super::SharedDatabase;
    use crate::failures::SqliteFailure;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use timbre_kernel::{Ctx, Fallible};

    /// Flipped by the test once the cause of the first failure is gone;
    /// `prepare` is a plain function pointer and cannot capture it.
    static READY: AtomicBool = AtomicBool::new(false);

    fn prepare(_: &Ctx, path: &Path) -> Fallible {
        if !READY.load(Ordering::SeqCst) {
            return Err(Box::new(SqliteFailure::Busy));
        }
        let parent = path.parent().expect("the database path has a directory");
        std::fs::create_dir_all(parent).expect("the scratch directory is created");
        Ok(())
    }

    #[test]
    fn a_failed_open_is_retried_by_the_next_caller() {
        let kernel = timbre_kernel::Registry::new().finish(
            timbre_kernel::Logger::stdout(timbre_kernel::LogFilter::default()),
            Arc::new(timbre_kernel::ThreadRuntime::new()),
        );
        let ctx = Ctx::root(&kernel);
        let path = std::env::temp_dir()
            .join(format!("timbre-shared-{}", std::process::id()))
            .join("timbre.db");
        let shared = SharedDatabase::new(path.clone(), prepare);
        let failure = shared.get(&ctx).err().expect("the first open fails");
        assert_eq!(failure.code().as_str(), "sqlite.busy");
        READY.store(true, Ordering::SeqCst);
        assert!(shared.get(&ctx).is_ok(), "the next caller retries the open");
        assert!(shared.get(&ctx).is_ok(), "the open is kept afterwards");
        drop(shared);
        let _ = std::fs::remove_dir_all(path.parent().expect("a directory"));
    }
}
