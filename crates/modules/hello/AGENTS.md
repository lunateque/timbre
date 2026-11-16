# crates/modules/hello

## Purpose

An example module that exists to exercise the kernel contracts end to end:
registration, a `Started` handler and a logged event.

## Ports

None.

## Signals

Handles `signals::Started` by logging `hello.greeted`.

## Failures

- `hello.unavailable` (`HelloFailure::Unavailable`): declared to exercise the
  failure shape; the module never returns it.
