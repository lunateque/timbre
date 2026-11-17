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

/// A root context on a default logger: `migrate` logs its applied steps
/// and needs a `Ctx`, but these tests assert on the data, not the lines
/// (one dedicated test reads the sink).
fn quiet_ctx() -> timbre_kernel::Ctx {
    let kernel = timbre_kernel::Registry::new().finish(
        timbre_kernel::Logger::stdout(timbre_kernel::LogFilter::default()),
        std::sync::Arc::new(timbre_kernel::ThreadRuntime::new()),
    );
    timbre_kernel::Ctx::root(&kernel)
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

/// A logger writing JSON into a shared buffer, plus the context on it.
#[derive(Clone, Default)]
struct Sink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl Sink {
    fn text(&self) -> String {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        String::from_utf8(bytes).unwrap_or_default()
    }
}

impl std::io::Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capturing() -> (timbre_kernel::Ctx, Sink) {
    let sink = Sink::default();
    let kernel = timbre_kernel::Registry::new().finish(
        timbre_kernel::Logger::new(
            timbre_kernel::LogFilter::default(),
            timbre_kernel::Format::Json,
            Box::new(sink.clone()),
            std::time::SystemTime::now,
        ),
        std::sync::Arc::new(timbre_kernel::ThreadRuntime::new()),
    );
    (timbre_kernel::Ctx::root(&kernel), sink)
}

#[test]
fn an_applied_migration_step_is_logged_once_after_the_commit() {
    let path = scratch("migration-log");
    let database = Database::open(&path).expect("a fresh database opens");
    let (ctx, sink) = capturing();
    let steps = migrations(&[(
        "logged",
        1,
        "CREATE TABLE logged_rows (id INTEGER PRIMARY KEY)",
    )]);
    database.migrate(&ctx, &steps).expect("the step applies");
    ctx.kernel().flush_log();
    let first = sink.text();
    let lines: Vec<&str> = first
        .lines()
        .filter(|line| line.contains(r#""event":"sqlite.migration_applied""#))
        .collect();
    assert_eq!(lines.len(), 1, "one line per applied step: {first}");
    assert!(
        lines[0].contains(r#""module":"logged""#) && lines[0].contains(r#""version":1"#),
        "the line names module and version: {}",
        lines[0]
    );
    // A rerun applies nothing and must not claim it did.
    database
        .migrate(&ctx, &steps)
        .expect("the second run is a no-op");
    ctx.kernel().flush_log();
    assert_eq!(
        sink.text()
            .lines()
            .filter(|line| line.contains(r#""event":"sqlite.migration_applied""#))
            .count(),
        1,
        "the second run logs nothing new"
    );
    // A failing module's steps were rolled back: they never appear.
    let broken = migrations(&[(
        "breaker",
        9,
        "CREATE TABLE breaker_ok (id INTEGER); CREATE TABLE breaker_ok (id INTEGER)",
    )]);
    database.migrate(&ctx, &broken).expect_err("the step fails");
    ctx.kernel().flush_log();
    assert!(
        !sink.text().contains(r#""module":"breaker""#),
        "a rolled-back step is not logged as applied"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}

#[test]
fn busy_answers_503_and_query_timeout_504() {
    // The status is what a caller on the wire sees; a retryable pressure
    // signal (503) and a watchdog cut (504) must not blend into 500.
    use timbre_kernel::Failure;
    assert_eq!(crate::failures::SqliteFailure::Busy.status(), 503);
    assert_eq!(crate::failures::SqliteFailure::QueryTimeout.status(), 504);
}

mod backup;
mod connections;
mod schema;
