//! Journal storage in `state.db`. Every append validates the record against the replayed journal inside one
//! `BEGIN IMMEDIATE` transaction, so a rejected record (stale epoch, illegal step) is never stored.
use crate::journal::{Entry, Invalid, Record, Replay, Row};
use crate::manifest::Admitted;
use crate::recovery::{from_replay, Disk, Recovery};
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

/// Lane C6 (#535) test seam: fail a chosen append as the database would, to prove the executor stops
/// without a record and recovery settles honestly. Compiled only with the `fault-injection` feature.
#[cfg(feature = "fault-injection")]
pub mod fault {
    use crate::journal::Record;
    use rusqlite::ffi;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Kind {
        /// SQLITE_FULL: the database or disk is full.
        Full,
        /// SQLITE_BUSY: another connection holds the write lock past the busy timeout.
        Busy,
    }

    /// Where inside one append the failure lands. `Row` and `Head` roll the transaction back; `Ack` is a
    /// commit that is durable while its caller is told it failed (a lost acknowledgement).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Stage {
        /// After validation, before the journal row is inserted.
        Row,
        /// After the row, before the head anchor is written.
        Head,
        /// After the commit: the record is stored, the append still reports the error.
        Ack,
    }

    /// Called at each stage of every append with the record being appended; `Some` fails that stage.
    pub type Hook = Box<dyn FnMut(&Record, Stage) -> Option<Kind> + Send>;

    pub(super) fn fire(
        hook: &mut Option<Hook>,
        record: &Record,
        stage: Stage,
    ) -> rusqlite::Result<()> {
        let Some(kind) = hook.as_mut().and_then(|h| h(record, stage)) else {
            return Ok(());
        };
        let (code, text) = match kind {
            Kind::Full => (ffi::SQLITE_FULL, "database or disk is full"),
            Kind::Busy => (ffi::SQLITE_BUSY, "database is locked"),
        };
        Err(rusqlite::Error::SqliteFailure(
            ffi::Error::new(code),
            Some(text.into()),
        ))
    }
}

pub struct Store {
    conn: Connection,
    #[cfg(feature = "fault-injection")]
    fault: Option<fault::Hook>,
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

/// Called inside a transaction, so the head and tail belong to the same SQLite snapshot.
fn checked_tail(conn: &Connection, op: u64) -> rusqlite::Result<Result<(u64, Vec<u8>), Invalid>> {
    let read = |sql| -> rusqlite::Result<Option<(u64, Vec<u8>)>> {
        conn.query_row(sql, [op as i64], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get(1)?))
        })
        .optional()
    };
    let head = read("SELECT seq,chain FROM op_journal_head WHERE op_id=?1")?;
    let tail = read("SELECT seq,chain FROM op_journal WHERE op_id=?1 ORDER BY seq DESC LIMIT 1")?;
    Ok(match (head, tail) {
        (Some(head), Some(tail)) if head == tail => Ok(tail),
        (head, _) => Err(Invalid {
            seq: head.map_or(0, |(seq, _)| seq),
            why: "journal head does not match tail",
        }),
    })
}

impl Store {
    /// Open the journal's database with the precious-state pragmas of ADR-V3-22 and ensure its tables.
    pub fn open(path: &Path) -> rusqlite::Result<Store> {
        Store::new(Connection::open(path)?)
    }

