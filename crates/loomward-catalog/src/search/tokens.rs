//! Keeping the token index (`search-token.sql`) equal to the tables: the writer calls
//! `drain_tokens` inside every transaction that may have changed a name.
use super::analyze::tokens;
use super::FILE_KEY;
use crate::Result;
use rusqlite::{params, Connection};

/// Rows handled per round: bounds the memory a 1M-row first scan needs.
const ROUND: i64 = 20_000;

/// The text the token index holds for a name: its analysed tokens, one space apart.
pub(crate) fn analysed(name: &str) -> String {
    tokens(name).join(" ")
}

/// Whether this store has the token index (it is created only on request, see `CatalogOptions`).
pub(crate) fn enabled(conn: &Connection) -> Result<bool> {
    Ok(conn
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='search_dirty')")?
        .query_row([], |r| r.get(0))?)
}

/// Whether the open-time backfill (`search_backfill`, #520) still has keys to index.
pub(crate) fn backfilling(conn: &Connection) -> Result<bool> {
    Ok(conn
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='search_backfill')")?
        .query_row([], |r| r.get(0))?)
}

/// Queue every existing row for the background backfill (#520): opening a store with the option
/// on must not index a whole catalogue inside the open transaction. A no-op on an empty store.
pub(crate) fn start_backfill(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!(
        "CREATE TABLE search_backfill(key INTEGER PRIMARY KEY) STRICT;
         INSERT INTO search_backfill SELECT id FROM dir WHERE parent_id IS NOT NULL ORDER BY id;
         INSERT INTO search_backfill SELECT id+{FILE_KEY} FROM file ORDER BY id;"
    ))?;
    if !conn.query_row("SELECT EXISTS(SELECT 1 FROM search_backfill)", [], |r| {
        r.get::<_, bool>(0)
    })? {
        conn.execute_batch("DROP TABLE search_backfill")?;
    }
    Ok(())
}

/// One bounded backfill round in the caller's transaction: index up to `ROUND` waiting keys under
/// their current names, and drop the table once it is empty. Whether keys remain.
pub(crate) fn backfill_round(conn: &Connection) -> Result<bool> {
    if !backfilling(conn)? {
        return Ok(false);
    }
    let rows: Vec<(i64, Option<String>)> = {
        let mut select = conn.prepare_cached(&format!(
            "SELECT b.key,coalesce(f.name,d.name) FROM search_backfill b
             LEFT JOIN file f ON b.key>={FILE_KEY} AND f.id=b.key-{FILE_KEY}
             LEFT JOIN dir d ON b.key<{FILE_KEY} AND d.id=b.key AND d.parent_id IS NOT NULL
             ORDER BY b.key LIMIT {ROUND}"
        ))?;
        let found = select
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        found
    };
    let Some(&(last, _)) = rows.last() else {
        conn.execute_batch("DROP TABLE search_backfill")?;
        return Ok(false);
    };
    let mut insert = conn.prepare_cached("INSERT INTO search_token(rowid,name) VALUES (?1,?2)")?;
    for (key, name) in &rows {
        if let Some(name) = name {
            insert.execute(params![key, analysed(name)])?;
        }
    }
    conn.execute("DELETE FROM search_backfill WHERE key<=?1", [last])?;
    if rows.len() < ROUND as usize {
        conn.execute_batch("DROP TABLE search_backfill")?;
        return Ok(false);
    }
    Ok(true)
}

