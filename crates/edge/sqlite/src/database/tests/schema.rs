use super::*;

#[test]
fn migrations_apply_once_and_a_second_run_changes_nothing() {
    let path = scratch("migrate");
    let database = Database::open(&path).expect("a fresh database opens");
    let steps = migrations(&[(
        "demo",
        1,
        "CREATE TABLE demo_rows (id INTEGER PRIMARY KEY); INSERT INTO demo_rows (id) VALUES (1)",
    )]);
    database
        .migrate(&quiet_ctx(), &steps)
        .expect("the first run applies");
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
    reopened
        .migrate(&quiet_ctx(), &steps)
        .expect("a second run is a no-op");
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
    let error = database
        .migrate(&quiet_ctx(), &steps)
        .expect_err("the step fails");
    assert_eq!(error.code(), crate::failures::MIGRATION_FAILED);
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
    database
        .migrate(&quiet_ctx(), &fixed)
        .expect("a fixed rerun applies");
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
fn a_module_name_with_an_underscore_owns_its_tables_by_longest_prefix() {
    let path = scratch("prune-prefix");
    let database = Database::open(&path).expect("a fresh database opens");
    database
        .write(|connection| {
            batch(
                connection,
                "CREATE TABLE stream_rows (id INTEGER PRIMARY KEY);
                 CREATE TABLE stream_v2_rows (id INTEGER PRIMARY KEY);
                 CREATE TABLE orphan_rows (id INTEGER PRIMARY KEY)",
            )
        })
        .expect("the tables exist");
    // `stream_v2_rows` starts with the `stream_` prefix of another registered
    // module, but the longest matching prefix wins: both modules own their
    // tables, and `orphan_rows` matches no module at all.
    let unowned = database
        .owned_tables(&["stream", "stream_v2"])
        .expect("the catalog reads");
    assert!(
        !unowned.contains(&"stream_rows".to_owned())
            && !unowned.contains(&"stream_v2_rows".to_owned()),
        "both modules own their tables: {unowned:?}"
    );
    assert!(
        unowned.contains(&"orphan_rows".to_owned()),
        "a table no module's `<module>_` prefix matches is a candidate: {unowned:?}"
    );
    // With only `stream_v2` registered, the old first-segment rule would
    // hand `stream_v2_rows` to the unregistered `stream` and prune it.
    let unowned = database
        .owned_tables(&["stream_v2"])
        .expect("the catalog reads");
    assert!(
        !unowned.contains(&"stream_v2_rows".to_owned()),
        "a longer module name still owns its table: {unowned:?}"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}
