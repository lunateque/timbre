# timbre-activity

## Purpose

Per-user listening activity: stars on tracks, albums and artists; 1-5 ratings;
completed play counts and recency; in-memory now-playing reports; saved play
queues (with index-based current track); and per-track bookmarks.

All persisted rows live in `activity_*` tables without foreign keys to the
catalog or user tables. When a `Catalog` port is registered, writes (`star`,
`set_rating` for non-zero ratings, `scrobble`) validate against the catalog and
store canonical ids, and reads skip items the catalog no longer has (advancing
bounded SQL windows so pages stay full while live rows exist, and adjusting a
play queue's `current_index` across vanished tracks). When `Catalog` is absent,
stored rows are returned unfiltered and track plays are still recorded.

When `Ctx::principal()` is present, per-user calls enforce that the principal
matches the target user and reject mismatches with `activity.forbidden`. The
in-memory now-playing table tracks at most 8 concurrent clients per user and
evicts oldest entries under per-user or global caps (`activity.now_playing_evicted`).
Every state change is logged through events in `src/log_events.rs`.

```toml
[activity]
now_playing_ttl_secs = 1800     # 60..=86400
now_playing_max_entries = 10000 # 1..=100000
max_queue_tracks = 10000        # 1..=100000
max_bookmarks_per_user = 1000   # 1..=10000
max_stars_per_user = 50000      # 1..=500000
max_ratings_per_user = 50000    # 1..=500000
```

## Ports

- Provides `timbre_kernel::activity::Activity`.
- Optionally consumes `timbre_kernel::catalog::Catalog` and
  `timbre_kernel::directory::Directory` when registered.
- Uses `timbre_edge_sqlite::SharedDatabase` and `timbre_edge_config::Config`.

## Signals

- Publishes `timbre_kernel::signals::Played` after each committed play
  submission (`scrobble` with `submission = true`), never on now-playing
  reports or failed writes.
- Subscribes to `timbre_kernel::signals::UserRemoved` and
  `timbre_kernel::signals::UserAdded` to purge persisted rows and in-memory
  now-playing entries for a removed or recreated account name.

## Failures

- `activity.invalid_config`: a `[activity]` setting is outside its supported
  range.
- `activity.invalid_input`: a rating outside `0..=5`, a future scrobble
  timestamp, an invalid catalog id, client label, comment, `position_ms` or
  out-of-range queue index.
- `activity.forbidden`: a present `Ctx::principal()` does not match the target
  user.
- `activity.not_found`: the target item is absent from a registered catalog, or
  the bookmark to delete does not exist.
- `activity.limit_reached`: the write would exceed a configured per-user cap.
- `activity.not_configured`: no shared database resource was installed or
  startup configuration validation has not run.
- Paging and batch limits use `kernel.page_too_large` and
  `kernel.cursor_invalid`.
