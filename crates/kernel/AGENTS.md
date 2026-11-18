# crates/kernel

## Purpose

The kernel every module is written against: registration (guarded routes,
jobs, CLI commands, permissions, credential providers, a throttle,
migrations, signals, ports), a structured logger whose event names are
checked when the code compiles, failures with stable codes, the HTTP
request/response shapes, and the runtime services a server and its CLI run
on (`Runtime`, `ThreadRuntime`, `block_on`).

## Routes and guards (deny by default)

Every route registers with a mandatory `Guard`: `Guard::Public`, or
`Guard::Requires { permission, extract, deny }`. `Kernel::find_route`
returns a `RouteMatch::Found(Matched)` whose handler is private — an edge
cannot hold a handler as a value, so it cannot call one around its guard.
`Kernel::guarded(ctx, &matched, &request)` runs the check (the protocol's
`extract` on the request, authentication through `Ctx::authenticate`, the
permission check) and returns an opaque `Authorized`; its
`Authorized::run(request)` is the only way a handler is ever invoked. Public
routes go through the same path and yield an `Authorized` without a
principal. A failed check answers with the protocol's own
`deny(&request, Denial)` and the handler never runs. `Denial::Rejected`
carries no reason: a wrong secret and a disabled account answer identically
so the wire never leaks account state (protocols must answer every
`Rejected` the same way); `Throttled` stays distinct. `Principal`
construction and `Ctx::with_principal` are crate-private: a handler can only
receive a principal from the guard. The kernel knows no wire protocol —
`Credential::Scheme { name, user, fields }` is an opaque container it
routes by `SchemeName` only, never interpreting its fields; a `Scheme`
with an empty `user` names no user (the owner may derive one from its
fields). User names are canonical `UserName`s: ASCII lowercase
`[a-z0-9._-]`, 1-64 bytes, trimmed and lowercased from input by
`authenticate` before any throttle or provider sees them; an
unnormalizable name is rejected as `invalid` without asking anyone, and
the audit line carries the candidate clipped to 64 bytes at a char
boundary. A forbidden request (recognized identity, missing permission)
is audited: `kernel.access_denied` at `warn` regardless of the filter,
plus the `AccessDenied` signal.

## Ports

None implemented. The kernel defines the `Port` trait and the `Ports`
registry; consumers must tolerate a port's absence. The `Throttle`
(brute-force limiting of credential checks) is deliberately not a port:
exactly one may exist and only the kernel calls it, so it registers through
`Registry::throttle`; its absence means no in-process limiting.

## Test support

The cargo feature `testing` exposes `timbre_kernel::testing`: a collecting
sink (`Shared`), fixed clocks (`fixed`, `epoch`) and JSON line/field helpers,
so crates asserting on log output need not copy the machinery; new test
sinks should use it. The kernel's own tests use it without the feature.
The edge crates still carry private copies of the same helpers.

## Signals

Provides `signals::Started` (published by the binary once all modules are
registered), `UserRemoved`, `DeviceIssued`, `DeviceRevoked`, `AuthFailed`
(published by `Ctx::authenticate` after every rejection; its `user` is the
canonical `UserName` or `None` when the given text was not a user name)
and `AccessDenied` (published by the guard check after every forbidden
request: client, canonical user, required permission).

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
  credential kind (or scheme name) acquired a second owner; the failure
  names both registering modules.
- `kernel.throttle_taken` (`KernelFailure::ThrottleTaken`): a second
  `Registry::throttle` registration; the failure names both modules.
- `kernel.migration_taken` (`KernelFailure::MigrationTaken`): a module
  registered the same migration version twice, or with a foreign module id.
- `kernel.command_taken` / `kernel.command_invalid`
  (`KernelFailure::CommandTaken` / `CommandInvalid`): a CLI command name
  registered twice, or malformed.
- `kernel.module_invalid` (`KernelFailure::ModuleInvalid`): `Registry::module`
  was given a module id that is not one lowercase name segment or is one of
  the reserved ids (`kernel`, `timbre`, `xtask`).
- `kernel.blocking_panicked` (`KernelFailure::BlockingPanicked`): a closure
  run through `Ctx::blocking` panicked, so it produced no value; the awaiter
  receives this failure instead of waiting forever.

## Log events

Declared in `src/log_events.rs`. These are worth knowing about:

- `kernel.auth_failed` (`warn`, fields `client`, `user`, `reason`): one line
  per rejected credential. Written at `warn` regardless of the log filter,
  like `kernel.access_denied` and `kernel.log_dropped`: fail2ban and the
  audit trail must not be blinded by a filter that hides `warn`, so the line
  arrives even with `TIMBRE_LOG=kernel=error` or quieter. The credential's
  secret never reaches the line; an unnormalizable user name reaches it
  clipped to `UserName::MAX_LENGTH` bytes at a char boundary.
- `kernel.access_denied` (`warn`, fields `client`, `user`, `permission`):
  one line per forbidden request — a recognized identity that does not hold
  the permission a guarded route requires. Written unconditionally like the
  auth-failure line, and the guard also publishes `signals::AccessDenied`
  with the same fields.
- `kernel.handler_failed` (`warn`, fields `signal`, `code`/`panicked`): one
  signal handler failed - `code` carries the `FailureCode` of an `Err`
  handler, `panicked: true` (and no `code`) a handler that unwound. The bus
  catches each handler's panic, so one module's bug cannot kill the thread
  that published the signal (every failed login publishes one); the other
  handlers still run and the publisher is unaffected.
- `kernel.log_dropped` (`warn`, field `count`): that many lines were dropped
  while the sink lagged: queued (info/debug/trace) lines when the bounded
  queue filled, or an inline `warn`/`error` line when the sink mutex stayed
  locked past the 50 ms inline wait. `warn` and `error` lines never take the
  queue — they are written inline under the sink mutex — and the report
  bypasses the module filter, so it arrives even when `kernel` is filtered
  above `warn`. A flush or shutdown wait holds on a wedged sink for at most
  2 s, then returns or detaches the writer thread.
- `kernel.audit_dropped` (`warn`, field `count`): that many `write_always`
  audit lines (`kernel.auth_failed`, `kernel.access_denied`) were dropped
  while the sink lagged, counted apart from ordinary lines so a watcher on
  the audit trail sees the gap without reading the generic report. The line
  is unfiltered like the audit lines themselves and arrives with the next
  dropped-lines report, from the same mechanism. A line either bucket lost
  counts a sink that refused it too: an inline or queued `write` that ends
  in an IO error (disk full, closed pipe) is a drop, never a delivery.
