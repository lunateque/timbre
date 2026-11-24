CREATE TABLE audit_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  time INTEGER NOT NULL,
  kind TEXT NOT NULL,
  user TEXT,
  client TEXT,
  detail TEXT NOT NULL,
  count INTEGER NOT NULL CHECK (count > 0)
) STRICT;

CREATE INDEX audit_events_recent ON audit_events(time DESC, id DESC);
