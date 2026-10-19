use crate::{Error, Result, APPLICATION_ID, SCHEMA_VERSION};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{path::Path, time::Duration};

#[cfg(feature = "test-hooks")]
thread_local! {
    static INTEGRITY_CHECKS: std::cell::RefCell<Vec<std::path::PathBuf>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

/// Take this thread's integrity-check executions; absent from production builds.
#[cfg(feature = "test-hooks")]
pub fn take_integrity_checks() -> Vec<std::path::PathBuf> {
    INTEGRITY_CHECKS.with(|checks| checks.take())
}

pub(crate) fn local_path(path: &Path) -> Result<()> {
    let text = path.to_string_lossy().replace('/', "\\");
    let refused = if let Some(rest) = text.strip_prefix("\\\\?\\") {
        let b = rest.as_bytes();
        !(b.len() >= 2
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b.len() == 2 || b[2] == b'\\'))
    } else {
        text.starts_with("\\\\") || text.starts_with("\\??\\")
    };
    if refused {
        return Err(Error::Invalid("network or device database path"));
    }
    Ok(())
}

fn inspection_error(error: rusqlite::Error) -> Error {
    match error {
        rusqlite::Error::SqliteFailure(ref e, _)
            if matches!(
                e.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            ) =>
        {
            Error::CorruptDatabase
        }
        _ => Error::Sql(error),
    }
}

pub(crate) fn inspect(path: &Path, dataset: &str, integrity: bool) -> Result<i64> {
    if !path.exists() {
        return Ok(0);
    }
    local_path(&path.canonicalize()?)?;
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(inspection_error)?;
    let id: i64 = conn
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .map_err(inspection_error)?;
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(inspection_error)?;
    let tables: i64 = conn
        .query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))
        .map_err(inspection_error)?;
    if id != APPLICATION_ID && !(id == 0 && version == 0 && tables == 0) {
        return Err(Error::ForeignDatabase);
    }
    if version > SCHEMA_VERSION {
        return Err(Error::NewerDatabase);
    }
    if integrity {
        #[cfg(feature = "test-hooks")]
        INTEGRITY_CHECKS.with(|checks| checks.borrow_mut().push(path.to_path_buf()));
        let check: String = conn
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(inspection_error)?;
        if check != "ok" {
            return Err(Error::CorruptDatabase);
        }
    }
    if version > 0 {
        let class: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key='dataset_class'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(inspection_error)?;
        if class.as_deref() != Some(dataset) {
            return Err(Error::DatasetMismatch);
        }
    }
    Ok(version)
}

