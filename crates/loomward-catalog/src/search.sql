-- Name search index (q-34, q-36): derived from dir/file, kept exact by triggers in the same
-- transaction as every change, so any connection that writes the tables maintains it. One FTS5
-- trigram table over both kinds: a directory's key is its id, a file's is id + 2^48. The indexed
-- text is the name between two sentinels, U+0001 before and U+0002 after (stripped from the name),
-- so a whole-name and a name-start match are phrase queries on the same index as a substring.
-- detail=full: phrases are exact, so every match tier is decided by SQLite under one case fold.
-- contentless_delete: a delete needs only the key, never the old text. Root directories (the grant
-- itself) are not indexed. Extension point: further facets (labels, collections, content metadata)
-- get their own tables keyed the same way, never columns here.
CREATE VIRTUAL TABLE IF NOT EXISTS search_name USING fts5(name, tokenize='trigram case_sensitive 0', content='', contentless_delete=1);
-- A 64 MiB in-memory term buffer per write transaction: a 1M-row first scan publishes in far fewer,
-- larger segments (measured: about 37 percent less insert time than the 1 MiB default).
INSERT INTO search_name(search_name, rank) VALUES('hashsize', 67108864);
CREATE TRIGGER IF NOT EXISTS search_file_insert AFTER INSERT ON file BEGIN
  INSERT INTO search_name(rowid,name) VALUES (new.id+281474976710656, char(1)||replace(replace(new.name,char(1),' '),char(2),' ')||char(2));
END;
CREATE TRIGGER IF NOT EXISTS search_file_delete AFTER DELETE ON file BEGIN
  DELETE FROM search_name WHERE rowid=old.id+281474976710656;
END;
CREATE TRIGGER IF NOT EXISTS search_file_rename AFTER UPDATE OF name ON file WHEN new.name IS NOT old.name BEGIN
  DELETE FROM search_name WHERE rowid=old.id+281474976710656;
  INSERT INTO search_name(rowid,name) VALUES (new.id+281474976710656, char(1)||replace(replace(new.name,char(1),' '),char(2),' ')||char(2));
END;
CREATE TRIGGER IF NOT EXISTS search_dir_insert AFTER INSERT ON dir WHEN new.parent_id IS NOT NULL BEGIN
  INSERT INTO search_name(rowid,name) VALUES (new.id, char(1)||replace(replace(new.name,char(1),' '),char(2),' ')||char(2));
END;
CREATE TRIGGER IF NOT EXISTS search_dir_delete AFTER DELETE ON dir WHEN old.parent_id IS NOT NULL BEGIN
  DELETE FROM search_name WHERE rowid=old.id;
END;
CREATE TRIGGER IF NOT EXISTS search_dir_rename AFTER UPDATE OF name ON dir WHEN new.name IS NOT old.name AND new.parent_id IS NOT NULL BEGIN
  DELETE FROM search_name WHERE rowid=old.id;
  INSERT INTO search_name(rowid,name) VALUES (new.id, char(1)||replace(replace(new.name,char(1),' '),char(2),' ')||char(2));
END;
