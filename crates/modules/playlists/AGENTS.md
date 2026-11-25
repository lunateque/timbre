# timbre-playlists

## Purpose

User-owned playlists of ordered catalog track ids. Each playlist carries an
opaque 128-bit random base32 id, a canonical owner, a visibility flag (private
by default), a title, an optional comment, and an ordered list of track ids
where duplicates are preserved.

Object-level authorization is enforced inside the module on every call:
when `Ctx::principal()` is present, it must match `actor` (else
`playlists.forbidden`); private playlists belong solely to their owner and
answer `playlists.not_found` to any other caller so playlist ids cannot be
probed; public playlists are readable by any user and return
`playlists.forbidden` if a non-owner tries to modify or delete them.

Entries reference catalog tracks without foreign keys. When a `Catalog` port is
registered, reads skip track ids the catalog no longer has via lightweight
`Catalog::track_durations`, summaries reflect the live count and combined
duration, and `update` prunes vanished tracks before applying `remove_indices`
(retrying if concurrent edits shifted the snapshot). When `Catalog` is absent,
stored entries are returned unfiltered with zero duration.

```toml
[playlists]
max_playlists_per_user = 1000     # 1..=10000
max_tracks_per_playlist = 10000   # 1..=100000
max_total_entries_per_user = 50000 # 1..=500000
```

## Ports

- Provides `timbre_kernel::playlists::Playlists`.
- Optionally consumes `timbre_kernel::catalog::Catalog` when registered.
- Uses `timbre_edge_sqlite::SharedDatabase`, `timbre_edge_config::Config`, and
  `timbre_edge_crypto` for opaque random playlist ids.

## Signals

- Subscribes to `timbre_kernel::signals::UserRemoved` and
  `timbre_kernel::signals::UserAdded` to purge all playlists and entries owned
  by a removed or recreated account name.
- Publishes nothing.

## Failures

- `playlists.invalid_config`: a `[playlists]` setting is outside its supported
  range.
- `playlists.invalid_input`: an empty or malformed title, comment, id or
  out-of-range removal index.
- `playlists.not_found`: the playlist does not exist or is private to another
  user.
- `playlists.forbidden`: a present principal does not match `actor`, or the
  caller tried to modify or delete another user's public playlist.
- `playlists.conflict`: concurrent edits repeatedly changed the playlist during
  an index-based removal.
- `playlists.limit_reached`: creating or updating would exceed the configured
  per-user playlist, per-playlist track, or per-user total entry cap.
- `playlists.stored_owner_invalid`: a persisted playlist row holds a
  non-canonical owner name.
- `playlists.not_configured`: no shared database resource was installed or
  startup configuration validation has not run.
- Paging errors use `kernel.page_too_large` and `kernel.cursor_invalid`.
