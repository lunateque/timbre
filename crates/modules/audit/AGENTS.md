# timbre-audit

## Purpose

The security journal: who was refused, who was let in and refused a
permission, and every change to users, their permissions and their devices.
Rows live in `audit_events` and are read back with `timbre audit`.

Server handlers queue events and a job writes them every few seconds. The
server drains once more after all request handlers have finished or been
cancelled and jobs have had time to stop, whether serving ended cleanly or
with a failure. Standalone CLI work drains
inline; it has no background jobs. The read command writes to stdout,
independent of log filtering.
The final `ServerDrained` pass follows completion or cancellation of tracked
HTTP request futures and a bounded wait for jobs. Work detached from those
futures, including blocking work whose awaiter was cancelled, is not awaited
and can enqueue a record after the final pass.

Identical events queued between writes share a row with an occurrence
`count`. The queue is bounded (`queue_capacity`, at least 2 and 4096 by
default), with a quarter of its slots (at least one, never the last free
slot) reserved for events other than
`auth_failed`. A failed write returns its batch ahead of newer entries.
Dropped occurrences are counted and reported at `warn` as
`audit.dropped` regardless of the log filter.

A second job deletes rows older than `keep_days` (365 by default) once a day
and once at startup.

## Ports

Uses `timbre_edge_sqlite::SharedDatabase` and `timbre_edge_config::Config`.
Provides none.

## Signals

Subscribes to `AuthFailed`, `AccessDenied`, `UserAdded`, `UserRemoved`,
`PermissionsChanged`, `DeviceIssued` (the secret it carries is never read),
`DeviceRevoked` and `ServerDrained`. Publishes nothing.

## Failures

`kernel.usage` for malformed commands, `audit.invalid_config` for invalid
settings, and `audit.not_configured` when no journal database is installed.
