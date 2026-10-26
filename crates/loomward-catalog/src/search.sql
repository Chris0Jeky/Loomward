-- Name search index (q-34, q-36): derived from dir/file, kept exact by triggers in the same
-- transaction as every change, so any connection that writes the tables maintains it. One FTS5
-- trigram table over both kinds: a directory's key is its id, a file's is id + 2^48. The indexed
-- text is the name between two sentinels, U+0001 before and U+0002 after (stripped from the name),
-- so a whole-name and a name-start match are phrase queries on the same index as a substring.
-- `root` (#887) is the entry's root id as one trigram of three C1 control characters (five bits
-- each), so a client's MATCH is ANDed with its roots and out-of-grant rows are never visited. A
-- cleaned query never holds a control character, so a name phrase cannot match it; ids that share
-- a code (32768 apart) only cost a visibility check, which still runs per hit.
-- detail=full: phrases are exact, so every match tier is decided by SQLite under one case fold.
-- columnsize=0: search uses tier/key order, never bm25 or token counts. The external-content
-- view derives the exact indexed text without storing another copy. Deletes supply the old text
-- in the same transaction, including cascades and raw SQL writes: a cascade deletes the folder row
-- before its files, so the folder's BEFORE DELETE trigger removes its files' entries while their
-- root is still known, and the file trigger only handles files whose folder remains.
-- Root directories are not indexed.
-- Extension point: further facets (labels, collections, content metadata) get their own tables
-- keyed the same way; `root` is the one scoping column, never a facet.
CREATE VIEW IF NOT EXISTS search_name_content AS
  SELECT id AS rowid, char(1)||replace(replace(name,char(1),' '),char(2),' ')||char(2) AS name, char(128+(root_id>>10&31),128+(root_id>>5&31),128+(root_id&31)) AS root FROM dir WHERE parent_id IS NOT NULL
  UNION ALL
  SELECT f.id+281474976710656, char(1)||replace(replace(f.name,char(1),' '),char(2),' ')||char(2), char(128+(d.root_id>>10&31),128+(d.root_id>>5&31),128+(d.root_id&31)) FROM file f LEFT JOIN dir d ON d.id=f.dir_id;
CREATE VIRTUAL TABLE IF NOT EXISTS search_name USING fts5(name, root, tokenize='trigram case_sensitive 0', content='search_name_content', columnsize=0);
-- A 64 MiB in-memory term buffer per write transaction: a 1M-row first scan publishes in far fewer,
-- larger segments (measured: about 37 percent less insert time than the 1 MiB default).
INSERT INTO search_name(search_name, rank) VALUES('hashsize', 67108864);
CREATE TRIGGER IF NOT EXISTS search_file_insert AFTER INSERT ON file BEGIN
  INSERT INTO search_name(rowid,name,root) VALUES (new.id+281474976710656, char(1)||replace(replace(new.name,char(1),' '),char(2),' ')||char(2), (SELECT char(128+(root_id>>10&31),128+(root_id>>5&31),128+(root_id&31)) FROM dir WHERE id=new.dir_id));
END;
CREATE TRIGGER IF NOT EXISTS search_file_delete AFTER DELETE ON file WHEN EXISTS(SELECT 1 FROM dir WHERE id=old.dir_id) BEGIN
  INSERT INTO search_name(search_name,rowid,name,root) VALUES ('delete',old.id+281474976710656,char(1)||replace(replace(old.name,char(1),' '),char(2),' ')||char(2),(SELECT char(128+(root_id>>10&31),128+(root_id>>5&31),128+(root_id&31)) FROM dir WHERE id=old.dir_id));
END;
CREATE TRIGGER IF NOT EXISTS search_file_rename AFTER UPDATE OF name,dir_id ON file WHEN new.name IS NOT old.name OR new.dir_id IS NOT old.dir_id BEGIN
  INSERT INTO search_name(search_name,rowid,name,root) VALUES ('delete',old.id+281474976710656,char(1)||replace(replace(old.name,char(1),' '),char(2),' ')||char(2),(SELECT char(128+(root_id>>10&31),128+(root_id>>5&31),128+(root_id&31)) FROM dir WHERE id=old.dir_id));
  INSERT INTO search_name(rowid,name,root) VALUES (new.id+281474976710656, char(1)||replace(replace(new.name,char(1),' '),char(2),' ')||char(2), (SELECT char(128+(root_id>>10&31),128+(root_id>>5&31),128+(root_id&31)) FROM dir WHERE id=new.dir_id));
END;
CREATE TRIGGER IF NOT EXISTS search_dir_files_delete BEFORE DELETE ON dir BEGIN
  INSERT INTO search_name(search_name,rowid,name,root) SELECT 'delete',id+281474976710656,char(1)||replace(replace(name,char(1),' '),char(2),' ')||char(2),char(128+(old.root_id>>10&31),128+(old.root_id>>5&31),128+(old.root_id&31)) FROM file WHERE dir_id=old.id;
END;
CREATE TRIGGER IF NOT EXISTS search_dir_insert AFTER INSERT ON dir WHEN new.parent_id IS NOT NULL BEGIN
  INSERT INTO search_name(rowid,name,root) VALUES (new.id, char(1)||replace(replace(new.name,char(1),' '),char(2),' ')||char(2), char(128+(new.root_id>>10&31),128+(new.root_id>>5&31),128+(new.root_id&31)));
END;
CREATE TRIGGER IF NOT EXISTS search_dir_delete AFTER DELETE ON dir WHEN old.parent_id IS NOT NULL BEGIN
  INSERT INTO search_name(search_name,rowid,name,root) VALUES ('delete',old.id,char(1)||replace(replace(old.name,char(1),' '),char(2),' ')||char(2),char(128+(old.root_id>>10&31),128+(old.root_id>>5&31),128+(old.root_id&31)));
END;
CREATE TRIGGER IF NOT EXISTS search_dir_rename AFTER UPDATE OF name ON dir WHEN new.name IS NOT old.name AND new.parent_id IS NOT NULL BEGIN
  INSERT INTO search_name(search_name,rowid,name,root) VALUES ('delete',old.id,char(1)||replace(replace(old.name,char(1),' '),char(2),' ')||char(2),char(128+(old.root_id>>10&31),128+(old.root_id>>5&31),128+(old.root_id&31)));
  INSERT INTO search_name(rowid,name,root) VALUES (new.id, char(1)||replace(replace(new.name,char(1),' '),char(2),' ')||char(2), char(128+(new.root_id>>10&31),128+(new.root_id>>5&31),128+(new.root_id&31)));
END;
