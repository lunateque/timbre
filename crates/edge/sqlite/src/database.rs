//! The connections: one writer, a pool of readers, blocking calls.

use crate::failures::{SqliteFailure, open_error, query_error};
use crate::gate::Gate;
use crate::pool::{Lease, Pool};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use timbre_kernel::Fallible;

/// The default cap on one statement's run time before the watchdog cuts it.
const DEFAULT_STATEMENT_TIMEOUT: Duration = Duration::from_secs(30);
/// VM instructions between watchdog checks: fine-grained enough to cut a
/// runaway query close to its limit, coarse enough to cost nothing.
const PROGRESS_EVERY_OPS: i32 = 1000;

/// An open database with the access pattern phase 1 needs: one writer, several
/// readers, every call blocking (wrap it in `Ctx::blocking` from async code).
pub struct Database {
    // The gate bounds how long a caller waits to *hold* the writer (5 s,
    // `sqlite.busy`); the mutex inside only ever hands an acquired connection
    // over, so its own lock cannot block past contention on a single handoff.
    gate: Gate,
    pub(crate) writer: Arc<Mutex<Connection>>,
    readers: Arc<Pool>,
    path: std::path::PathBuf,
}

impl Database {
    /// Open (creating if needed) the database at `path` with WAL journaling,
    /// a five-second busy timeout and foreign keys on, and fill the reader
    /// pool with one connection per available parallel core (at least two).
    /// On Unix, a file owned by a different user is refused: a planted
    /// database must never be trusted by whoever opens it first.
    ///
    /// # Errors
    ///
    /// `sqlite.open_failed` when the file cannot be opened or one of these
    /// settings cannot be applied, or (Unix) when the file's owner is not
    /// the running user.
    pub fn open(path: &Path) -> Fallible<Database> {
        Database::open_with(path, DEFAULT_STATEMENT_TIMEOUT, readers_wanted())
    }

    /// `open` with the per-statement run-time cap and reader-pool size chosen
    /// by the caller; the binary uses the defaults, tests a shorter cap and
    /// a smaller pool.
    ///
    /// # Errors
    ///
    /// `sqlite.open_failed` when the file cannot be opened or configured.
    pub fn open_with(
        path: &Path,
        statement_timeout: Duration,
        readers: usize,
    ) -> Fallible<Database> {
        let open = || {
            Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_CREATE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(open_error)
        };
        let configure = |connection: &Connection| {
            for statement in [
                "PRAGMA journal_mode = WAL",
                "PRAGMA busy_timeout = 5000",
                "PRAGMA foreign_keys = ON",
            ] {
                connection.execute_batch(statement).map_err(open_error)?;
            }
            Ok(())
        };
        // The watchdog needs `&mut`, so connections are fitted out before
        // they move into the writer mutex and the idle pool.
        let arm = |mut connection: Connection| -> Fallible<Connection> {
            arm_watchdog(&mut connection, statement_timeout).map_err(open_error)?;
            Ok(connection)
        };
        let writer = arm(open()?)?;
        configure(&writer)?;
        let mut pool = Vec::new();
        for _ in 0..readers {
            let reader = arm(open()?)?;
            configure(&reader)?;
            pool.push(reader);
        }
        // SQLite creates the -wal and -shm siblings on the first write; the
        // mode is set on all three here anyway so a database that already
        // exists ends up private.
        crate::files::restrict_to_owner(path)?;
        // A database a different user planted (first run into a shared or
        // pre-made data dir) is not ours to trust: fail closed.
        crate::files::verify_owner(path)?;
        Ok(Database {
            gate: Gate::new(),
            writer: Arc::new(Mutex::new(writer)),
            readers: Arc::new(Pool::new(pool)),
            path: path.to_path_buf(),
        })
    }

    /// The file this database was opened from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run `work` on the writer inside one transaction: it commits when
    /// `work` returns `Ok` and rolls back completely when it fails. The
    /// wait to acquire the writer is bounded (5 s, then `sqlite.busy`);
    /// the statement watchdog caps each statement inside the transaction,
    /// not the transaction as a whole, so a long transaction is made of
    /// individually capped statements.
    ///
    /// # Errors
    ///
    /// Whatever `work` returns, `sqlite.busy` when the writer stayed held
    /// for the whole gate timeout or no reader was free, or
    /// `sqlite.query_failed` when the transaction cannot be started or
    /// committed.
    pub fn write<T>(&self, work: impl FnOnce(&Connection) -> Fallible<T>) -> Fallible<T> {
        let _held = self.acquire_writer()?;
        let mut guard = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        // A poisoned mutex means an earlier writer panicked; the connection
        // itself is still usable (SQLite never leaves a half-written frame),
        // so recovering keeps one bad command from killing the process.
        let transaction = guard.transaction().map_err(query_error)?;
        let value = work(&transaction)?;
        transaction.commit().map_err(query_error)?;
        Ok(value)
    }

