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

/// Apply every queued key: drop its old entry (unless the key was new), then index the row's
/// current name if the row still exists. Idempotent; a no-op on an empty queue.
pub(crate) fn drain_tokens(conn: &Connection) -> Result<()> {
    loop {
        let rows: Vec<(i64, bool, Option<String>)> = {
            let mut select = conn.prepare_cached(&format!(
                "SELECT s.key,s.fresh,coalesce(f.name,d.name) FROM search_dirty s
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
        conn.execute("DELETE FROM search_dirty WHERE key<=?1", [last])?;
    }
}

/// Whether changes are waiting in the queue (a reader's honesty check).
pub(crate) fn pending(conn: &Connection) -> Result<bool> {
    Ok(
        conn.query_row("SELECT EXISTS(SELECT 1 FROM search_dirty)", [], |r| {
            r.get(0)
        })?,
    )
}
