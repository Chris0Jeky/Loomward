//! Per-client disclosure grants (LW-067) and the grant-scoped shared lexical search adapter
//! (LW-069, docs/46 sections 4 and 8.3).
//!
//! A client principal (`ask:<class>`, `mcp:<name>`) is set by the in-process adapter that hosts
//! it, never by a payload. It reaches two reads only, `search.query` and `node.inspect`, and only
//! under a live owner-issued grant loaded from `state.db` on every call, so a revocation holds
//! from the next call on. The grant names durable root grants and optional fields: rows outside the
//! roots are absent from results, root states and the generation (the catalogue applies the scope
//! before anything is counted), and fields outside the grant are removed, not nulled. The grant's
//! row id is its epoch: cursors bind the client and the epoch, so a cursor dies with a revocation
//! and cannot be replayed by another client, a re-grant or the owner.
//!
//! The projection runs after the owner-shaped result passed its contract check, at the boundary,
//! then passes the client result contract check, so no handler or adapter redacts on its own.

use crate::handlers::encode;
use crate::{bare_row, fail, now_ns, sql_error, Inner, Out, Service};
use loomward_catalog::NodeKey;
use loomward_protocol::{
    Command, ErrorBody, ErrorCode, NodeRefRequest, RequestEnvelope, RootId, SearchRequest,
};
use rusqlite::{OptionalExtension, TransactionBehavior};
use serde_json::{Map, Value};

type Handled = Result<Out, ErrorBody>;

/// Legacy grants have no catalogue-instance witness, so their original mapping cannot be proven
/// after recovery. Invalidate them atomically with the format marker before serving any client.
pub(crate) fn migrate(conn: &mut rusqlite::Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let versioned: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('client_grant','st') WHERE name='durable_roots')",
        [],
        |r| r.get(0),
    )?;
    if !versioned {
        tx.execute_batch("ALTER TABLE st.client_grant ADD COLUMN durable_roots INTEGER NOT NULL DEFAULT 0 CHECK(durable_roots IN (0,1))")?;
    }
    tx.execute(
        "UPDATE st.client_grant SET revoked_at_ns=?1 WHERE durable_roots=0 AND revoked_at_ns IS NULL",
        [now_ns()],
    )?;
    tx.commit()
}

/// The optional fields a grant may disclose. Name, kind, extension, coverage, match and the
/// node reference are always part of a hit.
pub const CLIENT_FIELDS: [&str; 4] = ["path", "size", "modified", "attributes"];

/// A live grant as one call sees it.
#[derive(Debug, Clone)]
pub(crate) struct ClientGrant {
    pub client: String,
    /// The `client_grant` row id: a new grant is a new epoch.
    pub epoch: i64,
    /// Current catalogue IDs resolved from the still-active durable root grants.
    pub roots: Vec<i64>,
    pub fields: Vec<String>,
}

impl ClientGrant {
    fn has(&self, field: &str) -> bool {
        self.fields.iter().any(|f| f == field)
    }

    /// A filter on a withheld field would disclose it by bisection; so would an extension-only
    /// listing, which runs largest first. The catalogue's search reads a text as empty when it is
    /// only whitespace and control characters (its private `clean`), so the guard must too.
    pub(crate) fn check_filters(&self, r: &SearchRequest) -> Result<(), ErrorBody> {
        let no_text = r.text.chars().all(|c| c.is_whitespace() || c.is_control());
        let size =
            r.min_bytes.is_some() || r.max_bytes.is_some() || (no_text && r.extension.is_some());
        let modified = r.modified_after.is_some() || r.modified_before.is_some();
        if (size && !self.has("size")) || (modified && !self.has("modified")) {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "this filter needs a field the client's grant does not disclose",
            ));
        }
        Ok(())
    }
}

fn valid_client(client: &str) -> bool {
    let name = client
        .strip_prefix("ask:")
        .or_else(|| client.strip_prefix("mcp:"));
    name.is_some_and(|n| {
        (1..=64).contains(&n.len())
            && n.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    })
}

impl Service {
    /// Owner-issued disclosure grant for `client`; see [`Inner::grant_client`].
    pub fn grant_client(
        &self,
        client: &str,
        roots: &[RootId],
        fields: &[&str],
        expires_at_ns: Option<i64>,
    ) -> Result<String, ErrorBody> {
        self.inner
            .grant_client(client, roots, fields, expires_at_ns)
    }

    /// Revokes `client`'s live grant; see [`Inner::revoke_client`].
    pub fn revoke_client(&self, client: &str) -> Result<bool, ErrorBody> {
        self.inner.revoke_client(client)
    }
}

