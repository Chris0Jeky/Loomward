CREATE TABLE IF NOT EXISTS root_forgotten (   -- v7: anonymised audit of a forgotten revoked root (#288): dates only, no name
  grant_id INTEGER PRIMARY KEY REFERENCES root_grant(id), root_number INTEGER NOT NULL, forgotten_at_ns INTEGER NOT NULL) STRICT;
