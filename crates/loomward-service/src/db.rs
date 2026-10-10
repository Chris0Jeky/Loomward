//! The service's own connection: `catalog.db` as `main` with `state.db` attached as `st`, the same
//! layout the catalogue writer uses. Reads that the catalogue `Reader` does not project (roots,
//! volumes, grants, revisions) and the service's owner-state writes go through here. A write
//! touches `st` only, at `synchronous=FULL`; no transaction writes both files (ADR-V3-22).

use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::time::Duration;

pub type Sql<T> = rusqlite::Result<T>;

pub fn open(dir: &Path) -> Sql<Connection> {
    let conn = Connection::open(dir.join("catalog.db"))?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute(
        "ATTACH DATABASE ?1 AS st",
        [dir.join("state.db").to_string_lossy().as_ref()],
    )?;
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;PRAGMA st.synchronous=FULL;PRAGMA st.foreign_keys=ON",
    )?;
    Ok(conn)
}

/// `(catalog_rev, state_rev)`, read in one statement.
pub fn revisions(conn: &Connection) -> Sql<(i64, i64)> {
    conn.query_row(
        "SELECT (SELECT catalog_rev FROM main.revision),(SELECT state_rev FROM st.revision)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}

pub fn instance(conn: &Connection) -> Sql<String> {
    conn.query_row(
        "SELECT value FROM main.meta WHERE key='instance_id'",
        [],
        |r| r.get(0),
    )
}

/// The per-install node-ID key, created once from the OS random source. Not owner data, so it
/// does not move `state_rev`.
pub fn install_key(conn: &Connection) -> Sql<[u8; 32]> {
    let read = |conn: &Connection| -> Sql<Option<String>> {
        conn.query_row(
            "SELECT value FROM st.meta WHERE key='node_id_key'",
            [],
            |r| r.get(0),
        )
        .optional()
    };
    let hex = match read(conn)? {
        Some(h) => h,
        None => {
            let mut raw = [0u8; 32];
            getrandom::fill(&mut raw).expect("OS random source");
            let fresh: String = raw.iter().map(|b| format!("{b:02x}")).collect();
            conn.execute(
                "INSERT OR IGNORE INTO st.meta(key,value) VALUES('node_id_key',?1)",
                [&fresh],
            )?;
            read(conn)?.unwrap_or(fresh)
        }
    };
    // A damaged key must stop the service, never silently become a weaker (zeroed) key.
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            "the stored node-ID key is malformed".into(),
        ));
    }
    let mut key = [0u8; 32];
    for (i, b) in key.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("checked hex");
    }
    Ok(key)
}

/// `born_run` of each catalogue row id. Row ids are never reused (AUTOINCREMENT) and a row keeps
/// its `born_run` for life, so a lookup outside the reader's transaction cannot misattribute one.
pub fn born_runs(conn: &Connection, table: &str, ids: &[i64]) -> Sql<Vec<(i64, i64)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let list = serde_json::to_string(ids).expect("ids serialise");
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT id,born_run FROM main.{table} WHERE id IN (SELECT value FROM json_each(?1))"
    ))?;
    let rows = stmt
        .query_map([list], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect();
    rows
}

/// Catalogue root of a dir or file row.
pub fn root_of(conn: &Connection, dir: Option<i64>, file: Option<i64>) -> Sql<Option<i64>> {
    conn.query_row(
        "SELECT coalesce((SELECT root_id FROM main.dir WHERE id=?1),
                         (SELECT d.root_id FROM main.file f JOIN main.dir d ON d.id=f.dir_id WHERE f.id=?2))",
        params![dir, file],
        |r| r.get(0),
    )
}

pub fn root_generation(conn: &Connection, root: i64) -> Sql<Option<i64>> {
    conn.query_row(
        "SELECT generation FROM main.root WHERE id=?1",
        [root],
        |r| r.get(0),
    )
    .optional()
    .map(Option::flatten)
}

/// Directory row of an active root (the slice anchor for `Anchor::Root`).
pub fn active_root_dir(conn: &Connection, root: i64) -> Sql<Option<i64>> {
    conn.query_row(
        "SELECT d.id FROM main.dir d JOIN main.root r ON r.id=d.root_id JOIN st.root_grant g ON g.id=r.grant_id
         WHERE r.id=?1 AND d.parent_id IS NULL AND g.state='active'",
        [root],
        |r| r.get(0),
    )
    .optional()
}

#[derive(Debug, Clone)]
pub struct RootRow {
    pub root: i64,
    pub grant: i64,
    pub volume: Option<i64>,
    pub generation: Option<i64>,
    pub state: String,
    pub display_path: String,
    pub origin: String,
    pub granted_via: String,
    pub grant_state: String,
    pub granted_at_ns: i64,
    pub revoked_at_ns: Option<i64>,
    pub dir: Option<i64>,
    pub finished_at_ns: Option<i64>,
}

