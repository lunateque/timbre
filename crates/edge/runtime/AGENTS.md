# crates/edge/runtime

## Purpose

The serving implementation of the kernel's `Runtime` on tokio's
multi-threaded executor: `TokioRuntime` spawns tasks and blocking work on
one runtime shared with `edge/http`, and resolves `Runtime::shutdown` on
Ctrl-C or SIGTERM (or `TokioRuntime::trigger_shutdown`). The binary blocks
its top level on `TokioRuntime::block_on`.

## Ports

None. The runtime is wired into the `Kernel` at `Registry::finish`, not
through a port.

## Signals

None. It observes OS signals only.

## Failures

None; spawning on a runtime that already stopped is the caller's problem,
and tokio's own errors never cross this edge as `Failure`s.
