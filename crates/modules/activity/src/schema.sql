CREATE TABLE activity_stars (
  user TEXT NOT NULL,
  kind INTEGER NOT NULL,
  item_id TEXT NOT NULL,
  starred_at INTEGER NOT NULL,
  PRIMARY KEY(user, kind, item_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX activity_stars_order
  ON activity_stars(user, kind, starred_at DESC, item_id);

CREATE TABLE activity_ratings (
  user TEXT NOT NULL,
  kind INTEGER NOT NULL,
  item_id TEXT NOT NULL,
  rating INTEGER NOT NULL CHECK(rating BETWEEN 1 AND 5),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(user, kind, item_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX activity_ratings_item
  ON activity_ratings(kind, item_id, rating);

CREATE TABLE activity_track_plays (
  user TEXT NOT NULL,
  track_id TEXT NOT NULL,
  play_count INTEGER NOT NULL CHECK(play_count > 0),
  last_played INTEGER NOT NULL,
  PRIMARY KEY(user, track_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX activity_track_plays_track
  ON activity_track_plays(track_id, play_count, last_played);

CREATE INDEX activity_track_plays_user_top
  ON activity_track_plays(user, play_count DESC, last_played DESC, track_id);

CREATE TABLE activity_album_plays (
  user TEXT NOT NULL,
  album_id TEXT NOT NULL,
  play_count INTEGER NOT NULL CHECK(play_count > 0),
  last_played INTEGER NOT NULL,
  PRIMARY KEY(user, album_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX activity_album_plays_frequent
  ON activity_album_plays(user, play_count DESC, last_played DESC, album_id);

CREATE INDEX activity_album_plays_recent
  ON activity_album_plays(user, last_played DESC, album_id);

CREATE TABLE activity_play_queues (
  user TEXT PRIMARY KEY,
  current_index INTEGER CHECK(current_index IS NULL OR current_index >= 0),
  position_ms INTEGER NOT NULL CHECK(position_ms >= 0),
  changed_by TEXT NOT NULL,
  changed_at INTEGER NOT NULL
) STRICT;

CREATE TABLE activity_play_queue_tracks (
  user TEXT NOT NULL REFERENCES activity_play_queues(user) ON DELETE CASCADE,
  position INTEGER NOT NULL CHECK(position >= 0),
  track_id TEXT NOT NULL,
  PRIMARY KEY(user, position)
) STRICT, WITHOUT ROWID;

CREATE TABLE activity_bookmarks (
  user TEXT NOT NULL,
  track_id TEXT NOT NULL,
  position_ms INTEGER NOT NULL CHECK(position_ms >= 0),
  comment TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(user, track_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX activity_bookmarks_order
  ON activity_bookmarks(user, updated_at DESC, track_id);
