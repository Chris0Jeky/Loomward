//! Whole-store SQLITE_FULL (docs/45 section 5): scan-shaped writes through the real writer until
//! `PRAGMA max_page_count` refuses growth. The failure is a typed error that no reader sees;
//! the cap is then lifted by a reopen, the remaining writes run, and the closed and reopened
//! store equals a fresh store given the same successful writes.
//!
//! The writer owns its connection, so the page limit can only be set before the writer starts
//! and only lifted by a new connection. Each reopen runs the same recovery (`open_pair` in `db.rs`,
//! which fails any run left open) in the capped store and in the fresh replay, so the replay
//! reopens at the same log positions.
use super::*;
use crate::{Basis, Catalog, ChildrenRequest, ListingOutcome, NodeKey, Sort};
use rusqlite::{params, types::Value, OpenFlags};
use std::path::{Path, PathBuf};

/// Roots scanned into one store. Twelve roots are enough to exhaust either cap part way through.
const ROOTS: u8 = 12;

/// One thing the writer did: a write whose reply the caller got, or a close and reopen.
enum Event {
    Write(Box<WriteCommand>, WriteReply),
    Reopen,
}

struct Scan {
    dir: PathBuf,
    writer: Option<Writer>,
    /// The page limit is still on the writer's connection.
    capped: bool,
    /// SQLITE_FULL errors the writer returned to callers.
    full: usize,
    log: Vec<Event>,
}

/// A writer over the store. `cap` pins one file to its current size plus the headroom, on the
/// connection the writer will own (`true` for state.db, `false` for catalog.db).
fn start_writer(dir: &Path, cap: Option<(bool, i64)>) -> Writer {
    let conn = crate::db::open_pair(dir, "synthetic").unwrap();
    if let Some((state, headroom)) = cap {
        let schema = if state { "st" } else { "main" };
        let pages: i64 = conn
            .query_row(&format!("PRAGMA {schema}.page_count"), [], |r| r.get(0))
            .unwrap();
        if state {
            conn.pragma_update(Some("st"), "max_page_count", pages + headroom)
                .unwrap();
        } else {
            conn.pragma_update(None, "max_page_count", pages + headroom)
                .unwrap();
        }
    }
    Writer::start(conn).unwrap()
}

impl Scan {
    fn new(dir: &Path, cap: (bool, i64)) -> Self {
        Self {
            dir: dir.to_path_buf(),
            writer: Some(start_writer(dir, Some(cap))),
            capped: true,
            full: 0,
            log: Vec::new(),
        }
    }

    /// Sends one write and returns its reply, or None when it failed with a typed SQLITE_FULL.
    /// A failed write must be invisible to a reader, so the store is compared before and after.
    fn send(&mut self, command: WriteCommand) -> Option<WriteReply> {
        let before = snapshot(&self.dir);
        let result = self
            .writer
            .as_ref()
            .expect("open writer")
            .call(command.clone());
        match result {
            Ok(reply) => {
                self.log
                    .push(Event::Write(Box::new(command), reply.clone()));
                Some(reply)
            }
            Err(Error::Sql(e)) if e.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull) => {
                assert!(
                    self.capped,
                    "SQLITE_FULL after the page limit was lifted: {command:?}"
                );
                assert_eq!(
                    snapshot(&self.dir),
                    before,
                    "a reader saw the failed write {command:?}"
                );
                self.full += 1;
                None
            }
            // After a full disk the writer may close with its failed transaction; callers then
            // get Closed. Any other error is a bug.
            Err(Error::Closed) if self.full > 0 && self.capped => None,
            Err(e) => panic!("unexpected {e:?} for {command:?}"),
        }
    }

    /// One root's scan: register, begin, listings, references, end. A write that hit the full
    /// disk stops the root; its run then ends failed when the disk still accepts that.
    fn scan_root(&mut self, key: u8) {
        if let Some(WriteReply::Root {
            grant_id, dir_id, ..
        }) = self.send(register(key))
        {
            if let Some(WriteReply::Run(run)) = self.send(begin(grant_id)) {
                let state = if self.scan_listings(run, dir_id, key).is_some() {
                    "completed"
                } else {
                    "failed"
                };
                self.send(end(run, state));
            }
        }
        self.lift_cap_after_full();
    }

    fn scan_listings(&mut self, run: i64, root_dir: i64, key: u8) -> Option<()> {
        let names = ["a", "b", "c", "d"];
        let root_dirs = names
            .iter()
            .enumerate()
            .map(|(i, name)| dir_entry(key, i + 1, name))
            .collect();
        self.send(WriteCommand::DirListing(listing(
            run,
            root_dir,
            listing_files(key, 0, 24),
            root_dirs,
        )))?;
        for (i, name) in ["a", "b", "c"].iter().enumerate() {
            let dir = child(&self.dir, root_dir, name);
            self.send(WriteCommand::DirListing(listing(
                run,
                dir,
                listing_files(key, i + 1, 40),
                Vec::new(),
            )))?;
        }
        // A large directory arrives in staged chunks, as the engine sends it.
        let big = child(&self.dir, root_dir, "d");
        self.send(WriteCommand::StageChunk {
            run_id: run,
            dir_id: big,
            seq: 0,
            files: listing_files(key, 4, 30),
            dirs: Vec::new(),
        })?;
        self.send(WriteCommand::ListingDone {
            run_id: run,
            dir_id: big,
            outcome: ListingOutcome::Complete,
            skipped: 0,
            errors: 0,
        })?;
        let a = child(&self.dir, root_dir, "a");
        self.send(WriteCommand::ObjectReference {
            node: NodeKey::Dir(a),
            observed_at_ns: 3,
        })?;
        for file in first_files(&self.dir, a, 12) {
            self.send(WriteCommand::ObjectReference {
                node: NodeKey::File(file),
                observed_at_ns: 3,
            })?;
        }
        Some(())
    }

    /// Lifts the page limit at a root boundary: the capped connection closes and the reopened
    /// one has no limit. Recovery at this reopen ends any run the failure left open.
    fn lift_cap_after_full(&mut self) {
        if self.capped && self.full > 0 {
            drop(self.writer.take());
            self.writer = Some(start_writer(&self.dir, None));
            self.capped = false;
            self.log.push(Event::Reopen);
        }
    }
}

