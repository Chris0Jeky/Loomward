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

fn inspect(path: &Path, dataset: &str) -> Result<i64> {
    if !path.exists() {
        return Ok(0);
    }
    local_path(&path.canonicalize()?)?;
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let id: i64 = conn
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .map_err(|_| Error::CorruptDatabase)?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))?;
    if id != APPLICATION_ID && !(id == 0 && version == 0 && tables == 0) {
        return Err(Error::ForeignDatabase);
    }
    if version > SCHEMA_VERSION {
        return Err(Error::NewerDatabase);
    }
    let check: String = conn
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(|_| Error::CorruptDatabase)?;
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
            .optional()?;
        if class.as_deref() != Some(dataset) {
            return Err(Error::DatasetMismatch);
        }
    }
    Ok(version)
}

fn migrate(path: &Path, schema: &str, version: i64, dataset: &str, precious: bool) -> Result<()> {
    let mut conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    if version < SCHEMA_VERSION {
        if precious {
            // Never overwrite a previous migration backup.
            let mut backup = path.with_file_name(format!("state.before-v{}.db", version));
            let mut suffix = 0;
            while backup.exists() {
                suffix += 1;
                backup = path.with_file_name(format!("state.before-v{version}-{suffix}.db"));
            }
            conn.execute("VACUUM INTO ?1", [backup.to_string_lossy().as_ref()])?;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(schema)?;
        tx.execute(
            "INSERT INTO meta(key,value) VALUES ('dataset_class',?1),('engine_version','0.3.0')",
            [dataset],
        )?;
        tx.execute(
            "INSERT INTO meta(key,value) VALUES ('instance_id',lower(hex(randomblob(16))))",
            [],
        )?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
    }
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}

pub(crate) fn open_pair(dir: &Path, dataset: &str) -> Result<Connection> {
    let state = dir.join("state.db");
    let catalog = dir.join("catalog.db");
    // Preflight both before creating or migrating either file.
    let sv = inspect(&state, dataset)?;
    let cv = match inspect(&catalog, dataset) {
        Err(Error::CorruptDatabase) => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            std::fs::rename(&catalog, dir.join(format!("catalog.corrupt-{stamp}.db")))?;
            for suffix in ["-wal", "-shm"] {
                let path = dir.join(format!("catalog.db{suffix}"));
                if path.exists() {
                    std::fs::rename(
                        path,
                        dir.join(format!("catalog.corrupt-{stamp}.db{suffix}")),
                    )?;
                }
            }
            0
        }
        other => other?,
    };
    migrate(&state, include_str!("state.sql"), sv, dataset, true)?;
    migrate(&catalog, include_str!("catalog.sql"), cv, dataset, false)?;
    let mut conn = Connection::open(catalog)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "cache_size", -65536)?;
    conn.execute(
        "ATTACH DATABASE ?1 AS st",
        [state.to_string_lossy().as_ref()],
    )?;
    conn.execute_batch("PRAGMA st.synchronous=NORMAL")?;
    let tx = conn.transaction()?;
    let interrupted = {
        let mut stmt =
            tx.prepare("SELECT DISTINCT grant_id FROM scan_run WHERE state='running'")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    tx.execute("UPDATE dir SET state='stale',sub_complete=0 WHERE grant_id IN (SELECT grant_id FROM scan_run WHERE state='running')", [])?;
    for grant in interrupted {
        crate::writer::rollup(&tx, grant, false)?;
    }
    tx.execute(r#"UPDATE scan_run SET state='failed',error_json='{"code":"restart_interrupted"}' WHERE state='running'"#, [])?;
    tx.commit()?;
    Ok(conn)
}
