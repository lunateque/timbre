use super::*;

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
    database.backup(&target, false).expect("the backup runs");
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

#[test]
fn a_backup_refuses_an_existing_destination_unless_forced() {
    let path = scratch("backup-exists");
    let database = Database::open(&path).expect("a fresh database opens");
    let target = path.parent().expect("the scratch dir").join("backup.db");
    std::fs::write(&target, "keep me").expect("the destination file exists");
    let error = database
        .backup(&target, false)
        .expect_err("an existing file is refused");
    assert_eq!(error.code(), crate::failures::OPEN_FAILED);
    assert_eq!(
        std::fs::read_to_string(&target).expect("the file still reads"),
        "keep me",
        "the refused backup leaves the file alone"
    );
    database
        .backup(&target, true)
        .expect("--force replaces the file");
    let content = std::fs::read(&target).expect("the backup wrote something");
    assert!(
        content.starts_with(b"SQLite format 3"),
        "the destination is a database now"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}

#[test]
#[cfg(unix)]
fn opened_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let path = scratch("perms");
    let database = Database::open(&path).expect("a fresh database opens");
    // Force the -wal and -shm sidecars into existence, as the first write does.
    database
        .write(|connection| {
            batch(
                connection,
                "CREATE TABLE demo_rows (id INTEGER PRIMARY KEY)",
            )
        })
        .expect("a write runs");
    let mode_of = |target: &std::path::Path| -> u32 {
        std::fs::metadata(target)
            .unwrap_or_else(|error| panic!("{}: {error}", target.display()))
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode_of(&path), 0o600, "the database file is owner-only");
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    assert_eq!(
        mode_of(std::path::Path::new(&wal)),
        0o600,
        "the -wal is owner-only"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}

#[test]
#[cfg(unix)]
fn a_forced_backup_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let path = scratch("backup-perms");
    let database = Database::open(&path).expect("a fresh database opens");
    let target = path.parent().expect("the scratch dir").join("backup.db");
    database.backup(&target, false).expect("the backup runs");
    let mode = std::fs::metadata(&target)
        .expect("the backup exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the copy is owner-only");
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}

#[test]
fn a_backup_to_the_live_file_itself_is_refused() {
    let path = scratch("backup-self");
    let database = Database::open(&path).expect("a fresh database opens");
    let error = database
        .backup(&path, true)
        .expect_err("the live file is not a destination");
    assert_eq!(error.code(), crate::failures::OPEN_FAILED);
    // `--force` must not have deleted the live database on the way out.
    drop(database);
    let reopened = Database::open(&path).expect("the live database survives the refusal");
    reopened.check().expect("and is intact");
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}

#[test]
#[cfg(unix)]
fn a_backup_destination_is_owner_only_from_its_first_byte_and_never_follows_a_symlink() {
    use std::os::unix::fs::PermissionsExt;
    let path = scratch("backup-exclusive");
    let database = Database::open(&path).expect("a fresh database opens");
    let target = path.parent().expect("the scratch dir").join("backup.db");
    database.backup(&target, false).expect("the backup runs");
    let mode = std::fs::metadata(&target)
        .expect("the backup exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "create_new wrote the file private from the start"
    );
    // A symlinked destination under `--force` is replaced by a real file,
    // never written through the link: the link's target stays untouched.
    let outside = path.parent().expect("the scratch dir").join("outside.txt");
    std::fs::write(&outside, "do not touch me").expect("the target exists");
    std::fs::remove_file(&target).expect("the old backup goes");
    std::os::unix::fs::symlink(&outside, &target).expect("the symlink is planted");
    database.backup(&target, true).expect("--force runs");
    assert_eq!(
        std::fs::read_to_string(&outside).expect("the outside file reads"),
        "do not touch me",
        "the copy never went through the symlink"
    );
    assert!(
        std::fs::symlink_metadata(&target)
            .expect("the destination exists")
            .file_type()
            .is_file(),
        "the destination is a real file now"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}

#[test]
#[cfg(unix)]
fn a_failed_backup_deletes_the_partial_destination() {
    let path = scratch("backup-partial");
    let database = Database::open(&path).expect("a fresh database opens");
    database
        .write(|connection| {
            connection
                .execute_batch("CREATE TABLE demo_rows (id INTEGER PRIMARY KEY)")
                .map_err(crate::failures::query_error)
        })
        .expect("the row lands");
    let target = path.parent().expect("the scratch dir").join("backup.db");
    // A directory sitting at `backup.db-journal` lets the exclusive create
    // of the main file succeed but kills the copy itself (SQLite cannot
    // open its rollback journal): the cleanup must then remove the
    // half-written main file, not leave a bogus `.db` behind.
    std::fs::create_dir(format!("{}-journal", target.display()))
        .expect("the -journal directory exists");
    let error = database
        .backup(&target, false)
        .expect_err("the copy cannot run with the journal blocked");
    assert!(
        error.code() == crate::failures::OPEN_FAILED
            || error.code() == crate::failures::QUERY_FAILED,
        "the failure names the blocked copy, got {}",
        error.code().as_str()
    );
    assert!(
        std::fs::symlink_metadata(&target).is_err(),
        "the partial destination file is gone"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}

/// Under `--force` the old destination is removed only after the writer
/// gate is held: a busy timeout leaves the previous backup exactly as it
/// was, instead of destroying it without writing a new one. The removal
/// itself deletes only the link, so a pre-existing symlink is replaced by
/// a real file and never followed (asserted in the symlink test above).
#[test]
fn a_busy_forced_backup_keeps_the_old_destination() {
    let path = scratch("backup-busy-force");
    let database = Database::open_with(&path, std::time::Duration::from_secs(60), 1)
        .expect("a fresh database opens");
    let target = path.parent().expect("the scratch dir").join("backup.db");
    std::fs::write(&target, "old backup").expect("the destination exists");
    let held = database.acquire_writer().expect("the writer is free");
    let error = database
        .backup(&target, true)
        .expect_err("the writer is held");
    assert_eq!(error.code(), crate::failures::BUSY);
    assert_eq!(
        std::fs::read_to_string(&target).expect("the destination still reads"),
        "old backup",
        "the busy timeout must not have removed the old destination"
    );
    drop(held);
    database
        .backup(&target, true)
        .expect("--force replaces the file once the writer frees");
    let content = std::fs::read(&target).expect("the backup wrote something");
    assert!(
        content.starts_with(b"SQLite format 3"),
        "the destination is a database now"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}

#[test]
fn a_held_writer_times_out_with_busy_like_the_reader_lease() {
    let path = scratch("writer-busy");
    let database = Database::open_with(&path, std::time::Duration::from_secs(60), 1)
        .expect("a fresh database opens");
    // Hold the gate exactly as a long transaction would; `backup` and
    // `migrate` must fail with `sqlite.busy` rather than wait forever.
    let held = database.acquire_writer().expect("the writer is free");
    let started = std::time::Instant::now();
    let error = database
        .backup(
            &path.parent().expect("the scratch dir").join("backup.db"),
            false,
        )
        .expect_err("the writer is held");
    assert_eq!(error.code(), crate::failures::BUSY);
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(500)
            && started.elapsed() < std::time::Duration::from_secs(10),
        "the wait lasts the gate timeout: {:?}",
        started.elapsed()
    );
    drop(held);
    let ctx = quiet_ctx();
    database
        .migrate(
            &ctx,
            &migrations(&[("free", 1, "CREATE TABLE free_rows (id INTEGER)")]),
        )
        .expect("the writer serves again once the gate is released");
    let _ = std::fs::remove_dir_all(path.parent().expect("the scratch dir"));
}
