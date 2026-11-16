# crates/timbre

## Purpose

The server binary: reads `TIMBRE_LOG`, builds the logger, registers every
module behind its own cargo feature and publishes `Started` when the server is
up.

## Ports

None.

## Signals

Publishes `signals::Started`. Declares its own events `timbre.started`,
`timbre.log_filter_invalid` and `timbre.registration_failed`.

## Failures

None of its own; a failed filter or registration is logged with the code of the
failure that stopped startup.
