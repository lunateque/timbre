# crates/modules/library

## Purpose

Index configured music roots into the catalog, library tree, lyrics and artwork.
The scanner never writes into a music root. Only the SQLite database and the
thumbnail cache (`<data_dir>/art/`) are written. Removing the module removes its
ports, jobs and commands; `timbre db prune` controls existing tables.

```toml
[library]
roots = ["/srv/music"]
scan_on_start = true
watch = true
scan_workers = 4 # omitted: available parallelism
art_cache_mb = 1024 # 0 disables caching, not thumbnails
append_version_to_title = false

[library.tags]
artist = [" feat. ", " ft. "]
```

`scan` reconciles folder hashes; `scan --full` rereads tags without changing
identity. `library explain <path>` traces the saved grouping key, credits, art,
lyrics and diagnostics. A user initiating a scan needs `library.scan`. Ignore
markers are reported on folders for consumers; they do not hide catalog tracks.
Artwork rows store hashes and source locators, never image bodies. Originals
are verified against the scanned hash; stale sources log `library.art_source_stale`.
Thumbnails are cached under the data directory and evicted least-recently used.

## Ports

Provides kernel `Catalog`, `Artwork` and `LibraryTree`. The binary supplies the
loaded config and the shared lazy database as runtime resources. The library
uses edge crates for native audio tags, SQLite, image processing, filesystem
watching and text normalization.

## Jobs

`library.scan` is the optional startup scan. `library.watch` queues the paths
a watcher reports, ingests each one once its size and modification time have
held still across two observations two seconds apart, and reconciles the
whole tree after an overflow. Paths that are not audio, image, lyric or
ignore files never enter the queue.

## Signals

The module publishes and consumes no signals.

## Failures

Failure codes live in `src/failures.rs`. Query errors are mapped by `edge/sqlite`.
Scan lifecycle and progress, unreadable inputs, ambiguous relinks, watcher
overflow and explanations are declared in `src/log_events.rs`.
