# crates/kernel

## Purpose

The kernel every module is written against: registration, signals between
modules, ports that consumers must tolerate being absent, a structured logger
whose event names are checked when the code compiles, and failures with stable
codes.

## Ports

None. The kernel defines the `Port` trait and the `Ports` registry; it provides
no implementations.

## Signals

Provides `signals::Started`, published by the binary once all modules are
registered and the server is running.

## Failures

- `kernel.port_taken` (`KernelFailure::PortTaken`): a port already provided was
  provided again.
- `kernel.log_filter_invalid` (`KernelFailure::LogFilterInvalid`): a
  `TIMBRE_LOG`-style filter specification did not parse.
