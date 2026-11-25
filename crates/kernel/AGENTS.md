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
`[a-z0-9._-]` starting with `[a-z0-9]`, 1-64 bytes, trimmed and lowercased from input by
`authenticate` before any throttle or provider sees them; an
unnormalizable name is rejected as `invalid` after reserving by address only,
without asking a provider. Its audit line carries the candidate clipped to
64 bytes at a char boundary. A check abandoned before its verdict (a
dropped future, a closed connection) settles its throttle reservation as a
failure, so a cancelled attempt buys no free guess; it publishes no
`AuthFailed` and writes no journal row, because no verdict ever said what
went wrong. A forbidden request (recognized identity, missing permission)
is audited: `kernel.access_denied` at `warn` regardless of the filter,
plus the `AccessDenied` signal.

## Ports

`Catalog`, `Artwork`, `LibraryTree`, `Folders`, `Directory`, `Activity` and
`Playlists` describe the music catalog, art, scanned directory tree, visible
folder browsing, paged account/device listing, per-user listening activity and
user-owned playlists respectively.
Consumers must tolerate a port's absence. The host
installs shared runtime resources at `Registry::finish_with_resources`;
modules can read them with `Ctx::resource` but cannot register resources.
The host calls `Kernel::check_config` after installing resources and before
dispatching commands or starting jobs. Modules register one-time validation
with `Registry::config_check` and retain the validated settings in their own
state.

CLI commands use `Ctx::say` for output independent of the log filter.
`kernel.usage` denotes incorrect command usage and exits with status 2.
The `Throttle` (brute-force limiting of credential checks) is deliberately not a port:
exactly one may exist and only the kernel calls it, so it registers through
`Registry::throttle`; its absence means no in-process limiting. The kernel
reserves capacity before asking the provider and settles it exactly once,
including when the check is cancelled.

Every list method pages with a `Cursor` and serves at most
`page::MAX_PAGE_LIMIT` (500) items; a larger `limit` is refused by the
provider, never silently clamped. `Catalog::search` is the single exception to
cursor paging: it takes a count and an offset per category, because one
`search3` call asks for three independent windows at once.

A port implementation decodes its request with `page::Window`:
`Window::check_limit` refuses an oversize `limit` with
`kernel.page_too_large`, `Window::new` rejects a malformed cursor
with `kernel.cursor_invalid`, and the window takes the lookahead row.
Concurrent inserts or deletes can shift later offset pages.

## Test support

The cargo feature `testing` exposes `timbre_kernel::testing`: a collecting
sink (`Shared`), fixed clocks (`fixed`, `epoch`) and JSON line/field helpers,
so crates asserting on log output need not copy the machinery; new test
sinks should use it. The kernel's own tests use it without the feature.
The edge crates still carry private copies of the same helpers.

## Signals

The built-in signals are:

- `Started`, published by the binary before starting jobs.
- `ServerDrained`, published by the binary after HTTP stops accepting and the
  bounded shutdown wait for jobs finishes, whether serving ended cleanly or
  with a failure.
- `UserAdded`, `UserRemoved`, `PermissionsChanged`, `DeviceIssued` and
  `DeviceRevoked`, published by the `auth` module; users are identified by
  canonical `UserName`.
- `AuthFailed`, published for credential rejections other than throttle
  refusals; its `user` is the canonical `UserName` or `None` when the given
  text was not a user name.
- `AccessDenied`, published after every forbidden request: client, canonical
  user, required permission.
- `Played`, published by `activity` after every committed play submission:
  canonical user, track id and play timestamp.

## Failures

- `kernel.port_taken` (`KernelFailure::PortTaken`): a port already provided was
  provided again.
- `kernel.resource_taken` (`KernelFailure::ResourceTaken`): the host
  installed two runtime resources of the same concrete type.
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
- `kernel.usage` (`KernelFailure::Usage`): a CLI command was called wrongly;
  the binary exits with status 2.
- `kernel.output_failed` (`KernelFailure::OutputFailed`): a command's output
  writer returned an IO error.
- `kernel.page_too_large` / `kernel.cursor_invalid`
  (`KernelFailure::PageTooLarge` / `CursorInvalid`): a page asked for more
  than `MAX_PAGE_LIMIT` items, or its cursor did not decode; neither is a
  usage failure, so a command reporting one exits with status 1.

## Log events

Declared in `src/log_events.rs`. These are worth knowing about:

- `kernel.auth_failed` (`warn`, fields `client`, `user`, `reason`): one line
  per rejected credential except throttle refusals. Bypasses the log filter,
  like `kernel.access_denied` and `kernel.log_dropped`: fail2ban and the
  audit trail must not be blinded by a filter that hides `warn`, so the line
  arrives even with `TIMBRE_LOG=kernel=error` or quieter. The credential's
  secret never reaches the line; an unnormalizable user name reaches it
  clipped to `UserName::MAX_LENGTH` bytes at a char boundary.
- `kernel.auth_throttled` (`warn`, fields `client`, `count`): one unfiltered
  line per canonical address with a refusal during each one-minute interval.
  Beyond 4096 distinct addresses in an interval, additional addresses share
  one `kernel.auth_throttled_untracked` line. Refusals do not publish
  `AuthFailed`.
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
