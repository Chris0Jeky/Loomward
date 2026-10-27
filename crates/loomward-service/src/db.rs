//! The service's own connection: `catalog.db` as `main` with `state.db` attached as `st`, the same
//! layout the catalogue writer uses. Reads that the catalogue `Reader` does not project (roots,
//! volumes, grants, revisions) and the service's owner-state writes go through here. A write
//! touches `st` only, at `synchronous=FULL`; no transaction writes both files (ADR-V3-22).

use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::time::Duration;

pub type Sql<T> = rusqlite::Result<T>;

pub fn open(dir: &Path) -> Sql<Connection> {
    let dir = &loomward_catalog::sqlite_dir(dir);
    let conn = Connection::open(dir.join("catalog.db"))?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute(
        "ATTACH DATABASE ?1 AS st",
        [dir.join("state.db").to_string_lossy().as_ref()],
    )?;
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;PRAGMA st.synchronous=FULL;PRAGMA st.foreign_keys=ON",
    )?;
    // Every durable boundary of the service's owner-state writes, in a crash sweep only (q-39).
    loomward_catalog::crash_arm(&conn);
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS st.root_freshness (
        grant_id INTEGER PRIMARY KEY REFERENCES root_grant(id),
        catalog_instance TEXT NOT NULL,
        journal_id BLOB CHECK(journal_id IS NULL OR length(journal_id)=8),
        next_usn INTEGER,
        at_ns INTEGER NOT NULL,
        CHECK((journal_id IS NULL)=(next_usn IS NULL))
    )",
    )?;
    // #939 (docs/52 section 6.1): one relief verdict per reclaim candidate, valid only while the
    // root's generation, the node's incarnation (born_run) and the catalogue instance still match.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS st.reclaim_relief (
        grant_id INTEGER NOT NULL REFERENCES root_grant(id),
        node_kind TEXT NOT NULL CHECK(node_kind IN ('dir','file')),
        node_id INTEGER NOT NULL,
        volume_id INTEGER NOT NULL,
        generation INTEGER NOT NULL,
        incarnation INTEGER NOT NULL,
        catalog_instance TEXT NOT NULL,
        observed_at_ns INTEGER NOT NULL,
        unique_allocated_bytes INTEGER CHECK(unique_allocated_bytes IS NULL OR unique_allocated_bytes>=0),
        reason TEXT,
        CHECK((unique_allocated_bytes IS NULL)<>(reason IS NULL)),
        PRIMARY KEY(grant_id,node_kind,node_id)
    )",
    )?;
    // Repeat an interrupted forget's derived-state cleanup before serving this session.
    conn.execute_batch(
        "DELETE FROM st.root_freshness WHERE grant_id IN (SELECT grant_id FROM st.root_forgotten);
         DELETE FROM st.reclaim_relief WHERE grant_id IN (SELECT grant_id FROM st.root_forgotten)",
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
    /// Complete with a current aggregate witness; a retained sub_complete bit alone is not proof.
    pub subtree_complete: bool,
}

/// At most [`MAX_ACTIVE_ROOTS`] rows (the contract's list cap): every active grant first, then the
/// newest revoked ones, returned in root id order. Grants are refused past that many active roots.
pub fn roots(conn: &Connection, only: Option<i64>) -> Sql<Vec<RootRow>> {
    rows(conn, only, 64)
}

/// The same projection for `grants.list`, whose contract allows 256 entries.
pub fn grant_rows(conn: &Connection) -> Sql<Vec<RootRow>> {
    rows(conn, None, 256)
}

fn rows(conn: &Connection, only: Option<i64>, limit: i64) -> Sql<Vec<RootRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT r.id,r.grant_id,r.volume_id,r.generation,r.state,g.display_path,g.origin,g.granted_via,g.state,
                g.granted_at_ns,g.revoked_at_ns,
                (SELECT id FROM main.dir WHERE root_id=r.id AND parent_id IS NULL),
                (SELECT max(finished_at_ns) FROM main.scan_run WHERE root_id=r.id AND state='completed'),
                coalesce((SELECT sub_complete AND agg_valid_rev>=dirty_rev FROM main.dir WHERE root_id=r.id AND parent_id IS NULL),0)
         FROM main.root r JOIN st.root_grant g ON g.id=r.grant_id
         WHERE (?1 IS NULL OR r.id=?1) AND g.id NOT IN (SELECT grant_id FROM st.root_forgotten) ORDER BY g.state='active' DESC,r.id DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![only, limit], |r| {
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
                subtree_complete: r.get(13)?,
            })
        })?
        .collect::<Sql<Vec<RootRow>>>();
    rows.map(|mut r| {
        r.sort_by_key(|r| r.root);
        r
    })
}

pub struct ForgottenGrant {
    pub grant: i64,
    pub root: i64,
    pub granted_via: String,
    pub granted_at_ns: i64,
    pub revoked_at_ns: Option<i64>,
    pub forgotten_at_ns: i64,
}

/// The anonymised records of forgotten roots (#288), newest first.
pub fn forgotten_grants(conn: &Connection, limit: i64) -> Sql<Vec<ForgottenGrant>> {
    conn.prepare_cached(
        "SELECT g.id,f.root_number,g.granted_via,g.granted_at_ns,g.revoked_at_ns,f.forgotten_at_ns
         FROM st.root_forgotten f JOIN st.root_grant g ON g.id=f.grant_id ORDER BY g.id DESC LIMIT ?1",
    )?
    .query_map([limit.max(0)], |r| {
        Ok(ForgottenGrant {
            grant: r.get(0)?,
            root: r.get(1)?,
            granted_via: r.get(2)?,
            granted_at_ns: r.get(3)?,
            revoked_at_ns: r.get(4)?,
            forgotten_at_ns: r.get(5)?,
        })
    })?
    .collect()
}

