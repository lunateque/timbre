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
