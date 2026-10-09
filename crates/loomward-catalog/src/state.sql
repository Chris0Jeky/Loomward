
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
CREATE TABLE object_ref (                     -- durable reference that survives rescans and moves
  id INTEGER PRIMARY KEY, volume_key TEXT NOT NULL, file_id BLOB NOT NULL CHECK (length(file_id) = 16),
  kind TEXT NOT NULL CHECK (kind IN ('file','dir')), UNIQUE (volume_key, file_id)) STRICT;
CREATE TABLE taxonomy (version INTEGER PRIMARY KEY, labels_json TEXT NOT NULL, created_at_ns INTEGER NOT NULL) STRICT;
CREATE TABLE human_feedback (
  id INTEGER PRIMARY KEY, event_id TEXT NOT NULL UNIQUE, object_ref_id INTEGER NOT NULL REFERENCES object_ref(id),
  revision INTEGER NOT NULL CHECK (revision > 0), label TEXT, retracted INTEGER NOT NULL CHECK (retracted IN (0,1)),
  taxonomy_version INTEGER NOT NULL REFERENCES taxonomy(version), features_json TEXT NOT NULL,
  recorded_at_ns INTEGER NOT NULL) STRICT;
CREATE TABLE disclosure_grant (
  id INTEGER PRIMARY KEY, recipient TEXT NOT NULL, fields_json TEXT NOT NULL, item_count INTEGER NOT NULL,
  payload_digest TEXT NOT NULL, confirmed_via TEXT NOT NULL CHECK (confirmed_via IN ('desktop_dialog','synthetic_policy')),
  created_at_ns INTEGER NOT NULL, expires_at_ns INTEGER NOT NULL, revoked_at_ns INTEGER, used_at_ns INTEGER) STRICT;
CREATE TABLE teacher_request (
  id INTEGER PRIMARY KEY, grant_id INTEGER NOT NULL UNIQUE REFERENCES disclosure_grant(id),
  payload_json TEXT NOT NULL, payload_digest TEXT NOT NULL, handle_map_json TEXT NOT NULL,
  state TEXT NOT NULL, started_at_ns INTEGER, finished_at_ns INTEGER, response_digest TEXT, rejection TEXT) STRICT;
CREATE TABLE teacher_label (                  -- a separate table: teacher labels are never human feedback
  id INTEGER PRIMARY KEY, request_id INTEGER NOT NULL REFERENCES teacher_request(id),
  object_ref_id INTEGER NOT NULL REFERENCES object_ref(id), label TEXT, abstain INTEGER NOT NULL,
  reason TEXT NOT NULL, evidence_json TEXT NOT NULL, taxonomy_version INTEGER NOT NULL,
  features_json TEXT NOT NULL, received_at_ns INTEGER NOT NULL) STRICT;
CREATE TABLE student_model (
  id TEXT PRIMARY KEY, algorithm TEXT NOT NULL, taxonomy_version INTEGER NOT NULL, dataset_digest TEXT NOT NULL,
  training_count INTEGER NOT NULL, state_json TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('candidate','active','retired')), fitted_at_ns INTEGER NOT NULL) STRICT;
CREATE TABLE collection (
  id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, kind TEXT NOT NULL CHECK (kind IN ('human','label_view')),
  created_at_ns INTEGER NOT NULL) STRICT;
CREATE TABLE collection_member (
  collection_id INTEGER NOT NULL REFERENCES collection(id) ON DELETE CASCADE,
  object_ref_id INTEGER NOT NULL REFERENCES object_ref(id), added_at_ns INTEGER NOT NULL,
  PRIMARY KEY (collection_id, object_ref_id)) STRICT;
CREATE TABLE tier_declaration (               -- human preference, append-only; latest row wins
  id INTEGER PRIMARY KEY, volume_key TEXT NOT NULL, tier INTEGER CHECK (tier BETWEEN 0 AND 9),
  declared_at_ns INTEGER NOT NULL) STRICT;
CREATE TABLE proposal (
  id INTEGER PRIMARY KEY, kind TEXT NOT NULL CHECK (kind = 'placement_simulation'),
  status TEXT NOT NULL CHECK (status IN ('simulation_only','stale')),
  inputs_digest TEXT NOT NULL, inputs_json TEXT NOT NULL, plan_json TEXT NOT NULL, created_at_ns INTEGER NOT NULL) STRICT;
