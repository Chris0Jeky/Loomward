//! Journal storage in `state.db`. Every append validates the record against the replayed journal inside one
//! `BEGIN IMMEDIATE` transaction, so a rejected record (stale epoch, illegal step) is never stored.
use crate::journal::{Entry, Invalid, Record, Replay, Row};
use crate::recovery::{recover, Disk, Recovery};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::{collections::HashMap, path::Path, time::Duration};

/// Idempotent schema for the journal tables. Lane C5 folds it into the state.db migration.
pub const SCHEMA_SQL: &str = include_str!("journal.sql");

#[derive(Debug)]
pub enum AppendError {
    Sql(rusqlite::Error),
    /// The record does not follow from the journal (or the journal itself does not replay).
    Rejected(Invalid),
}

impl From<rusqlite::Error> for AppendError {
    fn from(e: rusqlite::Error) -> Self {
        AppendError::Sql(e)
    }
}

impl From<Invalid> for AppendError {
    fn from(e: Invalid) -> Self {
        AppendError::Rejected(e)
    }
}

pub struct Store {
    conn: Connection,
    /// Replays by operation, reused while the stored tail still matches (another writer, such as the
    /// revocation path, invalidates it by appending).
    cache: HashMap<u64, Replay>,
}

fn load(conn: &Connection, op: u64) -> rusqlite::Result<Vec<Row>> {
    let mut s =
        conn.prepare_cached("SELECT seq,body,chain FROM op_journal WHERE op_id=?1 ORDER BY seq")?;
    let rows = s.query_map([op as i64], |r| {
        Ok(Row {
            seq: r.get::<_, i64>(0)? as u64,
            body: r.get(1)?,
            chain: r.get(2)?,
        })
    })?;
    rows.collect()
}

impl Store {
    /// Open the journal's database with the precious-state pragmas of ADR-V3-22 and ensure its tables.
    pub fn open(path: &Path) -> rusqlite::Result<Store> {
        Store::new(Connection::open(path)?)
    }

    /// Adopt a connection to `state.db`, forcing the precious-state pragmas whatever it was set to.
    pub fn new(conn: Connection) -> rusqlite::Result<Store> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.execute_batch(SCHEMA_SQL)?;
        Ok(Store {
            conn,
            cache: HashMap::new(),
        })
    }

    /// Start a journal: the approval was consumed. Returns the new operation id.
    pub fn start(&mut self, epoch: u64, digest: [u8; 32], items: u32) -> Result<u64, AppendError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let op: i64 = tx.query_row("SELECT coalesce(max(op_id),0)+1 FROM op_journal", [], |r| {
            r.get(0)
        })?;
        let op = op as u64;
        let mut replay = Replay::new(op);
        let row = replay.row_for(&Entry {
            epoch,
            record: Record::Started { digest, items },
        });
        replay.apply(&row)?;
        insert(&tx, op, &row)?;
        tx.commit()?;
        self.cache.insert(op, replay);
        Ok(op)
    }

    pub fn append(&mut self, op: u64, epoch: u64, record: Record) -> Result<u64, AppendError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let tail: Option<(i64, Vec<u8>)> = tx
            .query_row(
                "SELECT seq,chain FROM op_journal WHERE op_id=?1 ORDER BY seq DESC LIMIT 1",
                [op as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((seq, chain)) = tail else {
            return Err(AppendError::Rejected(Invalid {
                seq: 0,
                why: "unknown operation",
            }));
        };
        let mut replay = match self.cache.remove(&op) {
            Some(r) if r.seq == seq as u64 && r.chain.as_slice() == chain => r,
            _ => Replay::from_rows(op, &load(&tx, op)?)?,
        };
        let row = replay.row_for(&Entry { epoch, record });
        replay.apply(&row)?;
        insert(&tx, op, &row)?;
        tx.commit()?;
        self.cache.insert(op, replay);
        Ok(row.seq)
    }

    /// The connection's `synchronous` level (2 = FULL, required for precious state, ADR-V3-22).
    pub fn synchronous(&self) -> rusqlite::Result<i64> {
        self.conn.query_row("PRAGMA synchronous", [], |r| r.get(0))
    }

    pub fn rows(&self, op: u64) -> rusqlite::Result<Vec<Row>> {
        load(&self.conn, op)
    }

    pub fn operations(&self) -> rusqlite::Result<Vec<u64>> {
        let mut s = self
            .conn
            .prepare("SELECT DISTINCT op_id FROM op_journal ORDER BY op_id")?;
        let ops = s.query_map([], |r| Ok(r.get::<_, i64>(0)? as u64))?;
        ops.collect()
    }

    /// The current fence epoch and state as the journal replays them (`Err` when it does not replay).
    pub fn replay(&self, op: u64) -> rusqlite::Result<Result<Replay, Invalid>> {
        Ok(Replay::from_rows(op, &self.rows(op)?))
    }

    pub fn recover(&self, op: u64, disk: &impl Disk) -> rusqlite::Result<Recovery> {
        Ok(recover(op, &self.rows(op)?, disk))
    }
}

fn insert(conn: &Connection, op: u64, row: &Row) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO op_journal(op_id,seq,body,chain) VALUES(?1,?2,?3,?4)",
        params![op as i64, row.seq as i64, row.body, row.chain],
    )?;
    Ok(())
}