    /// Take the writer gate or fail with `sqlite.busy` after its timeout:
    /// every call that writes on the writer connection passes through it,
    /// so none waits behind a long transaction forever.
    pub(crate) fn acquire_writer(&self) -> Fallible<crate::gate::Guard<'_>> {
        self.gate
            .acquire()
            .ok_or_else(|| Box::new(SqliteFailure::Busy) as Box<dyn timbre_kernel::Failure>)
    }

    /// Run `work` on a pooled reader connection.
    ///
    /// # Errors
    ///
    /// Whatever `work` returns, or `sqlite.busy` when no reader was free
    /// within the lease timeout, or the edge's query failure when the
    /// statement failed.
    pub fn read<T>(&self, work: impl FnOnce(&Connection) -> Fallible<T>) -> Fallible<T> {
        let guard = self.lease()?;
        let value = work(&guard);
        drop(guard);
        value
    }

    /// Borrow a reader or fail with `sqlite.busy`.
    fn lease(&self) -> Fallible<Lease<'_>> {
        self.readers
            .take()
            .ok_or_else(|| Box::new(SqliteFailure::Busy) as Box<dyn timbre_kernel::Failure>)
    }

    /// `PRAGMA integrity_check`: `Ok(())` when the database reports `ok`.
    ///
    /// # Errors
    ///
    /// `sqlite.query_failed` when the pragma fails or reports problems; the
    /// reason carries every reported line.
    pub fn check(&self) -> Fallible {
        let guard = self.lease()?;
        let outcome: Result<String, rusqlite::Error> =
            guard.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0));
        let text = match outcome {
            Ok(text) => text,
            Err(error) => return Err(query_error(error)),
        };
        if text == "ok" {
            Ok(())
        } else {
            Err(Box::new(SqliteFailure::Query { reason: text }) as Box<dyn timbre_kernel::Failure>)
        }
    }

    /// Copy the whole database to `to` with SQLite's online backup API.
    /// An existing destination (file or symlink) is refused unless `force`
    /// replaces it. The destination is created exclusively (`create_new`,
    /// mode 0600 on Unix) before a byte is copied, so it is never
    /// world-readable mid-copy and a destination a raced symlink points at
    /// fails closed instead of being followed; a destination naming the
    /// live database file itself is refused, and a failed copy deletes the
    /// partial file. Under `force` the old destination is removed only
    /// after the writer gate is held, so a `sqlite.busy` timeout never
    /// destroys an old backup without writing a new one.
    ///
    /// # Errors
    ///
    /// `sqlite.busy` when the writer stayed held for the whole gate
    /// timeout, `sqlite.open_failed` when the destination exists without
    /// `force`, names the live database, or could not be created,
    /// `sqlite.query_failed` for a failed backup step.
    pub fn backup(&self, to: &Path, force: bool) -> Fallible {
        // A destination that *is* the live file would be deleted by `force`
        // and then copied onto itself: refuse it by identity, not by name,
        // so a hard or symlinked path is caught too.
        if crate::files::same_file(to, &self.path) {
            return Err(Box::new(SqliteFailure::Open {
                reason: format!("{} is the database itself", to.display()),
            }) as Box<dyn timbre_kernel::Failure>);
        }
        // `symlink_metadata` sees a dangling symlink as existing too:
        // following it would write through the link, possibly outside the
        // data directory.
        let replaces = match std::fs::symlink_metadata(to) {
            Ok(_) if !force => {
                return Err(Box::new(SqliteFailure::Open {
                    reason: format!(
                        "{} already exists; pass --force to replace it",
                        to.display()
                    ),
                }) as Box<dyn timbre_kernel::Failure>);
            }
            Ok(_) => true,
            Err(_) => false,
        };
        let _held = self.acquire_writer()?;
        if replaces {
            // Remove the destination and its sidecars only now that the
            // gate is held: a busy failure above leaves the old backup
            // intact. The exclusive create below turns a symlink planted
            // between this removal and the create into a refusal, not a
            // follow.
            crate::files::remove_with_sidecars(to)?;
        }
        let file = crate::files::create_exclusive(to)
            .map_err(|error| crate::files::open_failure(to, &error))?;
        drop(file);
        let outcome = self.copy_backup(to);
        if outcome.is_err() {
            // The partial copy is not a usable backup and must not linger
            // with a database's name; the sidecars go with it.
            let _ = crate::files::remove_with_sidecars(to);
        }
        outcome?;
        crate::files::restrict_to_owner(to)
    }

    /// The copy itself, once the destination exists exclusively: open it
    /// with the backup API and page the live database across.
    fn copy_backup(&self, to: &Path) -> Fallible {
        let writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let mut destination = Connection::open(to).map_err(open_error)?;
        let backup =
            rusqlite::backup::Backup::new(&writer, &mut destination).map_err(query_error)?;
        // 100 pages per step with no pause: a CLI backup may take its time,
        // and `run_backup` propagates any step failure.
        backup
            .run_to_completion(100, std::time::Duration::ZERO, None)
            .map_err(query_error)?;
        drop(backup);
        drop(destination);
        drop(writer);
        Ok(())
    }

    /// Every user table no registered module owns; the kernel's own
    /// bookkeeping table belongs to no module but is never a candidate.
    ///
    /// # Errors
    ///
    /// `sqlite.busy` when no reader was free within the lease timeout,
    /// `sqlite.query_failed` when the catalog cannot be read.
    pub fn owned_tables(&self, registered_modules: &[&str]) -> Fallible<Vec<String>> {
        let guard = self.lease()?;
        let mut statement = guard
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .map_err(query_error)?;
        let names: Vec<String> = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(query_error)?
            .filter_map(std::result::Result::ok)
            .collect();
        drop(statement);
        drop(guard);
        Ok(names
            .into_iter()
            .filter(|name| {
                !crate::migrate::is_kernel_table(name)
                    && owner_of(name, registered_modules).is_none()
            })
            .collect())
    }

    /// Drop the named tables, each with `DROP TABLE IF EXISTS` on the writer.
    ///
    /// # Errors
    ///
    /// `sqlite.busy` when the writer stayed held for the whole gate
    /// timeout, `sqlite.query_failed` on the first statement that fails.
    pub fn drop_tables(&self, names: &[String]) -> Fallible {
        let _held = self.acquire_writer()?;
        let mut guard = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let transaction = guard.transaction().map_err(query_error)?;
        for name in names {
            if !valid_table_name(name) {
                return Err(Box::new(SqliteFailure::Query {
                    reason: format!("`{name}` is not a table name to drop"),
                }) as Box<dyn timbre_kernel::Failure>);
            }
            // Table names cannot be bound as parameters; the guard above
            // keeps this interpolation safe.
            transaction
                .execute_batch(&format!("DROP TABLE IF EXISTS \"{name}\""))
                .map_err(query_error)?;
        }
        transaction.commit().map_err(query_error)
    }
}

