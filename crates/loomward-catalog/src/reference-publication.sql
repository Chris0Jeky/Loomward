CREATE TABLE pending_reference (
  object_ref_id INTEGER PRIMARY KEY REFERENCES object_ref(id),
  catalog_instance TEXT NOT NULL, publication_token TEXT NOT NULL,
  row_id INTEGER, born_run INTEGER,
  CHECK ((row_id IS NULL) = (born_run IS NULL))
) STRICT;