fn backup(conn: &Connection, path: &Path, label: &str) -> Result<std::path::PathBuf> {
    crate::crashpoint::hit("backup VACUUM INTO");
    let mut backup = path.with_file_name(format!("state.{label}.db"));
    let mut suffix = 0;
    while backup.exists() {
        suffix += 1;
        backup = path.with_file_name(format!("state.{label}-{suffix}.db"));
    }
    conn.execute("VACUUM INTO ?1", [backup.to_string_lossy().as_ref()])?;
    Ok(backup)
}
fn migrate(path: &Path, schema: &str, version: i64, dataset: &str, precious: bool) -> Result<()> {
    let mut conn = Connection::open(path)?;
    crate::crashpoint::arm(&conn);
    conn.busy_timeout(Duration::from_secs(5))?;
    if version < SCHEMA_VERSION {
        if precious {
            backup(&conn, path, &format!("before-v{version}"))?;
        }
        let tx = conn.transaction()?;
        if version == 1 {
            // Retain the entire old precious schema, including records lacking v2 provenance.
            let tables = {
                let mut s=tx.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'")?;
                let rows = s
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for table in &tables {
                tx.execute_batch(&format!(
                    "ALTER TABLE \"{table}\" RENAME TO \"legacy_v1_{table}\""
                ))?;
            }
            tx.execute_batch(schema)?;
            tx.execute_batch("INSERT INTO object_ref(id,volume_key,file_id,creation_ft,kind,state,retired) SELECT id,volume_key,file_id,0,kind,'unresolved',1 FROM legacy_v1_object_ref")?;
            for table in [
                "taxonomy",
                "student_model",
                "collection",
                "collection_member",
                "tier_declaration",
                "proposal",
            ] {
                tx.execute_batch(&format!(
                    "INSERT INTO {table} SELECT * FROM legacy_v1_{table}"
                ))?;
            }
            // Legacy labels/disclosures remain in their archived tables: no invented withdrawal or grant.
        } else if version == 0 {
            tx.execute_batch(schema)?;
        }
        if precious && version < 3 {
            tx.execute_batch(include_str!("reference-publication.sql"))?;
        }
        if precious && !tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('root_grant') WHERE name='identity_mode')", [], |r| r.get::<_, bool>(0))? {
            // v5 (#219): existing grants were all observed on identity-verifying volumes.
            tx.execute_batch("ALTER TABLE root_grant ADD COLUMN identity_mode TEXT NOT NULL DEFAULT 'verified' CHECK (identity_mode IN ('verified','unverified'))")?;
        }
        if precious && version < 7 {
            // v6 (backups stage 1): normalised evidence only; v5 above is #372. v7 (#288): the
            // anonymised forget record. Both are CREATE IF NOT EXISTS and keyed on version < 7, so a
            // v5 store gains both in order, a v6 store only v7's table, and a store an unreleased
            // branch build stamped v6 with only one of them still ends with both.
            tx.execute_batch(include_str!("backup-evidence.sql"))?;
            tx.execute_batch(include_str!("root-forgotten.sql"))?;
        }
        if precious && version < 8 {
            tx.execute_batch(include_str!("client-grant.sql"))?;
        }
        // v9 persists filesystem evidence; v10 binds grants to a volume GUID.
        // Column guards also accept the earlier scan branch's v9 GUID-only stores.
        if precious && !tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('root_grant') WHERE name='filesystem')", [], |r| r.get::<_, bool>(0))? {
            tx.execute_batch("ALTER TABLE root_grant ADD COLUMN filesystem TEXT CHECK (filesystem IS NULL OR length(filesystem) <= 32)")?;
        }
        if precious && !tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('root_grant') WHERE name='volume_guid')", [], |r| r.get::<_, bool>(0))? {
            tx.execute_batch("ALTER TABLE root_grant ADD COLUMN volume_guid TEXT")?;
        }
        // v11: lab provenance must be explicitly re-established, never adopted from a scan.
        if precious && !tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('lab_root') WHERE name='volume_guid')", [], |r| r.get::<_, bool>(0))? {
            tx.execute_batch("ALTER TABLE lab_root ADD COLUMN volume_guid TEXT")?;
        }
        if !precious && !tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('dir') WHERE name='listing_outcome')", [], |r| r.get::<_, bool>(0))? {
            tx.execute_batch("ALTER TABLE dir ADD COLUMN listing_outcome TEXT CHECK (listing_outcome IN ('complete','partial','denied','excluded','cancelled','unscanned','stale'))")?;
        }
        if version < 2 {
            tx.execute(
            "INSERT INTO meta(key,value) VALUES ('dataset_class',?1),('engine_version','0.3.0')",
            [dataset],
        )?;
            tx.execute(
                "INSERT INTO meta(key,value) VALUES ('instance_id',lower(hex(randomblob(16))))",
                [],
            )?;
        }
        tx.pragma_update(None, "application_id", APPLICATION_ID)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
    }
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(
        None,
        "synchronous",
        if precious { "FULL" } else { "NORMAL" },
    )?;
    if precious {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        backup(&conn, path, &format!("start-{stamp}"))?;
        let mut backups =
            std::fs::read_dir(path.parent().ok_or(Error::Invalid("state directory"))?)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|s| s.to_string_lossy().starts_with("state.start-"))
                })
                .collect::<Vec<_>>();
        backups.sort();
        let remove = backups.len().saturating_sub(7);
        for old in backups.into_iter().take(remove) {
            crate::crashpoint::hit("remove old start backup");
            std::fs::remove_file(old)?;
        }
    }
    Ok(())
}
pub(crate) fn derive_root(
    conn: &Connection,
    grant: i64,
    observation: Option<&crate::RootObservation>,
) -> Result<(i64, i64)> {
    let (key, id, path, state, mode, stored_filesystem): (String, Vec<u8>, String, String, String, Option<String>) = conn.query_row(
        "SELECT volume_key,root_file_id,display_path,state,identity_mode,filesystem FROM st.root_grant WHERE id=?1",
        [grant],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
    )?;
    if state != "active" && state != "identity_changed" {
        return Err(Error::NotFound);
    }
    let name = observation.map_or(path.as_str(), |o| o.display_name.as_str());
    let filesystem = observation
        .and_then(|o| o.filesystem.as_deref())
        .or(stored_filesystem.as_deref());
    let at = observation.map_or(0, |o| o.observed_at_ns);
    conn.execute("INSERT INTO volume(volume_key,display_name,filesystem,identity_json,capabilities_json,device_json,online,observed_at_ns) VALUES(?1,?2,?3,'{}','{}','{}',1,?4) ON CONFLICT(volume_key) DO UPDATE SET filesystem=coalesce(excluded.filesystem,volume.filesystem)",rusqlite::params![key,name,filesystem,at])?;
    let volume: i64 = conn.query_row("SELECT id FROM volume WHERE volume_key=?1", [key], |r| {
        r.get(0)
    })?;
    conn.execute("INSERT INTO root(grant_id,volume_id,root_file_id,state) VALUES(?1,?2,?3,'never_scanned') ON CONFLICT(grant_id) DO NOTHING",rusqlite::params![grant,volume,id])?;
    let root: i64 = conn.query_row("SELECT id FROM root WHERE grant_id=?1", [grant], |r| {
        r.get(0)
    })?;
    let dir: Option<i64> = conn
        .query_row(
            "SELECT id FROM dir WHERE root_id=?1 AND parent_id IS NULL",
            [root],
            |r| r.get(0),
        )
        .optional()?;
    let dir = if let Some(dir) = dir {
        dir
    } else {
        if mode == "unverified" {
            // #219: the grant's weak root ID binds the grant only; the row carries no identity.
            conn.execute("INSERT INTO dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,flags,listing_state,born_run,seen_run) VALUES(?1,NULL,?2,'path_observation','none',0,0,?3,'unlisted',0,0)",rusqlite::params![root,name,crate::IDENTITY_UNVERIFIED|crate::IDENTITY_UNAVAILABLE])?;
        } else {
            conn.execute("INSERT INTO dir(root_id,parent_id,name,file_id,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run) VALUES(?1,NULL,?2,?3,?4,'post_open',0,0,'unlisted',0,0)",rusqlite::params![root,name,id,if id.len()==16{"native_file_id_128"}else{"native_file_id_64"}])?;
        }
        conn.last_insert_rowid()
    };
    Ok((root, dir))
}
fn preserve_catalog(dir: &Path, label: &str) -> Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for suffix in ["", "-wal", "-shm"] {
        let file = dir.join(format!("catalog.db{suffix}"));
        if file.exists() {
            crate::crashpoint::hit("preserve catalogue rename");
            std::fs::rename(
                file,
                dir.join(format!("catalog.{label}-{stamp}.db{suffix}")),
            )?;
        }
    }
    Ok(())
}
#[cfg(test)]
pub(crate) fn open_pair(dir: &Path, dataset: &str) -> Result<Connection> {
    open_pair_with(dir, dataset, false)
}
/// `ranked`: create the ranked-search token index if the store lacks it (a store that has it keeps it).
#[cfg(test)]
pub(crate) fn open_pair_with(dir: &Path, dataset: &str, ranked: bool) -> Result<Connection> {
    open_pair_validated(dir, dataset, ranked, false)
}
pub(crate) fn open_pair_validated(
    dir: &Path,
    dataset: &str,
    ranked: bool,
    state_validated: bool,
) -> Result<Connection> {
    let state = dir.join("state.db");
    let catalog = dir.join("catalog.db");
    let sv = inspect(&state, dataset, !state_validated)?;
    let cv = match inspect(&catalog, dataset, true) {
        Err(Error::CorruptDatabase) => {
            preserve_catalog(dir, "corrupt")?;
            0
        }
        other => other?,
    };
    migrate(&state, include_str!("state.sql"), sv, dataset, true)?;
    if cv == 1 {
        // Consent migrates first; archive the derived file only after that durable commit.
        let mut st = Connection::open(&state)?;
        crate::crashpoint::arm(&st);
        st.pragma_update(None, "synchronous", "FULL")?;
        st.execute(
            "ATTACH DATABASE ?1 AS old",
            [catalog.to_string_lossy().as_ref()],
        )?;
        let tx = st.transaction()?;
        tx.execute_batch("INSERT INTO root_grant(id,volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns,revoked_at_ns) SELECT g.id,v.volume_key,coalesce(g.root_file_id,zeroblob(8)),g.display_path,g.origin,g.granted_via,CASE WHEN g.root_file_id IS NULL THEN 'identity_changed' ELSE g.state END,g.granted_at_ns,g.revoked_at_ns FROM old.root_grant g JOIN old.volume v ON v.id=g.volume_id WHERE NOT EXISTS(SELECT 1 FROM root_grant n WHERE n.id=g.id)")?;
        tx.commit()?;
        drop(st);
        preserve_catalog(dir, "before-v1")?;
    }
    migrate(
        &catalog,
        include_str!("catalog.sql"),
        if cv == 1 { 0 } else { cv },
        dataset,
        false,
    )?;
    let mut conn = Connection::open(catalog)?;
    crate::crashpoint::arm(&conn);
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // SQLite can allocate a sorter buffer as large as the main cache as well.
    conn.pragma_update(None, "cache_size", -131072)?;
    // Listing reconciliation and rollup scratch can be larger than the catalogue cache.
    conn.pragma_update(None, "temp_store", "FILE")?;
    conn.pragma_update(Some("temp"), "cache_size", -16384)?;
    conn.wal_hook(Some(|wal, pages| {
        crate::crashpoint::hit("post-commit");
        let threshold = if wal.name().to_bytes() == b"main" {
            16384
        } else {
            1000
        };
        if pages >= threshold {
            // Match SQLite's passive autocheckpoint; precious state keeps its default threshold.
            let _ = wal.checkpoint();
        }
        Ok(())
    }));
    conn.set_prepared_statement_cache_capacity(128);
    conn.execute(
        "ATTACH DATABASE ?1 AS st",
        [state.to_string_lossy().as_ref()],
    )?;
    conn.execute_batch(
        "PRAGMA st.synchronous=FULL;PRAGMA st.foreign_keys=ON;PRAGMA st.secure_delete=ON",
    )?;
    if cv >= 2 {
        // Also retries an interrupted migration's backfill; NULL stays unknown without an observation.
        let tx = conn.transaction()?;
        if tx.execute("UPDATE st.root_grant AS g SET filesystem=(SELECT v.filesystem FROM main.root r JOIN main.volume v ON v.id=r.volume_id WHERE r.grant_id=g.id) WHERE g.filesystem IS NULL AND EXISTS(SELECT 1 FROM main.root r JOIN main.volume v ON v.id=r.volume_id WHERE r.grant_id=g.id AND v.filesystem IS NOT NULL)", [])? > 0 {
            tx.execute("UPDATE st.revision SET state_rev=state_rev+1", [])?;
        }
        tx.commit()?;
    }
    reconcile_references(&mut conn)?;
    let instance: String =
        conn.query_row("SELECT value FROM meta WHERE key='instance_id'", [], |r| {
            r.get(0)
        })?;
    {
        let tx = conn.transaction()?;
        if tx.execute("UPDATE st.object_ref SET state='unresolved',retired=1 WHERE state='resolved' AND catalog_instance IS NOT ?1",[instance])?>0{tx.execute("UPDATE st.revision SET state_rev=state_rev+1",[])?;}
        tx.commit()?;
    }
    let mut tx = conn.transaction()?;
    let grants = {
        let mut s = tx
            .prepare("SELECT id FROM st.root_grant WHERE state IN ('active','identity_changed')")?;
        let rows = s
            .query_map([], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for grant in grants {
        derive_root(&tx, grant, None)?;
    }
    let interrupted = {
        let mut s = tx.prepare("SELECT DISTINCT root_id FROM scan_run WHERE state='running'")?;
        let rows = s
            .query_map([], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for root in interrupted {
        tx.execute("UPDATE root SET state='repairing' WHERE id=?1", [root])?;
        let repair = tx.savepoint()?;
        match crate::writer::rollup(&repair, root, false) {
            Ok(()) => {
                repair.execute(
                    "UPDATE root SET state='partial',active_run=NULL WHERE id=?1",
                    [root],
                )?;
                repair.commit()?;
            }
            Err(Error::Invalid("tree depth limit")) => {
                drop(repair);
                // Quarantine legacy overdeep trees without certifying their old sums or blocking other roots.
                tx.execute(
                    "UPDATE dir SET agg_valid_rev=dirty_rev-1,sub_complete=0 WHERE root_id=?1",
                    [root],
                )?;
                tx.execute("UPDATE root SET active_run=NULL WHERE id=?1", [root])?;
            }
            Err(e) => return Err(e),
        }
    }
    tx.execute("UPDATE scan_run SET state='failed',error_json='{\"code\":\"restart_interrupted\"}' WHERE state='running'",[])?;
    tx.execute("DELETE FROM stage_entry", [])?;
    // Add the derived catch-up index to existing catalogues without migrating precious state.
    tx.execute_batch("CREATE INDEX IF NOT EXISTS dir_missing_identity ON dir(root_id) WHERE file_id IS NULL AND listing_state!='absent_pending'")?;
    search_index(&tx, ranked)?;
    bulk_tables(&tx, false)?;
    tx.execute("UPDATE revision SET catalog_rev=catalog_rev+1", [])?;
    tx.commit()?;
    grant_view(&conn)?;
    // A forget committed its state half and stopped before the catalogue half: finish it.
    let tx = conn.transaction()?;
    let forgotten = {
        let mut s = tx
            .prepare("SELECT r.id FROM root r JOIN st.root_forgotten f ON f.grant_id=r.grant_id")?;
        let rows = s
            .query_map([], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    if !forgotten.is_empty() {
        // As the normal command: freed pages are zeroed and the logs cut once the rows are gone.
        tx.execute_batch("PRAGMA main.secure_delete=ON")?;
    }
    for &root in &forgotten {
        crate::writer::purge_root(&tx, root)?;
    }
    tx.commit()?;
    // Also when the catalogue half finished before the crash and only the rewrite is owed.
    if !forgotten.is_empty() || crate::writer::scrub_pending(&conn)? {
        crate::writer::wal_cut(&conn, Duration::from_millis(750), &mut true);
    }
    Ok(conn)
}
/// The name search indexes: the trigram table (q-34) always, the ranked-search token table (F2)
/// only where the caller asks for it (`ranked`) or the store already has it. Each is created with
/// its triggers by the first open that wants it. The trigram table is filled from the rows already
/// there in the same transaction; the token table's rows are queued for the writer's background
/// backfill (#520). A store opened without `ranked` that has no token table gets no new object.
pub(crate) fn search_index(conn: &Connection, ranked: bool) -> Result<()> {
    let has = |name: &str| -> Result<bool> {
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
            [name],
            |r| r.get::<_, bool>(0),
        )?)
    };
    if !has("search_name")? {
        conn.execute_batch(include_str!("search.sql"))?;
        conn.execute_batch(
            "INSERT INTO search_name(rowid,name) SELECT id, char(1)||replace(replace(name,char(1),' '),char(2),' ')||char(2) FROM dir WHERE parent_id IS NOT NULL;
             INSERT INTO search_name(rowid,name) SELECT id+281474976710656, char(1)||replace(replace(name,char(1),' '),char(2),' ')||char(2) FROM file;",
        )?;
    }
    if ranked && !has("search_token")? {
        conn.execute_batch(include_str!("search-token.sql"))?;
        // Every existing row waits for the writer's background backfill (#520): indexing them here
        // would hold the open for about 10 s per million names. `search_ranked` says "building" until it is done.
        crate::search::start_backfill(conn)?;
    }
    // Keys a raw writer queued while no writer was running.
    crate::search::drain_tokens(conn)?;
    Ok(())
}
/// First-scan bulk build tables (#148): the live `dir`/`file` definitions and the `dir` indexes,
/// but none of the `file` secondary indexes. Rebuilt at every open, which also discards any build
/// an interrupted run left behind; no reader queries them. `file` alone is recreated after a swap:
/// dropping it is far cheaper than deleting its rows one by one.
pub(crate) fn bulk_tables(conn: &Connection, file_only: bool) -> Result<()> {
    conn.execute_batch(if file_only {
        "DROP TABLE IF EXISTS bulk_file"
    } else {
        "DROP TABLE IF EXISTS bulk_file; DROP TABLE IF EXISTS bulk_dir"
    })?;
    let mut s = conn.prepare(
        "SELECT type,name,sql FROM sqlite_schema WHERE tbl_name IN ('dir','file') AND sql IS NOT NULL AND type IN ('table','index') AND (type='table' OR tbl_name='dir') AND NOT (?1 AND tbl_name='dir') ORDER BY type DESC,tbl_name",
    )?;
    let ddl = s
        .query_map([file_only], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (kind, name, sql) in ddl {
        let bulk = if kind == "table" {
            sql.replacen(
                &format!("CREATE TABLE {name} "),
                &format!("CREATE TABLE bulk_{name} "),
                1,
            )
            .replace("REFERENCES dir(id)", "REFERENCES bulk_dir(id)")
        } else {
            sql.replacen(&name, &format!("bulk_{name}"), 1)
                .replacen("ON dir(", "ON bulk_dir(", 1)
        };
        if !bulk.contains("bulk_") || bulk.contains("REFERENCES dir(") || bulk.contains("ON dir(") {
            return Err(Error::Invalid("bulk table definition"));
        }
        conn.execute_batch(&bulk)?;
    }
    conn.execute_batch("CREATE INDEX bulk_file_by_dir ON bulk_file(dir_id)")?;
    Ok(())
}
/// Automatic rebinds keep an existing survivor's reference; otherwise the lowest reference ID
/// wins. Losing references retain all meaning data and retire only when the witness commits.
pub(crate) fn resolve_pending_collisions(conn: &Connection, token: &str) -> Result<()> {
    conn.execute("WITH candidates AS MATERIALIZED (
        SELECT p.object_ref_id,
            row_number() OVER (PARTITION BY p.catalog_instance,o.kind,p.row_id,p.born_run ORDER BY o.id) AS rank,
            EXISTS(SELECT 1 FROM st.object_ref bound
                WHERE bound.catalog_instance=p.catalog_instance AND bound.kind=o.kind
                AND bound.row_id=p.row_id AND bound.born_run=p.born_run
                AND bound.state='resolved' AND bound.id!=o.id
                AND NOT EXISTS(SELECT 1 FROM st.pending_reference moving
                    WHERE moving.object_ref_id=bound.id AND moving.catalog_instance=p.catalog_instance
                    AND moving.publication_token=p.publication_token)) AS occupied
        FROM st.pending_reference p JOIN st.object_ref o ON o.id=p.object_ref_id
        WHERE p.publication_token=?1 AND p.catalog_instance=(SELECT value FROM meta WHERE key='instance_id')
        AND p.row_id IS NOT NULL
    ) UPDATE st.pending_reference SET row_id=NULL,born_run=NULL
        WHERE object_ref_id IN (SELECT object_ref_id FROM candidates WHERE occupied OR rank>1)", [token])?;
    Ok(())
}
/// Only state.db is written; read the publication witness before recovery changes main.
pub(crate) fn reconcile_references(conn: &mut Connection) -> Result<()> {
    if !conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM st.pending_reference)",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let instance: String =
        conn.query_row("SELECT value FROM meta WHERE key='instance_id'", [], |r| {
            r.get(0)
        })?;
    let token: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key='reference_publication'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let tx = conn.transaction()?;
    // Also repair conflicting intents persisted by older writers before #363.
    if let Some(token) = token.as_deref() {
        resolve_pending_collisions(&tx, token)?;
    }
    let changed = tx.execute("UPDATE st.object_ref AS o SET
        state=CASE WHEN p.row_id IS NULL THEN 'unresolved' ELSE 'resolved' END,
        retired=(p.row_id IS NULL), row_id=coalesce(p.row_id,o.row_id), born_run=coalesce(p.born_run,o.born_run)
        FROM st.pending_reference p WHERE o.id=p.object_ref_id AND p.catalog_instance=?1 AND p.publication_token=?2",
        rusqlite::params![instance,token])?;
    // An unmatched token never landed: discarding the intent preserves the original binding.
    tx.execute("DELETE FROM st.pending_reference", [])?;
    if changed > 0 {
        tx.execute("UPDATE st.revision SET state_rev=state_rev+1", [])?;
    }
    tx.commit()?;
    Ok(())
}
pub(crate) fn grant_view(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TEMP VIEW root_grant AS SELECT r.id,r.volume_id,g.volume_key,g.display_path,g.state,g.root_file_id,g.origin,g.granted_via,g.granted_at_ns,g.revoked_at_ns FROM main.root r JOIN st.root_grant g ON g.id=r.grant_id;
    CREATE TEMP VIEW visible_dir AS WITH RECURSIVE visible(id) AS (SELECT d.id FROM main.dir d JOIN root_grant g ON g.id=d.root_id WHERE d.parent_id IS NULL AND g.state='active' UNION ALL SELECT d.id FROM main.dir d JOIN visible v ON d.parent_id=v.id WHERE d.listing_state!='absent_pending') SELECT id FROM visible")?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn writer_connection_uses_full_for_precious_state_and_normal_for_catalogue() {
        let temp = tempfile::tempdir().unwrap();
        let conn = open_pair(temp.path(), "synthetic").unwrap();
        assert_eq!(
            conn.query_row("PRAGMA main.synchronous", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("PRAGMA st.synchronous", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn checkpoint_threshold_is_raised_only_for_catalogue() {
        let temp = tempfile::tempdir().unwrap();
        let conn = open_pair(temp.path(), "synthetic").unwrap();
        conn.execute_batch("PRAGMA main.wal_checkpoint(TRUNCATE); PRAGMA st.wal_checkpoint(TRUNCATE);
            CREATE TABLE main.checkpoint_probe(value BLOB); CREATE TABLE st.checkpoint_probe(value BLOB);
            INSERT INTO main.checkpoint_probe VALUES(zeroblob(5000000));
            INSERT INTO st.checkpoint_probe VALUES(zeroblob(5000000));").unwrap();
        let main: (i64, i64) = conn
            .query_row("PRAGMA main.wal_checkpoint(NOOP)", [], |r| {
                Ok((r.get(1)?, r.get(2)?))
            })
            .unwrap();
        let state: (i64, i64) = conn
            .query_row("PRAGMA st.wal_checkpoint(NOOP)", [], |r| {
                Ok((r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert!(main.0 > 1000 && main.0 < 16384);
        assert!(
            main.1 < main.0,
            "catalogue must not checkpoint at the state threshold"
        );
        assert!(state.0 > 1000);
        assert_eq!(
            state.0, state.1,
            "precious state must keep the default checkpoint"
        );
        assert_eq!(
            conn.query_row("PRAGMA st.synchronous", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            conn.query_row("PRAGMA st.cache_size", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            -2000
        );
    }
}
