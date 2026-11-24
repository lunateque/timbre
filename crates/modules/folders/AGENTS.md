# crates/modules/folders

## Purpose

Render the scanned music roots and their visible directory tree, without storing data.

## Ports

Provides `Folders`; consumes `LibraryTree`. Folders with `.nomedia` or `.timbreignore`
hide their entire subtree. Directories without visible media are hidden.

## Signals

None.

## Failures

- `folders.not_available`: no `LibraryTree` is registered.
- `folders.not_found`: the requested root or directory is absent or hidden.
  A `LibraryTree` provider's own `not_found` is translated into this code.
- Paging errors come from the kernel: `kernel.page_too_large` for a `limit`
  above `MAX_PAGE_LIMIT` (500), refused, never silently shortened, and
  `kernel.cursor_invalid` for a malformed cursor. Roots pass the opaque
  cursor through to `LibraryTree`.
