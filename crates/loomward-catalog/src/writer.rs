use crate::*;
use crossbeam_channel::{bounded, Receiver, Sender};
use rusqlite::{params, types::Value, Connection, OptionalExtension};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub const CHUNK_ENTRIES: usize = 16_384;
pub const QUEUE_BYTES: usize = 64 * 1024 * 1024;
const WRITER_BYTES: usize = 32 * 1024 * 1024;
type Quota = Arc<(Mutex<usize>, Condvar)>;

/// Disconnect wakes every waiter; cancellation is not consumed by one producer.
#[derive(Clone)]
pub struct Cancellation {
    sender: Arc<Mutex<Option<Sender<()>>>>,
    rx: Receiver<()>,
}
impl Default for Cancellation {
    fn default() -> Self {
        let (tx, rx) = bounded(0);
        Self {
            sender: Arc::new(Mutex::new(Some(tx))),
            rx,
        }
    }
}
impl Cancellation {
    pub fn cancel(&self) {
        self.sender.lock().expect("cancel lock").take();
    }
    pub fn is_cancelled(&self) -> bool {
        matches!(
            self.rx.try_recv(),
            Err(crossbeam_channel::TryRecvError::Disconnected)
        )
    }
}
/// Reserve before building a chunk; send transfers the reservation to the writer.
pub struct BytePermit {
    quota: Quota,
    bytes: usize,
}
impl Drop for BytePermit {
    fn drop(&mut self) {
        release(&self.quota, self.bytes);
    }
}
struct Pending {
    command: WriteCommand,
    reply: Sender<Result<WriteReply>>,
    _permit: BytePermit,
}
pub struct Writer {
    tx: Option<Sender<Pending>>,
    quota: Quota,
    stopped: Receiver<()>,
    join: Option<JoinHandle<()>>,
    timings: Arc<Mutex<WriterTimings>>,
}
pub struct Receipt {
    rx: Receiver<Result<WriteReply>>,
}
impl Receipt {
    pub fn wait(self) -> Result<WriteReply> {
        self.rx.recv().map_err(|_| Error::Closed)?
    }
}
impl Writer {
    pub(crate) fn start(conn: Connection) -> Result<Self> {
        let (tx, rx) = bounded(64);
        let (alive, stopped) = bounded(0);
        let quota = Arc::new((Mutex::new(WRITER_BYTES), Condvar::new()));
        let scratch = BytePermit {
            quota: quota.clone(),
            bytes: WRITER_BYTES,
        };
        let timings = Arc::new(Mutex::new(WriterTimings::default()));
        let measured = timings.clone();
        let join = std::thread::Builder::new()
            .name("loomward-catalog".into())
            .spawn(move || {
                let _scratch = scratch;
                work(conn, rx, measured, alive);
            })?;
        Ok(Self {
            tx: Some(tx),
            quota,
            stopped,
            join: Some(join),
            timings,
        })
    }
    pub fn reserve_bytes(&self, bytes: usize, cancel: &Cancellation) -> Result<BytePermit> {
        if bytes > QUEUE_BYTES - WRITER_BYTES {
            return Err(Error::Invalid("chunk exceeds byte budget"));
        }
        let mut used = self.quota.0.lock().map_err(|_| Error::Closed)?;
        loop {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if matches!(
                self.stopped.try_recv(),
                Err(crossbeam_channel::TryRecvError::Disconnected)
            ) {
                return Err(Error::Closed);
            }
            if *used + bytes <= QUEUE_BYTES {
                *used += bytes;
                return Ok(BytePermit {
                    quota: self.quota.clone(),
                    bytes,
                });
            }
            used = self
                .quota
                .1
                .wait_timeout(used, Duration::from_millis(10))
                .map_err(|_| Error::Closed)?
                .0;
        }
    }
    pub fn send_reserved(
        &self,
        command: WriteCommand,
        permit: BytePermit,
        cancel: &Cancellation,
    ) -> Result<Receipt> {
        let bytes = validate(&command)?;
        if !Arc::ptr_eq(&permit.quota, &self.quota) || bytes > permit.bytes {
            return Err(Error::Invalid("insufficient shared byte reservation"));
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let (reply, rx) = bounded(1);
        let pending = Pending {
            command,
            reply,
            _permit: permit,
        };
        crossbeam_channel::select! {
            send(self.tx.as_ref().ok_or(Error::Closed)?,pending)->result=>{result.map_err(|_|Error::Closed)?;}
            recv(cancel.rx)->_=>return Err(Error::Cancelled),
            recv(self.stopped)->_=>return Err(Error::Closed),
        }
        Ok(Receipt { rx })
    }
    pub fn send_cancellable(
        &self,
        command: WriteCommand,
        cancel: &Cancellation,
    ) -> Result<Receipt> {
        let permit = self.reserve_bytes(validate(&command)?, cancel)?;
        self.send_reserved(command, permit, cancel)
    }
    pub fn send(&self, command: WriteCommand) -> Result<Receipt> {
        self.send_cancellable(command, &Cancellation::default())
    }
    pub fn call(&self, command: WriteCommand) -> Result<WriteReply> {
        self.send(command)?.wait()
    }
    pub fn timings(&self) -> WriterTimings {
        self.timings.lock().expect("timing lock").clone()
    }
    pub fn queued_bytes(&self) -> usize {
        self.quota
            .0
            .lock()
            .expect("quota lock")
            .saturating_sub(WRITER_BYTES)
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}
fn release(q: &Quota, n: usize) {
    let mut used = q.0.lock().expect("quota lock");
    *used -= n;
    q.1.notify_all();
}
fn observation_bytes(o: &Observation) -> usize {
    o.name.capacity()
        + o.name_utf16.as_ref().map_or(0, Vec::capacity)
        + o.file_id.as_ref().map_or(0, Vec::capacity)
        + o.id_basis.capacity()
        + o.extension.as_ref().map_or(0, String::capacity)
        + o.family.capacity()
}
fn entries_bytes(files: &Vec<Observation>, dirs: &Vec<Observation>) -> Result<usize> {
    if files.len() + dirs.len() > CHUNK_ENTRIES {
        return Err(Error::Invalid("use staged chunks for large listings"));
    }
    let mut bytes = (files.capacity() + dirs.capacity()) * std::mem::size_of::<Observation>();
    let mut names = HashSet::new();
    for o in files.iter().chain(dirs) {
        o.validate()?;
        if !names.insert((&o.name, &o.name_utf16)) {
            return Err(Error::Invalid("duplicate listing name"));
        }
        bytes += observation_bytes(o);
    }
    // Covers validation maps and serialization scratch as well as the retained payload.
    Ok(bytes * 8 + 1024)
}
fn validate(c: &WriteCommand) -> Result<usize> {
    match c {
        WriteCommand::DirListing(l) => {
            if !STATES.contains(&l.state.as_str())
                || l.skipped > i64::MAX as u64
                || l.errors > i64::MAX as u64
            {
                return Err(Error::Invalid("listing bounds"));
            }
            entries_bytes(&l.files, &l.dirs)
        }
        WriteCommand::StageChunk {
            files, dirs, seq, ..
        } => {
            if *seq < 0 {
                return Err(Error::Invalid("chunk sequence"));
            }
            entries_bytes(files, dirs)
        }
        WriteCommand::ListingDone {
            skipped, errors, ..
        } => {
            if *skipped > i64::MAX as u64 || *errors > i64::MAX as u64 {
                return Err(Error::Invalid("listing bounds"));
            }
            Ok(1024)
        }
        WriteCommand::DirFinal { totals: t, .. } => {
            if [
                t.files,
                t.dirs,
                t.logical,
                t.allocated,
                t.allocation_unknown,
                t.skipped,
                t.errors,
            ]
            .into_iter()
            .any(|v| v > i64::MAX as u64)
            {
                return Err(Error::Invalid("totals bounds"));
            }
            Ok(1024)
        }
        _ => Ok(1024),
    }
}
#[derive(Clone, Default, serde::Serialize)]
pub struct WriterTimings {
    pub listing_seconds: f64,
    pub file_insert_seconds: f64,
    pub file_seen_seconds: f64,
    pub name_validation_seconds: f64,
    pub commit_seconds: f64,
    pub dirty_updates: u64,
}
struct Context {
    timings: Arc<Mutex<WriterTimings>>,
    parents: HashMap<i64, Option<i64>>,
    marked: HashSet<i64>,
    revision: i64,
}
impl Context {
    fn dirty(&mut self, conn: &Connection, start: i64, run: i64) -> Result<()> {
        let mut current = Some(start);
        let mut count = 0;
        let mut update = conn.prepare_cached(
            "UPDATE dir SET subtree_rev=?2,dirty_rev=?2,dirty_run=?3,sub_complete=0 WHERE id=?1",
        )?;
        while let Some(id) = current {
            if !self.marked.insert(id) {
                break;
            }
            update.execute(params![id, self.revision, run])?;
            self.timings.lock().expect("timing lock").dirty_updates += 1;
            current = if let Some(parent) = self.parents.get(&id) {
                *parent
            } else {
                let parent =
                    conn.query_row("SELECT parent_id FROM dir WHERE id=?1", [id], |r| r.get(0))?;
                self.parents.insert(id, parent);
                parent
            };
            count += 1;
            if count > 512 {
                return Err(Error::Invalid("directory identity cycle"));
            }
        }
        Ok(())
    }
}
fn work(
    mut conn: Connection,
    rx: Receiver<Pending>,
    timings: Arc<Mutex<WriterTimings>>,
    alive: Sender<()>,
) {
    let mut alive = Some(alive);
    let mut pending = None;
    let mut context = Context {
        timings,
        parents: HashMap::new(),
        marked: HashSet::new(),
        revision: 0,
    };
    loop {
        let Some(first) = pending.take().or_else(|| rx.recv().ok()) else {
            break;
        };
        let checkpoint = matches!(first.command, WriteCommand::EndRun { .. });
        let mut batch = vec![first];
        if matches!(
            batch[0].command,
            WriteCommand::DirListing(_)
                | WriteCommand::StageChunk { .. }
                | WriteCommand::ListingDone { .. }
        ) && !conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM st.object_ref WHERE state='resolved')",
                [],
                |r| r.get::<_, bool>(0),
            )
            .unwrap_or(true)
        {
            let start = Instant::now();
            let mut rows = 0;
            while rows < 50_000 && start.elapsed() < Duration::from_millis(250) {
                match rx.try_recv() {
                    Ok(p)
                        if matches!(
                            p.command,
                            WriteCommand::DirListing(_)
                                | WriteCommand::StageChunk { .. }
                                | WriteCommand::ListingDone { .. }
                        ) =>
                    {
                        rows += match &p.command {
                            WriteCommand::DirListing(l) => l.files.len() + l.dirs.len(),
                            WriteCommand::StageChunk { files, dirs, .. } => {
                                files.len() + dirs.len()
                            }
                            _ => 0,
                        };
                        batch.push(p);
                    }
                    Ok(p) => {
                        pending = Some(p);
                        break;
                    }
                    Err(_) => break,
                }
            }
        }
        let mut results = Vec::with_capacity(batch.len());
        let mut failed = false;
        // Precious writes commit before the catalogue transaction, which writes main only.
        let prepared: Vec<_> = batch
            .iter()
            .map(|p| prepare_state(&mut conn, &p.command))
            .collect();
        match conn.transaction() {
            Ok(mut tx) => {
                context.marked.clear();
                let revision: Result<i64> = (|| {
                    tx.execute(
                        "UPDATE revision SET catalog_rev=catalog_rev+1 WHERE id=1",
                        [],
                    )?;
                    Ok(tx.query_row("SELECT catalog_rev FROM revision", [], |r| r.get(0))?)
                })();
                match revision {
                    Ok(rev) => {
                        context.revision = rev;
                        for (p, ready) in batch.iter().zip(prepared) {
                            results.push(ready.and_then(|prepared| {
                                let save = tx.savepoint()?;
                                let result = apply(&save, &p.command, &mut context, prepared);
                                match result {
                                    Ok(reply) => {
                                        save.commit()?;
                                        Ok(reply)
                                    }
                                    Err(e) => {
                                        drop(save);
                                        context.marked.clear();
                                        context.parents.clear();
                                        Err(e)
                                    }
                                }
                            }));
                        }
                    }
                    Err(e) => {
                        results.push(Err(e));
                    }
                }
                let commit_start = Instant::now();
                let committed = tx.commit();
                context.timings.lock().expect("timing lock").commit_seconds +=
                    commit_start.elapsed().as_secs_f64();
                if let Err(e) = committed {
                    failed = true;
                    results.clear();
                    results.push(Err(Error::Sql(e)));
                    context.parents.clear();
                    context.marked.clear();
                }
            }
            Err(e) => {
                failed = true;
                results.push(Err(Error::Sql(e)));
            }
        }
        while results.len() < batch.len() {
            results.push(Err(Error::Closed));
        }
        if failed {
            alive.take();
        }
        if checkpoint {
            let _ = conn.execute_batch("PRAGMA main.wal_checkpoint(TRUNCATE)");
        }
        for (p, result) in batch.into_iter().zip(results) {
            let Pending { reply, _permit, .. } = p;
            drop(_permit);
            let _ = reply.send(result);
        }
        if failed {
            break;
        }
    }
    let _ = conn.execute_batch("PRAGMA main.wal_checkpoint(TRUNCATE)");
}
fn active_run(conn: &Connection, run: i64, dir: Option<i64>) -> Result<i64> {
    let root:Option<i64>=conn.query_row("SELECT s.root_id FROM scan_run s JOIN root r ON r.id=s.root_id JOIN st.root_grant g ON g.id=r.grant_id WHERE s.id=?1 AND s.state='running' AND r.active_run=s.id AND g.state='active'",[run],|r|r.get(0)).optional()?;
    let root = root.ok_or(Error::Invalid("inactive run or grant"))?;
    if let Some(dir) = dir {
        if !conn.query_row("SELECT EXISTS(SELECT 1 FROM dir WHERE id=?1 AND root_id=?2 AND listing_state!='absent_pending')",params![dir,root],|r|r.get::<_,bool>(0))?{return Err(Error::NotFound);}
    }
    Ok(root)
}
fn state_revision(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE st.revision SET state_rev=state_rev+1 WHERE id=1",
        [],
    )?;
    Ok(())
}
fn prepare_state(conn: &mut Connection, c: &WriteCommand) -> Result<Option<i64>> {
    match c {
        WriteCommand::RegisterRoot(o) => {
            if o.volume_key.is_empty()
                || o.volume_key.len() > 80
                || o.display_name.chars().count() > 260
                || o.display_path.chars().count() > 32768
                || !matches!(
                    o.origin.as_str(),
                    "fixture" | "owner_granted" | "lab_generated"
                )
                || !matches!(
                    o.granted_via.as_str(),
                    "fixture" | "cli_flag" | "desktop_picker"
                )
                || o.filesystem.as_ref().is_some_and(|fs| fs.len() > 32)
                || o.root_file_id
                    .as_ref()
                    .is_none_or(|id| !matches!(id.len(), 8 | 16))
            {
                return Err(Error::Invalid(
                    "root observation; lab roots require registered provenance",
                ));
            }
            if o.filesystem
                .as_deref()
                .is_some_and(|fs| fs.eq_ignore_ascii_case("ReFS"))
                && o.root_file_id.as_ref().is_some_and(|id| id.len() != 16)
            {
                return Err(Error::Invalid("ReFS root needs 128-bit identity"));
            }
            let lab: Option<i64> = if o.origin == "lab_generated" {
                Some(
                    conn.query_row(
                        "SELECT id FROM st.lab_root WHERE volume_key=?1 AND root_file_id=?2",
                        params![o.volume_key, o.root_file_id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .ok_or(Error::Invalid("unregistered lab root"))?,
                )
            } else {
                None
            };
            let tx = conn.transaction()?;
            tx.execute("INSERT INTO st.root_grant(volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns,lab_root_id) VALUES (?1,?2,?3,?4,?5,'active',?6,?7)",params![o.volume_key,o.root_file_id,o.display_path,o.origin,o.granted_via,o.observed_at_ns,lab])?;
            let grant = tx.last_insert_rowid();
            state_revision(&tx)?;
            tx.commit()?;
            Ok(Some(grant))
        }
        WriteCommand::RevokeGrant {
            grant_id,
            revoked_at_ns,
        } => {
            let tx = conn.transaction()?;
            if tx.execute(
                "UPDATE st.root_grant SET state='revoked',revoked_at_ns=?2 WHERE id=?1",
                params![grant_id, revoked_at_ns],
            )? == 0
            {
                return Err(Error::NotFound);
            }
            state_revision(&tx)?;
            tx.commit()?;
            Ok(None)
        }
        WriteCommand::ObjectReference {
            node,
            observed_at_ns,
        } => reference(conn, *node, None, *observed_at_ns).map(Some),
        WriteCommand::ReconcileReference {
            reference_id,
            node,
            observed_at_ns,
        } => reference(conn, *node, Some(*reference_id), *observed_at_ns).map(Some),
        WriteCommand::DirListing(l) => {
            retire_files(
                conn,
                l.run_id,
                l.dir_id,
                Some((&l.files, &l.dirs)),
                l.state == "complete",
            )?;
            Ok(None)
        }
        WriteCommand::ListingDone {
            run_id,
            dir_id,
            outcome,
            ..
        } => {
            retire_files(
                conn,
                *run_id,
                *dir_id,
                None,
                matches!(outcome, ListingOutcome::Complete),
            )?;
            Ok(None)
        }
        WriteCommand::EndRun { run_id, state, .. } if state == "completed" => {
            retire_pending(conn, *run_id)?;
            Ok(None)
        }
        _ => Ok(None),
    }
}
fn reference(conn: &mut Connection, node: NodeKey, reconcile: Option<i64>, at: i64) -> Result<i64> {
    let r = crate::query::get(conn, node)?;
    let (volume,fs):(String,Option<String>)=conn.query_row("SELECT v.volume_key,v.filesystem FROM root rt JOIN volume v ON v.id=rt.volume_id WHERE rt.id=?1",[r.grant],|row|Ok((row.get(0)?,row.get(1)?)))?;
    let identity = r
        .file_id
        .ok_or(Error::Invalid("durable identity unavailable"))?;
    if !matches!(fs.as_deref(), Some("NTFS" | "ReFS"))
        || r.created.is_none()
        || (fs.as_deref() == Some("ReFS") && identity.len() != 16)
    {
        return Err(Error::Invalid("durable identity unavailable"));
    }
    let (kind, table, id) = match node {
        NodeKey::Dir(id) => ("dir", "dir", id),
        NodeKey::File(id) => ("file", "file", id),
        _ => return Err(Error::NotFound),
    };
    let born: i64 = conn.query_row(
        &format!("SELECT born_run FROM {table} WHERE id=?1"),
        [id],
        |r| r.get(0),
    )?;
    let instance: String =
        conn.query_row("SELECT value FROM meta WHERE key='instance_id'", [], |r| {
            r.get(0)
        })?;
    let existing:Option<i64>=conn.query_row("SELECT id FROM st.object_ref WHERE volume_key=?1 AND file_id=?2 AND kind=?3 AND catalog_instance=?4 AND row_id=?5 AND born_run=?6 AND state='resolved' AND retired=0",params![volume,identity,kind,instance,id,born],|r|r.get(0)).optional()?;
    if reconcile.is_none() {
        if let Some(existing) = existing {
            return Ok(existing);
        }
    }
    let tx = conn.transaction()?;
    let reference = if let Some(reference) = reconcile {
        tx.execute("UPDATE st.object_ref SET state='unresolved',retired=1 WHERE id!=?1 AND catalog_instance=?2 AND row_id=?3 AND born_run=?4 AND kind=?5 AND state='resolved'",params![reference,instance,id,born,kind])?;
        if tx.execute("UPDATE st.object_ref SET creation_ft=?2,state='resolved',retired=0,catalog_instance=?3,row_id=?4,born_run=?5,last_resolved_ns=?6 WHERE id=?1 AND volume_key=?7 AND file_id=?8 AND kind=?9",params![reference,r.created,instance,id,born,at,volume,identity,kind])?!=1{return Err(Error::Invalid("reconciliation identity mismatch"));}
        reference
    } else {
        tx.execute("INSERT INTO st.object_ref(volume_key,file_id,creation_ft,kind,state,last_resolved_ns,incarnation,catalog_instance,row_id,born_run) VALUES (?1,?2,?3,?4,'resolved',?5,(SELECT coalesce(max(incarnation),0)+1 FROM st.object_ref WHERE volume_key=?1 AND file_id=?2),?6,?7,?8)",params![volume,identity,r.created,kind,at,instance,id,born])?;
        tx.last_insert_rowid()
    };
    state_revision(&tx)?;
    tx.commit()?;
    Ok(reference)
}
fn retire_selected(
    conn: &mut Connection,
    select: &str,
    values: impl rusqlite::Params,
) -> Result<()> {
    conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS retiring_reference(id INTEGER PRIMARY KEY);DELETE FROM retiring_reference")?;
    conn.execute(
        &format!("INSERT OR IGNORE INTO retiring_reference {select}"),
        values,
    )?;
    let tx = conn.transaction()?;
    if tx.execute("UPDATE st.object_ref SET state='unresolved',retired=1 WHERE id IN (SELECT id FROM retiring_reference)",[])?>0{state_revision(&tx)?;}
    tx.commit()?;
    Ok(())
}
fn retire_files(
    conn: &mut Connection,
    run: i64,
    dir: i64,
    incoming: Option<(&[Observation], &[Observation])>,
    complete: bool,
) -> Result<()> {
    active_run(conn, run, Some(dir))?;
    if !conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM st.object_ref WHERE state='resolved')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS continuity_entry(kind INTEGER,name TEXT,raw BLOB,identity BLOB,created_ft INTEGER);CREATE INDEX IF NOT EXISTS continuity_by_identity ON continuity_entry(kind,identity,created_ft);CREATE INDEX IF NOT EXISTS continuity_by_name ON continuity_entry(kind,name,raw);DELETE FROM continuity_entry")?;
    let mut direct = incoming;
    {
        let mut chunks = conn.prepare_cached(
            "SELECT entries FROM stage_entry WHERE run_id=?1 AND dir_id=?2 ORDER BY seq",
        )?;
        let mut rows = chunks.query(params![run, dir])?;
        let mut insert =
            conn.prepare_cached("INSERT INTO continuity_entry VALUES(?1,?2,?3,?4,?5)")?;
        loop {
            let decoded;
            let (files, dirs) = if let Some(direct) = direct.take() {
                direct
            } else if let Some(row) = rows.next()? {
                let bytes: Vec<u8> = row.get(0)?;
                decoded = serde_json::from_slice::<(Vec<Observation>, Vec<Observation>)>(&bytes)
                    .map_err(|_| Error::CorruptDatabase)?;
                (&decoded.0[..], &decoded.1[..])
            } else {
                break;
            };
            for (kind, entries) in [(1, files), (0, dirs)] {
                for o in entries {
                    insert.execute(params![kind, o.name, o.name_utf16, o.file_id, o.created_ft])?;
                }
            }
        }
    }
    retire_selected(conn,"SELECT o.id FROM st.object_ref o JOIN file f ON f.id=o.row_id WHERE o.kind='file' AND o.state='resolved' AND f.dir_id=?1 AND o.catalog_instance=(SELECT value FROM meta WHERE key='instance_id') AND o.born_run=f.born_run AND NOT EXISTS(SELECT 1 FROM continuity_entry e WHERE e.kind=1 AND e.identity=f.file_id AND e.created_ft IS f.created_ft) AND (?2 OR EXISTS(SELECT 1 FROM continuity_entry e WHERE e.kind=1 AND ((e.name=f.name AND e.raw IS f.name_utf16) OR e.identity=f.file_id))) UNION SELECT o.id FROM st.object_ref o JOIN dir d ON d.id=o.row_id JOIN continuity_entry e ON e.kind=0 AND e.identity=d.file_id AND e.created_ft IS NOT d.created_ft WHERE o.kind='dir' AND o.state='resolved' AND d.root_id=(SELECT root_id FROM dir WHERE id=?1) AND o.catalog_instance=(SELECT value FROM meta WHERE key='instance_id') AND o.born_run=d.born_run",params![dir,complete])
}
fn full_traversal(conn: &Connection, run: i64, root: i64) -> Result<bool> {
    Ok(conn.query_row("WITH RECURSIVE visible(id) AS (SELECT id FROM dir WHERE root_id=?2 AND parent_id IS NULL UNION ALL SELECT d.id FROM dir d JOIN visible v ON d.parent_id=v.id WHERE d.listing_state!='absent_pending') SELECT mode!='targeted' AND NOT EXISTS(SELECT 1 FROM dir WHERE id IN (SELECT id FROM visible) AND (seen_run!=?1 OR listing_state!='complete')) FROM scan_run WHERE id=?1",params![run,root],|r|r.get(0))?)
}
fn retire_pending(conn: &mut Connection, run: i64) -> Result<()> {
    let root = active_run(conn, run, None)?;
    if !full_traversal(conn, run, root)? {
        return Ok(());
    }
    retire_selected(conn,"WITH RECURSIVE removed(id) AS (SELECT id FROM dir WHERE root_id=?1 AND listing_state='absent_pending' UNION ALL SELECT d.id FROM dir d JOIN removed x ON d.parent_id=x.id) SELECT o.id FROM st.object_ref o WHERE o.state='resolved' AND o.catalog_instance=(SELECT value FROM meta WHERE key='instance_id') AND ((o.kind='dir' AND o.row_id IN (SELECT id FROM removed)) OR (o.kind='file' AND o.row_id IN (SELECT id FROM file WHERE dir_id IN (SELECT id FROM removed))))",[root])
}
fn apply(
    conn: &Connection,
    c: &WriteCommand,
    ctx: &mut Context,
    prepared: Option<i64>,
) -> Result<WriteReply> {
    match c {
        WriteCommand::RegisterRoot(o) => {
            let grant = prepared.ok_or(Error::Invalid("grant not committed"))?;
            let (root_id, dir) = crate::db::derive_root(conn, grant, Some(o))?;
            Ok(WriteReply::Root {
                grant_id: grant,
                root_id,
                dir_id: dir,
            })
        }
        WriteCommand::BeginRun {
            grant_id,
            mode,
            strategy,
            started_at_ns,
        } => {
            if !matches!(mode.as_str(), "full" | "refresh" | "targeted") || strategy.len() > 128 {
                return Err(Error::Invalid("run bounds"));
            }
            let root:Option<i64>=conn.query_row("SELECT r.id FROM root r JOIN st.root_grant g ON g.id=r.grant_id WHERE g.id=?1 AND g.state='active'",[grant_id],|r|r.get(0)).optional()?;
            let root = root.ok_or(Error::NotFound)?;
            if conn.query_row(
                "SELECT active_run IS NOT NULL FROM root WHERE id=?1",
                [root],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(Error::Invalid("run already active"));
            }
            conn.execute("INSERT INTO scan_run(root_id,mode,strategy,state,started_at_ns,counters_json,budget_json) VALUES (?1,?2,?3,'running',?4,'{}','{}')",params![root,mode,strategy,started_at_ns])?;
            let run = conn.last_insert_rowid();
            conn.execute(
                "UPDATE root SET active_run=?2,state='scanning' WHERE id=?1",
                params![root, run],
            )?;
            Ok(WriteReply::Run(run))
        }
        WriteCommand::DirListing(l) => publish(
            conn,
            l.run_id,
            l.dir_id,
            if l.state == "complete" {
                ListingOutcome::Complete
            } else {
                ListingOutcome::Incomplete(l.state.clone())
            },
            (l.skipped, l.errors),
            ctx,
            Some((&l.files, &l.dirs)),
        ),
        WriteCommand::StageChunk {
            run_id,
            dir_id,
            seq,
            files,
            dirs,
        } => {
            stage(conn, *run_id, *dir_id, *seq, files, dirs)?;
            Ok(WriteReply::Done)
        }
        WriteCommand::ListingDone {
            run_id,
            dir_id,
            outcome,
            skipped,
            errors,
        } => publish(
            conn,
            *run_id,
            *dir_id,
            outcome.clone(),
            (*skipped, *errors),
            ctx,
            None,
        ),
        WriteCommand::DirFinal {
            run_id,
            dir_id,
            totals: t,
            input_revision,
        } => {
            active_run(conn, *run_id, Some(*dir_id))?;
            let count=conn.execute("UPDATE dir SET sub_files=?2,sub_dirs=?3,sub_logical=?4,sub_allocated=?5,sub_alloc_unknown=?6,sub_skipped=?7,sub_errors=?8,sub_newest_ft=?9,sub_complete=?10,agg_valid_rev=?11 WHERE id=?1 AND dirty_run=?12 AND dirty_rev=?11",params![dir_id,t.files as i64,t.dirs as i64,t.logical as i64,t.allocated as i64,t.allocation_unknown as i64,t.skipped as i64,t.errors as i64,t.newest_ft,t.complete,input_revision,run_id])?;
            if count == 0 {
                return Err(Error::StaleGeneration);
            }
            Ok(WriteReply::Done)
        }
        WriteCommand::EndRun {
            run_id,
            state,
            finished_at_ns,
        } => {
            let root = active_run(conn, *run_id, None)?;
            if !matches!(state.as_str(), "completed" | "cancelled" | "failed") {
                return Err(Error::Invalid("end state"));
            }
            let full = state == "completed" && full_traversal(conn, *run_id, root)?;
            if full {
                conn.execute(
                    "DELETE FROM dir WHERE root_id=?1 AND listing_state='absent_pending'",
                    [root],
                )?;
                ctx.parents.clear();
            }
            conn.execute("DELETE FROM stage_entry WHERE run_id=?1", [run_id])?;
            rebuild_multilink(conn, root)?;
            rollup(conn, root, full)?;
            conn.execute(
                "UPDATE scan_run SET state=?2,finished_at_ns=?3 WHERE id=?1",
                params![run_id, state, finished_at_ns],
            )?;
            let completed=state=="completed" && conn.query_row("SELECT mode='targeted' AND NOT EXISTS(SELECT 1 FROM dir WHERE root_id=?2 AND seen_run=?1 AND listing_state='incomplete') FROM scan_run WHERE id=?1",params![run_id,root],|r|r.get::<_,bool>(0))?;
            conn.execute("UPDATE root SET active_run=NULL,generation=CASE WHEN ?2 THEN ?3 ELSE generation END,state=CASE WHEN ?4 THEN 'complete' ELSE 'partial' END WHERE id=?1",params![root,full||completed,run_id,full])?;
            Ok(WriteReply::Done)
        }
        WriteCommand::RevokeGrant { grant_id, .. } => {
            conn.execute("UPDATE scan_run SET state='cancelled' WHERE root_id IN (SELECT id FROM root WHERE grant_id=?1) AND state='running'",[grant_id])?;
            conn.execute(
                "UPDATE root SET active_run=NULL,state='stale' WHERE grant_id=?1",
                [grant_id],
            )?;
            Ok(WriteReply::Done)
        }
        WriteCommand::Repair { root_id } => {
            rollup(conn, *root_id, false)?;
            Ok(WriteReply::Done)
        }
        WriteCommand::ObjectReference { .. } | WriteCommand::ReconcileReference { .. } => Ok(
            WriteReply::Reference(prepared.ok_or(Error::Invalid("reference not committed"))?),
        ),
        WriteCommand::Barrier => Ok(WriteReply::Done),
    }
}
fn stage(
    conn: &Connection,
    run: i64,
    dir: i64,
    seq: i64,
    files: &Vec<Observation>,
    dirs: &Vec<Observation>,
) -> Result<()> {
    active_run(conn, run, Some(dir))?;
    let previous:Option<(i64,i64)>=conn.query_row("SELECT seq,entry_count_total FROM stage_entry WHERE run_id=?1 AND dir_id=?2 ORDER BY seq DESC LIMIT 1",params![run,dir],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (next, count) = previous.map_or((0, 0), |(seq, count)| (seq + 1, count));
    if seq != next {
        return Err(Error::Invalid("non-contiguous chunk sequence"));
    }
    let count = count + files.len() as i64 + dirs.len() as i64;
    if count > 2_000_000 {
        return Err(Error::ResourceBudget);
    }
    let bytes =
        serde_json::to_vec(&(files, dirs)).map_err(|_| Error::Invalid("chunk serialization"))?;
    conn.prepare_cached("INSERT INTO stage_entry VALUES(?1,?2,?3,?4,?5)")?
        .execute(params![run, dir, seq, bytes, count])?;
    Ok(())
}
fn native_id<'a>(o: &'a Observation, fs: Option<&str>) -> Option<&'a [u8]> {
    o.file_id
        .as_deref()
        .filter(|id| !fs.is_some_and(|fs| fs.eq_ignore_ascii_case("ReFS")) || id.len() == 16)
}
type DirectoryMatch = (i64, Option<i64>, String, Option<Vec<u8>>, String);
fn publish(
    conn: &Connection,
    run: i64,
    dir: i64,
    outcome: ListingOutcome,
    counts: (u64, u64),
    ctx: &mut Context,
    mut direct: Option<(&[Observation], &[Observation])>,
) -> Result<WriteReply> {
    let (skipped, errors) = counts;
    let listing_start = Instant::now();
    let mut file_seconds = 0.0;
    let mut seen_seconds = 0.0;
    let mut names_seconds = 0.0;
    let root = active_run(conn, run, Some(dir))?;
    let (depth,fs):(i64,Option<String>)=conn.query_row("SELECT d.depth,v.filesystem FROM dir d JOIN root r ON r.id=d.root_id LEFT JOIN volume v ON v.id=r.volume_id WHERE d.id=?1",[dir],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let complete = matches!(outcome, ListingOutcome::Complete);
    let is_direct = direct.is_some();
    conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS old_file(id INTEGER PRIMARY KEY,name TEXT,raw BLOB,identity BLOB,used INTEGER);CREATE INDEX IF NOT EXISTS old_by_name ON old_file(name,raw);CREATE INDEX IF NOT EXISTS old_by_identity ON old_file(identity,used);DELETE FROM old_file")?;
    conn.execute(
        "INSERT INTO old_file SELECT id,name,name_utf16,file_id,0 FROM file WHERE dir_id=?1",
        [dir],
    )?;
    let has_old = conn.query_row("SELECT EXISTS(SELECT 1 FROM old_file)", [], |r| {
        r.get::<_, bool>(0)
    })?;
    conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS listing_seen(kind INTEGER,id INTEGER,PRIMARY KEY(kind,id));DELETE FROM listing_seen;CREATE TEMP TABLE IF NOT EXISTS listing_names(name TEXT,raw BLOB,kind INTEGER);DELETE FROM listing_names")?;
    let mut own = Totals {
        complete,
        ..Totals::default()
    };
    let mut noid = 0i64;
    let mut members_changed = false;
    let mut extensions = HashMap::new();
    let mut chunks = conn.prepare_cached(
        "SELECT entries FROM stage_entry WHERE run_id=?1 AND dir_id=?2 ORDER BY seq",
    )?;
    let mut rows = chunks.query(params![run, dir])?;
    let mut entry_count = 0;
    loop {
        let decoded;
        let (files, dirs) = if let Some(direct) = direct.take() {
            direct
        } else if let Some(row) = rows.next()? {
            let blob: Vec<u8> = row.get(0)?;
            decoded = serde_json::from_slice::<(Vec<Observation>, Vec<Observation>)>(&blob)
                .map_err(|_| Error::CorruptDatabase)?;
            (&decoded.0[..], &decoded.1[..])
        } else {
            break;
        };
        entry_count += files.len() + dirs.len();
        if entry_count > 2_000_000 {
            return Err(Error::Invalid(
                "listing entry limit; publish incomplete before limit",
            ));
        }
        let mut values = Vec::new();
        let mut tuples = Vec::new();
        for o in files {
            own.files += 1;
            own.logical = checked(own.logical, o.logical)?;
            own.allocated = checked(own.allocated, o.allocated.unwrap_or(0))?;
            own.allocation_unknown += u64::from(o.allocated.is_none());
            own.newest_ft = own.newest_ft.max(o.modified_ft);
            let identity = native_id(o, fs.as_deref());
            noid += i64::from(identity.is_none());
            let matched: Option<(i64, String, Option<Vec<u8>>)> = if has_old {
                let same=conn.prepare_cached("SELECT id,name,raw FROM old_file WHERE name=?1 AND raw IS ?2 AND identity IS ?3 AND used=0 LIMIT 1")?.query_row(params![o.name,o.name_utf16,identity],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
                if same.is_some() {
                    same
                } else if let Some(identity) = identity {
                    conn.prepare_cached(
                        "SELECT id,name,raw FROM old_file WHERE identity=?1 AND used=0 LIMIT 1",
                    )?
                    .query_row([identity], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                    .optional()?
                } else {
                    None
                }
            } else {
                None
            };
            let id = if let Some((id, name, raw)) = matched {
                conn.prepare_cached("UPDATE old_file SET used=1 WHERE id=?1")?
                    .execute([id])?;
                members_changed |= name != o.name || raw != o.name_utf16;
                Some(id)
            } else {
                None
            };
            members_changed |= id.is_none();
            let ext = if let Some(ext) = &o.extension {
                if let Some((id, family)) = extensions.get(ext) {
                    if family != &o.family {
                        return Err(Error::Invalid("conflicting extension family"));
                    }
                    Some(*id)
                } else {
                    conn.prepare_cached(
                        "INSERT INTO ext(ext,family) VALUES(?1,?2) ON CONFLICT(ext) DO NOTHING",
                    )?
                    .execute(params![ext, o.family])?;
                    let (id, family): (i64, String) = conn
                        .prepare_cached("SELECT id,family FROM ext WHERE ext=?1")?
                        .query_row([ext], |r| Ok((r.get(0)?, r.get(1)?)))?;
                    if family != o.family {
                        return Err(Error::Invalid("conflicting extension family"));
                    }
                    if extensions.len() < 1024 {
                        extensions.insert(ext.clone(), (id, family));
                    }
                    Some(id)
                }
            } else {
                None
            };
            values.extend([
                id.map_or(Value::Null, Value::Integer),
                Value::Integer(dir),
                Value::Text(o.name.clone()),
                o.name_utf16.clone().map_or(Value::Null, Value::Blob),
                ext.map_or(Value::Null, Value::Integer),
                identity.map_or(Value::Null, |v| Value::Blob(v.to_vec())),
                Value::Integer(o.logical as i64),
                o.allocated
                    .map_or(Value::Null, |v| Value::Integer(v as i64)),
                o.created_ft.map_or(Value::Null, Value::Integer),
                o.modified_ft.map_or(Value::Null, Value::Integer),
                o.changed_ft.map_or(Value::Null, Value::Integer),
                o.accessed_ft.map_or(Value::Null, Value::Integer),
                Value::Integer(o.attrs as i64),
                o.reparse_tag
                    .map_or(Value::Null, |v| Value::Integer(v as i64)),
                Value::Integer((o.flags & !HARDLINK_SUSPECTED) as i64),
                Value::Integer(run),
                Value::Integer(run),
            ]);
            tuples.push("(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)");
            if !is_direct {
                let name_start = Instant::now();
                conn.prepare_cached("INSERT INTO listing_names VALUES(?1,?2,1)")?
                    .execute(params![o.name, o.name_utf16])?;
                names_seconds += name_start.elapsed().as_secs_f64();
            }
            if tuples.len() == 128 {
                let (insert, seen) = insert_files(conn, &mut values, &mut tuples)?;
                file_seconds += insert;
                seen_seconds += seen;
            }
        }
        let (insert, seen) = insert_files(conn, &mut values, &mut tuples)?;
        file_seconds += insert;
        seen_seconds += seen;
        for o in dirs {
            if depth >= 511 {
                return Err(Error::Invalid("tree depth limit"));
            }
            let identity = native_id(o, fs.as_deref());
            let old_dir: Option<DirectoryMatch> = if let Some(id) = identity {
                conn.prepare_cached("SELECT id,parent_id,name,name_utf16,listing_state FROM dir WHERE root_id=?1 AND file_id=?2")?.query_row(params![root,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?
            } else {
                conn.prepare_cached("SELECT id,parent_id,name,name_utf16,listing_state FROM dir WHERE parent_id=?1 AND name=?2 AND name_utf16 IS ?3 AND file_id IS NULL")?.query_row(params![dir,o.name,o.name_utf16],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?
            };
            let id = if let Some((id, parent, name, utf16, state)) = old_dir {
                if parent != Some(dir) {
                    let parent = parent.ok_or(Error::Invalid("cannot reparent root"))?;
                    let cycle:bool=conn.query_row("WITH RECURSIVE a(id) AS (SELECT ?1 UNION ALL SELECT d.parent_id FROM dir d JOIN a ON d.id=a.id WHERE d.parent_id IS NOT NULL) SELECT EXISTS(SELECT 1 FROM a WHERE id=?2)",params![dir,id],|r|r.get(0))?;
                    if cycle {
                        return Err(Error::Invalid("directory identity cycle"));
                    }
                    ctx.dirty(conn, parent, run)?;
                    ctx.parents.insert(id, Some(dir));
                    ctx.dirty(conn, id, run)?;
                    members_changed = true;
                    conn.execute(
                        "UPDATE dir SET listing_rev=listing_rev+1 WHERE id=?1",
                        [parent],
                    )?;
                }
                members_changed |=
                    name != o.name || utf16 != o.name_utf16 || state == "absent_pending";
                conn.prepare_cached("UPDATE dir SET parent_id=?2,name=?3,name_utf16=?4,attrs=?5,reparse_tag=?6,flags=?7,created_ft=?8,modified_ft=?9,changed_ft=?10,listing_state=CASE WHEN listing_state='absent_pending' THEN 'unlisted' ELSE listing_state END WHERE id=?1")?.execute(params![id,dir,o.name,o.name_utf16,o.attrs,o.reparse_tag,o.flags,o.created_ft,o.modified_ft,o.changed_ft])?;
                id
            } else {
                let quality = identity.map_or("path_observation", |id| {
                    if id.len() == 16 {
                        "native_file_id_128"
                    } else {
                        "native_file_id_64"
                    }
                });
                conn.prepare_cached("INSERT INTO dir(root_id,parent_id,name,name_utf16,file_id,id_quality,id_basis,depth,attrs,reparse_tag,flags,created_ft,modified_ft,changed_ft,listing_state,born_run,seen_run) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,'unlisted',?15,0)")?.execute(params![root,dir,o.name,o.name_utf16,identity,quality,if identity.is_some(){o.id_basis.as_str()}else{"none"},depth+1,o.attrs,o.reparse_tag,o.flags,o.created_ft,o.modified_ft,o.changed_ft,run])?;
                let id = conn.last_insert_rowid();
                ctx.parents.insert(id, Some(dir));
                ctx.dirty(conn, id, run)?;
                members_changed = true;
                id
            };
            conn.prepare_cached("INSERT INTO listing_seen VALUES(0,?1)")?
                .execute([id])?;
            if !is_direct {
                conn.prepare_cached("INSERT INTO listing_names VALUES(?1,?2,0)")?
                    .execute(params![o.name, o.name_utf16])?;
            }
        }
    }
    if !is_direct
        && conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM listing_names GROUP BY name,raw HAVING count(*)>1)",
            [],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Err(Error::Invalid(
            "duplicate listing name or directory identity",
        ));
    }
    if complete {
        members_changed |= conn.execute(
            "DELETE FROM file WHERE id IN (SELECT id FROM old_file WHERE used=0)",
            [],
        )? > 0;
        let missing=conn.execute("UPDATE dir SET listing_state='absent_pending' WHERE parent_id=?1 AND listing_state!='absent_pending' AND id NOT IN (SELECT id FROM listing_seen WHERE kind=0)",[dir])?;
        members_changed |= missing > 0;
    } else {
        let sums:(i64,i64,i64,i64,i64,Option<i64>)=conn.query_row("SELECT count(*),coalesce(sum(logical),0),coalesce(sum(allocated),0),coalesce(sum(allocated IS NULL),0),coalesce(sum(file_id IS NULL),0),max(modified_ft) FROM file WHERE dir_id=?1",[dir],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
        own.files = sums.0 as u64;
        own.logical = sums.1 as u64;
        own.allocated = sums.2 as u64;
        own.allocation_unknown = sums.3 as u64;
        noid = sums.4;
        own.newest_ft = sums.5;
    }
    conn.execute("UPDATE dir SET listing_state=?2,seen_run=?3,listing_rev=listing_rev+?4,own_files=?5,own_logical=?6,own_allocated=?7,own_alloc_unknown=?8,own_noid=?9,own_newest_ft=?10,own_skipped=?11,own_errors=?12 WHERE id=?1",params![dir,if complete{"complete"}else{"incomplete"},run,i64::from(members_changed),own.files as i64,own.logical as i64,own.allocated as i64,own.allocation_unknown as i64,noid,own.newest_ft,skipped as i64,errors as i64])?;
    ctx.dirty(conn, dir, run)?;
    conn.execute(
        "DELETE FROM stage_entry WHERE run_id=?1 AND dir_id=?2",
        params![run, dir],
    )?;
    {
        let mut timing = ctx.timings.lock().expect("timing lock");
        timing.listing_seconds += listing_start.elapsed().as_secs_f64();
        timing.file_insert_seconds += file_seconds;
        timing.file_seen_seconds += seen_seconds;
        timing.name_validation_seconds += names_seconds;
    }
    Ok(WriteReply::Listing {
        revision: ctx.revision,
    })
}
fn insert_files(
    conn: &Connection,
    values: &mut Vec<Value>,
    tuples: &mut Vec<&str>,
) -> Result<(f64, f64)> {
    if tuples.is_empty() {
        return Ok((0.0, 0.0));
    }
    let insert_start = Instant::now();
    let sql=format!("INSERT INTO file(id,dir_id,name,name_utf16,ext_id,file_id,logical,allocated,created_ft,modified_ft,changed_ft,accessed_ft,attrs,reparse_tag,flags,born_run,seen_run) VALUES {} ON CONFLICT(id) DO UPDATE SET name=excluded.name,name_utf16=excluded.name_utf16,ext_id=excluded.ext_id,logical=excluded.logical,allocated=excluded.allocated,created_ft=excluded.created_ft,modified_ft=excluded.modified_ft,changed_ft=excluded.changed_ft,accessed_ft=excluded.accessed_ft,attrs=excluded.attrs,reparse_tag=excluded.reparse_tag,flags=excluded.flags,seen_run=excluded.seen_run",tuples.join(","));
    conn.prepare_cached(&sql)?
        .execute(rusqlite::params_from_iter(values.iter()))?;
    let insert_seconds = insert_start.elapsed().as_secs_f64();
    values.clear();
    tuples.clear();
    Ok((insert_seconds, 0.0))
}
fn checked(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b)
        .filter(|v| *v <= i64::MAX as u64)
        .ok_or(Error::Invalid("aggregate byte overflow"))
}
fn rebuild_multilink(conn: &Connection, root: i64) -> Result<()> {
    let volume: i64 = conn.query_row("SELECT volume_id FROM root WHERE id=?1", [root], |r| {
        r.get(0)
    })?;
    conn.execute("DELETE FROM multilink WHERE volume_id=?1", [volume])?;
    conn.execute("INSERT INTO multilink SELECT ?1,f.file_id,count(*),max(f.allocated) FROM file f JOIN dir d ON d.id=f.dir_id JOIN root r ON r.id=d.root_id WHERE r.volume_id=?1 AND f.file_id IS NOT NULL AND d.listing_state!='absent_pending' GROUP BY f.file_id HAVING count(*)>1",[volume])?;
    conn.execute("UPDATE file SET flags=flags & ~?2 WHERE dir_id IN (SELECT id FROM dir WHERE root_id=?1) AND flags & ?2 != 0",params![root,HARDLINK_SUSPECTED])?;
    conn.execute("UPDATE file SET flags=flags | ?2 WHERE dir_id IN (SELECT id FROM dir WHERE root_id=?1) AND file_id IN (SELECT file_id FROM multilink WHERE volume_id=?3)",params![root,HARDLINK_SUSPECTED,volume])?;
    Ok(())
}
/// One directory read, one postorder propagation, one prepared update per visible directory.
pub(crate) fn rollup(conn: &Connection, root: i64, clean: bool) -> Result<()> {
    struct Item {
        id: i64,
        parent: Option<i64>,
        totals: Totals,
        noid: i64,
        children: Vec<usize>,
    }
    let mut items = {
        let mut s=conn.prepare("SELECT id,parent_id,own_files,own_logical,own_allocated,own_alloc_unknown,own_skipped,own_errors,own_newest_ft,listing_state,own_noid FROM dir WHERE root_id=?1 AND listing_state!='absent_pending'")?;
        let rows = s
            .query_map([root], |r| {
                Ok(Item {
                    id: r.get(0)?,
                    parent: r.get(1)?,
                    totals: Totals {
                        files: r.get::<_, i64>(2)? as u64,
                        logical: r.get::<_, i64>(3)? as u64,
                        allocated: r.get::<_, i64>(4)? as u64,
                        allocation_unknown: r.get::<_, i64>(5)? as u64,
                        skipped: r.get::<_, i64>(6)? as u64,
                        errors: r.get::<_, i64>(7)? as u64,
                        newest_ft: r.get(8)?,
                        complete: clean
                            && r.get::<_, String>(9)? == "complete"
                            && r.get::<_, i64>(6)? == 0
                            && r.get::<_, i64>(7)? == 0,
                        ..Totals::default()
                    },
                    noid: r.get(10)?,
                    children: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let by_id: HashMap<_, _> = items.iter().enumerate().map(|(i, d)| (d.id, i)).collect();
    let Some(start) = items.iter().position(|d| d.parent.is_none()) else {
        return Err(Error::NotFound);
    };
    for i in 0..items.len() {
        if let Some(p) = items[i].parent.and_then(|p| by_id.get(&p).copied()) {
            items[p].children.push(i);
        }
    }
    let mut order = vec![(start, 0)];
    let mut cursor = 0;
    while cursor < order.len() {
        let (i, depth) = order[cursor];
        if depth > 511 {
            return Err(Error::Invalid("tree depth limit"));
        }
        for child in &items[i].children {
            order.push((*child, depth + 1));
        }
        cursor += 1;
    }
    let mut update=conn.prepare_cached("UPDATE dir SET depth=?2,sub_files=?3,sub_dirs=?4,sub_logical=?5,sub_allocated=?6,sub_alloc_unknown=?7,sub_skipped=?8,sub_errors=?9,sub_newest_ft=?10,sub_complete=?11,sub_noid=?12,agg_valid_rev=dirty_rev WHERE id=?1")?;
    for (i, depth) in order.into_iter().rev() {
        let t = items[i].totals.clone();
        let noid = items[i].noid;
        update.execute(params![
            items[i].id,
            depth,
            t.files as i64,
            t.dirs as i64,
            t.logical as i64,
            t.allocated as i64,
            t.allocation_unknown as i64,
            t.skipped as i64,
            t.errors as i64,
            t.newest_ft,
            t.complete,
            noid
        ])?;
        if let Some(p) = items[i].parent.and_then(|p| by_id.get(&p).copied()) {
            let a = &mut items[p];
            a.totals.files = checked(a.totals.files, t.files)?;
            a.totals.dirs = checked(a.totals.dirs, t.dirs + 1)?;
            a.totals.logical = checked(a.totals.logical, t.logical)?;
            a.totals.allocated = checked(a.totals.allocated, t.allocated)?;
            a.totals.allocation_unknown =
                checked(a.totals.allocation_unknown, t.allocation_unknown)?;
            a.totals.skipped = checked(a.totals.skipped, t.skipped)?;
            a.totals.errors = checked(a.totals.errors, t.errors)?;
            a.totals.newest_ft = a.totals.newest_ft.max(t.newest_ft);
            a.totals.complete &= t.complete;
            a.noid += noid;
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_commit_closes_the_writer_and_wakes_producers() {
        let temp = tempfile::tempdir().unwrap();
        let conn = crate::db::open_pair(temp.path(), "synthetic").unwrap();
        conn.execute_batch("CREATE TABLE fault_parent(id INTEGER PRIMARY KEY);CREATE TABLE fault_child(id INTEGER REFERENCES fault_parent(id) DEFERRABLE INITIALLY DEFERRED);CREATE TRIGGER fail_commit AFTER UPDATE ON revision BEGIN INSERT INTO fault_child VALUES(1);END;").unwrap();
        let writer = Writer::start(conn).unwrap();
        assert!(matches!(
            writer.call(WriteCommand::Barrier),
            Err(Error::Sql(_))
        ));
        assert!(matches!(
            writer.send(WriteCommand::Barrier),
            Err(Error::Closed)
        ));
    }
}
