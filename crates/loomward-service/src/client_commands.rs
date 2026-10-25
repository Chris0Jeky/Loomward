//! Owner-only MCP disclosure controls (docs/48 E6); client admission stays in `clients`.

use crate::{fail, sql_error, ts_ns, Inner, Out, CLIENT_FIELDS};
use loomward_protocol::{ClientGrantRequest, ClientRevokeRequest, ErrorBody, ErrorCode, Timestamp};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

type Handled = Result<Out, ErrorBody>;

fn result(value: Value) -> Handled {
    Ok(Out {
        result: value,
        catalog_rev: None,
        state_rev: None,
    })
}

fn expiry_ns(at: &Timestamp) -> Result<i64, ErrorBody> {
    let ft = loomward_catalog::filetime(at.as_ref())
        .ok_or_else(|| fail(ErrorCode::InvalidRequest, "expiry timestamp out of range"))?;
    let ns = (i128::from(ft) - 116_444_736_000_000_000) * 100;
    // FILETIME has 100 ns precision; retain the timestamp's final two fractional digits.
    let fraction = at
        .as_str()
        .split_once('.')
        .map_or("", |(_, f)| f.trim_end_matches('Z'));
    let tail: i128 = format!("{fraction:0<9}")[7..].parse().unwrap_or(0);
    i64::try_from(ns + tail)
        .ok()
        .filter(|ns| *ns >= 0)
        .ok_or_else(|| fail(ErrorCode::InvalidRequest, "expiry timestamp out of range"))
}

impl Inner {
    pub(crate) fn clients_grant(&self, r: ClientGrantRequest) -> Handled {
        let fields: Vec<String> = r
            .fields
            .iter()
            .map(|f| {
                serde_json::to_value(f)
                    .expect("field enum")
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
        let expires = r.expires_at.as_ref().map(expiry_ns).transpose()?;
        let id = self.grant_client(r.client.as_ref(), &r.roots, &fields, expires)?;
        result(json!({"grant_id": id}))
    }

    pub(crate) fn clients_revoke(&self, r: ClientRevokeRequest) -> Handled {
        let revoked = self.revoke_client(r.client.as_ref())?;
        result(json!({"client": r.client, "revoked": revoked}))
    }

    pub(crate) fn clients_list(&self) -> Handled {
        let conn = self.db();
        let mut stmt = conn.prepare(
            "SELECT id,client,roots_json,fields_json,created_at_ns,expires_at_ns,revoked_at_ns,durable_roots FROM st.client_grant WHERE client LIKE 'mcp:%' ORDER BY id"
        ).map_err(sql_error)?;
        let mut rows = stmt.query([]).map_err(sql_error)?;
        let mut grants = Vec::new();
        while let Some(row) = rows.next().map_err(sql_error)? {
            let id: i64 = row.get(0).map_err(sql_error)?;
            let client: String = row.get(1).map_err(sql_error)?;
            let root_json: String = row.get(2).map_err(sql_error)?;
            let fields: String = row.get(3).map_err(sql_error)?;
            let created: i64 = row.get(4).map_err(sql_error)?;
            let expires: Option<i64> = row.get(5).map_err(sql_error)?;
            let revoked: Option<i64> = row.get(6).map_err(sql_error)?;
            let durable: bool = row.get(7).map_err(sql_error)?;
            let unreadable = || fail(ErrorCode::InternalError, "unreadable client grant");
            let fields: Value = serde_json::from_str(&fields).map_err(|_| unreadable())?;
            let mut roots = Vec::new();
            if durable {
                let ids: Vec<i64> = serde_json::from_str(&root_json).map_err(|_| unreadable())?;
                for grant in ids {
                    let root: Option<i64> = conn
                        .query_row("SELECT id FROM main.root WHERE grant_id=?1", [grant], |r| {
                            r.get(0)
                        })
                        .optional()
                        .map_err(sql_error)?;
                    if let Some(root) = root {
                        roots.push(self.root_id(root));
                    }
                }
            }
            grants.push(json!({"grant_id": format!("cg_{id}"), "client": client,
                "roots": roots, "fields": fields, "created_at": ts_ns(created),
                "expires_at": expires.map(ts_ns), "revoked_at": revoked.map(ts_ns)}));
        }
        result(json!({"grants": grants, "available_fields": CLIENT_FIELDS}))
    }
}
