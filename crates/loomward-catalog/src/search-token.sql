-- Name token index (LW-113 F2, docs/46 section 5.2), the second tier beside the trigram table in
-- search.sql: the same keys (a folder's id, a file's id + 2^48).
--
-- What is indexed is the name's analysed tokens (src/search/analyze.rs): separators, camelCase and
-- letter/digit boundaries, the extension as its own token. A trigger holds no Rust, so the triggers
-- below only note which keys changed in `search_dirty`; the writer drains it in the same
-- transaction (`search::drain_tokens`, before every commit that changes names), and a store that
-- opens with rows in it drains them first. `fresh` is 1 when the key's first change in the window
-- was an insert, so there is no old entry to delete. Raw writers (tests, tools) are therefore
-- safe: their keys wait in the queue, and a ranked search over a non-empty queue says "building".
--
-- unicode61 then splits the analysed text on spaces and folds diacritics. Its token characters are
-- widened to letters, marks, numbers and symbols (a superset of what the analyser calls a token
-- character, which includes Thai and Devanagari vowel signs that the default set would split on),
-- so one analysed token is always one index token and a query term is never a phrase. prefix='1 2 3' indexes
-- one- to three-character prefixes (the short-query path). detail=none: the ranker uses
-- membership only (AND of prefixes), never phrases or bm25. contentless_delete: a delete needs only the key. Root directories
-- are not indexed. Context (the two nearest ancestor names) is not a column: a folder rename would
-- have to reindex everything below it, so the ranker joins the folder rows of this table at query
-- time instead.
CREATE VIRTUAL TABLE IF NOT EXISTS search_token USING fts5(name, tokenize='unicode61 remove_diacritics 2 categories ''L* M* N* S* Co''', prefix='1 2 3', detail=none, content='', contentless_delete=1);
INSERT INTO search_token(search_token, rank) VALUES('hashsize', 67108864);
CREATE TABLE IF NOT EXISTS search_dirty(key INTEGER PRIMARY KEY, fresh INTEGER NOT NULL) STRICT;
CREATE TRIGGER IF NOT EXISTS search_tok_file_insert AFTER INSERT ON file BEGIN
  INSERT OR IGNORE INTO search_dirty VALUES (new.id+281474976710656, 1);
END;
CREATE TRIGGER IF NOT EXISTS search_tok_file_delete AFTER DELETE ON file BEGIN
  INSERT OR IGNORE INTO search_dirty VALUES (old.id+281474976710656, 0);
END;
CREATE TRIGGER IF NOT EXISTS search_tok_file_rename AFTER UPDATE OF name ON file WHEN new.name IS NOT old.name BEGIN
  INSERT OR IGNORE INTO search_dirty VALUES (new.id+281474976710656, 0);
END;
CREATE TRIGGER IF NOT EXISTS search_tok_dir_insert AFTER INSERT ON dir WHEN new.parent_id IS NOT NULL BEGIN
  INSERT OR IGNORE INTO search_dirty VALUES (new.id, 1);
END;
CREATE TRIGGER IF NOT EXISTS search_tok_dir_delete AFTER DELETE ON dir WHEN old.parent_id IS NOT NULL BEGIN
  INSERT OR IGNORE INTO search_dirty VALUES (old.id, 0);
END;
CREATE TRIGGER IF NOT EXISTS search_tok_dir_rename AFTER UPDATE OF name ON dir WHEN new.name IS NOT old.name AND new.parent_id IS NOT NULL BEGIN
  INSERT OR IGNORE INTO search_dirty VALUES (new.id, 0);
END;
