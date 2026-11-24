CREATE TABLE library_state (
  id INTEGER PRIMARY KEY CHECK(id = 1),
  tree_version INTEGER NOT NULL DEFAULT 0,
  policy TEXT NOT NULL DEFAULT '',
  dirty INTEGER NOT NULL DEFAULT 0,
  unreadable INTEGER NOT NULL DEFAULT 0,
  scanned_folders INTEGER NOT NULL DEFAULT 0,
  tag_reads INTEGER NOT NULL DEFAULT 0,
  finished INTEGER
) STRICT;
INSERT INTO library_state(id) VALUES(1);

CREATE TABLE library_roots (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  path BLOB NOT NULL UNIQUE,
  ordinal INTEGER NOT NULL
) STRICT;
CREATE TABLE library_folders (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  root_id INTEGER NOT NULL REFERENCES library_roots ON DELETE CASCADE,
  relative BLOB NOT NULL,
  absolute BLOB NOT NULL,
  parent_id INTEGER REFERENCES library_folders ON DELETE CASCADE,
  name BLOB NOT NULL,
  sort_name TEXT NOT NULL,
  folder_hash TEXT NOT NULL,
  child_signature TEXT NOT NULL,
  ignored INTEGER NOT NULL,
  UNIQUE(root_id, relative)
) STRICT;
CREATE INDEX library_folders_absolute ON library_folders(absolute);
CREATE INDEX library_folders_children ON library_folders(parent_id, sort_name, id);

CREATE TABLE library_albums (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  album_key TEXT NOT NULL UNIQUE,
  key_source TEXT NOT NULL,
  raw_group TEXT NOT NULL,
  name TEXT NOT NULL,
  order_name TEXT NOT NULL,
  artist_order TEXT NOT NULL,
  display_artist TEXT NOT NULL,
  release_id TEXT,
  release_group_id TEXT,
  version TEXT,
  compilation TEXT,
  year INTEGER,
  mature_metadata TEXT NOT NULL,
  cover_art TEXT,
  song_count INTEGER NOT NULL,
  duration_ms INTEGER NOT NULL,
  created INTEGER NOT NULL,
  folded_name TEXT NOT NULL,
  folded_artist TEXT NOT NULL
) STRICT;
CREATE INDEX library_albums_name ON library_albums(order_name, id);
CREATE INDEX library_albums_added ON library_albums(created DESC, id);
CREATE INDEX library_albums_artist ON library_albums(artist_order, order_name, id);
CREATE INDEX library_albums_year ON library_albums(year, id);

CREATE TABLE library_tracks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  canonical BLOB NOT NULL UNIQUE,
  fingerprint TEXT,
  recording_id TEXT,
  album_id INTEGER REFERENCES library_albums ON DELETE SET NULL,
  disc INTEGER,
  track_number INTEGER,
  title TEXT NOT NULL,
  order_name TEXT NOT NULL,
  folded_name TEXT NOT NULL,
  folded_artist TEXT NOT NULL,
  size INTEGER NOT NULL,
  mtime TEXT NOT NULL,
  source TEXT NOT NULL,
  art_hash TEXT,
  art_front INTEGER NOT NULL,
  cover_art TEXT,
  created INTEGER NOT NULL
) STRICT;
CREATE INDEX library_tracks_album ON library_tracks(album_id, disc, track_number, id);
CREATE INDEX library_tracks_order ON library_tracks(order_name, id);
CREATE INDEX library_tracks_art ON library_tracks(art_hash);
CREATE TABLE library_track_paths (
  track_id INTEGER NOT NULL REFERENCES library_tracks ON DELETE CASCADE,
  root_id INTEGER NOT NULL REFERENCES library_roots ON DELETE CASCADE,
  folder_id INTEGER NOT NULL REFERENCES library_folders ON DELETE CASCADE,
  relative BLOB NOT NULL,
  absolute BLOB NOT NULL,
  size INTEGER NOT NULL,
  PRIMARY KEY(root_id, relative)
) STRICT;
CREATE INDEX library_track_paths_folder ON library_track_paths(folder_id);
CREATE INDEX library_track_paths_track ON library_track_paths(track_id);
CREATE INDEX library_track_paths_absolute ON library_track_paths(absolute);

