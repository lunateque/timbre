# crates/edge/sqlite

## Purpose

The only place SQLite is spoken: `Database::open` (WAL, 5 s busy timeout,
foreign keys on, the database file and its `-wal`/`-shm` sidecars mode 0600
on Unix, and on Unix a database file owned by a different user is refused
by `sqlite.open_failed` rather than trusted), one writer connection behind
a mutex and a pool of reader connections opened `SQLITE_OPEN_READ_ONLY`
(a query cannot change stored data, whatever it says), blocking
`write`/`read`/`read_transaction` calls for `Ctx::blocking`. Related queries
in `read_transaction` see one snapshot. A 30 s progress-handler cap
applies to any single statement —
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
fails closed instead of being followed — opens it for the copy with
`SQLITE_OPEN_NOFOLLOW` (a symlink swapped in after the probe is refused at
open, not written through) and deletes the partial file when
the copy fails) and `owned_tables`/`drop_tables` (what `timbre db prune`
lists and drops; a table belongs to the registered module with the longest
`<module>_` prefix of its name).

`SharedDatabase` opens and migrates the host's one database on first use,
one caller at a time, the others waiting at most 5 s before `sqlite.busy`.
A failed attempt is reported to its caller only and never remembered: the
next `get` prepares, opens and migrates again.
Use `SharedDatabase::require` to look up the host resource without opening
it, supplying the owning module's failure for a missing resource.

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
- `sqlite.json_failed` (`SqliteFailure::Json`): a JSON column could not be
  encoded, or a stored one has the wrong shape.
- `sqlite.busy` (`SqliteFailure::Busy`): no reader connection was free for
  the whole 5 s lease wait, the writer stayed held for the whole 5 s gate
  wait, or another caller's `SharedDatabase` open did not finish in 5 s.
- `sqlite.query_timeout` (`SqliteFailure::QueryTimeout`): one statement ran
  past the per-statement watchdog and was interrupted.
- `sqlite.migration_failed` (`SqliteFailure::Migration`): a migration step
  failed; its module's earlier state was left untouched.

Status codes: `sqlite.busy` answers 503 (Service Unavailable: the pressure
is temporary, a retry may succeed) and `sqlite.query_timeout` answers 504
(Gateway Timeout: the watchdog cut the statement); every other failure
here is a 500 server fault.

