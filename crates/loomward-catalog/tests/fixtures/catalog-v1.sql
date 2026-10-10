
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT; -- dataset_class, instance_id, engine_version
CREATE TABLE volume (
  id INTEGER PRIMARY KEY, volume_key TEXT NOT NULL UNIQUE, display_name TEXT NOT NULL,
  identity_json TEXT NOT NULL, capabilities_json TEXT NOT NULL, device_json TEXT NOT NULL,
  online INTEGER NOT NULL CHECK (online IN (0,1)), observed_at_ns INTEGER NOT NULL) STRICT;
CREATE TABLE root_grant (
  id INTEGER PRIMARY KEY, volume_id INTEGER REFERENCES volume(id),
  root_file_id BLOB CHECK (root_file_id IS NULL OR length(root_file_id) = 16),
  display_path TEXT NOT NULL,
  origin TEXT NOT NULL CHECK (origin IN ('fixture','lab_generated','owner_granted')),
  granted_via TEXT NOT NULL CHECK (granted_via IN ('desktop_picker','cli_flag','fixture')),
  state TEXT NOT NULL CHECK (state IN ('active','revoked','identity_changed')),
  granted_at_ns INTEGER NOT NULL, revoked_at_ns INTEGER) STRICT;
CREATE TABLE scan_run (
  id INTEGER PRIMARY KEY, grant_id INTEGER NOT NULL REFERENCES root_grant(id),
  mode TEXT NOT NULL CHECK (mode IN ('full','refresh')), strategy TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('running','cancelled','failed','completed')),
  started_at_ns INTEGER NOT NULL, finished_at_ns INTEGER,
  counters_json TEXT NOT NULL, budget_json TEXT NOT NULL, error_json TEXT) STRICT;
CREATE TABLE ext (id INTEGER PRIMARY KEY, ext TEXT NOT NULL UNIQUE, family TEXT NOT NULL) STRICT;
CREATE TABLE dir (
  id INTEGER PRIMARY KEY, grant_id INTEGER NOT NULL REFERENCES root_grant(id) ON DELETE CASCADE,
  parent_id INTEGER REFERENCES dir(id) ON DELETE CASCADE,          -- NULL only for the grant root
  name TEXT NOT NULL, name_utf16 BLOB,                             -- raw UTF-16LE only when lossy
  file_id BLOB CHECK (file_id IS NULL OR length(file_id) = 16),
  depth INTEGER NOT NULL, attrs INTEGER NOT NULL, reparse_tag INTEGER, flags INTEGER NOT NULL DEFAULT 0,
  created_ft INTEGER, modified_ft INTEGER, changed_ft INTEGER,
  state TEXT NOT NULL CHECK (state IN ('complete','partial','denied','excluded','unscanned','stale','cancelled')),
  seen_run INTEGER NOT NULL, listing_rev INTEGER NOT NULL DEFAULT 0,
  own_files INTEGER NOT NULL DEFAULT 0, own_logical INTEGER NOT NULL DEFAULT 0,
  own_allocated INTEGER NOT NULL DEFAULT 0, own_alloc_unknown INTEGER NOT NULL DEFAULT 0,
  own_skipped INTEGER NOT NULL DEFAULT 0, own_errors INTEGER NOT NULL DEFAULT 0,
  sub_files INTEGER NOT NULL DEFAULT 0, sub_dirs INTEGER NOT NULL DEFAULT 0,
  sub_logical INTEGER NOT NULL DEFAULT 0, sub_allocated INTEGER NOT NULL DEFAULT 0,
  sub_alloc_unknown INTEGER NOT NULL DEFAULT 0, sub_skipped INTEGER NOT NULL DEFAULT 0,
  sub_errors INTEGER NOT NULL DEFAULT 0, sub_newest_ft INTEGER,
  sub_complete INTEGER NOT NULL DEFAULT 0 CHECK (sub_complete IN (0,1))) STRICT;
CREATE INDEX dir_by_logical   ON dir(parent_id, sub_logical DESC);
CREATE INDEX dir_by_allocated ON dir(parent_id, sub_allocated DESC);
CREATE UNIQUE INDEX dir_identity ON dir(grant_id, file_id) WHERE file_id IS NOT NULL;
CREATE TABLE file (
  id INTEGER PRIMARY KEY, dir_id INTEGER NOT NULL REFERENCES dir(id) ON DELETE CASCADE,
  name TEXT NOT NULL, name_utf16 BLOB, ext_id INTEGER REFERENCES ext(id),
  file_id BLOB CHECK (file_id IS NULL OR length(file_id) = 16),
  logical INTEGER NOT NULL, allocated INTEGER,                      -- NULL allocated = unknown
  created_ft INTEGER, modified_ft INTEGER, changed_ft INTEGER, accessed_ft INTEGER,
  attrs INTEGER NOT NULL, reparse_tag INTEGER, flags INTEGER NOT NULL DEFAULT 0,
  seen_run INTEGER NOT NULL) STRICT;
CREATE INDEX file_by_logical ON file(dir_id, logical DESC);
CREATE INDEX file_by_ext     ON file(ext_id, logical DESC);
CREATE INDEX file_identity   ON file(file_id) WHERE file_id IS NOT NULL; -- hard links share an ID
