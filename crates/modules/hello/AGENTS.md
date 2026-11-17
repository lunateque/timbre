# crates/modules/hello

## Purpose

An example module that exists to exercise the kernel contracts end to end:
registration, a `Started` handler and a logged event, a route, a CLI command
and a declared permission. Its tests assert on log output through
`timbre_kernel::testing` (dev-dependency feature), not a private sink copy.

## Ports

None.

## Signals

Handles `signals::Started` by logging `hello.greeted`. Declares the permission
`hello.greet`, contributes the `hello` CLI command, which logs the same event,
and answers two routes: `GET /hello` is `Guard::Public`; `GET /hello/me` is
`Guard::Requires { permission: hello.greet, .. }`, so the kernel authenticates
before the handler runs and the handler only ever sees a caller who holds the
permission.

## Routes and the guard rule

Deny by default: every route declares a guard. `/hello/me` reads one
credential, `Authorization: Bearer <secret>`, and renders the refusal itself
as plain text:

- 401 when the request carries no usable bearer credential (no header, a
  scheme other than `Bearer`, an empty or non-UTF-8 secret), when the token is
  rejected, or when no module owns bearer tokens (the unsupported denial);
- 429 when the throttle refuses to ask the provider;
- 403 when the recognized caller lacks `hello.greet`;
- 400 when the request offers more than one credential: a comma in the
  secret, or more than one `Authorization` value.

While no module in the tree owns `CredentialKind::Bearer`, every well-formed
token lands on the unsupported denial.

## Failures

- `hello.unavailable` (`HelloFailure::Unavailable`): declared to exercise the
  failure shape; the module never returns it.
- `hello.principal_missing` (`HelloFailure::PrincipalMissing`, 500): returned
  by `/hello/me` when the guard passed without a principal. The guard
  establishes one for every `Guard::Requires` route, so this marks a broken
  guard flow; the route never greets a placeholder name instead.