pub fn roots(conn: &Connection, only: Option<i64>) -> Sql<Vec<RootRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT r.id,r.grant_id,r.volume_id,r.generation,r.state,g.display_path,g.origin,g.granted_via,g.state,
                g.granted_at_ns,g.revoked_at_ns,
                (SELECT id FROM main.dir WHERE root_id=r.id AND parent_id IS NULL),
                (SELECT max(finished_at_ns) FROM main.scan_run WHERE root_id=r.id AND state='completed')
         FROM main.root r JOIN st.root_grant g ON g.id=r.grant_id
         WHERE ?1 IS NULL OR r.id=?1 ORDER BY r.id LIMIT 64",
    )?;
    let rows = stmt
        .query_map([only], |r| {
            Ok(RootRow {
                root: r.get(0)?,
                grant: r.get(1)?,
                volume: r.get(2)?,
                generation: r.get(3)?,
                state: r.get(4)?,
                display_path: r.get(5)?,
                origin: r.get(6)?,
                granted_via: r.get(7)?,
                grant_state: r.get(8)?,
                granted_at_ns: r.get(9)?,
                revoked_at_ns: r.get(10)?,
                dir: r.get(11)?,
                finished_at_ns: r.get(12)?,
            })
        })?
        .collect();
    rows
}

/// The catalogue root observing a grant.
pub fn root_for_grant(conn: &Connection, grant: i64) -> Sql<Option<i64>> {
    conn.query_row("SELECT id FROM main.root WHERE grant_id=?1", [grant], |r| {
        r.get(0)
    })
    .optional()
}

pub fn lab_root(conn: &Connection, volume_key: &str, file_id: &[u8]) -> Sql<Option<i64>> {
    conn.query_row(
        "SELECT id FROM st.lab_root WHERE volume_key=?1 AND root_file_id=?2",
        params![volume_key, file_id],
        |r| r.get(0),
    )
    .optional()
}

pub fn active_grant(conn: &Connection, volume_key: &str, file_id: &[u8]) -> Sql<Option<i64>> {
    conn.query_row(
        "SELECT id FROM st.root_grant WHERE volume_key=?1 AND root_file_id=?2 AND state='active' ORDER BY id DESC LIMIT 1",
        params![volume_key, file_id],
        |r| r.get(0),
    )
    .optional()
}

#[derive(Debug, Clone)]
pub struct VolumeRow {
    pub id: i64,
    pub key: String,
    pub display_name: String,
    pub filesystem: Option<String>,
    pub online: bool,
    pub observed_at_ns: i64,
    pub declared: Option<Option<i64>>,
}

pub fn volumes(conn: &Connection, only: Option<i64>) -> Sql<Vec<VolumeRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT v.id,v.volume_key,v.display_name,v.filesystem,v.online,v.observed_at_ns,
                (SELECT 1 FROM st.tier_declaration t WHERE t.volume_key=v.volume_key),
                (SELECT t.tier FROM st.tier_declaration t WHERE t.volume_key=v.volume_key ORDER BY t.declared_at_ns DESC,t.id DESC LIMIT 1)
         FROM main.volume v WHERE ?1 IS NULL OR v.id=?1 ORDER BY v.id LIMIT 64",
    )?;
    let rows = stmt
        .query_map([only], |r| {
            Ok(VolumeRow {
                id: r.get(0)?,
                key: r.get(1)?,
                display_name: r.get(2)?,
                filesystem: r.get(3)?,
                online: r.get(4)?,
                observed_at_ns: r.get(5)?,
                declared: r.get::<_, Option<i64>>(6)?.map(|_| r.get(7)).transpose()?,
            })
        })?
        .collect();
    rows
}

/// Appends a tier preference under an optional `state_rev` precondition, in one FULL `state.db`
/// transaction. `Ok(Err(current))` when the precondition failed and nothing was written.
pub fn declare_tier(
    conn: &mut Connection,
    volume_key: &str,
    tier: Option<i64>,
    expected_state_rev: Option<i64>,
    at_ns: i64,
) -> Sql<Result<i64, i64>> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let current: i64 = tx.query_row("SELECT state_rev FROM st.revision", [], |r| r.get(0))?;
    if expected_state_rev.is_some_and(|e| e != current) {
        return Ok(Err(current));
    }
    tx.execute(
        "INSERT INTO st.tier_declaration(volume_key,tier,declared_at_ns) VALUES(?1,?2,?3)",
        params![volume_key, tier, at_ns],
    )?;
    tx.execute(
        "UPDATE st.revision SET state_rev=state_rev+1 WHERE id=1",
        [],
    )?;
    let next: i64 = tx.query_row("SELECT state_rev FROM st.revision", [], |r| r.get(0))?;
    tx.commit()?;
    Ok(Ok(next))
}

#[cfg(test)]
pub fn declarations(conn: &Connection, volume_key: &str) -> Sql<Vec<Option<i64>>> {
    let mut stmt = conn
        .prepare_cached("SELECT tier FROM st.tier_declaration WHERE volume_key=?1 ORDER BY id")?;
    let rows = stmt.query_map([volume_key], |r| r.get(0))?.collect();
    rows
}

pub fn counts(conn: &Connection) -> Sql<(i64, i64)> {
    conn.query_row(
        "SELECT (SELECT count(*) FROM main.file),(SELECT count(*) FROM main.dir)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}

/// Registers a lab root in a synthetic `state.db` (ADR-V3-15). FULL durability; not owner data.
pub fn insert_lab_root(
    conn: &Connection,
    volume_key: &str,
    file_id: &[u8],
    manifest_digest: &str,
    seed: &str,
    tier: &str,
    at_ns: i64,
) -> Sql<i64> {
    conn.execute(
        "INSERT INTO st.lab_root(volume_key,root_file_id,manifest_digest,seed,tier,registered_at_ns) VALUES(?1,?2,?3,?4,?5,?6)",
        params![volume_key, file_id, manifest_digest, seed, tier, at_ns],
    )?;
    Ok(conn.last_insert_rowid())
}