CREATE TABLE library_files (
  folder_id INTEGER NOT NULL REFERENCES library_folders ON DELETE CASCADE,
  name BLOB NOT NULL,
  sort_name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('audio','image','lyrics')),
  track_id INTEGER REFERENCES library_tracks ON DELETE CASCADE,
  PRIMARY KEY(folder_id, name)
) STRICT;

CREATE TABLE library_artists (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  artist_key TEXT NOT NULL UNIQUE,
  mbid TEXT,
  name TEXT NOT NULL,
  sort_name TEXT,
  order_name TEXT NOT NULL,
  folded_name TEXT NOT NULL,
  cover_art TEXT
) STRICT;
CREATE INDEX library_artists_order ON library_artists(order_name, id);
CREATE TABLE library_credits (
  track_id INTEGER REFERENCES library_tracks ON DELETE CASCADE,
  album_id INTEGER REFERENCES library_albums ON DELETE CASCADE,
  position INTEGER NOT NULL,
  artist_id INTEGER NOT NULL REFERENCES library_artists,
  credit_name TEXT NOT NULL,
  role TEXT NOT NULL,
  sub_role TEXT,
  join_phrase TEXT NOT NULL,
  CHECK((track_id IS NULL) != (album_id IS NULL))
) STRICT;
CREATE UNIQUE INDEX library_credits_track ON library_credits(track_id, position) WHERE track_id IS NOT NULL;
CREATE UNIQUE INDEX library_credits_album ON library_credits(album_id, position) WHERE album_id IS NOT NULL;
CREATE INDEX library_credits_artist ON library_credits(artist_id, album_id, role);

CREATE TABLE library_track_genres (
  track_id INTEGER NOT NULL REFERENCES library_tracks ON DELETE CASCADE,
  name TEXT NOT NULL,
  folded_name TEXT NOT NULL,
  PRIMARY KEY(track_id, name)
) STRICT;
CREATE INDEX library_track_genres_name ON library_track_genres(folded_name, name, track_id);
CREATE TABLE library_lyrics (
  track_id INTEGER NOT NULL REFERENCES library_tracks ON DELETE CASCADE,
  position INTEGER NOT NULL,
  language TEXT,
  description TEXT,
  synced INTEGER NOT NULL,
  source TEXT NOT NULL,
  text TEXT,
  path BLOB,
  root BLOB,
  PRIMARY KEY(track_id, position)
) STRICT;
CREATE TABLE library_art (
  hash TEXT PRIMARY KEY,
  mime TEXT NOT NULL
) STRICT;
CREATE TABLE library_embedded_art (
  track_id INTEGER PRIMARY KEY REFERENCES library_tracks ON DELETE CASCADE,
  offset INTEGER,
  length INTEGER,
  picture INTEGER NOT NULL,
  CHECK((offset IS NULL) = (length IS NULL))
) STRICT;
CREATE TABLE library_folder_art (
  folder_id INTEGER PRIMARY KEY REFERENCES library_folders ON DELETE CASCADE,
  hash TEXT NOT NULL REFERENCES library_art,
  path BLOB NOT NULL
) STRICT;
CREATE INDEX library_folder_art_hash ON library_folder_art(hash);
CREATE TABLE library_diagnostics (
  track_id INTEGER NOT NULL REFERENCES library_tracks ON DELETE CASCADE,
  kind TEXT NOT NULL,
  detail TEXT NOT NULL,
  PRIMARY KEY(track_id, kind)
) STRICT;
CREATE TABLE library_unreadable (
  path BLOB PRIMARY KEY,
  folder_id INTEGER NOT NULL REFERENCES library_folders ON DELETE CASCADE,
  reason TEXT NOT NULL,
  size INTEGER NOT NULL,
  mtime TEXT NOT NULL
) STRICT;
CREATE INDEX library_unreadable_folder ON library_unreadable(folder_id);

CREATE VIRTUAL TABLE library_fts USING fts5(kind UNINDEXED, entity UNINDEXED, name, artist, album, tokenize='unicode61 remove_diacritics 0');
CREATE VIRTUAL TABLE library_trigram USING fts5(kind UNINDEXED, entity UNINDEXED, name, artist, album, tokenize='trigram');
