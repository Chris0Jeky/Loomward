-- Copy-and-verify write-ahead journal (doc 47 section 5, lane C3). Lives in state.db (synchronous=FULL).
-- Append-only and hash-chained per operation: replay rejects any gap, reorder or edited body.
CREATE TABLE IF NOT EXISTS op_journal (
  op_id INTEGER NOT NULL CHECK (op_id > 0),
  seq INTEGER NOT NULL CHECK (seq > 0),
  body TEXT NOT NULL,                              -- JSON entry: fence epoch plus one record
  chain BLOB NOT NULL CHECK (length(chain) = 32),  -- SHA-256(previous chain, op_id, seq, body)
  PRIMARY KEY (op_id, seq)) STRICT, WITHOUT ROWID;
-- Independent tail anchor; Store writes it in the journal append's transaction.
CREATE TABLE IF NOT EXISTS op_journal_head (
  op_id INTEGER PRIMARY KEY CHECK (op_id > 0),
  seq INTEGER NOT NULL CHECK (seq > 0),
  chain BLOB NOT NULL CHECK (length(chain) = 32)) STRICT;
-- REPLACE's implicit delete need not run the delete trigger (recursive_triggers defaults to off).
CREATE TRIGGER IF NOT EXISTS op_journal_no_replace BEFORE INSERT ON op_journal
  WHEN EXISTS (SELECT 1 FROM op_journal WHERE op_id=NEW.op_id AND seq=NEW.seq)
  BEGIN SELECT RAISE(ABORT, 'op_journal is append-only'); END;
CREATE TRIGGER IF NOT EXISTS op_journal_no_update BEFORE UPDATE ON op_journal
  BEGIN SELECT RAISE(ABORT, 'op_journal is append-only'); END;
CREATE TRIGGER IF NOT EXISTS op_journal_no_delete BEFORE DELETE ON op_journal
  BEGIN SELECT RAISE(ABORT, 'op_journal is append-only'); END;
-- Lane C5: the executor's plan for one operation (manifest plus the observed source signatures), written
-- once right after `started` and before any effect. Immutable: recovery reads the plan the run used.
CREATE TABLE IF NOT EXISTS op_plan (
  op_id INTEGER PRIMARY KEY CHECK (op_id > 0),
  plan TEXT NOT NULL) STRICT;
CREATE TRIGGER IF NOT EXISTS op_plan_no_update BEFORE UPDATE ON op_plan
  BEGIN SELECT RAISE(ABORT, 'op_plan is immutable'); END;
-- Durable plan lookups: written in the plan transaction, never used as effect authority.
CREATE TABLE IF NOT EXISTS op_plan_lookup (
  op_id INTEGER PRIMARY KEY CHECK (op_id > 0),
  wire_id TEXT UNIQUE) STRICT;
CREATE TABLE IF NOT EXISTS op_plan_grant (
  grant_id INTEGER NOT NULL,
  op_id INTEGER NOT NULL CHECK (op_id > 0),
  PRIMARY KEY (grant_id, op_id)) STRICT, WITHOUT ROWID;
-- Older indexed builds insert this same row after the plan; allow that exact retry only.
CREATE TRIGGER IF NOT EXISTS op_plan_lookup_same BEFORE INSERT ON op_plan_lookup
  WHEN EXISTS (SELECT 1 FROM op_plan_lookup WHERE op_id=NEW.op_id AND wire_id IS NEW.wire_id)
  BEGIN SELECT RAISE(IGNORE); END;
-- Plan-only writers (including rollbacks) must maintain both routing indexes too.
CREATE TRIGGER IF NOT EXISTS op_plan_index AFTER INSERT ON op_plan
  BEGIN
    INSERT INTO op_plan_lookup(op_id,wire_id)
      SELECT NEW.op_id, CASE WHEN json_type(p,'$.operation_id')='text'
        THEN nullif(json_extract(p,'$.operation_id'),'') END
      FROM (SELECT CASE WHEN json_valid(NEW.plan) THEN NEW.plan ELSE '{}' END AS p);
    INSERT OR IGNORE INTO op_plan_grant(grant_id,op_id)
      SELECT value, NEW.op_id
      FROM (SELECT CASE WHEN json_valid(NEW.plan) THEN NEW.plan ELSE '{}' END AS p),
        json_each(p, '$.grants')
      WHERE json_type(p,'$.grants')='array' AND type='integer';
  END;
