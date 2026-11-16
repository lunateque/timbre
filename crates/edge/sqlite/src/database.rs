//! The connections: one writer, a pool of readers, blocking calls.

use crate::failures::{SqliteFailure, open_error, query_error};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use timbre_kernel::Fallible;

/// An open database with the access pattern phase 1 needs: one writer, several
/// readers, every call blocking (wrap it in `Ctx::blocking` from async code).
pub struct Database {
    pub(crate) writer: Arc<Mutex<Connection>>,
    readers: Arc<Pool>,
    path: std::path::PathBuf,
}

impl Database {
    /// Open (creating if needed) the database at `path` with WAL journaling,
    /// a five-second busy timeout and foreign keys on, and fill the reader
    /// pool with one connection per available parallel core (at least two).
    ///
    /// # Errors
    ///
    /// `sqlite.open_failed` when the file cannot be opened or one of these
    /// settings cannot be applied.
    pub fn open(path: &Path) -> Fallible<Database> {
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
        let writer = open()?;
        configure(&writer)?;
        let mut pool = Vec::new();
        for _ in 0..readers_wanted() {
            let reader = open()?;
            configure(&reader)?;
            pool.push(reader);
        }
        Ok(Database {
            writer: Arc::new(Mutex::new(writer)),
            readers: Arc::new(Pool {
                idle: Mutex::new(pool),
                available: Condvar::new(),
            }),
            path: path.to_path_buf(),
        })
    }

    /// The file this database was opened from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run `work` on the writer inside one transaction: it commits when
    /// `work` returns `Ok` and rolls back completely when it fails.
    ///
    /// # Errors
    ///
    /// Whatever `work` returns, or `sqlite.query_failed` when the transaction
    /// cannot be started or committed.
    pub fn write<T>(&self, work: impl FnOnce(&Connection) -> Fallible<T>) -> Fallible<T> {
        let mut guard = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        // A poisoned mutex means an earlier writer panicked; the connection
        // itself is still usable (SQLite never leaves a half-written frame),
        // so recovering keeps one bad command from killing the process.
        let transaction = guard.transaction().map_err(query_error)?;
        let value = work(&transaction)?;
        transaction.commit().map_err(query_error)?;
        Ok(value)
    }

    /// Run `work` on a pooled reader connection.
    ///
    /// # Errors
    ///
    /// Whatever `work` returns, or `sqlite.query_failed` when the connection
    /// could not be obtained or the statement failed.
    pub fn read<T>(&self, work: impl FnOnce(&Connection) -> Fallible<T>) -> Fallible<T> {
        let guard = self.readers.take();
        let value = work(&guard);
        drop(guard);
        value
    }

    /// `PRAGMA integrity_check`: `Ok(())` when the database reports `ok`.
    ///
    /// # Errors
    ///
    /// `sqlite.query_failed` when the pragma fails or reports problems; the
    /// reason carries every reported line.
    pub fn check(&self) -> Fallible {
        let guard = self.readers.take();
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

    /// Copy the whole database to `to` with SQLite's online backup API; the
    /// destination is replaced when it exists.
    ///
    /// # Errors
    ///
    /// `sqlite.open_failed` for the destination, `sqlite.query_failed` for a
    /// failed backup step.
    pub fn backup(&self, to: &Path) -> Fallible {
        let writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let mut destination = Connection::open(to).map_err(open_error)?;
        let backup =
            rusqlite::backup::Backup::new(&writer, &mut destination).map_err(query_error)?;
        // 100 pages per step with no pause: a CLI backup may take its time,
        // and `run_backup` propagates any step failure.
        backup
            .run_to_completion(100, std::time::Duration::ZERO, None)
            .map_err(query_error)
    }

    /// Every user table whose name does not start with one of the prefixes of
    /// `registered_modules` (plus the kernel's own bookkeeping table, which
    /// belongs to no module).
    ///
    /// # Errors
    ///
    /// `sqlite.query_failed` when the catalog cannot be read.
    pub fn owned_tables(&self, registered_modules: &[&str]) -> Fallible<Vec<String>> {
        let guard = self.readers.take();
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
                    && !registered_modules
                        .iter()
                        .any(|module| table_module(name).is_some_and(|owner| owner == *module))
            })
            .collect())
    }

    /// Drop the named tables, each with `DROP TABLE IF EXISTS` on the writer.
    ///
    /// # Errors
    ///
    /// `sqlite.query_failed` on the first statement that fails.
    pub fn drop_tables(&self, names: &[String]) -> Fallible {
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

/// The module a prefixed table name belongs to, when it has a `_<module>` shape.
fn table_module(name: &str) -> Option<&str> {
    // The kernel check enforces `<module>_<rest>`; recognise exactly that.
    name.split_once('_').map(|(module, _)| module)
}

/// Whether `name` is plain enough to interpolate into `DROP TABLE`.
fn valid_table_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let first_ok = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte == b'_');
    first_ok && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// The idle reader connections; a caller that takes one puts it back on drop.
struct Pool {
    idle: Mutex<Vec<Connection>>,
    available: Condvar,
}

impl Pool {
    fn take(&self) -> Lease<'_> {
        let mut idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(connection) = idle.pop() {
                return Lease {
                    connection: Some(connection),
                    pool: self,
                };
            }
            idle = self
                .available
                .wait(idle)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// One borrowed reader; returns its connection to the pool.
struct Lease<'a> {
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

fn unreachable_with(why: &str) -> ! {
    // A broken invariant here is a logic error in this file, not bad data:
    // abort is the only way out that neither panics nor prints in a library.
    let _ = why;
    std::process::abort()
}

#[cfg(test)]
#[path = "database/tests.rs"]
mod tests;
