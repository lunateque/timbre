CREATE TABLE auth_users (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL UNIQUE,
  admin INTEGER NOT NULL DEFAULT 0,
  created INTEGER NOT NULL
) STRICT;

CREATE TABLE auth_user_permissions (
  user_id INTEGER NOT NULL REFERENCES auth_users ON DELETE CASCADE,
  permission TEXT NOT NULL,
  PRIMARY KEY(user_id, permission)
) STRICT, WITHOUT ROWID;

CREATE TABLE auth_devices (
  id TEXT PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES auth_users ON DELETE CASCADE,
  label TEXT NOT NULL,
  prefix TEXT NOT NULL UNIQUE,
  digest BLOB NOT NULL,
  created INTEGER NOT NULL,
  expires INTEGER,
  last_used INTEGER,
  last_client TEXT
) STRICT;

CREATE INDEX auth_devices_owner ON auth_devices(user_id, created, id);
