# crates/edge/sqlite

## Purpose

The only place SQLite is spoken: `Database::open` (WAL, 5 s busy timeout,
foreign keys on, the database file and its `-wal`/`-shm` sidecars mode 0600
on Unix, and on Unix a database file owned by a different user is refused
by `sqlite.open_failed` rather than trusted), one writer connection behind
a mutex and a pool of reader connections, blocking `write`/`read` calls for
`Ctx::blocking`, a 30 s progress-handler cap on any single statement —
the watchdog is per statement, not per transaction: a transaction may run
many statements, and each one individually gets the 30 s; a transaction
made of 100 quick statements legitimately runs for minutes. Every wait for
a connection is bounded at 5 s and answers `sqlite.busy`: the reader pool
through its lease timeout, the writer through a timed gate (`gate.rs`) that
`write`, `migrate`, `backup` and `drop_tables` pass through, so a long
transaction turns waiting writers into busy failures instead of an
unbounded queue. The migration runner (`migrate` under `BEGIN EXCLUSIVE`,
all-or-nothing per module) logs one `sqlite.migration_applied` info line
(module, version) per step, only after the run's `COMMIT` landed. The
operator commands: `check` (integrity), `backup` (online backup API;
refuses an existing destination without `force`, refuses a destination
that is the live database file itself, creates the destination exclusively
with mode 0600 on Unix before the first copied byte — so a raced symlink
fails closed instead of being followed — and deletes the partial file when
the copy fails) and `owned_tables`/`drop_tables` (what `timbre db prune`
lists and drops; a table belongs to the registered module with the longest
`<module>_` prefix of its name).

## Ports

None. Modules depend on this edge crate directly, per the phase 1 rule that
storage is an edge API, not a kernel port.

## Signals

None.

## Log events

- `sqlite.migration_applied` (`MIGRATION_APPLIED`, info): one migration
  step was applied; fields `module`, `version`. Written only after the
  whole run committed, so the lines and the `kernel_migrations` rows
  always agree.

## Failures

- `sqlite.open_failed` (`SqliteFailure::Open`): the database file could not
  be opened or configured, is owned by another user (Unix), or a backup
  destination refused, named the live database, or failed.
- `sqlite.query_failed` (`SqliteFailure::Query`): a statement or transaction
  failed.
- `sqlite.busy` (`SqliteFailure::Busy`): no reader connection was free for
  the whole 5 s lease wait, or the writer stayed held for the whole 5 s
  gate wait.
- `sqlite.query_timeout` (`SqliteFailure::QueryTimeout`): one statement ran
  past the per-statement watchdog and was interrupted.
- `sqlite.migration_failed` (`SqliteFailure::Migration`): a migration step
  failed; its module's earlier state was left untouched.

Status codes: `sqlite.busy` answers 503 (Service Unavailable: the pressure
is temporary, a retry may succeed) and `sqlite.query_timeout` answers 504
(Gateway Timeout: the watchdog cut the statement); every other failure
here is a 500 server fault.

