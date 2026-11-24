# crates/timbre

## Purpose

The server binary: configuration, data dir, database, migrations, CLI
dispatch, runtimes. One startup path: config (argument, `TIMBRE_CONFIG`,
`./timbre.toml`), log filter (`TIMBRE_LOG` wins over `[log].filter`), the
registry with every module and the binary's own built-in commands, the
kernel, and the migrated database. `serve` runs on tokio
(`edge/runtime`); CLI commands on the kernel's `ThreadRuntime`, with their
async handlers driven by `block_on`. `serve` creates `[server].data_dir`
(mode 700 on Unix; a refused create or chmod is `timbre.data_dir_failed`),
opens the database (a file owned by another user is refused by
`edge/sqlite`, never trusted) and applies every registered migration before
`Started` is published; every other command but `help` opens and migrates
the same database on first use and shares the connection for the rest of
the run. Built-in commands (`serve`, `help`, `db check`, `db backup`,
`db prune`, and the bare `db` catch-all) are registered into the kernel
command table through `Registry::command` before `finish`, and dispatch
consults `Kernel::find_command` only: a module that registers e.g.
`db vacuum` coexists with them, a module that registers `db check` fails
registration with `kernel.command_taken` — a built-in is never silently
shadowed, and a module command is never hidden behind a hard-coded branch.
`help` and an unknown command write the usage line and the command table
to stdout through `Ctx::say`, so the list is there whatever the log filter
is set to.
On shutdown `serve` gives background jobs up to ten seconds to finish
(`Kernel::wait_jobs`), publishes `signals::ServerDrained` so their last
work reaches the database, and then logs `timbre.stopped`; the HTTP edge
has already returned from its own drain by then. The process exit is 0 on success, 2 on a usage error (an
unusable command line, an unknown verb, a backup destination that exists
without `--force`, a missing target, a wrong-positioned flag; each logged
as `timbre.usage` with `reason`), 1 on any other failure. `--force` and
`--yes` are accepted in any position of their command; each belongs to
exactly one of them (`--force` to `backup`, `--yes` to `prune`).

## Ports

None.

## Signals

Publishes `signals::Started` and `signals::ServerDrained`.

## Log events

- `timbre.started` (info): the server finished starting; `listen`.
- `timbre.stopped` (info): `serve` returned — accepting stopped and the
  connection drain finished. A drain that expired with responses still in
  flight is noticed only inside `edge/http` (its `serve` does not surface
  it), so no `timbre.drain_timeout` line exists yet; when the HTTP edge
  can report the expired drain, the binary logs it at warn here.
- `timbre.log_filter_invalid` (error): the filter did not parse; `value`,
  `code`.
- `timbre.registration_failed` (error): a module or the command table
  refused to register; `code`, `message`.
- `timbre.runtime_failed` (error): the async executor could not start;
  `code`, `message`, `reason`.
- `timbre.config_failed` (error): a section the startup reads is
  malformed; `code`, `message`.
- `timbre.database_failed` (error): the database could not be opened,
  protected or migrated; the `code` is the original failure's own — never
  relabeled as `sqlite.open_failed` — with its `message` and `reason`.
- `timbre.data_dir_permissive` (warn): the data directory was already
  there and other users may read it; `path`, `mode`. Unix only.
- `timbre.command_failed` (error): a command failed with a failure nobody
  logged yet; `code`, `message`.
- `timbre.usage` (error): the command line is unusable; `reason`. Exit 2.
- `timbre.db_check` (error): the integrity check failed; `ok=false` and the
  check's `code`. Successful `db` results go to stdout outside the log filter.

## Failures

- `timbre.runtime_failed` (`TimbreFailure::Runtime`): the async executor
  could not start; the OS reason is logged beside the code.
- `timbre.data_dir_failed` (`TimbreFailure::DataDir`): `[server].data_dir`
  could not be created or kept owner-only (mode 700 refused on Unix); the
  binary owns this step, so it is never reported as `sqlite.open_failed`.
- `timbre.usage` (`TimbreFailure::Usage`): a usage error the `db` handlers
  raised after their own `timbre.usage` line; the exit is 2 and no second
  line belongs to it.
- `kernel.usage` (`KernelFailure::Usage`): a module command's argument error;
  also reports `timbre.usage` and exits 2.
- `timbre.reported` (`TimbreFailure::Reported`): an outcome whose
  explanatory line the raising code already logged (a failed `db check`,
  a remembered database-open failure); the exit is 1 without a second
  `timbre.command_failed`.