impl Inner {
    /// Owner-issued disclosure grant for `client` over `roots` with the optional `fields`
    /// (from [`CLIENT_FIELDS`]); `expires_at_ns` is a Unix time in nanoseconds. It replaces the
    /// client's live grant, if any, in one transaction: a new epoch. Returns the grant ID.
    pub(crate) fn grant_client(
        &self,
        client: &str,
        roots: &[RootId],
        fields: &[&str],
        expires_at_ns: Option<i64>,
    ) -> Result<String, ErrorBody> {
        if !valid_client(client)
            || roots.is_empty()
            || roots.len() > 64
            || fields.iter().any(|f| !CLIENT_FIELDS.contains(f))
        {
            return Err(fail(ErrorCode::InvalidRequest, "invalid client grant"));
        }
        let inner = self;
        let mut rows = roots
            .iter()
            .map(|r| inner.root_row(r.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;
        rows.sort_unstable();
        rows.dedup();
        let mut fields: Vec<&str> = fields.to_vec();
        fields.sort_unstable();
        fields.dedup();
        let _m = inner.mutation.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = inner.db();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let mut grants = Vec::with_capacity(rows.len());
        for row in &rows {
            let grant: Option<i64> = tx
                .query_row(
                    "SELECT r.grant_id FROM main.root r JOIN st.root_grant g ON g.id=r.grant_id WHERE r.id=?1 AND g.state='active'",
                    [row],
                    |r| r.get(0),
                )
                .optional()
                .map_err(sql_error)?;
            grants.push(grant.ok_or_else(|| fail(ErrorCode::NotFound, "unknown root"))?);
        }
        let now = now_ns();
        tx.execute(
            "UPDATE st.client_grant SET revoked_at_ns=?2 WHERE client=?1 AND revoked_at_ns IS NULL",
            rusqlite::params![client, now],
        )
        .map_err(sql_error)?;
        tx.execute(
            "INSERT INTO st.client_grant(client,roots_json,fields_json,created_at_ns,expires_at_ns,durable_roots) VALUES(?1,?2,?3,?4,?5,1)",
            rusqlite::params![
                client,
                Value::from(grants).to_string(),
                Value::from(fields).to_string(),
                now,
                expires_at_ns
            ],
        )
        .map_err(sql_error)?;
        let id = tx.last_insert_rowid();
        tx.commit().map_err(sql_error)?;
        Ok(format!("cg_{id}"))
    }

    /// Revokes `client`'s live grant; its next call and every cursor it holds are refused.
    /// Returns whether a live grant was revoked.
    pub(crate) fn revoke_client(&self, client: &str) -> Result<bool, ErrorBody> {
        let _m = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        let n = self
            .db()
            .execute(
                "UPDATE st.client_grant SET revoked_at_ns=?2 WHERE client=?1 AND revoked_at_ns IS NULL",
                rusqlite::params![client, now_ns()],
            )
            .map_err(sql_error)?;
        Ok(n > 0)
    }
}

/// The only commands a client principal reaches.
pub(crate) fn open_to_clients(command: Command) -> bool {
    matches!(command, Command::SearchQuery | Command::NodeInspect)
}

impl Inner {
    /// `client`'s live, unexpired grant, with its roots narrowed to those still granted to the
    /// owner. No grant, an expired one and a revoked one all read the same.
    pub(crate) fn client_grant(&self, client: &str) -> Result<ClientGrant, ErrorBody> {
        let conn = self.db();
        let row: Option<(i64, String, String)> = conn
            .query_row(
                "SELECT id,roots_json,fields_json FROM st.client_grant WHERE client=?1 AND durable_roots=1 AND revoked_at_ns IS NULL AND (expires_at_ns IS NULL OR expires_at_ns>?2)",
                rusqlite::params![client, now_ns()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(sql_error)?;
        let Some((epoch, roots, fields)) = row else {
            return Err(fail(
                ErrorCode::PermissionDenied,
                "this client has no disclosure grant",
            ));
        };
        let roots = conn
            .prepare("SELECT r.id FROM main.root r JOIN st.root_grant g ON g.id=r.grant_id WHERE g.state='active' AND g.id IN (SELECT value FROM json_each(?1)) ORDER BY r.id")
            .and_then(|mut s| {
                s.query_map([roots], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<i64>>>()
            })
            .map_err(sql_error)?;
        let fields = serde_json::from_str(&fields)
            .map_err(|_| fail(ErrorCode::InternalError, "unreadable client grant"))?;
        Ok(ClientGrant {
            client: client.to_owned(),
            epoch,
            roots,
            fields,
        })
    }

    /// A client's call, owner-shaped (the caller checks it against the contract, then projects).
    pub(crate) fn client_handle(
        &self,
        req: &RequestEnvelope,
        command: Command,
        grant: &ClientGrant,
        work: Option<&crate::ReadWork>,
    ) -> Handled {
        match command {
            Command::SearchQuery => self.search(req, work, Some(grant)),
            Command::NodeInspect => {
                // Scope first: a node outside the grant answers as an unknown one before a
                // generation check or a read could tell it apart (Codex review of #541).
                let r: NodeRefRequest = req.decode_payload()?;
                let key = self.node(&r.node_id, &mut self.read_reader(work)?)?;
                if !self
                    .node_root(key)?
                    .is_some_and(|root| grant.roots.contains(&root))
                {
                    return Err(fail(ErrorCode::NotFound, "unknown node"));
                }
                self.client_inspect(req, work, grant)
            }
            _ => Err(fail(
                ErrorCode::PermissionDenied,
                "this command is not open to a client",
            )),
        }
    }

    /// The catalogue root a directory or file lies under.
    fn node_root(&self, key: NodeKey) -> Result<Option<i64>, ErrorBody> {
        let (sql, id) = match key {
            NodeKey::Dir(id) => ("SELECT root_id FROM main.dir WHERE id=?1", id),
            NodeKey::File(id) => (
                "SELECT d.root_id FROM main.file f JOIN main.dir d ON d.id=f.dir_id WHERE f.id=?1",
                id,
            ),
            _ => return Ok(None),
        };
        self.db()
            .query_row(sql, [id], |r| r.get(0))
            .optional()
            .map_err(sql_error)
    }

    fn client_inspect(
        &self,
        req: &RequestEnvelope,
        work: Option<&crate::ReadWork>,
        grant: &ClientGrant,
    ) -> Handled {
        // The root is checked again on the read itself: the row may have moved in between.
        self.node_read(req, work, |reader, key| {
            let mut d = reader.inspect(key)?;
            // The owner's display path is the grant's absolute path; a client's starts at the
            // granted root's own name, as search's paths do, never at the folders above it.
            let path = reader.path(key)?;
            let text = path
                .ancestors
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join("\\");
            // Bounded as every display path is (1,024 characters).
            d.display_path.truncated |= path.truncated || text.chars().count() > 1024;
            d.display_path.text = text.chars().take(1024).collect();
            Ok(
                if bare_row(&d.root_id, "rt_").is_some_and(|r| grant.roots.contains(&r)) {
                    encode(&d)
                } else {
                    // As an unknown node: nothing says the node exists.
                    Err(fail(ErrorCode::NotFound, "unknown node"))
                },
            )
        })
    }
}

/// Removes what `grant` does not disclose from a checked result: absent, never null.
pub(crate) fn project(command: Command, grant: &ClientGrant, map: &mut Map<String, Value>) {
    match command {
        Command::SearchQuery => {
            for hit in map
                .get_mut("hits")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if let Some(hit) = hit.as_object_mut() {
                    if !grant.has("path") {
                        hit.remove("path");
                    }
                    if let Some(row) = hit.get_mut("row").and_then(Value::as_object_mut) {
                        project_row(grant, row);
                    }
                }
            }
        }
        Command::NodeInspect => {
            // A client sees the row, its root and, with `path`, where it is. Identity, times,
            // threads, exclusions, memberships and suggestions are the owner's.
            let mut out = Map::new();
            for key in ["row", "root_id", "display_path"] {
                if key == "display_path" && !grant.has("path") {
                    continue;
                }
                if let Some(v) = map.remove(key) {
                    out.insert(key.into(), v);
                }
            }
            if let Some(row) = out.get_mut("row").and_then(Value::as_object_mut) {
                project_row(grant, row);
            }
            *map = out;
        }
        _ => map.clear(),
    }
}

fn project_row(grant: &ClientGrant, row: &mut Map<String, Value>) {
    for (field, keys) in [
        ("path", &["location_hint"][..]),
        (
            "size",
            &["logical_bytes", "allocated_bytes", "files", "dirs"][..],
        ),
        ("modified", &["modified_at"][..]),
        ("attributes", &["attributes", "flags"][..]),
    ] {
        if !grant.has(field) {
            for k in keys {
                row.remove(*k);
            }
        }
    }
}

#[cfg(test)]
#[path = "clients_tests.rs"]
pub(crate) mod tests;
