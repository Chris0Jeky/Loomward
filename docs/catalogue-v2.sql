-- DESIGN SCHEMA, NOT A MIGRATION OR AN IMPLEMENTED CATALOGUE.
-- Run only against a disposable empty database. IDs and exact timestamps are text.
-- JSON fields are versioned payloads; runtime validators and handle revalidation are required.
PRAGMA foreign_keys = ON;
BEGIN;
CREATE TABLE scope (id TEXT PRIMARY KEY, display_name TEXT NOT NULL, grant_version INTEGER NOT NULL, policy_json TEXT NOT NULL);
CREATE TABLE volume (id TEXT PRIMARY KEY, scope_id TEXT NOT NULL REFERENCES scope(id), identity_json TEXT NOT NULL, capabilities_json TEXT NOT NULL, observed_at_ns TEXT NOT NULL);
CREATE TABLE object (id TEXT PRIMARY KEY, volume_id TEXT NOT NULL REFERENCES volume(id), native_file_id TEXT NOT NULL, generation TEXT NOT NULL, kind TEXT NOT NULL, evidence_json TEXT NOT NULL, UNIQUE(volume_id,native_file_id,generation));
CREATE TABLE location (id TEXT PRIMARY KEY, object_id TEXT NOT NULL REFERENCES object(id), scope_id TEXT NOT NULL REFERENCES scope(id), path_encoding TEXT NOT NULL, path_bytes BLOB NOT NULL, observed_at_ns TEXT NOT NULL);
CREATE TABLE item_group (id TEXT PRIMARY KEY, scope_id TEXT NOT NULL REFERENCES scope(id), provider TEXT NOT NULL, anchor_json TEXT NOT NULL, policy_json TEXT NOT NULL);
CREATE TABLE group_member (group_id TEXT NOT NULL REFERENCES item_group(id), object_id TEXT NOT NULL REFERENCES object(id), PRIMARY KEY(group_id,object_id));
CREATE TABLE feature (object_id TEXT NOT NULL REFERENCES object(id), schema_version TEXT NOT NULL, source_version TEXT NOT NULL, consent_version INTEGER NOT NULL, payload_json TEXT NOT NULL, PRIMARY KEY(object_id,schema_version,source_version));
CREATE TABLE usage_observation (id TEXT PRIMARY KEY, object_id TEXT REFERENCES object(id), group_id TEXT REFERENCES item_group(id), provider TEXT NOT NULL, start_ns TEXT NOT NULL, end_ns TEXT NOT NULL, coverage_json TEXT NOT NULL, payload_json TEXT NOT NULL);
CREATE TABLE feedback (id TEXT PRIMARY KEY, scope_id TEXT NOT NULL REFERENCES scope(id), item_id TEXT NOT NULL, provenance TEXT NOT NULL CHECK(provenance IN ('human','teacher')), revision INTEGER NOT NULL CHECK(revision>0), taxonomy_version TEXT NOT NULL, payload_json TEXT NOT NULL);
CREATE TABLE model (id TEXT PRIMARY KEY, scope_id TEXT NOT NULL REFERENCES scope(id), feature_version TEXT NOT NULL, taxonomy_version TEXT NOT NULL, dataset_digest TEXT NOT NULL, evaluation_json TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('candidate','shadow','approved','retired')));
CREATE TABLE proposal (id TEXT PRIMARY KEY, scope_id TEXT NOT NULL REFERENCES scope(id), model_id TEXT REFERENCES model(id), manifest_digest TEXT NOT NULL, manifest_json TEXT NOT NULL, expires_at_ns TEXT NOT NULL, status TEXT NOT NULL);
CREATE TABLE approval (id TEXT PRIMARY KEY, proposal_id TEXT NOT NULL REFERENCES proposal(id), manifest_digest TEXT NOT NULL, grant_version INTEGER NOT NULL, expires_at_ns TEXT NOT NULL, proof_json TEXT NOT NULL);
CREATE TABLE operation (id TEXT PRIMARY KEY, proposal_id TEXT NOT NULL REFERENCES proposal(id), approval_id TEXT NOT NULL REFERENCES approval(id), state TEXT NOT NULL, journal_json TEXT NOT NULL, updated_at_ns TEXT NOT NULL);
CREATE TABLE backup_evidence (id TEXT PRIMARY KEY, group_id TEXT REFERENCES item_group(id), object_id TEXT REFERENCES object(id), provider TEXT NOT NULL, snapshot_identity TEXT NOT NULL, verified_at_ns TEXT NOT NULL, restore_test_json TEXT NOT NULL, coverage_json TEXT NOT NULL);
CREATE TABLE index_checkpoint (volume_id TEXT PRIMARY KEY REFERENCES volume(id), journal_id TEXT, next_usn TEXT, index_generation TEXT NOT NULL, status TEXT NOT NULL);
CREATE INDEX location_object ON location(object_id);
CREATE INDEX usage_group_time ON usage_observation(group_id,start_ns);
CREATE INDEX feedback_scope ON feedback(scope_id,revision);
COMMIT;
