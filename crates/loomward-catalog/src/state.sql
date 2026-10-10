CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;   -- dataset_class, instance_id
CREATE TABLE root_grant (
  id INTEGER PRIMARY KEY AUTOINCREMENT, volume_key TEXT NOT NULL,
  root_file_id BLOB NOT NULL CHECK (length(root_file_id) IN (8,16)), display_path TEXT NOT NULL,
  origin TEXT NOT NULL CHECK (origin IN ('fixture','lab_generated','owner_granted')),
  granted_via TEXT NOT NULL CHECK (granted_via IN ('desktop_picker','cli_flag','fixture')),
  lab_root_id INTEGER REFERENCES lab_root(id),                    -- required when origin = 'lab_generated'
  state TEXT NOT NULL CHECK (state IN ('active','revoked','identity_changed')),
  granted_at_ns INTEGER NOT NULL, revoked_at_ns INTEGER) STRICT;
CREATE TABLE lab_root (                   -- synthetic state.db only; written by `loomward-lab generate --register`
  id INTEGER PRIMARY KEY AUTOINCREMENT, volume_key TEXT NOT NULL, root_file_id BLOB NOT NULL,
  manifest_digest TEXT NOT NULL, seed TEXT NOT NULL, tier TEXT NOT NULL, registered_at_ns INTEGER NOT NULL,
  UNIQUE (volume_key, root_file_id)) STRICT;
CREATE TABLE object_ref (                 -- durable reference, continuity-keyed (section 6.3)
  id INTEGER PRIMARY KEY AUTOINCREMENT, volume_key TEXT NOT NULL,
  file_id BLOB NOT NULL CHECK (length(file_id) IN (8,16)), creation_ft INTEGER NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('file','dir')),
  state TEXT NOT NULL CHECK (state IN ('resolved','unresolved')), last_resolved_ns INTEGER,
  incarnation INTEGER NOT NULL DEFAULT 1, retired INTEGER NOT NULL DEFAULT 0,
  catalog_instance TEXT, row_id INTEGER, born_run INTEGER,
  UNIQUE (volume_key, file_id, creation_ft, incarnation)) STRICT;
CREATE TABLE taxonomy (version INTEGER PRIMARY KEY, labels_json TEXT NOT NULL, created_at_ns INTEGER NOT NULL) STRICT;
CREATE TABLE human_feedback (
  id INTEGER PRIMARY KEY AUTOINCREMENT, event_id TEXT NOT NULL UNIQUE, client_event_id TEXT NOT NULL UNIQUE,
  content_digest TEXT NOT NULL, object_ref_id INTEGER NOT NULL REFERENCES object_ref(id),
  revision INTEGER NOT NULL CHECK (revision > 0),
  label TEXT NOT NULL,                    -- for a retraction: the label being withdrawn
  retracted INTEGER NOT NULL CHECK (retracted IN (0,1)),
  taxonomy_version INTEGER NOT NULL REFERENCES taxonomy(version), features_json TEXT NOT NULL,
  recorded_at_ns INTEGER NOT NULL, UNIQUE (object_ref_id, revision)) STRICT;
CREATE TABLE teacher_preview (            -- immutable once written
  id INTEGER PRIMARY KEY AUTOINCREMENT, preview_key TEXT NOT NULL UNIQUE, request_json TEXT NOT NULL,
  serialization_version TEXT NOT NULL, payload_digest TEXT NOT NULL, runner_profile_digest TEXT NOT NULL,
  handle_map_json TEXT NOT NULL, created_at_ns INTEGER NOT NULL, expires_at_ns INTEGER NOT NULL) STRICT;
CREATE TABLE disclosure_grant (
  id INTEGER PRIMARY KEY AUTOINCREMENT, preview_id INTEGER NOT NULL UNIQUE REFERENCES teacher_preview(id),
  recipient TEXT NOT NULL, payload_digest TEXT NOT NULL, runner_profile_digest TEXT NOT NULL,
  confirmed_via TEXT NOT NULL CHECK (confirmed_via IN ('desktop_dialog','synthetic_policy')),
  created_at_ns INTEGER NOT NULL, expires_at_ns INTEGER NOT NULL, revoked_at_ns INTEGER, consumed_at_ns INTEGER) STRICT;
CREATE TABLE teacher_request (            -- created in the same transaction that consumes the grant
  id INTEGER PRIMARY KEY AUTOINCREMENT, grant_id INTEGER NOT NULL UNIQUE REFERENCES disclosure_grant(id),
  state TEXT NOT NULL CHECK (state IN ('consumed','running','completed','rejected','failed','interrupted')),
  started_at_ns INTEGER, finished_at_ns INTEGER, response_digest TEXT, rejection TEXT) STRICT;
CREATE TABLE teacher_label (              -- never human feedback
  id INTEGER PRIMARY KEY AUTOINCREMENT, event_id TEXT NOT NULL UNIQUE,   -- 'tl_<request>_<handle>'
  request_id INTEGER NOT NULL REFERENCES teacher_request(id),
  object_ref_id INTEGER NOT NULL REFERENCES object_ref(id), revision INTEGER NOT NULL CHECK (revision > 0),
  label TEXT, abstain INTEGER NOT NULL, reason TEXT NOT NULL, evidence_json TEXT NOT NULL,
  taxonomy_version INTEGER NOT NULL, features_json TEXT NOT NULL, received_at_ns INTEGER NOT NULL,
  UNIQUE (object_ref_id, revision)) STRICT;
CREATE TABLE student_model (
  id TEXT PRIMARY KEY, algorithm TEXT NOT NULL, taxonomy_version INTEGER NOT NULL, dataset_digest TEXT NOT NULL,
  training_count INTEGER NOT NULL, state_json TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('candidate','active','retired')), fitted_at_ns INTEGER NOT NULL) STRICT;
CREATE TABLE collection (
  id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL CHECK (kind IN ('human','label_view')), created_at_ns INTEGER NOT NULL) STRICT;
CREATE TABLE collection_member (
  collection_id INTEGER NOT NULL REFERENCES collection(id) ON DELETE CASCADE,
  object_ref_id INTEGER NOT NULL REFERENCES object_ref(id), added_at_ns INTEGER NOT NULL,
  PRIMARY KEY (collection_id, object_ref_id)) STRICT;
CREATE TABLE tier_declaration (
  id INTEGER PRIMARY KEY AUTOINCREMENT, volume_key TEXT NOT NULL, tier INTEGER CHECK (tier BETWEEN 0 AND 9),
  declared_at_ns INTEGER NOT NULL) STRICT;
CREATE TABLE proposal (
  id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL CHECK (kind = 'placement_simulation'),
  status TEXT NOT NULL CHECK (status IN ('simulation_only','stale')),
  inputs_digest TEXT NOT NULL, inputs_json TEXT NOT NULL, plan_json TEXT NOT NULL, created_at_ns INTEGER NOT NULL) STRICT;

CREATE TABLE revision(id INTEGER PRIMARY KEY CHECK(id=1),state_rev INTEGER NOT NULL) STRICT;
INSERT INTO revision VALUES(1,0);

CREATE UNIQUE INDEX object_ref_binding ON object_ref(catalog_instance,kind,row_id,born_run) WHERE state='resolved';
