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
`hello.greet`, answers `GET /hello` with plain text and contributes the
`hello` CLI command, which logs the same event.

## Failures

- `hello.unavailable` (`HelloFailure::Unavailable`): declared to exercise the
  failure shape; the module never returns it.
