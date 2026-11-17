# crates/kernel

## Purpose

The kernel every module is written against: registration (routes, jobs, CLI
commands, permissions, credential providers, migrations, signals, ports), a
structured logger whose event names are checked when the code compiles,
failures with stable codes, the HTTP request/response shapes, and the runtime
services a server and its CLI run on (`Runtime`, `ThreadRuntime`, `block_on`).

## Ports

None implemented. The kernel defines the `Port` trait, the `Ports` registry
and the `Throttle` port (brute-force limiting of credential checks) that the
`auth_guard` module is expected to provide; consumers must tolerate its
absence.

## Test support

The cargo feature `testing` exposes `timbre_kernel::testing`: a collecting
sink (`Shared`), fixed clocks (`fixed`, `epoch`) and JSON line/field helpers,
so crates asserting on log output need not copy the machinery; new test
sinks should use it. The kernel's own tests use it without the feature.
The edge crates still carry private copies of the same helpers.

## Signals

Provides `signals::Started` (published by the binary once all modules are
registered), `UserRemoved`, `DeviceIssued`, `DeviceRevoked` and `AuthFailed`
(published by `Ctx::authenticate` after every rejection).

## Failures

- `kernel.port_taken` (`KernelFailure::PortTaken`): a port already provided was
  provided again.
- `kernel.log_filter_invalid` (`KernelFailure::LogFilterInvalid`): a
  `TIMBRE_LOG`-style filter specification did not parse.
- `kernel.route_invalid` / `kernel.route_taken` (`KernelFailure::RouteInvalid`
  / `RouteTaken`): a malformed route pattern, or a method and pattern pair
  registered twice.
- `kernel.job_taken` (`KernelFailure::JobTaken`): a job event name registered
  twice.
- `kernel.permission_taken` (`KernelFailure::PermissionTaken`): a permission
  declared twice.
- `kernel.credential_kind_taken` (`KernelFailure::CredentialKindTaken`): a
  credential kind acquired a second owner.
- `kernel.migration_taken` (`KernelFailure::MigrationTaken`): a module
  registered the same migration version twice, or with a foreign module id.
- `kernel.command_taken` / `kernel.command_invalid`
  (`KernelFailure::CommandTaken` / `CommandInvalid`): a CLI command name
  registered twice, or malformed.
- `kernel.blocking_panicked` (`KernelFailure::BlockingPanicked`): a closure
  run through `Ctx::blocking` panicked, so it produced no value; the awaiter
  receives this failure instead of waiting forever.

## Log events

Declared in `src/log_events.rs`. One worth knowing about:

- `kernel.log_dropped` (`warn`, field `count`): that many lines were dropped
  while the sink lagged: queued (info/debug/trace) lines when the bounded
  queue filled, or an inline `warn`/`error` line when the sink mutex stayed
  locked past the 50 ms inline wait. `warn` and `error` lines never take the
  queue — they are written inline under the sink mutex — and the report
  bypasses the module filter, so it arrives even when `kernel` is filtered
  above `warn`. A flush or shutdown wait holds on a wedged sink for at most
  2 s, then returns or detaches the writer thread.
