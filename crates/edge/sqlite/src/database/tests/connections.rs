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