fn identity(key: u8, tag: usize, index: usize) -> Vec<u8> {
    let mut id = vec![0u8; 16];
    id[0] = key;
    id[1] = u8::try_from(tag).unwrap();
    id[2..4].copy_from_slice(&u16::try_from(index).unwrap().to_le_bytes());
    id
}

/// Files of one directory. `tag` is that directory's identity tag, so no two files of a root
/// share a native identity (which would make them hard links).
fn listing_files(key: u8, tag: usize, count: usize) -> Vec<Observation> {
    (0..count)
        .map(|i| {
            let mut o = Observation::file(
                format!("r{key:02}-t{tag}-{i:03}.bin"),
                1000 + i as u64,
                Some(4096),
            );
            o.file_id = Some(identity(key, tag, i));
            o.created_ft = Some(1);
            o
        })
        .collect()
}

/// A subdirectory as its parent's listing reports it (the writer creates the directory row).
fn dir_entry(key: u8, index: usize, name: &str) -> Observation {
    let mut o = Observation::file(name, 0, None);
    o.file_id = Some(identity(key, 9, index));
    o.created_ft = Some(1);
    o
}

fn register(key: u8) -> WriteCommand {
    WriteCommand::RegisterRoot(RootObservation {
        volume_guid: None,
        volume_key: "fixture".into(),
        display_name: format!("Fixture {key}"),
        display_path: format!("Synthetic root {key}"),
        root_file_id: Some(vec![key; 16]),
        filesystem: Some("NTFS".into()),
        origin: "fixture".into(),
        granted_via: "fixture".into(),
        observed_at_ns: 0,
    })
}

fn begin(grant_id: i64) -> WriteCommand {
    WriteCommand::BeginRun {
        grant_id,
        mode: "full".into(),
        strategy: "fixture".into(),
        started_at_ns: 1,
    }
}

fn end(run_id: i64, state: &str) -> WriteCommand {
    WriteCommand::EndRun {
        run_id,
        state: state.into(),
        finished_at_ns: 2,
    }
}

fn listing(
    run_id: i64,
    dir_id: i64,
    files: Vec<Observation>,
    dirs: Vec<Observation>,
) -> DirListing {
    DirListing {
        run_id,
        dir_id,
        files,
        dirs,
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }
}

