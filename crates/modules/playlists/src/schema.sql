CREATE TABLE playlists_lists (
  id TEXT PRIMARY KEY,
  owner TEXT NOT NULL,
  name TEXT NOT NULL,
  comment TEXT,
  public INTEGER NOT NULL DEFAULT 0 CHECK(public IN (0, 1)),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
) STRICT;

CREATE INDEX playlists_lists_owner
  ON playlists_lists(owner, name, id);

CREATE INDEX playlists_lists_public
  ON playlists_lists(public, name, id);

CREATE TABLE playlists_entries (
  playlist_id TEXT NOT NULL REFERENCES playlists_lists(id) ON DELETE CASCADE,
  position INTEGER NOT NULL CHECK(position >= 0),
  track_id TEXT NOT NULL,
  PRIMARY KEY(playlist_id, position)
) STRICT, WITHOUT ROWID;