    /// Adopt a connection to `state.db`, forcing the precious-state pragmas whatever it was set to.
    pub fn new(mut conn: Connection) -> rusqlite::Result<Store> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let anchored: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='op_journal_head')",
            [], |r| r.get(0),
        )?;
        tx.execute_batch(SCHEMA_SQL)?;
        if !anchored {
            // Trust legacy tails once, only when introducing the head table, never on later opens.
            tx.execute_batch(
                "INSERT INTO op_journal_head(op_id,seq,chain)
                SELECT op_id,seq,chain FROM op_journal AS j
                WHERE seq=(SELECT max(seq) FROM op_journal WHERE op_id=j.op_id)",
            )?;
        }
        tx.commit()?;
        Ok(Store {
            conn,
            #[cfg(feature = "fault-injection")]
            fault: None,
            cache: HashMap::new(),
        })
    }

    /// Install (or clear) the append fault hook. Test builds only.
    #[cfg(feature = "fault-injection")]
    pub fn set_fault(&mut self, hook: Option<fault::Hook>) {
        self.fault = hook;
    }

    /// Start a journal from a consumed approval (only `Approval::admit` makes an `Admitted`, and this
    /// takes it by value). Returns the new operation id.
    pub fn start(&mut self, epoch: u64, admitted: Admitted) -> Result<u64, AppendError> {
        let (digest, items) = (admitted.digest(), admitted.items());
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let op: i64 = tx.query_row(
            "SELECT coalesce(max(op_id),0)+1 FROM (SELECT op_id FROM op_journal UNION SELECT op_id FROM op_journal_head)",
            [], |r| r.get(0),
        )?;
        let op = op as u64;
        let mut replay = Replay::new(op);
        let row = replay.row_for(&Entry {
            epoch,
            record: Record::Started { digest, items },
        });
        replay.apply(&row)?;
        insert_row(&tx, op, &row)?;
        insert_head(&tx, op, &row)?;
        tx.commit()?;
        self.cache.insert(op, replay);
        Ok(op)
    }

    pub fn append(&mut self, op: u64, epoch: u64, record: Record) -> Result<u64, AppendError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (seq, chain) = checked_tail(&tx, op)??;
        let mut replay = match self.cache.remove(&op) {
            Some(r) if r.seq == seq && r.chain.as_slice() == chain => r,
            _ => Replay::from_rows(op, &load(&tx, op)?)?,
        };
        let entry = Entry { epoch, record };
        let row = replay.row_for(&entry);
        replay.apply(&row)?;
        // A fault returns here with `tx` dropped: SQLite rolls the append back, row and head together.
        #[cfg(feature = "fault-injection")]
        fault::fire(&mut self.fault, &entry.record, fault::Stage::Row)?;
        insert_row(&tx, op, &row)?;
        #[cfg(feature = "fault-injection")]
        fault::fire(&mut self.fault, &entry.record, fault::Stage::Head)?;
        insert_head(&tx, op, &row)?;
        tx.commit()?;
        #[cfg(feature = "fault-injection")]
        fault::fire(&mut self.fault, &entry.record, fault::Stage::Ack)?;
        self.cache.insert(op, replay);
        Ok(row.seq)
    }

    /// Record the executor's plan for `op`, once. Written after `started` and before any effect; a second
    /// write is refused, so recovery always reads the plan the run used.
    pub fn set_plan(&mut self, op: u64, plan: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO op_plan(op_id,plan) VALUES(?1,?2)",
            params![op as i64, plan],
        )?;
        Ok(())
    }

    pub fn plan(&self, op: u64) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT plan FROM op_plan WHERE op_id=?1",
                [op as i64],
                |r| r.get(0),
            )
            .optional()
    }

    /// The connection's `synchronous` level (2 = FULL, required for precious state, ADR-V3-22).
    pub fn synchronous(&self) -> rusqlite::Result<i64> {
        self.conn.query_row("PRAGMA synchronous", [], |r| r.get(0))
    }

    pub fn rows(&self, op: u64) -> rusqlite::Result<Vec<Row>> {
        load(&self.conn, op)
    }

    pub fn operations(&self) -> rusqlite::Result<Vec<u64>> {
        let mut s = self.conn.prepare(
            "SELECT op_id FROM op_journal UNION SELECT op_id FROM op_journal_head ORDER BY op_id",
        )?;
        let ops = s.query_map([], |r| Ok(r.get::<_, i64>(0)? as u64))?;
        ops.collect()
    }

    /// The current fence epoch and state as the journal replays them (`Err` when it does not replay).
    pub fn replay(&self, op: u64) -> rusqlite::Result<Result<Replay, Invalid>> {
        let tx = self.conn.unchecked_transaction()?;
        let replay = match checked_tail(&tx, op)? {
            Ok(_) => Replay::from_rows(op, &load(&tx, op)?),
            Err(e) => Err(e),
        };
        tx.commit()?;
        Ok(replay)
    }

    pub fn recover(&self, op: u64, disk: &impl Disk) -> rusqlite::Result<Recovery> {
        Ok(from_replay(op, self.replay(op)?, disk))
    }
}

fn insert_row(conn: &Connection, op: u64, row: &Row) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO op_journal(op_id,seq,body,chain) VALUES(?1,?2,?3,?4)",
        params![op as i64, row.seq as i64, row.body, row.chain],
    )?;
    Ok(())
}

fn insert_head(conn: &Connection, op: u64, row: &Row) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO op_journal_head(op_id,seq,chain) VALUES(?1,?2,?3)
         ON CONFLICT(op_id) DO UPDATE SET seq=excluded.seq,chain=excluded.chain",
        params![op as i64, row.seq as i64, row.chain],
    )?;
    Ok(())
}