/// Reader pool size: the parallelism the machine offers, at least two.
fn readers_wanted() -> usize {
    std::thread::available_parallelism().map_or(2, |count| usize::from(count).max(2))
}

/// Arm one connection with the statement watchdog: a `SQLITE_TRACE_STMT`
/// callback records the deadline at the start of every statement, and a
/// progress callback interrupts the VM once that deadline has passed. The
/// deadline slot is per connection: SQLite invokes both callbacks on the
/// thread running the statement, and a connection serves one thread at a
/// time, so no lock is needed but a `Mutex` keeps the closure `Send`.
fn arm_watchdog(connection: &mut Connection, limit: Duration) -> rusqlite::Result<()> {
    let deadline = Arc::new(Mutex::new(None::<Instant>));
    let armed = Arc::clone(&deadline);
    connection.trace_v2(
        rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
        Some(move |event: rusqlite::trace::TraceEvent<'_>| {
            if matches!(event, rusqlite::trace::TraceEvent::Stmt(..)) {
                let mut slot = armed.lock().unwrap_or_else(PoisonError::into_inner);
                *slot = Some(Instant::now() + limit);
            }
        }),
    )?;
    let armed = deadline;
    connection.progress_handler(
        PROGRESS_EVERY_OPS,
        Some(move || {
            let mut slot = armed.lock().unwrap_or_else(PoisonError::into_inner);
            let expired = slot.is_some_and(|deadline| Instant::now() >= deadline);
            if expired {
                // Consume the deadline: SQLite also calls the progress
                // handler outside a statement, where a passed one left set
                // would interrupt the *next* statement before its `OP_Init`
                // trace can arm a fresh deadline.
                *slot = None;
            }
            expired
        }),
    )?;
    Ok(())
}

/// The module that owns `name`: the registered module whose `<module>_`
/// prefix is the longest one `name` starts with. `stream_v2_rows` belongs to
/// `stream_v2`, not to `stream`, when both are registered.
///
/// The xtask `migrations` check enforces at build time that a module only
/// ever creates tables prefixed with its own name; this is the runtime side
/// of that rule, and anything that matches no registered module is orphaned.
fn owner_of<'a>(name: &str, registered_modules: &[&'a str]) -> Option<&'a str> {
    registered_modules
        .iter()
        .copied()
        .filter(|module| {
            name.len() > module.len()
                && name.starts_with(module)
                && name.as_bytes().get(module.len()) == Some(&b'_')
        })
        .max_by_key(|module| module.len())
}

/// Whether `name` is plain enough to interpolate into `DROP TABLE`.
fn valid_table_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let first_ok = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte == b'_');
    first_ok && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
mod tests;
