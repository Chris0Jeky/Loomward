-- catalog.db: derived, rebuildable.
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;   -- dataset_class, instance_id, engine_version
CREATE TABLE revision (id INTEGER PRIMARY KEY CHECK (id = 1), catalog_rev INTEGER NOT NULL) STRICT;
CREATE TABLE volume (
  id INTEGER PRIMARY KEY AUTOINCREMENT, volume_key TEXT NOT NULL UNIQUE, display_name TEXT NOT NULL,
  filesystem TEXT, bytes_per_cluster INTEGER, identity_json TEXT NOT NULL, capabilities_json TEXT NOT NULL,
  device_json TEXT NOT NULL, online INTEGER NOT NULL CHECK (online IN (0,1)), observed_at_ns INTEGER NOT NULL) STRICT;
CREATE TABLE root (                       -- observation of a grant's root; the grant itself lives in state.db
  id INTEGER PRIMARY KEY AUTOINCREMENT, grant_id INTEGER NOT NULL UNIQUE,   -- st.root_grant.id (no cross-file FK)
  volume_id INTEGER REFERENCES volume(id), root_file_id BLOB CHECK (root_file_id IS NULL OR length(root_file_id) IN (8,16)),
  generation INTEGER, active_run INTEGER, state TEXT NOT NULL
    CHECK (state IN ('never_scanned','scanning','complete','partial','stale','repairing','failed'))) STRICT;
CREATE TABLE scan_run (
  id INTEGER PRIMARY KEY AUTOINCREMENT, root_id INTEGER NOT NULL REFERENCES root(id) ON DELETE CASCADE,
  mode TEXT NOT NULL CHECK (mode IN ('full','refresh','targeted')), strategy TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('running','cancelled','failed','completed')),
  started_at_ns INTEGER NOT NULL, finished_at_ns INTEGER,
  counters_json TEXT NOT NULL, budget_json TEXT NOT NULL, error_json TEXT) STRICT;
CREATE TABLE ext (id INTEGER PRIMARY KEY AUTOINCREMENT, ext TEXT NOT NULL UNIQUE, family TEXT NOT NULL) STRICT;
CREATE TABLE dir (
  id INTEGER PRIMARY KEY AUTOINCREMENT, root_id INTEGER NOT NULL REFERENCES root(id) ON DELETE CASCADE,
  parent_id INTEGER REFERENCES dir(id) ON DELETE CASCADE,          -- NULL only for the root directory
  name TEXT NOT NULL, name_utf16 BLOB,                             -- raw UTF-16LE only when lossy
  file_id BLOB CHECK (file_id IS NULL OR length(file_id) IN (8,16)),
  id_quality TEXT NOT NULL CHECK (id_quality IN ('native_file_id_128','native_file_id_64','path_observation')),
  id_basis TEXT NOT NULL CHECK (id_basis IN ('listed','post_open','none')),
  depth INTEGER NOT NULL, attrs INTEGER NOT NULL, reparse_tag INTEGER, flags INTEGER NOT NULL DEFAULT 0,
  created_ft INTEGER, modified_ft INTEGER, changed_ft INTEGER,
  listing_state TEXT NOT NULL CHECK (listing_state IN
    ('complete','incomplete','unlisted','excluded','denied','absent_pending')),
  born_run INTEGER NOT NULL, seen_run INTEGER NOT NULL,
  listing_rev INTEGER NOT NULL DEFAULT 0, subtree_rev INTEGER NOT NULL DEFAULT 0,
  dirty_rev INTEGER NOT NULL DEFAULT 0, dirty_run INTEGER, agg_valid_rev INTEGER NOT NULL DEFAULT 0,
  own_files INTEGER NOT NULL DEFAULT 0, own_logical INTEGER NOT NULL DEFAULT 0,
  own_allocated INTEGER NOT NULL DEFAULT 0, own_alloc_unknown INTEGER NOT NULL DEFAULT 0,
  own_newest_ft INTEGER,
  own_noid INTEGER NOT NULL DEFAULT 0, own_skipped INTEGER NOT NULL DEFAULT 0, own_errors INTEGER NOT NULL DEFAULT 0,
  sub_files INTEGER NOT NULL DEFAULT 0, sub_dirs INTEGER NOT NULL DEFAULT 0,
  sub_logical INTEGER NOT NULL DEFAULT 0, sub_allocated INTEGER NOT NULL DEFAULT 0,
  sub_alloc_unknown INTEGER NOT NULL DEFAULT 0, sub_noid INTEGER NOT NULL DEFAULT 0,
  sub_skipped INTEGER NOT NULL DEFAULT 0, sub_errors INTEGER NOT NULL DEFAULT 0, sub_newest_ft INTEGER,
  sub_complete INTEGER NOT NULL DEFAULT 0 CHECK (sub_complete IN (0,1))) STRICT;
CREATE INDEX dir_by_logical   ON dir(parent_id, sub_logical DESC, id);
CREATE INDEX dir_by_allocated ON dir(parent_id, sub_allocated DESC, id);
CREATE UNIQUE INDEX dir_identity ON dir(root_id, file_id) WHERE file_id IS NOT NULL;
CREATE TABLE file (
  id INTEGER PRIMARY KEY AUTOINCREMENT, dir_id INTEGER NOT NULL REFERENCES dir(id) ON DELETE CASCADE,
  name TEXT NOT NULL, name_utf16 BLOB, ext_id INTEGER REFERENCES ext(id),
  file_id BLOB CHECK (file_id IS NULL OR length(file_id) IN (8,16)),
  logical INTEGER NOT NULL, allocated INTEGER,                      -- default stream; NULL allocated = unknown
  created_ft INTEGER, modified_ft INTEGER, changed_ft INTEGER, accessed_ft INTEGER,
  attrs INTEGER NOT NULL, reparse_tag INTEGER, flags INTEGER NOT NULL DEFAULT 0,
  link_count INTEGER,                                               -- NULL until an accounting pass observes it
  born_run INTEGER NOT NULL, seen_run INTEGER NOT NULL) STRICT;
CREATE INDEX file_by_logical   ON file(dir_id, logical DESC, id);
CREATE INDEX file_by_allocated ON file(dir_id, allocated DESC, id);  -- NULLs sort last under DESC
CREATE INDEX file_identity     ON file(file_id) WHERE file_id IS NOT NULL;
CREATE INDEX file_by_ext       ON file(ext_id, logical DESC);        -- first index to drop if P14 fails
CREATE TABLE multilink (                  -- objects with more than one observed name, rebuilt at finalise
  volume_id INTEGER NOT NULL, file_id BLOB NOT NULL, names INTEGER NOT NULL, allocated INTEGER,
  PRIMARY KEY (volume_id, file_id)) STRICT;
CREATE TABLE stage_entry (                -- chunks of large or in-progress listings, published atomically
  run_id INTEGER NOT NULL, dir_id INTEGER NOT NULL, seq INTEGER NOT NULL, entries BLOB NOT NULL, entry_count_total INTEGER NOT NULL CHECK(entry_count_total BETWEEN 0 AND 2000000),
  PRIMARY KEY (run_id, dir_id, seq)) STRICT;


CREATE INDEX dir_by_root ON dir(root_id);
INSERT INTO revision VALUES(1,0);
