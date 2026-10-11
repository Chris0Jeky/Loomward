-- v6 (backups stage 1): normalised restic receipts, one per owner-chosen repository label. Holds the
-- normalised evidence only: never raw receipt bytes, a picked path, a file name or a credential.
CREATE TABLE IF NOT EXISTS backup_evidence (
  id INTEGER PRIMARY KEY AUTOINCREMENT, repository_label TEXT NOT NULL UNIQUE,
  evidence_json TEXT NOT NULL CHECK (json_valid(evidence_json)),
  snapshot_observed_ns INTEGER NOT NULL, integrity_observed_ns INTEGER,
  imported_at_ns INTEGER NOT NULL) STRICT;
