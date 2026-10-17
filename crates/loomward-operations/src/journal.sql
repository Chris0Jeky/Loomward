-- Copy-and-verify write-ahead journal (doc 47 section 5, lane C3). Lives in state.db (synchronous=FULL).
-- Append-only and hash-chained per operation: replay rejects any gap, reorder or edited body.
CREATE TABLE IF NOT EXISTS op_journal (
  op_id INTEGER NOT NULL CHECK (op_id > 0),
  seq INTEGER NOT NULL CHECK (seq > 0),
  body TEXT NOT NULL,                              -- JSON entry: fence epoch plus one record
  chain BLOB NOT NULL CHECK (length(chain) = 32),  -- SHA-256(previous chain, op_id, seq, body)
  PRIMARY KEY (op_id, seq)) STRICT, WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS op_journal_no_update BEFORE UPDATE ON op_journal
  BEGIN SELECT RAISE(ABORT, 'op_journal is append-only'); END;
CREATE TRIGGER IF NOT EXISTS op_journal_no_delete BEFORE DELETE ON op_journal
  BEGIN SELECT RAISE(ABORT, 'op_journal is append-only'); END;
