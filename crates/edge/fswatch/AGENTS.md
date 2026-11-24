# crates/edge/fswatch

## Purpose

Watch canonical roots recursively without following symlinks below a root.
`Watcher::recv_timeout` reports bounded batches of changed paths, or a
`Rescan` when notifications are lost. The caller decides when a file is
stable and reconciles the roots after a rescan hint.

## Ports

None. The library calls this edge directly.

## Signals

None. Watch events are returned to the caller, not published to the bus.

## Failures

`fswatch.watch_failed` means a watch could not be established or its
notification channel closed.
