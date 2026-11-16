use super::*;
use rusqlite::OptionalExtension;
use std::sync::atomic::{AtomicU32, Ordering};
use timbre_kernel::{Fallible, Migration};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "timbre-sqlite-test-{}-{tag}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&path).expect("the scratch directory exists");
    path.join("timbre.db")
}

// `Migration` is built by `Registry::migration`; this mirrors what the
// binary hands to `migrate`.
fn migrations(entries: &[(&'static str, u32, &'static str)]) -> Vec<Migration> {
    let mut registry = timbre_kernel::Registry::new();
    for (module, version, sql) in entries {
        registry
            .migration(module, *version, sql)
            .expect("the test migrations register");
    }
    let kernel = registry.finish(
        timbre_kernel::Logger::stdout(timbre_kernel::LogFilter::default()),
        std::sync::Arc::new(timbre_kernel::ThreadRuntime::new()),
    );
    kernel.migrations().to_vec()
}

fn table_exists(database: &Database, name: &str) -> bool {
    database
        .read(|connection| {
            let found: Option<String> = connection
                .query_row(
                    "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [name],
                    |row| row.get(0),
                )
                .optional()
                .map_err(crate::failures::query_error)?;
            Ok(found.is_some())
        })
        .expect("the read runs")
}

fn batch(connection: &rusqlite::Connection, sql: &str) -> Fallible {
    connection
        .execute_batch(sql)
        .map_err(crate::failures::query_error)
}

#[test]
fn migrations_apply_once_and_a_second_run_changes_nothing() {
    let path = scratch("migrate");
    let database = Database::open(&path).expect("a fresh database opens");
    let steps = migrations(&[(
        "demo",
        1,
        "CREATE TABLE demo_rows (id INTEGER PRIMARY KEY); INSERT INTO demo_rows (id) VALUES (1)",
    )]);
    database.migrate(&steps).expect("the first run applies");
    let count = |database: &Database| -> i64 {
        database
            .read(|connection| {
                let n: i64 = connection
                    .query_row("SELECT count(*) FROM demo_rows", [], |row| row.get(0))
                    .map_err(crate::failures::query_error)?;
                Ok(n)
            })
            .expect("the read runs")
    };
    assert_eq!(count(&database), 1);
    // Re-opening and re-migrating must not re-run the step nor fail.
    drop(database);
    let reopened = Database::open(&path).expect("the database reopens");
    reopened.migrate(&steps).expect("a second run is a no-op");
    assert_eq!(count(&reopened), 1, "the step did not run twice");
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}

#[test]
fn a_failing_migration_rolls_back_its_own_module_and_keeps_the_others() {
    let path = scratch("rollback");
    let database = Database::open(&path).expect("a fresh database opens");
    let steps = migrations(&[
        (
            "keeper",
            1,
            "CREATE TABLE keeper_rows (id INTEGER PRIMARY KEY)",
        ),
        (
            "breaker",
            1,
            "CREATE TABLE breaker_rows (id INTEGER PRIMARY KEY)",
        ),
        (
            "breaker",
            2,
            "INSERT INTO breaker_rows (id) VALUES (1); CREATE TABLE breaker_bad (id INTEGER) REFERENCES nowhere_here (x)",
        ),
    ]);
    let error = database.migrate(&steps).expect_err("the step fails");
    assert_eq!(error.code(), crate::MIGRATION_FAILED);
    assert!(
        table_exists(&database, "keeper_rows"),
        "the other module landed"
    );
    assert!(
        !table_exists(&database, "breaker_rows"),
        "the failed module rolled back whole"
    );
    // The rolled-back versions are not recorded: a fixed rerun applies them.
    let fixed = migrations(&[(
        "breaker",
        1,
        "CREATE TABLE breaker_rows (id INTEGER PRIMARY KEY)",
    )]);
    database.migrate(&fixed).expect("a fixed rerun applies");
    assert!(table_exists(&database, "breaker_rows"));
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}

#[test]
fn prune_lists_only_tables_no_registered_module_owns() {
    let path = scratch("prune");
    let database = Database::open(&path).expect("a fresh database opens");
    database
        .write(|connection| {
            batch(
                connection,
                "CREATE TABLE hello_rows (id INTEGER PRIMARY KEY);
                 CREATE TABLE gone_rows (id INTEGER PRIMARY KEY);
                 CREATE TABLE oddname (id INTEGER PRIMARY KEY)",
            )
        })
        .expect("the tables exist");
    let unowned = database
        .owned_tables(&["hello"])
        .expect("the catalog reads");
    assert!(
        unowned.contains(&"gone_rows".to_owned()) && unowned.contains(&"oddname".to_owned()),
        "unowned tables are listed: {unowned:?}"
    );
    assert!(
        !unowned.contains(&"hello_rows".to_owned())
            && !unowned.contains(&"kernel_migrations".to_owned()),
        "owned and kernel tables stay: {unowned:?}"
    );
    database
        .drop_tables(&["gone_rows".to_owned()])
        .expect("the drop runs");
    assert!(!table_exists(&database, "gone_rows"));
    assert!(
        table_exists(&database, "oddname"),
        "only the named table went"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}

#[test]
fn a_backup_is_an_independent_working_database() {
    let path = scratch("backup");
    let database = Database::open(&path).expect("a fresh database opens");
    let target = path.parent().expect("the scratch dir").join("backup.db");
    database
        .write(|connection| {
            batch(
                connection,
                "CREATE TABLE demo_rows (id INTEGER PRIMARY KEY); INSERT INTO demo_rows (id) VALUES (7)",
            )
        })
        .expect("the row lands");
    database.backup(&target).expect("the backup runs");
    drop(database);
    let copy = Database::open(&target).expect("the copy opens");
    copy.check().expect("the copy passes integrity");
    let value: i64 = copy
        .read(|connection| {
            connection
                .query_row("SELECT id FROM demo_rows", [], |row| row.get(0))
                .map_err(crate::failures::query_error)
        })
        .expect("the copy reads");
    assert_eq!(value, 7, "the backup carries the data");
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}