/// A separate read-only connection: it sees only committed state, as any reader does.
fn ro(dir: &Path) -> Connection {
    Connection::open_with_flags(dir.join("catalog.db"), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

fn child(dir: &Path, parent: i64, name: &str) -> i64 {
    ro(dir)
        .query_row(
            "SELECT id FROM dir WHERE parent_id=?1 AND name=?2",
            params![parent, name],
            |r| r.get(0),
        )
        .expect("the directory was listed before its child")
}

fn first_files(dir: &Path, parent: i64, count: usize) -> Vec<i64> {
    let db = ro(dir);
    let mut statement = db
        .prepare("SELECT id FROM file WHERE dir_id=?1 ORDER BY id LIMIT ?2")
        .unwrap();
    statement
        .query_map(params![parent, count as i64], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<i64>>>()
        .unwrap()
}

/// The only random values a store writes are 128-bit hex tokens (its instance id and publication
/// tokens). Each is replaced by a placeholder, so two stores compare on content alone.
fn normal(value: Value) -> Value {
    if let Value::Text(text) = &value {
        if text.len() == 32 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Value::Text("<token>".into());
        }
    }
    value
}

/// Every table of both files, row for row with its rowid, and each file's schema, in a fixed
/// order.
fn snapshot(dir: &Path) -> Vec<String> {
    let db = ro(dir);
    db.execute(
        "ATTACH DATABASE ?1 AS st",
        [dir.join("state.db").to_string_lossy().as_ref()],
    )
    .unwrap();
    let mut out = Vec::new();
    for schema in ["main", "st"] {
        let tables: Vec<String> = db
            .prepare(&format!(
                "SELECT name FROM {schema}.sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
            ))
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for table in tables {
            // A rowid table is ordered and compared by rowid; a WITHOUT ROWID table by all columns.
            let has_rowid = db
                .prepare(&format!("SELECT rowid FROM {schema}.\"{table}\" LIMIT 0"))
                .is_ok();
            let sql = if has_rowid {
                format!("SELECT rowid,* FROM {schema}.\"{table}\" ORDER BY rowid")
            } else {
                let width = db
                    .prepare(&format!("SELECT * FROM {schema}.\"{table}\""))
                    .unwrap()
                    .column_count();
                let order = (1..=width)
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                format!("SELECT * FROM {schema}.\"{table}\" ORDER BY {order}")
            };
            let mut statement = db.prepare(&sql).unwrap();
            let columns = statement.column_count();
            let rows: Vec<Vec<Value>> = statement
                .query_map([], |r| {
                    (0..columns)
                        .map(|i| r.get::<_, Value>(i).map(normal))
                        .collect::<rusqlite::Result<Vec<Value>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            out.push(format!("{schema}.{table}: {rows:?}"));
        }
        let schema_rows: Vec<Vec<Value>> = db
            .prepare(&format!(
                "SELECT type,name,tbl_name,sql FROM {schema}.sqlite_schema ORDER BY name"
            ))
            .unwrap()
            .query_map([], |r| (0..4).map(|i| r.get::<_, Value>(i)).collect())
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        out.push(format!("{schema}.sqlite_schema: {schema_rows:?}"));
    }
    out
}

/// Every directory's children page through the public Reader, both bases.
fn pages(catalog: &Catalog, dir: &Path) -> Vec<String> {
    let db = ro(dir);
    let instance: String = db
        .query_row("SELECT value FROM meta WHERE key='instance_id'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let dirs: Vec<i64> = db
        .prepare("SELECT id FROM dir ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let mut reader = catalog.reader().unwrap();
    let mut out = Vec::new();
    for dir_id in dirs {
        for (name, basis) in [("logical", Basis::Logical), ("allocated", Basis::Allocated)] {
            let page = reader.children(&ChildrenRequest {
                dir_id,
                sort: Sort::NameAsc,
                basis,
                limit: 200,
                cursor: None,
            });
            out.push(format!("{dir_id} {name}: {page:?}").replace(&instance, "<instance>"));
        }
    }
    out
}

/// The successful writes in order, with the same reopens, on a fresh store. Every reply must match.
fn fresh_replay(dir: &Path, log: &[Event]) {
    let mut writer = Some(start_writer(dir, None));
    for event in log {
        match event {
            Event::Write(command, reply) => {
                let got = writer
                    .as_ref()
                    .unwrap()
                    .call(command.as_ref().clone())
                    .unwrap();
                assert_eq!(
                    &got, reply,
                    "the fresh store replied differently to {command:?}"
                );
            }
            Event::Reopen => {
                drop(writer.take());
                writer = Some(start_writer(dir, None));
            }
        }
    }
}

fn scan_to_full_then_recover(state: bool, headroom: i64) {
    let capped = tempfile::tempdir().unwrap();
    let mut scan = Scan::new(capped.path(), (state, headroom));
    for key in 1..=ROOTS {
        scan.scan_root(key);
    }
    assert!(scan.full > 0, "the page limit was never reached");
    assert!(!scan.capped, "the page limit was never lifted");
    let writes_after_reopen = scan
        .log
        .iter()
        .rev()
        .take_while(|event| !matches!(event, Event::Reopen))
        .filter(|event| matches!(event, Event::Write(..)))
        .count();
    assert!(writes_after_reopen > 0, "no write followed the reopen");
    let log = std::mem::take(&mut scan.log);
    drop(scan.writer.take());
    let fresh = tempfile::tempdir().unwrap();
    fresh_replay(fresh.path(), &log);

    // Close and reopen both stores; each reopen runs recovery.
    let reopened = Catalog::open(capped.path(), "synthetic").unwrap();
    let rebuilt = Catalog::open(fresh.path(), "synthetic").unwrap();
    let roots: i64 = ro(capped.path())
        .query_row("SELECT count(*) FROM root", [], |r| r.get(0))
        .unwrap();
    let files: i64 = ro(capped.path())
        .query_row("SELECT count(*) FROM file", [], |r| r.get(0))
        .unwrap();
    assert!(
        roots > 1 && files > 0,
        "the scan wrote nothing that survived"
    );
    assert_eq!(snapshot(capped.path()), snapshot(fresh.path()));
    assert_eq!(
        pages(&reopened, capped.path()),
        pages(&rebuilt, fresh.path())
    );
}

#[test]
fn whole_store_disk_full_on_catalogue_recovers_to_the_fresh_state() {
    // 40 pages of headroom: the twelve roots need far more, so the limit is hit mid-scan.
    scan_to_full_then_recover(false, 40);
}

#[test]
fn whole_store_disk_full_on_state_db_recovers_to_the_fresh_state() {
    // Two pages of state.db headroom: the references and grants of a few roots exhaust it.
    scan_to_full_then_recover(true, 2);
}
