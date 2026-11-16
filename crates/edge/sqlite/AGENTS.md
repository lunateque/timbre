# crates/edge/sqlite

## Purpose

The only place SQLite is spoken: `Database::open` (WAL, 5 s busy timeout,
foreign keys on), one writer connection behind a mutex and a pool of reader
connections, blocking `write`/`read` calls for `Ctx::blocking`, the
migration runner (`migrate` under `BEGIN EXCLUSIVE`, all-or-nothing per
module), and the operator commands `check` (integrity), `backup` (online
backup API) and `owned_tables`/`drop_tables` (what `timbre db prune` lists
and drops).

## Ports

None. Modules depend on this edge crate directly, per the phase 1 rule that
storage is an edge API, not a kernel port.

## Signals

None.

## Failures

- `sqlite.open_failed` (`SqliteFailure::Open`): the database file could not
  be opened or configured.
- `sqlite.query_failed` (`SqliteFailure::Query`): a statement or transaction
  failed.
- `sqlite.migration_failed` (`SqliteFailure::Migration`): a migration step
  failed; its module's earlier state was left untouched.
