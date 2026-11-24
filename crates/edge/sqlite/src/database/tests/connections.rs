use super::*;

#[test]
fn a_third_reader_on_a_pool_of_two_times_out_with_busy() {
    let path = scratch("lease-busy");
    // Two readers, a generous statement timeout so only the lease can fail.
    let database = Database::open_with(&path, std::time::Duration::from_secs(60), 2)
        .expect("a fresh database opens");
    let first = database.readers.take().expect("one reader is free");
    let second = database.readers.take().expect("the second reader is free");
    let started = std::time::Instant::now();
    let error = database
        .read(|_| Ok(()))
        .expect_err("no reader is left for the third caller");
    let waited = started.elapsed();
    assert_eq!(
        error.code(),
        crate::failures::BUSY,
        "the exhaustion is `sqlite.busy`"
    );
    // The lease timeout is five seconds; the caller must not hang forever nor
    // fail instantly.
    assert!(
        waited >= std::time::Duration::from_millis(500)
            && waited < std::time::Duration::from_secs(10),
        "the wait lasts the lease timeout: {waited:?}"
    );
    drop(first);
    drop(second);
    database
        .read(|_| Ok(()))
        .expect("the read works once a lease returns");
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}

#[test]
fn a_long_statement_is_interrupted_by_the_watchdog() {
    let path = scratch("watchdog");
    // A one-second cap: the recursive CTE below runs far longer on every
    // machine the tests execute on, and much less than the whole suite.
    let database = Database::open_with(&path, std::time::Duration::from_secs(1), 1).expect("opens");
    let started = std::time::Instant::now();
    let error = database
        .read(|connection| {
            let mut statement = connection
                .prepare(
                    "WITH RECURSIVE count_to(n) AS (
                         SELECT 1 UNION ALL SELECT n + 1 FROM count_to WHERE n < 20000000
                     )
                     SELECT count(*) FROM (SELECT 1 FROM count_to, count_to AS more)",
                )
                .map_err(crate::failures::query_error)?;
            let mut rows = statement.query([]).map_err(crate::failures::query_error)?;
            while let Some(row) = rows.next().map_err(crate::failures::query_error)? {
                let _: i64 = row.get(0).map_err(crate::failures::query_error)?;
            }
            Ok(())
        })
        .expect_err("the statement is interrupted");
    assert_eq!(
        error.code(),
        crate::failures::QUERY_TIMEOUT,
        "`sqlite.query_timeout`"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "the cap holds, the run does not finish on its own"
    );
    // The connection survives its interrupted statement.
    database
        .read(|connection| {
            let one: i64 = connection
                .query_row("SELECT 1", [], |row| row.get(0))
                .expect("a small query works again");
            assert_eq!(one, 1);
            Ok(())
        })
        .expect("the reader still serves");
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}

/// The data directory may be an operator-made directory with a loose mode,
/// and the sidecars appear only on the first write: the database and both
/// of them must still be unreadable to other local users.
#[cfg(unix)]
#[test]
fn the_database_and_its_sidecars_are_owner_only_in_a_permissive_directory() {
    use std::os::unix::fs::PermissionsExt;

    let path = scratch("modes");
    let dir = path.parent().expect("the scratch dir").to_path_buf();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777))
        .expect("the data directory is made world-writable");
    let database = Database::open(&path).expect("a fresh database opens");
    database
        .write(|connection| {
            batch(
                connection,
                "CREATE TABLE rows_here (id INTEGER PRIMARY KEY); INSERT INTO rows_here VALUES (1)",
            )
        })
        .expect("the write runs");
    for suffix in ["", "-wal", "-shm"] {
        let file = crate::files::with_suffix(&path, suffix);
        let mode = std::fs::metadata(&file)
            .unwrap_or_else(|error| panic!("{} exists: {error}", file.display()))
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "{} is mode {mode:o}", file.display());
    }
    drop(database);
    let _ = std::fs::remove_dir_all(&dir);
}
