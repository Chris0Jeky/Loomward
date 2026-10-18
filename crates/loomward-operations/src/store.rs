//! Journal storage in `state.db`. Every append validates the record against the replayed journal inside one
//! `BEGIN IMMEDIATE` transaction, so a rejected record (stale epoch, illegal step) is never stored.
use crate::journal::{Entry, Invalid, Record, Replay, Row};
use crate::manifest::Admitted;
use crate::recovery::{from_replay, Disk, Recovery};
use rusqlite::{
    params, Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior,
};
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

// Decode only the routing metadata; manifests and source signatures are skipped without allocation.
#[derive(Default, serde::Deserialize)]
struct PlanLookup {
    #[serde(default)]
    operation_id: String,
    #[serde(default)]
    grants: Vec<i64>,
}

/// `backfill` keeps the first plan for a wire ID: old data may already hold a duplicate start, and a
/// UNIQUE abort there would stop every later open of the journal.
fn index_plan(conn: &Connection, op: u64, plan: &str, backfill: bool) -> rusqlite::Result<()> {
    // Old opaque/unreadable plans have no usable wire ID or grants, as before this index.
    let lookup = serde_json::from_str::<PlanLookup>(plan).unwrap_or_default();
    conn.execute(
        if backfill {
            "INSERT INTO op_plan_lookup(op_id,wire_id) VALUES(?1,?2) ON CONFLICT(wire_id) DO NOTHING"
        } else {
            "INSERT INTO op_plan_lookup(op_id,wire_id) VALUES(?1,?2)"
        },
        params![
            op as i64,
            (!lookup.operation_id.is_empty()).then_some(lookup.operation_id)
        ],
    )?;
    for grant in lookup.grants {
        conn.execute(
            "INSERT OR IGNORE INTO op_plan_grant(grant_id,op_id) VALUES(?1,?2)",
            params![grant, op as i64],
        )?;
    }
    Ok(())
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
        let current = {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let current: bool = tx.query_row(
                "SELECT count(*)=9 FROM sqlite_schema
                 WHERE (type='table' AND name IN ('op_journal','op_journal_head','op_plan',
                                                 'op_plan_lookup','op_plan_grant'))
                    OR (type='trigger' AND name IN ('op_journal_no_replace','op_journal_no_update',
                                                    'op_journal_no_delete','op_plan_no_update'))",
                [],
                |r| r.get(0),
            )?;
            tx.commit()?;
            current
        };
        if !current {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Re-check under the write lock: another opener may have migrated since our read.
            let anchored: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='op_journal_head')",
                [], |r| r.get(0),
            )?;
            let indexed: bool = tx.query_row(
                "SELECT count(*)=2 FROM sqlite_schema WHERE type='table'
                 AND name IN ('op_plan_lookup','op_plan_grant')",
                [],
                |r| r.get(0),
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
            if !indexed {
                // One-time legacy migration, one plan at a time, under the same write transaction.
                let mut plans = tx.prepare("SELECT op_id,plan FROM op_plan ORDER BY op_id")?;
                let mut rows = plans.query([])?;
                while let Some(row) = rows.next()? {
                    index_plan(
                        &tx,
                        row.get::<_, i64>(0)? as u64,
                        &row.get::<_, String>(1)?,
                        true,
                    )?;
                }
            }
            tx.commit()?;
        }
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
        let mut retries = 0;
        // Retry lock acquisition only; retrying a write or lost acknowledgement could duplicate it.
        let tx = loop {
            match Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate) {
                Ok(tx) => break tx,
                Err(e) if e.sqlite_error_code() == Some(ErrorCode::DatabaseBusy) && retries < 2 => {
                    std::thread::sleep(Duration::from_millis(25 << retries));
                    retries += 1;
                }
                Err(e) => return Err(e.into()),
            }
        };
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
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO op_plan(op_id,plan) VALUES(?1,?2)",
            params![op as i64, plan],
        )?;
        index_plan(&tx, op, plan, false)?;
        tx.commit()?;
        Ok(())
    }

    /// Resolve the service wire ID without reading a manifest. A match is not effect authority.
    pub fn operation_by_wire_id(&self, id: &str) -> rusqlite::Result<Option<u64>> {
        self.conn
            .query_row(
                "SELECT op_id FROM op_plan_lookup WHERE wire_id=?1",
                [id],
                |r| Ok(r.get::<_, i64>(0)? as u64),
            )
            .optional()
    }

    /// Operations using a grant, including recovered runs with no resident worker.
    pub fn operations_for_grant(&self, grant: i64) -> rusqlite::Result<Vec<u64>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT op_id FROM op_plan_grant WHERE grant_id=?1 ORDER BY op_id")?;
        let rows = stmt.query_map([grant], |r| Ok(r.get::<_, i64>(0)? as u64))?;
        rows.collect()
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
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Deferred)?;
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

