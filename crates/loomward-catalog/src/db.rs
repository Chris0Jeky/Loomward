use crate::{Error, Result, APPLICATION_ID, SCHEMA_VERSION};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{path::Path, time::Duration};

pub(crate) fn local_path(path: &Path) -> Result<()> {
    let text = path.to_string_lossy().replace('/', "\\");
    let upper = text.to_ascii_uppercase();
    if upper.starts_with("\\\\?\\UNC\\")
        || upper.starts_with("\\\\.\\")
        || (upper.starts_with("\\\\") && !upper.starts_with("\\\\?\\"))
    {
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

fn inspect(path: &Path, dataset: &str) -> Result<i64> {
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
    let check: String = conn
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(inspection_error)?;
    if check != "ok" {
        return Err(Error::CorruptDatabase);
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
    let (key, id, path, state): (String, Vec<u8>, String, String) = conn.query_row(
        "SELECT volume_key,root_file_id,display_path,state FROM st.root_grant WHERE id=?1",
        [grant],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    if state != "active" {
        return Err(Error::NotFound);
    }
    let name = observation.map_or(path.as_str(), |o| o.display_name.as_str());
    let filesystem = observation.and_then(|o| o.filesystem.as_deref());
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
        conn.execute("INSERT INTO dir(root_id,parent_id,name,file_id,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run) VALUES(?1,NULL,?2,?3,?4,'post_open',0,0,'unlisted',0,0)",rusqlite::params![root,name,id,if id.len()==16{"native_file_id_128"}else{"native_file_id_64"}])?;
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
            std::fs::rename(
                file,
                dir.join(format!("catalog.{label}-{stamp}.db{suffix}")),
            )?;
        }
    }
    Ok(())
}
pub(crate) fn open_pair(dir: &Path, dataset: &str) -> Result<Connection> {
    let state = dir.join("state.db");
    let catalog = dir.join("catalog.db");
    let sv = inspect(&state, dataset)?;
    let cv = match inspect(&catalog, dataset) {
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
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "cache_size", -65536)?;
    conn.set_prepared_statement_cache_capacity(128);
    conn.execute(
        "ATTACH DATABASE ?1 AS st",
        [state.to_string_lossy().as_ref()],
    )?;
    conn.execute_batch("PRAGMA st.synchronous=FULL;PRAGMA st.foreign_keys=ON")?;
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
    let tx = conn.transaction()?;
    let grants = {
        let mut s = tx.prepare("SELECT id FROM st.root_grant WHERE state='active'")?;
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
        crate::writer::rollup(&tx, root, false)?;
        tx.execute(
            "UPDATE root SET state='partial',active_run=NULL WHERE id=?1",
            [root],
        )?;
    }
    tx.execute("UPDATE scan_run SET state='failed',error_json='{\"code\":\"restart_interrupted\"}' WHERE state='running'",[])?;
    tx.execute("DELETE FROM stage_entry", [])?;
    tx.execute("UPDATE revision SET catalog_rev=catalog_rev+1", [])?;
    tx.commit()?;
    grant_view(&conn)?;
    Ok(conn)
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
}
