-- v8 (LW-067): an owner-issued disclosure grant per client principal (`ask:<class>`, `mcp:<name>`).
-- The row id is the grant's epoch: a re-grant is a new row, so a cursor bound to the old one dies.
-- `roots_json` holds catalogue root ids; `fields_json` the optional fields the client may see.
-- Never a credential, a path or a name: the roots are referenced by id.
CREATE TABLE IF NOT EXISTS client_grant (
  id INTEGER PRIMARY KEY AUTOINCREMENT, client TEXT NOT NULL,
  roots_json TEXT NOT NULL CHECK (json_valid(roots_json) AND json_type(roots_json)='array'),
  fields_json TEXT NOT NULL CHECK (json_valid(fields_json) AND json_type(fields_json)='array'),
  created_at_ns INTEGER NOT NULL, expires_at_ns INTEGER, revoked_at_ns INTEGER) STRICT;
CREATE UNIQUE INDEX IF NOT EXISTS client_grant_live ON client_grant(client) WHERE revoked_at_ns IS NULL;