pub const MAX_ACTIVE_ROOTS: i64 = 64;

pub fn active_root_paths(conn: &Connection) -> Sql<Vec<std::path::PathBuf>> {
    conn.prepare_cached("SELECT display_path FROM st.root_grant WHERE state='active'")?
        .query_map([], |r| r.get::<_, String>(0).map(Into::into))?
        .collect()
}

pub fn active_grants(conn: &Connection) -> Sql<i64> {
    conn.query_row(
        "SELECT count(*) FROM st.root_grant WHERE state='active'",
        [],
        |r| r.get(0),
    )
}

/// The catalogue root observing a grant.
pub fn root_for_grant(conn: &Connection, grant: i64) -> Sql<Option<i64>> {
    conn.query_row("SELECT id FROM main.root WHERE grant_id=?1", [grant], |r| {
        r.get(0)
    })
    .optional()
}

pub fn lab_root(
    conn: &Connection,
    volume_key: &str,
    file_id: &[u8],
    volume_guid: &str,
) -> Sql<Option<String>> {
    conn.query_row(
        "SELECT manifest_digest FROM st.lab_root WHERE volume_key=?1 AND root_file_id=?2 AND volume_guid=?3",
        params![volume_key, file_id, volume_guid],
        |r| r.get(0),
    )
    .optional()
}

pub fn active_grant(
    conn: &Connection,
    volume_key: &str,
    file_id: &[u8],
    display_path: &str,
    volume_guid: &str,
) -> Sql<Option<(i64, String)>> {
    conn.query_row(
        "SELECT id,identity_mode FROM st.root_grant WHERE volume_key=?1 AND root_file_id=?2 AND state='active' AND display_path=?3 AND (volume_guid=?4 OR volume_guid IS NULL) ORDER BY id DESC LIMIT 1",
        params![volume_key, file_id, display_path, volume_guid],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
}

/// Active grants (id, identity-unverified, old path) whose folder now answers at another path:
/// renamed or moved (#669).
pub fn renamed_grants(
    conn: &Connection,
    volume_key: &str,
    file_id: &[u8],
    display_path: &str,
    volume_guid: &str,
) -> Sql<Vec<(i64, bool, String)>> {
    conn.prepare_cached(
        "SELECT id,identity_mode='unverified',display_path FROM st.root_grant WHERE volume_key=?1 AND root_file_id=?2 AND state='active' AND display_path<>?3 AND (volume_guid=?4 OR volume_guid IS NULL)",
    )?
    .query_map(params![volume_key, file_id, display_path, volume_guid], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    })?
    .collect()
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
        "SELECT v.id,v.volume_key,
                CASE WHEN EXISTS(SELECT 1 FROM main.root r JOIN st.root_forgotten f ON f.grant_id=r.grant_id WHERE r.volume_id=v.id)
                     THEN v.volume_key ELSE v.display_name END,
                v.filesystem,v.online,v.observed_at_ns,
                (SELECT 1 FROM st.tier_declaration t WHERE t.volume_key=v.volume_key),
                (SELECT t.tier FROM st.tier_declaration t WHERE t.volume_key=v.volume_key ORDER BY t.declared_at_ns DESC,t.id DESC LIMIT 1)
         FROM main.volume v WHERE ?1 IS NULL OR v.id=?1 ORDER BY v.id LIMIT 65",
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

/// The owner's alert thresholds as stored JSON (`meta` key `alert_settings`), or `None` before the
/// owner configures anything. Reads any connection that has `state.db` attached as `st`.
pub fn alert_settings(conn: &Connection) -> Sql<Option<String>> {
    conn.query_row(
        "SELECT value FROM st.meta WHERE key='alert_settings'",
        [],
        |r| r.get(0),
    )
    .optional()
}

/// Replaces the stored alert thresholds in one FULL `state.db` transaction and bumps `state_rev`.
pub fn set_alert_settings(conn: &mut Connection, json: &str) -> Sql<i64> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT INTO st.meta(key,value) VALUES('alert_settings',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [json],
    )?;
    tx.execute(
        "UPDATE st.revision SET state_rev=state_rev+1 WHERE id=1",
        [],
    )?;
    let next: i64 = tx.query_row("SELECT state_rev FROM st.revision", [], |r| r.get(0))?;
    tx.commit()?;
    Ok(next)
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
    volume_guid: &str,
    manifest_digest: &str,
    seed: &str,
    tier: &str,
) -> Sql<i64> {
    conn.query_row(
        "INSERT INTO st.lab_root(volume_key,root_file_id,manifest_digest,seed,tier,registered_at_ns,volume_guid) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(volume_key,root_file_id) DO UPDATE SET manifest_digest=excluded.manifest_digest,seed=excluded.seed,tier=excluded.tier,registered_at_ns=excluded.registered_at_ns,volume_guid=excluded.volume_guid RETURNING id",
        params![volume_key, file_id, manifest_digest, seed, tier, crate::now_ns(), volume_guid],
        |r| r.get(0),
    )
}