/// Apply every queued key: drop its old entry (unless the key was new), then index the row's
/// current name if the row still exists. A key still waiting for the backfill has no entry yet:
/// it is indexed here as new and leaves the backfill. Idempotent; a no-op on an empty queue and
/// on a store without the token index.
pub(crate) fn drain_tokens(conn: &Connection) -> Result<()> {
    if !enabled(conn)? {
        return Ok(());
    }
    let (backfill, fresh) = if backfilling(conn)? {
        (
            "LEFT JOIN search_backfill b ON b.key=s.key",
            "s.fresh OR b.key IS NOT NULL",
        )
    } else {
        ("", "s.fresh")
    };
    loop {
        let rows: Vec<(i64, bool, Option<String>)> = {
            let mut select = conn.prepare_cached(&format!(
                "SELECT s.key,{fresh},coalesce(f.name,d.name) FROM search_dirty s {backfill}
                 LEFT JOIN file f ON s.key>={FILE_KEY} AND f.id=s.key-{FILE_KEY}
                 LEFT JOIN dir d ON s.key<{FILE_KEY} AND d.id=s.key AND d.parent_id IS NOT NULL
                 ORDER BY s.key LIMIT {ROUND}"
            ))?;
            let found = select
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            found
        };
        let Some(&(last, _, _)) = rows.last() else {
            return Ok(());
        };
        let mut delete = conn.prepare_cached("DELETE FROM search_token WHERE rowid=?1")?;
        let mut insert =
            conn.prepare_cached("INSERT INTO search_token(rowid,name) VALUES (?1,?2)")?;
        for (key, fresh, name) in &rows {
            if !fresh {
                delete.execute([key])?;
            }
            if let Some(name) = name {
                insert.execute(params![key, analysed(name)])?;
            }
        }
        if !backfill.is_empty() {
            conn.execute(
                "DELETE FROM search_backfill WHERE key IN (SELECT key FROM search_dirty WHERE key<=?1)",
                [last],
            )?;
        }
        conn.execute("DELETE FROM search_dirty WHERE key<=?1", [last])?;
    }
}

/// The insert triggers a first-scan swap holds off (#520): its rows are new keys, indexed in one
/// pass by `index_new` instead of being queued and read back by `drain_tokens`.
const INSERT_TRIGGERS: &str = "('search_tok_file_insert','search_tok_dir_insert')";

/// Drop the token insert triggers for the rest of this transaction, returning their definitions
/// for `restore_insert_triggers`. Empty on a store without the token index.
pub(crate) fn suspend_insert_triggers(conn: &Connection) -> Result<Vec<String>> {
    if !enabled(conn)? {
        return Ok(Vec::new());
    }
    let ddl = conn
        .prepare(&format!(
            "SELECT sql FROM sqlite_schema WHERE type='trigger' AND name IN {INSERT_TRIGGERS}"
        ))?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if ddl.len() != 2 {
        return Err(crate::Error::Invalid("token insert triggers missing"));
    }
    conn.execute_batch("DROP TRIGGER search_tok_file_insert; DROP TRIGGER search_tok_dir_insert;")?;
    Ok(ddl)
}

/// Recreate what `suspend_insert_triggers` dropped, byte for byte.
pub(crate) fn restore_insert_triggers(conn: &Connection, ddl: &[String]) -> Result<()> {
    for sql in ddl {
        conn.execute_batch(sql)?;
    }
    Ok(())
}

/// Index rows that are new to the token index in one pass: `select` yields (key, name) for keys
/// with no entry and no queued change, so this is what `drain_tokens` would do for them as fresh keys.
pub(crate) fn index_new(conn: &Connection, select: &str, root: i64) -> Result<()> {
    let mut rows = conn.prepare(select)?;
    let mut rows = rows.query([root])?;
    let mut insert = conn.prepare_cached("INSERT INTO search_token(rowid,name) VALUES (?1,?2)")?;
    while let Some(row) = rows.next()? {
        insert.execute(params![
            row.get::<_, i64>(0)?,
            analysed(&row.get::<_, String>(1)?)
        ])?;
    }
    Ok(())
}

/// Whether changes are waiting in the queue (a reader's honesty check).
pub(crate) fn pending(conn: &Connection) -> Result<bool> {
    Ok(
        conn.query_row("SELECT EXISTS(SELECT 1 FROM search_dirty)", [], |r| {
            r.get(0)
        })?,
    )
}
