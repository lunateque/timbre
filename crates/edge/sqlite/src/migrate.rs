//! The migration runner: `kernel_migrations` plus exclusive, all-or-nothing
//! per module application of the registered steps.

use crate::database::Database;
use crate::failures::{SqliteFailure, query_error};
use timbre_kernel::{Failure, Fallible, Migration};

/// The bookkeeping table; it belongs to the kernel, not to any module.
pub(crate) const TABLE: &str = "kernel_migrations";

/// Whether `name` is kernel-owned infrastructure excluded from pruning.
pub(crate) fn is_kernel_table(name: &str) -> bool {
    name == TABLE
}

impl Database {
    /// Apply the pending steps of `migrations` under `BEGIN EXCLUSIVE`, in
    /// the given order (the kernel hands them grouped by module, ascending by
    /// version). Each module applies inside its own savepoint: a failing step
    /// rolls that module back to where it started while the modules that
    /// succeeded still commit, and a module whose every version is recorded
    /// in `kernel_migrations` is skipped without touching its SQL.
    ///
    /// Returns the first module's failure after every module has been tried;
    /// version rows are written in the same savepoint as the SQL, so a rolled
    /// back module leaves nothing behind.
    ///
    /// # Errors
    ///
    /// `sqlite.migration_failed` when a step fails, `sqlite.query_failed`
    /// when the transaction or the bookkeeping cannot be maintained.
    pub fn migrate(&self, migrations: &[Migration]) -> Fallible {
        let mut connection = self
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        connection
            .execute_batch("BEGIN EXCLUSIVE")
            .map_err(query_error)?;
        let failure = apply(&mut connection, migrations);
        // The successful modules commit whatever their savepoints left; the
        // transaction is closed here, never left dangling on an error.
        let committed = connection.execute_batch("COMMIT").map_err(query_error);
        match (failure, committed) {
            (None, Ok(())) => Ok(()),
            (Some(failure), _) => Err(failure),
            (None, Err(error)) => Err(error),
        }
    }
}

/// Apply every module group, keeping the first failure.
fn apply(
    connection: &mut rusqlite::Connection,
    migrations: &[Migration],
) -> Option<Box<dyn Failure>> {
    let created = connection.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {TABLE} (
             module TEXT NOT NULL,
             version INTEGER NOT NULL,
             applied_at TEXT NOT NULL DEFAULT (datetime('now')),
             PRIMARY KEY (module, version)
         ) WITHOUT ROWID"
    ));
    if let Err(error) = created {
        return Some(query_error(error));
    }
    let mut first_error: Option<Box<dyn Failure>> = None;
    // The kernel groups a module's steps contiguously, so consecutive
    // same-module runs are exactly the per-module application order.
    for group in migrations.chunk_by(|left, right| left.module() == right.module()) {
        let Some(module) = group.first().map(Migration::module) else {
            continue;
        };
        if let Err(failure) = apply_module(connection, module, group)
            && first_error.is_none()
        {
            first_error = Some(failure);
        }
    }
    first_error
}

fn apply_module(
    connection: &mut rusqlite::Connection,
    module: &str,
    steps: &[Migration],
) -> Fallible {
    let savepoint = format!("m_{module}");
    connection
        .execute_batch(&format!("SAVEPOINT {savepoint}"))
        .map_err(query_error)?;
    for step in steps {
        let applied: Option<i64> = match connection.query_row(
            &format!("SELECT version FROM {TABLE} WHERE module = ?1 AND version = ?2"),
            rusqlite::params![module, i64::from(step.version())],
            |row| row.get(0),
        ) {
            Ok(version) => Some(version),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(error) => {
                release(connection, &savepoint, true);
                return Err(query_error(error));
            }
        };
        if applied.is_some() {
            continue;
        }
        if let Err(error) = connection.execute_batch(step.sql()) {
            let reason = error.to_string();
            release(connection, &savepoint, true);
            return Err(Box::new(SqliteFailure::Migration {
                module: step.module(),
                version: step.version(),
                reason,
            }));
        }
        if let Err(error) = connection.execute(
            &format!("INSERT INTO {TABLE} (module, version) VALUES (?1, ?2)"),
            rusqlite::params![module, i64::from(step.version())],
        ) {
            release(connection, &savepoint, true);
            return Err(query_error(error));
        }
    }
    release(connection, &savepoint, false);
    Ok(())
}

/// Close a module's savepoint: rolled back to its start on failure (the
/// rollback stays inside the outer transaction), released on success.
fn release(connection: &mut rusqlite::Connection, savepoint: &str, failed: bool) {
    if failed {
        // Failing to roll back leaves the outer transaction intact; the
        // COMMIT that follows either succeeds with it or reports its own
        // error, so nothing is silently lost.
        let _ = connection.execute_batch(&format!("ROLLBACK TO {savepoint}"));
    }
    let _ = connection.execute_batch(&format!("RELEASE {savepoint}"));
}