#[cfg(test)]
mod tests {
    #[test]
    fn plan_lookup_is_written_with_the_plan() {
        let mut store = Store::new(Connection::open_in_memory().unwrap()).unwrap();
        store
            .set_plan(
                1,
                r#"{"operation_id":"cp_first","grants":[7,8,7],"manifest":{"items":[]}}"#,
            )
            .unwrap();
        let wire: String = store
            .conn
            .query_row(
                "SELECT wire_id FROM op_plan_lookup WHERE op_id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(wire, "cp_first");
        let grants: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM op_plan_grant WHERE op_id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(grants, 2);
        assert_eq!(store.operation_by_wire_id("cp_first").unwrap(), Some(1));
        assert_eq!(store.operation_by_wire_id("cp_missing").unwrap(), None);
        assert_eq!(store.operations_for_grant(7).unwrap(), vec![1]);
        assert!(store.operations_for_grant(9).unwrap().is_empty());
        // A duplicate wire ID must roll back the plan as well as its lookup entries.
        assert!(store
            .set_plan(2, r#"{"operation_id":"cp_first","grants":[9]}"#)
            .is_err());
        assert!(store.plan(2).unwrap().is_none());
        // A failure after the wire lookup was inserted must roll back all three tables.
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER fail_grant BEFORE INSERT ON op_plan_grant
            WHEN NEW.grant_id=9 BEGIN SELECT RAISE(ABORT,'injected grant write failure'); END;",
            )
            .unwrap();
        assert!(store
            .set_plan(2, r#"{"operation_id":"cp_second","grants":[8,9]}"#)
            .is_err());
        assert!(store.plan(2).unwrap().is_none());
        assert_eq!(store.operation_by_wire_id("cp_second").unwrap(), None);
        assert_eq!(store.operations_for_grant(8).unwrap(), vec![1]);
    }

    #[test]
    fn legacy_plan_lookup_migrates_once_without_rewriting_plans_or_journals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = Connection::open(&path).unwrap();
        // Actual pre-index schema, including the independent journal head and immutable plan.
        conn.execute_batch(include_str!(
            "../tests/fixtures/journal-before-plan-index.sql"
        ))
        .unwrap();
        let row = Replay::new(1).row_for(&Entry {
            epoch: 0,
            record: Record::Started {
                digest: [1; 32],
                items: 1,
            },
        });
        insert_row(&conn, 1, &row).unwrap();
        insert_head(&conn, 1, &row).unwrap();
        let plan = r#"{"operation_id":"cp_legacy","grants":[4,5],"manifest":{"items":[]}}"#;
        conn.execute("INSERT INTO op_plan VALUES(1,?1)", [plan])
            .unwrap();
        conn.execute("INSERT INTO op_plan VALUES(2,'not a readable plan')", [])
            .unwrap();
        conn.execute("INSERT INTO op_plan VALUES(3,'{}')", [])
            .unwrap();
        // A duplicate start left by the old start-once race: the backfill keeps the first and still opens.
        conn.execute("INSERT INTO op_plan VALUES(4,?1)", [plan])
            .unwrap();
        drop(conn);
        let store = Store::open(&path).unwrap();
        let wire: String = store
            .conn
            .query_row(
                "SELECT wire_id FROM op_plan_lookup WHERE op_id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(wire, "cp_legacy");
        assert_eq!(store.plan(1).unwrap().as_deref(), Some(plan));
        assert_eq!(
            store.plan(2).unwrap().as_deref(),
            Some("not a readable plan")
        );
        assert_eq!(store.rows(1).unwrap(), vec![row.clone()]);
        assert_eq!(store.replay(1).unwrap().unwrap().seq, 1);
        assert_eq!(store.operation_by_wire_id("cp_legacy").unwrap(), Some(1));
        assert_eq!(store.operation_by_wire_id("").unwrap(), None);
        // The duplicate keeps no wire ID (first wins) but stays grant-indexed, so a revoke fences it.
        assert_eq!(store.operations_for_grant(4).unwrap(), vec![1, 4]);
        let grants: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM op_plan_grant WHERE op_id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(grants, 2);
        drop(store);
        let locker = Connection::open(&path).unwrap();
        locker.execute_batch("BEGIN IMMEDIATE").unwrap();
        // Reopening an indexed database is read-only, even while another writer holds the lock.
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.plan(1).unwrap().as_deref(), Some(plan));
        assert_eq!(reopened.operation_by_wire_id("cp_legacy").unwrap(), Some(1));
        assert_eq!(reopened.operations_for_grant(5).unwrap(), vec![1]);
        assert_eq!(reopened.rows(1).unwrap(), vec![row]);
        locker.execute_batch("ROLLBACK").unwrap();
    }

    use super::*;
    use std::cell::{Cell, RefCell};

    fn started(path: &Path) -> Store {
        let store = Store::open(path).unwrap();
        let replay = Replay::new(1);
        let row = replay.row_for(&Entry {
            epoch: 1,
            record: Record::Started {
                digest: [1; 32],
                items: 1,
            },
        });
        insert_row(&store.conn, 1, &row).unwrap();
        insert_head(&store.conn, 1, &row).unwrap();
        store
    }

    fn intent() -> Record {
        Record::StagingIntent {
            token: "lw-staging-1".into(),
        }
    }

    #[test]
    fn append_retries_busy_before_storing_exactly_one_record() {
        thread_local! {
            static LOCKER: RefCell<Option<Connection>> = const { RefCell::new(None) };
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = started(&path);
        let locker = Connection::open(&path).unwrap();
        locker.execute_batch("BEGIN IMMEDIATE").unwrap();
        LOCKER.set(Some(locker));
        // Deterministically return BUSY once, releasing the real lock for the next attempt.
        store
            .conn
            .busy_handler(Some(|_| {
                LOCKER.take().unwrap().execute_batch("ROLLBACK").unwrap();
                false
            }))
            .unwrap();
        assert_eq!(store.append(1, 1, intent()).unwrap(), 2);
        assert_eq!(store.rows(1).unwrap().len(), 2);
        assert_eq!(store.replay(1).unwrap().unwrap().seq, 2);
    }

    #[test]
    fn append_busy_retries_are_bounded_and_store_nothing() {
        thread_local! {
            static CALLS: Cell<usize> = const { Cell::new(0) };
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = started(&path);
        let locker = Connection::open(&path).unwrap();
        locker.execute_batch("BEGIN IMMEDIATE").unwrap();
        store
            .conn
            .busy_handler(Some(|_| {
                CALLS.set(CALLS.get() + 1);
                false
            }))
            .unwrap();
        assert!(matches!(store.append(1, 1, intent()),
            Err(AppendError::Sql(rusqlite::Error::SqliteFailure(e, _)))
                if e.code == rusqlite::ErrorCode::DatabaseBusy));
        assert_eq!(CALLS.get(), 3, "initial attempt plus two retries");
        assert_eq!(store.rows(1).unwrap().len(), 1);
        assert_eq!(store.replay(1).unwrap().unwrap().seq, 1);
        locker.execute_batch("ROLLBACK").unwrap();
        assert_eq!(store.append(1, 1, intent()).unwrap(), 2);
    }
}
