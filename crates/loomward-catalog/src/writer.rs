use crate::*;
use crossbeam_channel::{bounded, Receiver, Sender};
use rusqlite::{
    params, types::ToSqlOutput, types::ValueRef as Value, Connection, OptionalExtension,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub const CHUNK_ENTRIES: usize = 16_384;
pub const QUEUE_BYTES: usize = 64 * 1024 * 1024;
const WRITER_BYTES: usize = 32 * 1024 * 1024;
const TRANSACTION_BYTES: usize = 64 * 1024 * 1024;
const TRANSACTION_TIME: Duration = Duration::from_millis(250);
const REFERENCE_PUBLICATION_ATTEMPTS: usize = 8;
#[cfg(test)]
thread_local! {
    static TRANSACTION_WINDOW: std::cell::Cell<Duration> =
        const { std::cell::Cell::new(TRANSACTION_TIME) };
}
/// Whether the batch window has closed. A test that pre-queues its commands and asserts how they
/// batch holds the window open (`hold_batch_window_open`), so a loaded host cannot split the batch.
fn batch_window_closed(start: Instant) -> bool {
    #[cfg(test)]
    let window = TRANSACTION_WINDOW.with(|w| w.get());
    #[cfg(not(test))]
    let window = TRANSACTION_TIME;
    start.elapsed() >= window
}
#[cfg(test)]
pub(crate) fn hold_batch_window_open() {
    TRANSACTION_WINDOW.with(|w| w.set(Duration::from_secs(3600)));
}
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
    bulk: Arc<AtomicBool>,
    #[cfg(any(test, feature = "test-hooks"))]
    window: Arc<PublicationWindow>,
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
        let bulk = Arc::new(AtomicBool::new(false));
        let enabled = bulk.clone();
        #[cfg(any(test, feature = "test-hooks"))]
        let window = Arc::new(PublicationWindow::default());
        #[cfg(any(test, feature = "test-hooks"))]
        let paused = window.clone();
        let join = std::thread::Builder::new()
            .name("loomward-catalog".into())
            .spawn(move || {
                let _scratch = scratch;
                serve(
                    conn,
                    rx,
                    measured,
                    alive,
                    enabled,
                    #[cfg(test)]
                    None,
                    #[cfg(any(test, feature = "test-hooks"))]
                    Some(paused),
                );
            })?;
        Ok(Self {
            tx: Some(tx),
            quota,
            stopped,
            join: Some(join),
            timings,
            bulk,
            #[cfg(any(test, feature = "test-hooks"))]
            window,
        })
    }
    /// A full scan of a root with no earlier run then builds into unindexed bulk tables that
    /// readers never see, and its `EndRun` publishes the whole root atomically (#148). Decided per
    /// run at `BeginRun`; off by default.
    pub fn set_bulk_first_scan(&self, enabled: bool) {
        self.bulk.store(enabled, Ordering::Relaxed);
    }
    /// Test seam (#356): the gate inside the pending-publication window.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn publication_window(&self) -> &PublicationWindow {
        &self.window
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
            skipped,
            errors,
            outcome,
            ..
        } => {
            outcome.coverage()?;
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
    pub staging_seconds: f64,
    pub staging_calls: u64,
    pub listing_calls: u64,
    pub rollup_calls: u64,
    pub batch_wait_calls: u64,
    pub batch_wait_seconds: f64,
    pub validation_dry_run_seconds: f64,
    pub state_prepare_seconds: f64,
    pub invalidation_seconds: f64,
    pub rollup_seconds: f64,
    pub multilink_seconds: f64,
    pub transactions: u64,
    pub commands: u64,
    pub max_transaction_bytes: usize,
    pub max_transaction_commands: usize,
    pub listing_seconds: f64,
    pub file_insert_seconds: f64,
    pub file_seen_seconds: f64,
    pub name_validation_seconds: f64,
    pub commit_seconds: f64,
    pub checkpoint_seconds: f64,
    pub dirty_updates: u64,
}
/// The tables a run's listings publish into: the live catalogue, or a first-scan bulk build (#148)
/// that no reader queries until `EndRun` swaps it in. Directory IDs are unique across both pairs
/// except the root directory, whose bulk copy keeps its ID and parent.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Tables {
    dir: &'static str,
    file: &'static str,
}
const LIVE: Tables = Tables {
    dir: "dir",
    file: "file",
};
const BULK: Tables = Tables {
    dir: "bulk_dir",
    file: "bulk_file",
};
struct Context {
    timings: Arc<Mutex<WriterTimings>>,
    parents: HashMap<i64, Option<i64>>,
    dirty: Vec<(i64, i64, i64)>,
    extensions: HashMap<String, (i64, String)>,
    revision: i64,
    fenced_listings: HashSet<(i64, i64)>,
    /// Runs building into the bulk tables, with their root.
    bulk: HashMap<i64, i64>,
    bulk_enabled: Arc<AtomicBool>,
}
impl Context {
    fn tables(&self, run: i64) -> Tables {
        if self.bulk.contains_key(&run) {
            BULK
        } else {
            LIVE
        }
    }
    fn dirty(&mut self, start: i64, run: i64) {
        self.dirty.push((start, self.revision, run));
    }
    fn parent(&mut self, conn: &Connection, t: Tables, id: i64) -> Result<Option<i64>> {
        if let Some(parent) = self.parents.get(&id) {
            return Ok(*parent);
        }
        let parent = conn
            .prepare_cached(&format!("SELECT parent_id FROM {} WHERE id=?1", t.dir))?
            .query_row([id], |r| r.get(0))?;
        self.parents.insert(id, parent);
        Ok(parent)
    }
    fn chain_length(
        &mut self,
        conn: &Connection,
        t: Tables,
        start: i64,
        proven: &mut HashMap<i64, usize>,
    ) -> Result<usize> {
        let mut current = Some(start);
        let mut path = Vec::new();
        let mut length = 0;
        while let Some(id) = current {
            if let Some(known) = proven.get(&id) {
                length = *known;
                break;
            }
            path.push(id);
            if path.len() > MAX_DIRECTORY_CHAIN_LENGTH {
                return Err(Error::Invalid("tree depth limit"));
            }
            current = self.parent(conn, t, id)?;
        }
        if path.len() + length > MAX_DIRECTORY_CHAIN_LENGTH {
            return Err(Error::Invalid("tree depth limit"));
        }
        for id in path.into_iter().rev() {
            length += 1;
            proven.insert(id, length);
        }
        Ok(length)
    }
    /// Parent links are authoritative until rollup refreshes stored depths.
    fn check_chains(&mut self, conn: &Connection, from: usize) -> Result<()> {
        let mut proven = HashMap::new();
        for i in from..self.dirty.len() {
            let (start, _, run) = self.dirty[i];
            self.chain_length(conn, self.tables(run), start, &mut proven)?;
        }
        Ok(())
    }
    fn flush_dirty(&mut self, conn: &Connection) -> Result<()> {
        let started = Instant::now();
        let mut marked = HashSet::new();
        // Newest seeds win: every ancestor retains its latest input revision, not the batch's first.
        // Taking the seeds empties them on error too; the failed commit clears them anyway.
        for (start, revision, run) in std::mem::take(&mut self.dirty).into_iter().rev() {
            let t = self.tables(run);
            let mut update = conn.prepare_cached(&format!(
                "UPDATE {} SET subtree_rev=?2,dirty_rev=?2,dirty_run=?3,sub_complete=0 WHERE id=?1",
                t.dir
            ))?;
            let mut current = Some(start);
            let mut count = 0;
            while let Some(id) = current {
                if !marked.insert((t, id)) {
                    break;
                }
                update.execute(params![id, revision, run])?;
                self.timings.lock().expect("timing lock").dirty_updates += 1;
                current = self.parent(conn, t, id)?;
                count += 1;
                // Backstop only: check_chains already bounded every seed inside its savepoint.
                if count > MAX_DIRECTORY_CHAIN_LENGTH {
                    return Err(Error::Invalid("directory identity cycle"));
                }
            }
        }
        self.timings
            .lock()
            .expect("timing lock")
            .invalidation_seconds += started.elapsed().as_secs_f64();
        Ok(())
    }
}
#[cfg(test)]
#[derive(Clone, Copy)]
enum PublicationCrash {
    BeforeCatalog,
    AfterCatalog,
    Observe(fn(&Connection, bool)),
    BeforeFence(fn(&Connection)),
}
/// Test seam (#356): holds the writer after a publication's precious staging has committed and
/// before its catalogue transaction begins, so a test can act inside that window. Reference-staging
/// commands and listing-led batches (including StageChunk) pause; the gate is one-shot.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Default)]
pub struct PublicationWindow {
    /// (armed, paused)
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}
#[cfg(any(test, feature = "test-hooks"))]
impl PublicationWindow {
    /// The next reference-staging command or listing-led batch pauses inside the window.
    pub fn arm(&self) {
        *self.state.lock().expect("window lock") = (true, false);
    }
    /// Blocks until the writer is paused inside the window (panics after a minute).
    pub fn wait_paused(&self) {
        let give_up = Instant::now() + Duration::from_secs(60);
        let mut state = self.state.lock().expect("window lock");
        while !state.1 {
            assert!(Instant::now() < give_up, "writer never reached the window");
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(50))
                .expect("window lock")
                .0;
        }
    }
    /// Lets the paused writer continue towards the catalogue transaction.
    pub fn release(&self) {
        *self.state.lock().expect("window lock") = (false, false);
        self.changed.notify_all();
    }
    fn pause(&self) {
        let mut state = self.state.lock().expect("window lock");
        if !state.0 {
            return;
        }
        state.1 = true;
        self.changed.notify_all();
        // Bounded, so a test that fails while the writer is paused ends instead of hanging the
        // binary when the writer's drop joins this thread.
        let give_up = Instant::now() + Duration::from_secs(60);
        while state.0 && Instant::now() < give_up {
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(50))
                .expect("window lock")
                .0;
        }
    }
}
/// Commands whose precious preparation stages reference intents from a snapshot of `object_ref`.
fn stages_references(command: &WriteCommand) -> bool {
    matches!(
        command,
        WriteCommand::DirListing(_)
            | WriteCommand::ListingDone { .. }
            | WriteCommand::EndRun { .. }
    )
}
/// AUTOINCREMENT's high-water mark: every new reference raises it, even after a deletion.
fn reference_mark(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT coalesce((SELECT seq FROM st.sqlite_sequence WHERE name='object_ref'),0)",
        [],
        |r| r.get(0),
    )
}
/// Reference intents are staged from a snapshot of `object_ref` and commit before the catalogue
/// transaction begins, so another connection can bind a new reference in between (#356):
/// `collections.update_members` does, inside BEGIN IMMEDIATE over both files. Once this
/// transaction holds main's write lock no such binding can start or still be uncommitted, so an
/// unchanged mark proves the staging saw every reference this publication can move. Any future
/// reference writer outside this thread must also take main's write lock.
fn reference_fence(tx: &Connection, mark: i64) -> rusqlite::Result<bool> {
    tx.execute("UPDATE revision SET catalog_rev=catalog_rev WHERE id=1", [])?;
    Ok(reference_mark(tx)? == mark)
}
fn listing_command(command: &WriteCommand) -> bool {
    matches!(
        command,
        WriteCommand::DirListing(_)
            | WriteCommand::StageChunk { .. }
            | WriteCommand::ListingDone { .. }
    )
}
fn staged_run(command: &WriteCommand) -> Option<i64> {
    match command {
        WriteCommand::StageChunk { run_id, .. } | WriteCommand::ListingDone { run_id, .. } => {
            Some(*run_id)
        }
        _ => None,
    }
}
fn transaction_bytes(p: &Pending, first_run: Option<i64>) -> usize {
    if first_run.is_some() && staged_run(&p.command) == first_run {
        p._permit
            .bytes
            .min(validate(&p.command).unwrap_or(p._permit.bytes))
    } else {
        p._permit.bytes
    }
}
#[cfg(test)]
fn work(
    conn: Connection,
    rx: Receiver<Pending>,
    timings: Arc<Mutex<WriterTimings>>,
    alive: Sender<()>,
    crash: Option<PublicationCrash>,
) {
    serve(conn, rx, timings, alive, Arc::default(), crash, None)
}
fn serve(
    mut conn: Connection,
    rx: Receiver<Pending>,
    timings: Arc<Mutex<WriterTimings>>,
    alive: Sender<()>,
    bulk_enabled: Arc<AtomicBool>,
    #[cfg(test)] crash: Option<PublicationCrash>,
    #[cfg(any(test, feature = "test-hooks"))] window: Option<Arc<PublicationWindow>>,
) {
    let mut alive = Some(alive);
    let mut pending = None;
    let mut context = Context {
        timings,
        parents: HashMap::new(),
        dirty: Vec::new(),
        extensions: HashMap::new(),
        revision: 0,
        fenced_listings: HashSet::new(),
        bulk: HashMap::new(),
        bulk_enabled,
    };
    // The state rewrite and WAL cut after a forget, when a lock outlasted the first try: retried between commands.
    // A forget that committed its state half but whose rewrite and log cut never finished (a crash,
    // a lock that outlasted open) leaves the marker in state.db: finish it here.
    let mut cut_pending = scrub_pending(&conn).unwrap_or(true);
    let mut vacuum_pending = cut_pending;
    let mut retry_delay = Duration::from_millis(500);
    let mut next_retry = Instant::now() + retry_delay;
    // A store that has just opted into the ranked token index is backfilled here, one bounded
    // round per transaction, only while no command is waiting (#520).
    let mut backfill = crate::search::backfilling(&conn).unwrap_or(false);
    let mut backfill_after = Instant::now();
    loop {
        let first = loop {
            if cut_pending && Instant::now() >= next_retry {
                cut_pending = !wal_cut(&conn, Duration::ZERO, &mut vacuum_pending);
                retry_delay = (retry_delay * 2).min(Duration::from_secs(30));
                next_retry = Instant::now() + retry_delay;
            }
            if backfill && pending.is_none() && Instant::now() >= backfill_after {
                match rx.try_recv() {
                    Ok(p) => pending = Some(p),
                    Err(crossbeam_channel::TryRecvError::Empty) => {
                        // A failed round (a lock held elsewhere) rolls back and is retried a second later.
                        match backfill_step(&mut conn) {
                            Ok(more) => backfill = more,
                            Err(_) => backfill_after = Instant::now() + Duration::from_secs(1),
                        }
                        continue;
                    }
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {}
                }
            }
            if let Some(p) = pending.take() {
                break Some(p);
            }
            if !cut_pending && !backfill {
                break rx.recv().ok();
            }
            let wake = match (cut_pending, backfill) {
                (true, true) => next_retry.min(backfill_after),
                (true, false) => next_retry,
                _ => backfill_after,
            };
            match rx.recv_timeout(wake.saturating_duration_since(Instant::now())) {
                Ok(p) => break Some(p),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(_) => break None,
            }
        };
        let Some(first) = first else {
            break;
        };
        let checkpoint = matches!(first.command, WriteCommand::EndRun { .. });
        let compact_bulk = matches!(&first.command, WriteCommand::EndRun { run_id, .. } if context.bulk.contains_key(run_id));
        let forget = matches!(first.command, WriteCommand::ForgetGrant { .. });
        let mut batch = vec![first];
        let first_run = staged_run(&batch[0].command).filter(|run| {
            conn.query_row("SELECT s.mode='full' AND NOT EXISTS(SELECT 1 FROM scan_run prior WHERE prior.root_id=s.root_id AND prior.id<s.id) FROM scan_run s WHERE s.id=?1", [run], |r| r.get::<_, bool>(0)).unwrap_or(false)
        });
        let mut results = Vec::with_capacity(batch.len());
        let mut failed = false;
        // A batch led by any listing command may grow with listings that stage nothing, trusting
        // the no-resolved-reference check below: the fence proves no reference bound since (#389).
        let stages = stages_references(&batch[0].command) || listing_command(&batch[0].command);
        let mut batchable = false;
        let mut prepared = None;
        let mut full = false;
        // The transaction borrows the connection only until the batch is applied.
        {
            // Precious writes commit before the catalogue transaction, which writes main only.
            let mut attempts = 0;
            let opened = loop {
                attempts += 1;
                // A reference bound after this mark is invisible to the staging below (#356).
                let mark = match stages.then(|| reference_mark(&conn)).transpose() {
                    Ok(mark) => mark,
                    Err(e) => break Err((Error::Sql(e), false)),
                };
                batchable = listing_command(&batch[0].command)
                    && !conn
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM st.object_ref WHERE state='resolved')",
                            [],
                            |r| r.get::<_, bool>(0),
                        )
                        .unwrap_or(true);
                let prepare_start = Instant::now();
                prepared = Some(prepare_state(
                    &mut conn,
                    &batch[0].command,
                    &context.fenced_listings,
                    &context.bulk,
                ));
                context
                    .timings
                    .lock()
                    .expect("timing lock")
                    .state_prepare_seconds += prepare_start.elapsed().as_secs_f64();
                #[cfg(any(test, feature = "test-hooks"))]
                if let Some(window) = window.as_ref().filter(|_| stages) {
                    window.pause();
                }
                #[cfg(test)]
                if let Some(PublicationCrash::BeforeFence(observe)) = crash.filter(|_| stages) {
                    observe(&conn);
                }
                let durability = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM st.pending_reference)",
                    [],
                    |r| r.get::<_, bool>(0),
                );
                // Sticky across a re-stage, so NORMAL is always restored after the batch.
                full |= matches!(durability, Ok(true));
                // The publication must survive power loss before FULL state confirmation can retire it.
                let tx = match durability.and_then(|pending| {
                    if pending {
                        conn.execute_batch("PRAGMA main.synchronous=FULL")?;
                    }
                    conn.transaction()
                }) {
                    Ok(tx) => tx,
                    Err(e) => break Err((Error::Sql(e), true)),
                };
                let Some(mark) = mark else {
                    break Ok(tx);
                };
                match reference_fence(&tx, mark) {
                    Ok(true) => break Ok(tx),
                    Ok(false) => {}
                    // The command fails unpublished; reconciliation discards its intents.
                    Err(e) => break Err((Error::Sql(e), false)),
                }
                drop(tx);
                // Nothing is published: drop this attempt's intents and stage again over every reference.
                let unpublished = "DELETE FROM st.pending_reference WHERE publication_token IS NOT (SELECT value FROM meta WHERE key='reference_publication')";
                if let Err(e) = conn.execute(unpublished, []) {
                    break Err((Error::Sql(e), false));
                }
                if attempts == REFERENCE_PUBLICATION_ATTEMPTS {
                    break Err((Error::Invalid("reference publication retry limit"), false));
                }
            };
            #[cfg(test)]
            if matches!(crash, Some(PublicationCrash::BeforeCatalog)) {
                break;
            }
            match opened {
                Ok(mut tx) => {
                    let transaction_start = Instant::now();
                    let mut bytes = 0;
                    loop {
                        let p = batch.last_mut().expect("transaction command");
                        let command_bytes = transaction_bytes(p, first_run);
                        let ready = prepared.take().unwrap_or(Ok(None));
                        let bulk_before = matches!(
                            p.command,
                            WriteCommand::BeginRun { .. }
                                | WriteCommand::EndRun { .. }
                                | WriteCommand::RevokeGrant { .. }
                                | WriteCommand::GrantIdentityChanged { .. }
                        )
                        .then(|| context.bulk.clone());
                        results.push(ready.and_then(|prepared| {
                        let save = tx.savepoint()?;
                        save.execute(
                            "UPDATE revision SET catalog_rev=catalog_rev+1 WHERE id=1",
                            [],
                        )?;
                        context.revision =
                            save.query_row("SELECT catalog_rev FROM revision", [], |r| r.get(0))?;
                        let dirty_start = context.dirty.len();
                        let result = apply(&save, &p.command, &mut context, prepared)
                            .and_then(|reply| {
                                context.check_chains(&save, dirty_start)?;
                                // Names changed by this command reach the token index with it.
                                crate::search::drain_tokens(&save)?;
                                Ok(reply)
                            });
                        // The witness commits atomically with the catalogue publication.
                        let result = result.and_then(|reply| {
                            save.execute("INSERT INTO meta(key,value) SELECT 'reference_publication',publication_token FROM st.pending_reference LIMIT 1 ON CONFLICT(key) DO UPDATE SET value=excluded.value", [])?;
                            Ok(reply)
                        });
                        match result {
                            Ok(reply) => match save.commit() {
                                Ok(()) => Ok(reply),
                                Err(e) => {
                                    context.dirty.truncate(dirty_start);
                                    context.extensions.clear();
                                    context.parents.clear();
                                    Err(Error::Sql(e))
                                }
                            },
                            Err(e) => {
                                drop(save);
                                context.dirty.truncate(dirty_start);
                                context.extensions.clear();
                                context.parents.clear();
                                Err(e)
                            }
                        }
                    }));
                        if results.last().is_some_and(Result::is_err) {
                            if let Some(bulk) = bulk_before {
                                context.bulk = bulk;
                            }
                            if let WriteCommand::StageChunk { run_id, dir_id, .. } = &p.command {
                                // Savepoint rollback must not make the accepted prefix publishable as complete.
                                context.fenced_listings.insert((*run_id, *dir_id));
                            }
                            // A refused finish still releases a revoked run in this transaction (#733).
                            // Never open a savepoint after SQLite has rolled back the outer transaction.
                            if !tx.is_autocommit() {
                                if let Err(e) =
                                    settle_inactive_run(&mut tx, &p.command, &mut context)
                                {
                                    *results.last_mut().expect("refused command") = Err(e);
                                }
                            }
                        }
                        bytes += command_bytes;
                        // Free the applied payload so producers can refill the bounded queue before COMMIT.
                        p.command = WriteCommand::Barrier;
                        release(&p._permit.quota, p._permit.bytes);
                        p._permit.bytes = 0;
                        // An auto-rollback must not let the next savepoint commit a new transaction.
                        if tx.is_autocommit()
                            || !batchable
                            || bytes >= TRANSACTION_BYTES
                            || batch_window_closed(transaction_start)
                        {
                            break;
                        }
                        let wait_start = Instant::now();
                        // A synchronous caller cannot enqueue again until this transaction commits.
                        let next = rx.try_recv();
                        {
                            let mut timing = context.timings.lock().expect("timing lock");
                            timing.batch_wait_calls += 1;
                            timing.batch_wait_seconds += wait_start.elapsed().as_secs_f64();
                        }
                        match next {
                            Ok(next)
                                if listing_command(&next.command)
                                    && bytes + transaction_bytes(&next, first_run)
                                        <= TRANSACTION_BYTES =>
                            {
                                batch.push(next)
                            }
                            Ok(next) => {
                                pending = Some(next);
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    let commit_start = Instant::now();
                    #[cfg(test)]
                    if let Some(PublicationCrash::Observe(observe)) = crash {
                        observe(&tx, false);
                    }
                    // After an auto-rollback the connection is in autocommit mode: flushing dirty marks
                    // would commit them one by one for a batch that never happened. COMMIT then fails.
                    let committed = if tx.is_autocommit() {
                        tx.commit().map_err(Error::Sql)
                    } else {
                        context
                            .flush_dirty(&tx)
                            .and_then(|()| tx.commit().map_err(Error::Sql))
                    };
                    context.timings.lock().expect("timing lock").commit_seconds +=
                        commit_start.elapsed().as_secs_f64();
                    {
                        let mut timing = context.timings.lock().expect("timing lock");
                        timing.transactions += 1;
                        timing.commands += batch.len() as u64;
                        timing.max_transaction_bytes = timing.max_transaction_bytes.max(bytes);
                        timing.max_transaction_commands =
                            timing.max_transaction_commands.max(batch.len());
                    }
                    if let Err(e) = committed {
                        failed = true;
                        fail_batch(&mut results, e);
                        context.parents.clear();
                        context.dirty.clear();
                        context.extensions.clear();
                    }
                }
                Err((e, fatal)) => {
                    failed = fatal;
                    results.push(Err(e));
                }
            }
        }
        if full
            && conn
                .execute_batch("PRAGMA main.synchronous=NORMAL")
                .is_err()
        {
            failed = true;
        }
        #[cfg(test)]
        if let Some(PublicationCrash::Observe(observe)) = crash {
            observe(&conn, true);
        }
        #[cfg(test)]
        if matches!(crash, Some(PublicationCrash::AfterCatalog)) {
            assert!(!failed);
            break;
        }
        if crate::db::reconcile_references(&mut conn).is_err() {
            // Publication already committed; pending bindings remain readable and retry on open.
            failed = true;
        }
        while results.len() < batch.len() {
            results.push(Err(Error::Closed));
        }
        if failed {
            alive.take();
        }
        if checkpoint {
            let profile = std::env::var_os("LOOMWARD_SCAN_PROFILE").is_some();
            let before = profile.then(|| checkpoint_probe(&conn));
            let started = Instant::now();
            let result = end_checkpoint(&conn, compact_bulk && results.iter().all(Result::is_ok));
            let seconds = started.elapsed().as_secs_f64();
            if profile {
                eprintln!(
                    "scan-profile checkpoint {}",
                    serde_json::json!({
                        "before": before, "result": result.ok(), "seconds": seconds,
                        "after": checkpoint_probe(&conn),
                    })
                );
            }
            context
                .timings
                .lock()
                .expect("timing lock")
                .checkpoint_seconds += seconds;
        }
        if forget && results.iter().all(Result::is_ok) {
            vacuum_pending = true;
            cut_pending = !wal_cut(&conn, Duration::from_millis(750), &mut vacuum_pending);
            retry_delay = Duration::from_millis(500);
            next_retry = Instant::now() + retry_delay;
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
    crate::crashpoint::hit("shutdown checkpoint");
    let _ = conn.execute_batch("PRAGMA main.wal_checkpoint(TRUNCATE)");
}
/// Whether a forget's rewrite of the stores is still owed (the `scrub_pending` marker, written in
/// the forget's own state.db transaction and cleared only after both rewrites and both log cuts).
pub(crate) fn scrub_pending(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM st.meta WHERE key='scrub_pending')",
        [],
        |r| r.get(0),
    )?)
}
/// Page images from before a purge sit in the WALs until they are folded in and cut, and stale
/// cells freed without secure_delete (a staged listing deleted when it was published, a cell an
/// earlier build freed) sit in the free space and unused tails of both files until they are
/// rewritten: while `vacuum` is set, `VACUUM st` and `VACUUM main` run first (#391). Waits at most `wait` for a lock, so a
/// slow reader or writer never holds the committed receipt; the caller retries until the rewrite is
/// done and both logs are cut. Whether all of it was done.
pub(crate) fn wal_cut(conn: &Connection, wait: Duration, vacuum: &mut bool) -> bool {
    // A state half can commit before its catalogue purge fails. A different forget's rewrite
    // must not clear that durable health marker while the forgotten rows still exist.
    if !conn.query_row(
        "SELECT NOT EXISTS(SELECT 1 FROM main.root r JOIN st.root_forgotten f ON f.grant_id=r.grant_id)",
        [],
        |r| r.get::<_, bool>(0),
    ).unwrap_or(false) {
        return false;
    }
    let Ok(timeout) = conn.query_row("PRAGMA busy_timeout", [], |r| r.get::<_, u32>(0)) else {
        return false;
    };
    let _ = conn.busy_timeout(wait);
    if *vacuum {
        let rewrite = || -> Result<()> {
            let temp: i64 = conn.query_row("PRAGMA temp_store", [], |r| r.get(0))?;
            conn.pragma_update(None, "temp_store", "FILE")?;
            crate::crashpoint::hit("VACUUM st");
            let result = conn.execute_batch("VACUUM st").and_then(|()| {
                crate::crashpoint::hit("VACUUM main");
                conn.execute_batch("VACUUM main")
            });
            // Only a changed temp_store drops temporary objects; listing tables are recreated lazily.
            let restore = conn.pragma_update(None, "temp_store", temp);
            let views = if temp == 1 {
                Ok(())
            } else {
                crate::db::grant_view(conn)
            };
            result?;
            restore?;
            views
        };
        *vacuum = rewrite().is_err();
        if *vacuum {
            let _ = conn.busy_timeout(Duration::from_millis(u64::from(timeout)));
            return false;
        }
    }
    let cut = |db: &str| {
        crate::crashpoint::hit("wal cut");
        conn.query_row(&format!("PRAGMA {db}.wal_checkpoint(TRUNCATE)"), [], |r| {
            r.get::<_, i64>(0)
        })
        .is_ok_and(|busy| busy == 0)
    };
    let (state, catalog) = (cut("st"), cut("main"));
    let _ = conn.busy_timeout(Duration::from_millis(u64::from(timeout)));
    let done = state && catalog;
    if done && !*vacuum {
        crate::crashpoint::hit("clear scrub marker");
        return conn
            .execute("DELETE FROM st.meta WHERE key='scrub_pending'", [])
            .is_ok();
    }
    done
}
/// One backfill round in its own transaction. Whether keys remain.
fn backfill_step(conn: &mut Connection) -> Result<bool> {
    let tx = conn.transaction()?;
    let more = crate::search::backfill_round(&tx)?;
    tx.commit()?;
    Ok(more)
}
fn end_checkpoint(conn: &Connection, compact_bulk: bool) -> rusqlite::Result<[i64; 3]> {
    let timeout = conn.query_row("PRAGMA busy_timeout", [], |r| r.get::<_, u32>(0))?;
    // Best-effort WAL maintenance must not hold a committed EndRun receipt behind a reader.
    conn.busy_timeout(Duration::ZERO)?;
    if compact_bulk {
        // Best effort after publication: reclaim large build leftovers, never fail committed rows.
        let reclaim = conn.query_row(
            "SELECT freelist_count*page_size>=1048576 AND freelist_count*10>=page_count
             FROM pragma_freelist_count, pragma_page_size, pragma_page_count",
            [],
            |r| r.get::<_, bool>(0),
        );
        if matches!(reclaim, Ok(true)) {
            let _ = conn.execute_batch("VACUUM main");
        }
    }
    crate::crashpoint::hit("end-of-run checkpoint");
    let result = conn.query_row("PRAGMA main.wal_checkpoint(TRUNCATE)", [], |r| {
        Ok([r.get(0)?, r.get(1)?, r.get(2)?])
    });
    conn.busy_timeout(Duration::from_millis(u64::from(timeout)))?;
    result
}
fn checkpoint_probe(conn: &Connection) -> serde_json::Value {
    serde_json::json!({
        "transaction_state": format!("{:?}", conn.transaction_state(Some("main"))),
        "busy_statement": conn.is_busy(),
        "wal_bytes": conn.path().and_then(|p| std::fs::metadata(format!("{p}-wal")).ok()).map(|m| m.len()),
        "frames": conn.query_row("PRAGMA main.wal_checkpoint(NOOP)", [], |r| Ok([r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?])).ok(),
        "catalogue_synchronous": conn.query_row("PRAGMA main.synchronous", [], |r| r.get::<_, i64>(0)).ok(),
        "state_synchronous": conn.query_row("PRAGMA st.synchronous", [], |r| r.get::<_, i64>(0)).ok(),
        "journal_mode": conn.query_row("PRAGMA main.journal_mode", [], |r| r.get::<_, String>(0)).ok(),
    })
}
fn fail_batch(results: &mut [Result<WriteReply>], error: Error) {
    if results.iter().any(Result::is_err) {
        // An aborted batch loses successful rows, but keeps each caller's first failure.
        for result in results {
            if result.is_ok() {
                *result = Err(Error::Closed);
            }
        }
    } else {
        let reason = error.to_string();
        for result in results.iter_mut().skip(1) {
            *result = Err(Error::BatchAborted(reason.clone()));
        }
        if let Some(result) = results.first_mut() {
            *result = Err(error);
        }
    }
}
fn active_run(conn: &Connection, t: Tables, run: i64, dir: Option<i64>) -> Result<i64> {
    let root:Option<i64>=conn.query_row("SELECT s.root_id FROM scan_run s JOIN root r ON r.id=s.root_id JOIN st.root_grant g ON g.id=r.grant_id WHERE s.id=?1 AND s.state='running' AND r.active_run=s.id AND g.state='active'",[run],|r|r.get(0)).optional()?;
    let root = root.ok_or(Error::Invalid("inactive run or grant"))?;
    if let Some(dir) = dir {
        if !conn.prepare_cached(&format!("SELECT EXISTS(SELECT 1 FROM {} WHERE id=?1 AND root_id=?2 AND listing_state!='absent_pending')",t.dir))?.query_row(params![dir,root],|r|r.get::<_,bool>(0))?{return Err(Error::NotFound);}
    }
    Ok(root)
}
fn settle_inactive_run(
    tx: &mut rusqlite::Transaction<'_>,
    command: &WriteCommand,
    ctx: &mut Context,
) -> Result<()> {
    let WriteCommand::EndRun {
        run_id,
        finished_at_ns,
        ..
    } = command
    else {
        return Ok(());
    };
    let save = tx.savepoint()?;
    let root: Option<i64> = save.query_row(
        "SELECT s.root_id FROM scan_run s JOIN root r ON r.id=s.root_id JOIN st.root_grant g ON g.id=r.grant_id WHERE s.id=?1 AND s.state='running' AND r.active_run=s.id AND g.state!='active'",
        [run_id], |r| r.get(0),
    ).optional()?;
    let Some(root) = root else { return Ok(()) };
    save.execute("UPDATE scan_run SET state='cancelled',finished_at_ns=?2,error_json='{\"code\":\"grant_not_active\"}' WHERE id=?1", params![run_id, finished_at_ns])?;
    save.execute(
        "UPDATE root SET active_run=NULL,state='stale' WHERE id=?1",
        [root],
    )?;
    save.execute("DELETE FROM stage_entry WHERE run_id=?1", [run_id])?;
    save.execute("DELETE FROM bulk_dir WHERE root_id=?1", [root])?;
    let revision = save.query_row(
        "UPDATE revision SET catalog_rev=catalog_rev+1 WHERE id=1 RETURNING catalog_rev",
        [],
        |r| r.get(0),
    )?;
    save.commit()?;
    ctx.revision = revision;
    ctx.bulk.remove(run_id);
    ctx.fenced_listings.retain(|(run, _)| run != run_id);
    ctx.parents.clear();
    Ok(())
}
fn state_revision(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE st.revision SET state_rev=state_rev+1 WHERE id=1",
        [],
    )?;
    Ok(())
}
enum Prepared {
    Id(i64),
    Listing,
}
fn prepare_state(
    conn: &mut Connection,
    c: &WriteCommand,
    fenced: &HashSet<(i64, i64)>,
    bulk: &HashMap<i64, i64>,
) -> Result<Option<Prepared>> {
    match c {
        // No reference can bind a row that was never published, so a bulk build retires none.
        WriteCommand::DirListing(DirListing { run_id, .. })
        | WriteCommand::ListingDone { run_id, .. }
            if bulk.contains_key(run_id) =>
        {
            Ok(None)
        }
        WriteCommand::RegisterRoot(o) | WriteCommand::RefreshRoot { observation: o, .. } => {
            let dataset: String = conn.query_row(
                "SELECT value FROM st.meta WHERE key='dataset_class'",
                [],
                |r| r.get(0),
            )?;
            if !matches!(
                (dataset.as_str(), o.origin.as_str()),
                ("synthetic", "fixture" | "lab_generated") | ("personal", "owner_granted")
            ) {
                return Err(Error::DatasetMismatch);
            }
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
            let tx = conn.transaction()?;
            let lab: Option<i64> = if o.origin == "lab_generated" {
                Some(
                    tx.query_row(
                        "SELECT id FROM st.lab_root WHERE volume_key=?1 AND root_file_id=?2 AND volume_guid=?3",
                        params![o.volume_key, o.root_file_id, o.volume_guid],
                        |r| r.get(0),
                    )
                    .optional()?
                    .ok_or(Error::Invalid("unregistered lab root"))?,
                )
            } else {
                None
            };
            // #219: a root observed on a filesystem that cannot verify identity is granted
            // identity-unverified; the service confirmed a personal one with the owner first.
            let mode = if o.filesystem.as_deref().is_some_and(|fs| {
                !fs.eq_ignore_ascii_case("NTFS") && !fs.eq_ignore_ascii_case("ReFS")
            }) {
                "unverified"
            } else {
                "verified"
            };
            let grant = if let WriteCommand::RefreshRoot { grant_id, .. } = c {
                // A recyclable unverified ID also needs the same stored path.
                let stored_mode: String = tx.query_row(
                    "SELECT identity_mode FROM st.root_grant WHERE id=?1 AND volume_key=?2 AND root_file_id=?3 AND state='active' AND (identity_mode='verified' OR display_path=?4) AND (volume_guid IS ?5 OR volume_guid IS NULL)",
                    params![grant_id,o.volume_key,o.root_file_id,o.display_path,o.volume_guid],
                    |r| r.get(0),
                ).optional()?.ok_or(Error::NotFound)?;
                if o.filesystem.is_some() && stored_mode != mode {
                    return Err(Error::Invalid(
                        "root identity mode changed; revoke and grant again",
                    ));
                }
                if tx.execute("UPDATE st.root_grant SET filesystem=coalesce(?2,filesystem),volume_guid=coalesce(volume_guid,?3) WHERE id=?1 AND state='active' AND ((?2 IS NOT NULL AND filesystem IS NOT ?2) OR (volume_guid IS NULL AND ?3 IS NOT NULL))", params![grant_id,o.filesystem,o.volume_guid])? > 0 {
                    state_revision(&tx)?;
                }
                *grant_id
            } else {
                tx.execute("INSERT INTO st.root_grant(volume_key,root_file_id,display_path,origin,granted_via,state,granted_at_ns,lab_root_id,identity_mode,filesystem,volume_guid) VALUES (?1,?2,?3,?4,?5,'active',?6,?7,?8,?9,?10)",params![o.volume_key,o.root_file_id,o.display_path,o.origin,o.granted_via,o.observed_at_ns,lab,mode,o.filesystem,o.volume_guid])?;
                let grant = tx.last_insert_rowid();
                state_revision(&tx)?;
                grant
            };
            tx.commit()?;
            Ok(Some(Prepared::Id(grant)))
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
        WriteCommand::ForgetGrant {
            grant_id,
            forgotten_at_ns,
            expected_state_rev,
        } => {
            forget_state(conn, *grant_id, *forgotten_at_ns, *expected_state_rev)?;
            Ok(None)
        }
        WriteCommand::GrantIdentityChanged { grant_id } => {
            let tx = conn.transaction()?;
            if !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM st.root_grant WHERE id=?1)",
                [grant_id],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(Error::NotFound);
            }
            // Only an active grant changes; a revoked one keeps its revocation.
            if tx.execute(
                "UPDATE st.root_grant SET state='identity_changed' WHERE id=?1 AND state='active'",
                [grant_id],
            )? > 0
            {
                state_revision(&tx)?;
            }
            tx.commit()?;
            Ok(None)
        }
        WriteCommand::ObjectReference {
            node,
            observed_at_ns,
        } => reference(conn, *node, None, *observed_at_ns).map(|id| Some(Prepared::Id(id))),
        WriteCommand::ReconcileReference {
            reference_id,
            node,
            observed_at_ns,
        } => reference(conn, *node, Some(*reference_id), *observed_at_ns)
            .map(|id| Some(Prepared::Id(id))),
        WriteCommand::DirListing(l) => {
            let prepared = retire_files(
                conn,
                l.run_id,
                l.dir_id,
                Some((&l.files, &l.dirs)),
                l.state == "complete",
            )?;
            Ok(prepared.then_some(Prepared::Listing))
        }
        WriteCommand::ListingDone {
            run_id,
            dir_id,
            outcome,
            ..
        } => {
            let prepared = retire_files(
                conn,
                *run_id,
                *dir_id,
                None,
                matches!(outcome, ListingOutcome::Complete)
                    && !fenced.contains(&(*run_id, *dir_id)),
            )?;
            Ok(prepared.then_some(Prepared::Listing))
        }
        WriteCommand::EndRun { run_id, state, .. } if state == "completed" => {
            retire_pending(conn, *run_id)?;
            Ok(None)
        }
        _ => Ok(None),
    }
}
/// The state half of a forget (#288), one FULL `state.db` transaction. Refuses anything but a
/// revoked grant; a grant already forgotten is a no-op so the catalogue half can finish after a
/// crash. Deletes every row that names or labels the root's objects, then scrubs the grant row to
/// an anonymised audit record.
fn forget_state(conn: &mut Connection, grant: i64, at: i64, expected: Option<i64>) -> Result<()> {
    // Freed pages are zeroed, not just unlinked, so the removed name does not linger in the files.
    // ponytail: stays on for the writer's life; per-command restore if scan deletes get slow.
    conn.execute_batch("PRAGMA st.secure_delete=ON; PRAGMA main.secure_delete=ON")?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let (state, volume, done): (String, String, bool) = tx
        .query_row(
            "SELECT g.state,g.volume_key,EXISTS(SELECT 1 FROM st.root_forgotten f WHERE f.grant_id=g.id) FROM st.root_grant g WHERE g.id=?1",
            [grant],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or(Error::NotFound)?;
    if done {
        return Ok(());
    }
    if state != "revoked" {
        return Err(Error::Invalid("grant is not revoked"));
    }
    let current: i64 = tx.query_row("SELECT state_rev FROM st.revision", [], |r| r.get(0))?;
    if expected.is_some_and(|e| e != current) {
        return Err(Error::StaleGeneration);
    }
    let root: Option<i64> = tx
        .query_row("SELECT id FROM main.root WHERE grant_id=?1", [grant], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(root) = root {
        // The objects of this root: bound to its rows now, or (after a catalogue rebuild) matched
        // by native identity, unless another root of the same volume still observes the object
        // (a hard link seen under both): its labels and memberships are not only this root's.
        // A v1 reference migrated with creation_ft=0 and no row matches by native id alone, as before.
        let bound = "o.catalog_instance=(SELECT value FROM main.meta WHERE key='instance_id') AND (
                (o.kind='dir' AND o.row_id IN (SELECT id FROM main.dir WHERE root_id=?1))
                OR (o.kind='file' AND o.row_id IN (SELECT f.id FROM main.file f JOIN main.dir d ON d.id=f.dir_id WHERE d.root_id=?1)))";
        let elsewhere = "(SELECT r.id FROM main.root r JOIN main.volume v ON v.id=r.volume_id WHERE v.volume_key=?2 AND r.id!=?1)";
        tx.execute_batch("CREATE TEMP TABLE forget_ref(id INTEGER PRIMARY KEY)")?;
        tx.execute(&format!("INSERT OR IGNORE INTO forget_ref SELECT o.id FROM st.object_ref o WHERE o.volume_key=?2 AND (
              ({bound})
              OR EXISTS(SELECT 1 FROM main.file f JOIN main.dir d ON d.id=f.dir_id WHERE o.kind='file' AND f.file_id=o.file_id AND (f.created_ft IS o.creation_ft OR (o.creation_ft=0 AND o.row_id IS NULL)) AND d.root_id=?1)
              OR EXISTS(SELECT 1 FROM main.dir d WHERE o.kind='dir' AND d.root_id=?1 AND d.file_id=o.file_id AND (d.created_ft IS o.creation_ft OR (o.creation_ft=0 AND o.row_id IS NULL))))
            AND NOT EXISTS(SELECT 1 FROM main.file f JOIN main.dir d ON d.id=f.dir_id WHERE o.kind='file' AND f.file_id=o.file_id AND (f.created_ft IS o.creation_ft OR (o.creation_ft=0 AND o.row_id IS NULL)) AND d.root_id IN {elsewhere})
            AND NOT EXISTS(SELECT 1 FROM main.dir d WHERE o.kind='dir' AND d.file_id=o.file_id AND (d.created_ft IS o.creation_ft OR (o.creation_ft=0 AND o.row_id IS NULL)) AND d.root_id IN {elsewhere})"), params![root, volume])?;
        // A kept object bound to one of this root's rows loses the row: unresolved, as after a rebuild.
        tx.execute(&format!("UPDATE st.object_ref AS o SET state='unresolved',retired=1 WHERE o.state='resolved' AND {bound} AND o.id NOT IN (SELECT id FROM forget_ref)"), params![root])?;
        let has = |table: &str| {
            tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM st.sqlite_schema WHERE type='table' AND name=?1)",
                [table],
                |r| r.get::<_, bool>(0),
            )
        };
        if has("learning_receipt")? {
            tx.execute_batch("DELETE FROM st.learning_receipt WHERE client_event_id IN (SELECT client_event_id FROM st.human_feedback WHERE object_ref_id IN (SELECT id FROM forget_ref))")?;
        }
        if has("learning_node")? {
            tx.execute_batch(
                "DELETE FROM st.learning_node WHERE object_ref_id IN (SELECT id FROM forget_ref)",
            )?;
        }
        let labels = tx.execute(
            "DELETE FROM st.human_feedback WHERE object_ref_id IN (SELECT id FROM forget_ref)",
            [],
        )? + tx.execute(
            "DELETE FROM st.teacher_label WHERE object_ref_id IN (SELECT id FROM forget_ref)",
            [],
        )?;
        if labels > 0 {
            // A fitted candidate learned from those labels; the next refit starts from what is left.
            tx.execute(
                "UPDATE st.student_model SET status='retired' WHERE status='candidate'",
                [],
            )?;
        }
        tx.execute_batch(
            "DELETE FROM st.collection_member WHERE object_ref_id IN (SELECT id FROM forget_ref);
             DELETE FROM st.pending_reference WHERE object_ref_id IN (SELECT id FROM forget_ref);
             DELETE FROM st.object_ref WHERE id IN (SELECT id FROM forget_ref);
             DROP TABLE forget_ref",
        )?;
    }
    tx.execute(
        "UPDATE st.root_grant SET display_path='forgotten',volume_key='forgotten',volume_guid=NULL,root_file_id=zeroblob(8),lab_root_id=NULL WHERE id=?1",
        [grant],
    )?;
    tx.execute(
        "INSERT INTO st.root_forgotten(grant_id,root_number,forgotten_at_ns) VALUES (?1,?2,?3)",
        params![grant, root.unwrap_or(0), at],
    )?;
    // Owes the rewrite of both stores: durable with the forget, so a crash before it cannot lose it.
    tx.execute(
        "INSERT INTO st.meta(key,value) VALUES('scrub_pending','1') ON CONFLICT(key) DO NOTHING",
        [],
    )?;
    state_revision(&tx)?;
    tx.commit()?;
    Ok(())
}
/// Deletes a root's catalogue rows (#288): runs, staged listings, directories and files with their
/// aggregates, then whatever only it kept alive (extension vocabulary, a volume with no other root,
/// a volume name taken from its folder). The link counts of a volume that stays are recomputed.
pub(crate) fn purge_root(conn: &Connection, root: i64) -> Result<()> {
    let (volume, name): (Option<i64>, Option<String>) = conn.query_row(
        "SELECT r.volume_id,(SELECT name FROM dir WHERE root_id=r.id AND parent_id IS NULL) FROM root r WHERE r.id=?1",
        [root],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    conn.execute(
        "DELETE FROM stage_entry WHERE run_id IN (SELECT id FROM scan_run WHERE root_id=?1)",
        [root],
    )?;
    // A cascade through a parent_id chain is a trigger recursion, capped at 1000 levels: delete
    // from the deepest level up so each statement cascades at most one level, then sweep the rest.
    // The same for a bulk build the root left behind (#148): its rows hold names too.
    for (dirs, files) in [("dir", "file"), ("bulk_dir", "bulk_file")] {
        conn.execute(
            &format!(
                "DELETE FROM {files} WHERE dir_id IN (SELECT id FROM {dirs} WHERE root_id=?1)"
            ),
            [root],
        )?;
        let deepest: i64 = conn.query_row(
            &format!("SELECT coalesce(max(depth),0) FROM {dirs} WHERE root_id=?1"),
            [root],
            |r| r.get(0),
        )?;
        let mut level =
            conn.prepare_cached(&format!("DELETE FROM {dirs} WHERE root_id=?1 AND depth=?2"))?;
        for depth in (1..=deepest).rev() {
            level.execute(params![root, depth])?;
        }
    }
    conn.execute("DELETE FROM root WHERE id=?1", [root])?; // cascades to the remaining dir and scan_run rows
    conn.execute(
        "DELETE FROM ext WHERE NOT EXISTS(SELECT 1 FROM file WHERE ext_id=ext.id)",
        [],
    )?;
    if let Some(volume) = volume {
        let other: Option<i64> = conn
            .query_row(
                "SELECT id FROM root WHERE volume_id=?1 LIMIT 1",
                [volume],
                |r| r.get(0),
            )
            .optional()?;
        match other {
            Some(other) => {
                rebuild_multilink(conn, other)?;
                conn.execute(
                    "UPDATE volume SET display_name=volume_key WHERE id=?1 AND display_name=?2",
                    params![volume, name],
                )?;
            }
            None => {
                conn.execute("DELETE FROM multilink WHERE volume_id=?1", [volume])?;
                conn.execute("DELETE FROM volume WHERE id=?1", [volume])?;
            }
        }
    }
    // The triggers removed the root's search entries, but a contentless delete only records a
    // tombstone: the name trigrams stay in the index segments until a merge. Merging everything
    // now rewrites the index without them, and secure_delete zeroes the pages it frees.
    // ponytail: rewrites the whole index (about 1 s per million names here); per-segment if forgets get frequent.
    conn.execute(
        "INSERT INTO search_name(search_name) VALUES('optimize')",
        [],
    )?;
    // The token index holds the same names as plain tokens: drain the deletions, then merge it the same way.
    if crate::search::token_index_enabled(conn)? {
        crate::search::drain_tokens(conn)?;
        conn.execute(
            "INSERT INTO search_token(search_token) VALUES('optimize')",
            [],
        )?;
    }
    Ok(())
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
    let token: String = tx.query_row("SELECT coalesce((SELECT publication_token FROM st.pending_reference LIMIT 1),lower(hex(randomblob(16))))", [], |r| r.get(0))?;
    tx.execute("INSERT OR REPLACE INTO st.pending_reference(object_ref_id,catalog_instance,publication_token) SELECT id,catalog_instance,?1 FROM st.object_ref WHERE id IN (SELECT id FROM retiring_reference)", [token])?;
    tx.commit()?;
    Ok(())
}
fn retire_files(
    conn: &mut Connection,
    run: i64,
    dir: i64,
    incoming: Option<(&[Observation], &[Observation])>,
    complete: bool,
) -> Result<bool> {
    active_run(conn, LIVE, run, Some(dir))?;
    if !conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM st.object_ref WHERE state='resolved')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(false);
    }
    // Unmatched pending intents are discarded after rejection; bindings change only after a committed witness.
    conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS continuity_entry(kind INTEGER,name TEXT,raw BLOB,identity BLOB,created_ft INTEGER,matched_id INTEGER);CREATE INDEX IF NOT EXISTS continuity_by_identity ON continuity_entry(kind,identity,created_ft);CREATE INDEX IF NOT EXISTS continuity_by_name ON continuity_entry(kind,name,raw);DELETE FROM continuity_entry")?;
    old_files(conn, LIVE, dir)?;
    let fs: Option<String> = conn.query_row("SELECT v.filesystem FROM dir d JOIN root r ON r.id=d.root_id JOIN volume v ON v.id=r.volume_id WHERE d.id=?1", [dir], |r| r.get(0))?;
    let mut direct = incoming;
    {
        let mut chunks = conn.prepare_cached(
            "SELECT entries FROM stage_entry WHERE run_id=?1 AND dir_id=?2 ORDER BY seq",
        )?;
        let mut rows = chunks.query(params![run, dir])?;
        let mut insert =
            conn.prepare_cached("INSERT INTO continuity_entry VALUES(?1,?2,?3,?4,?5,?6)")?;
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
                    let identity = native_id(o, fs.as_deref());
                    let matched = if kind == 1 {
                        match_file(
                            conn,
                            &o.name,
                            o.name_utf16.as_deref(),
                            identity,
                            o.created_ft,
                        )?
                        .map(|m| m.0)
                    } else {
                        None
                    };
                    insert.execute(params![
                        kind,
                        o.name,
                        o.name_utf16,
                        identity,
                        o.created_ft,
                        matched
                    ])?;
                }
            }
        }
    }
    if complete {
        // Stage the same hard-link rebind as publication, preserving the old binding until commit.
        let tx = conn.transaction()?;
        let token: String = tx.query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))?;
        tx.execute("INSERT INTO st.pending_reference(object_ref_id,catalog_instance,publication_token,row_id,born_run) SELECT o.id,o.catalog_instance,?1,
            (SELECT s.id FROM old_file s JOIN file f ON f.id=o.row_id WHERE s.used=1 AND s.identity=f.file_id AND s.created_ft IS f.created_ft ORDER BY s.id LIMIT 1),
            (SELECT f.born_run FROM file f JOIN old_file s ON s.id=f.id WHERE s.used=1 AND s.identity=o.file_id AND s.created_ft IS o.creation_ft ORDER BY s.id LIMIT 1)
            FROM st.object_ref o WHERE o.kind='file' AND o.state='resolved' AND o.catalog_instance=(SELECT value FROM meta WHERE key='instance_id') AND EXISTS(SELECT 1 FROM file f JOIN old_file x ON x.id=f.id WHERE f.id=o.row_id AND f.born_run=o.born_run AND x.used=0 AND EXISTS(SELECT 1 FROM old_file s WHERE s.used=1 AND s.identity=f.file_id AND s.created_ft IS f.created_ft))", [&token])?;
        crate::db::resolve_pending_collisions(&tx, &token)?;
        tx.commit()?;
    }
    retire_selected(conn,"WITH RECURSIVE replaced(id) AS (SELECT d.id FROM dir d JOIN continuity_entry e ON e.kind=0 AND e.identity IS d.file_id AND e.created_ft IS NOT d.created_ft AND (d.file_id IS NOT NULL OR (d.parent_id=?1 AND e.name=d.name AND e.raw IS d.name_utf16)) WHERE d.root_id=(SELECT root_id FROM dir WHERE id=?1) UNION ALL SELECT d.id FROM dir d JOIN replaced x ON d.parent_id=x.id) SELECT o.id FROM st.object_ref o JOIN file f ON f.id=o.row_id WHERE o.kind='file' AND o.state='resolved' AND f.dir_id=?1 AND o.catalog_instance=(SELECT value FROM meta WHERE key='instance_id') AND o.born_run=f.born_run AND NOT EXISTS(SELECT 1 FROM continuity_entry e WHERE e.kind=1 AND e.identity=f.file_id AND e.created_ft IS f.created_ft) AND (?2 OR EXISTS(SELECT 1 FROM continuity_entry e WHERE e.kind=1 AND ((e.name=f.name AND e.raw IS f.name_utf16) OR e.identity=f.file_id))) UNION SELECT o.id FROM st.object_ref o WHERE o.state='resolved' AND o.catalog_instance=(SELECT value FROM meta WHERE key='instance_id') AND ((o.kind='dir' AND o.row_id IN (SELECT id FROM replaced)) OR (o.kind='file' AND o.row_id IN (SELECT id FROM file WHERE dir_id IN (SELECT id FROM replaced))))",params![dir,complete])?;
    Ok(true)
}
fn full_traversal(conn: &Connection, run: i64, root: i64) -> Result<bool> {
    Ok(conn.query_row("WITH RECURSIVE visible(id) AS (SELECT id FROM dir WHERE root_id=?2 AND parent_id IS NULL UNION ALL SELECT d.id FROM dir d JOIN visible v ON d.parent_id=v.id WHERE d.listing_state!='absent_pending') SELECT mode!='targeted' AND NOT EXISTS(SELECT 1 FROM dir WHERE id IN (SELECT id FROM visible) AND (seen_run!=?1 OR listing_state NOT IN ('complete','excluded'))) FROM scan_run WHERE id=?1",params![run,root],|r|r.get(0))?)
}
/// Rows inside an excluded directory, transitively. Its contents were never observed after it
/// became excluded, so their absence is unknown: neither `retire_pending` nor the EndRun delete
/// may treat them as gone (invariant 4). Both statements use this same set.
const SHIELDED: &str = "shielded(id) AS (SELECT id FROM dir WHERE root_id=?1 AND listing_state='excluded' UNION SELECT d.id FROM dir d JOIN shielded s ON d.parent_id=s.id)";
fn retire_pending(conn: &mut Connection, run: i64) -> Result<()> {
    let root = active_run(conn, LIVE, run, None)?;
    if !full_traversal(conn, run, root)? {
        return Ok(());
    }
    retire_selected(conn,&format!("WITH RECURSIVE {SHIELDED}, removed(id) AS (SELECT id FROM dir WHERE root_id=?1 AND listing_state='absent_pending' AND id NOT IN (SELECT id FROM shielded) UNION ALL SELECT d.id FROM dir d JOIN removed x ON d.parent_id=x.id) SELECT o.id FROM st.object_ref o WHERE o.state='resolved' AND o.catalog_instance=(SELECT value FROM meta WHERE key='instance_id') AND ((o.kind='dir' AND o.row_id IN (SELECT id FROM removed)) OR (o.kind='file' AND o.row_id IN (SELECT id FROM file WHERE dir_id IN (SELECT id FROM removed))))"),[root])
}
fn apply(
    conn: &Connection,
    c: &WriteCommand,
    ctx: &mut Context,
    prepared: Option<Prepared>,
) -> Result<WriteReply> {
    match c {
        WriteCommand::RegisterRoot(o) | WriteCommand::RefreshRoot { observation: o, .. } => {
            let grant = match prepared {
                Some(Prepared::Id(id)) => id,
                _ => return Err(Error::Invalid("grant not committed")),
            };
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
            // P4f's first scan, with nothing published under the root directory yet.
            if ctx.bulk_enabled.load(Ordering::Relaxed)
                && mode == "full"
                && conn.query_row("SELECT NOT EXISTS(SELECT 1 FROM scan_run WHERE root_id=?1 AND id<?2) AND NOT EXISTS(SELECT 1 FROM dir WHERE root_id=?1 AND parent_id IS NOT NULL) AND NOT EXISTS(SELECT 1 FROM file f JOIN dir d ON d.id=f.dir_id WHERE d.root_id=?1)", params![root, run], |r| r.get::<_, bool>(0))?
            {
                conn.execute("DELETE FROM bulk_dir WHERE root_id=?1", [root])?;
                conn.execute(
                    "INSERT INTO bulk_dir SELECT * FROM dir WHERE root_id=?1",
                    [root],
                )?;
                share_sequences(conn)?;
                ctx.bulk.insert(run, root);
            }
            Ok(WriteReply::Run(run))
        }
        WriteCommand::DirListing(l) => publish(
            conn,
            l.run_id,
            l.dir_id,
            (
                if l.state == "complete" {
                    ListingOutcome::Complete
                } else {
                    ListingOutcome::Incomplete(l.state.clone())
                },
                (l.skipped, l.errors),
            ),
            ctx,
            Some((&l.files, &l.dirs)),
            matches!(prepared, Some(Prepared::Listing)),
        ),
        WriteCommand::StageChunk {
            run_id,
            dir_id,
            seq,
            files,
            dirs,
        } => {
            if ctx.fenced_listings.contains(&(*run_id, *dir_id)) {
                return Err(Error::Invalid("failed staged listing"));
            }
            let started = Instant::now();
            stage(
                conn,
                ctx.tables(*run_id),
                *run_id,
                *dir_id,
                *seq,
                files,
                dirs,
            )?;
            ctx.timings.lock().expect("timing lock").staging_calls += 1;
            ctx.timings.lock().expect("timing lock").staging_seconds +=
                started.elapsed().as_secs_f64();
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
            (
                if matches!(outcome, ListingOutcome::Complete)
                    && ctx.fenced_listings.contains(&(*run_id, *dir_id))
                {
                    ListingOutcome::Incomplete("partial".into())
                } else {
                    outcome.clone()
                },
                (*skipped, *errors),
            ),
            ctx,
            None,
            matches!(prepared, Some(Prepared::Listing)),
        ),
        WriteCommand::DirFinal {
            run_id,
            dir_id,
            totals: t,
            input_revision,
        } => {
            let tables = ctx.tables(*run_id);
            active_run(conn, tables, *run_id, Some(*dir_id))?;
            let count=conn.prepare_cached(&format!("UPDATE {} SET sub_files=?2,sub_dirs=?3,sub_logical=?4,sub_allocated=?5,sub_alloc_unknown=?6,sub_skipped=?7,sub_errors=?8,sub_newest_ft=?9,sub_complete=?10,agg_valid_rev=?11 WHERE id=?1 AND dirty_run=?12 AND dirty_rev=?11",tables.dir))?.execute(params![dir_id,t.files as i64,t.dirs as i64,t.logical as i64,t.allocated as i64,t.allocation_unknown as i64,t.skipped as i64,t.errors as i64,t.newest_ft,t.complete,input_revision,run_id])?;
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
            let root = active_run(conn, LIVE, *run_id, None)?;
            if !matches!(state.as_str(), "completed" | "cancelled" | "failed") {
                return Err(Error::Invalid("end state"));
            }
            if ctx.bulk.contains_key(run_id) {
                // A failed run is an abort: its build is discarded and the root shows no rows.
                if state == "failed" {
                    conn.execute("DELETE FROM bulk_dir WHERE root_id=?1", [root])?;
                } else {
                    swap_bulk(conn, root)?;
                }
                ctx.parents.clear();
            }
            let full = state == "completed" && full_traversal(conn, *run_id, root)?;
            if full {
                conn.execute(
                    &format!("WITH RECURSIVE {SHIELDED} DELETE FROM dir WHERE root_id=?1 AND listing_state='absent_pending' AND id NOT IN (SELECT id FROM shielded)"),
                    [root],
                )?;
                ctx.parents.clear();
            }
            conn.execute("DELETE FROM stage_entry WHERE run_id=?1", [run_id])?;
            // A failed run is an abort that must always release the root: it skips the rollup
            // (which may be what failed) and leaves aggregates dirty for a reader's repair.
            if state != "failed" {
                let started = Instant::now();
                rebuild_multilink(conn, root)?;
                ctx.timings.lock().expect("timing lock").multilink_seconds +=
                    started.elapsed().as_secs_f64();
                let started = Instant::now();
                rollup(conn, root, full)?;
                ctx.timings.lock().expect("timing lock").rollup_calls += 1;
                ctx.timings.lock().expect("timing lock").rollup_seconds +=
                    started.elapsed().as_secs_f64();
            }
            conn.execute(
                "UPDATE scan_run SET state=?2,finished_at_ns=?3 WHERE id=?1",
                params![run_id, state, finished_at_ns],
            )?;
            if state == "completed" {
                // LW-121: the root's size at this run's end, so growth can be read back from scan
                // history. `complete` is the root aggregate's own coverage bit (set only by a full,
                // clean traversal); a run without these counters (older rows) is an unknown sample.
                conn.execute(
                    "UPDATE scan_run SET counters_json=coalesce((SELECT json_object('logical_bytes',sub_logical,'files',sub_files,'skipped',sub_skipped,'errors',sub_errors,'complete',json(CASE WHEN sub_complete=1 THEN 'true' ELSE 'false' END)) FROM dir WHERE root_id=?2 AND parent_id IS NULL),'{}') WHERE id=?1",
                    params![run_id, root],
                )?;
            }
            let completed=state=="completed" && conn.query_row("SELECT mode='targeted' AND NOT EXISTS(SELECT 1 FROM dir WHERE root_id=?2 AND seen_run=?1 AND listing_state='incomplete') FROM scan_run WHERE id=?1",params![run_id,root],|r|r.get::<_,bool>(0))?;
            conn.execute("UPDATE root SET active_run=NULL,generation=CASE WHEN ?2 THEN ?3 ELSE generation END,state=CASE WHEN ?4 THEN 'complete' ELSE 'partial' END WHERE id=?1",params![root,full||completed,run_id,full])?;
            ctx.fenced_listings.retain(|(run, _)| run != run_id);
            ctx.bulk.remove(run_id);
            Ok(WriteReply::Done)
        }
        WriteCommand::RevokeGrant { grant_id, .. }
        | WriteCommand::GrantIdentityChanged { grant_id } => {
            conn.execute("UPDATE scan_run SET state='cancelled' WHERE root_id IN (SELECT id FROM root WHERE grant_id=?1) AND state='running'",[grant_id])?;
            conn.execute(
                "UPDATE root SET active_run=NULL,state='stale' WHERE grant_id=?1",
                [grant_id],
            )?;
            // Link counts cover visible rows only, and this grant's rows just stopped being visible.
            let roots: Vec<i64> = conn
                .prepare("SELECT id FROM root WHERE grant_id=?1")?
                .query_map([grant_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for root in roots {
                // Revocation mid-build discards the build: its rows were never visible.
                conn.execute("DELETE FROM bulk_dir WHERE root_id=?1", [root])?;
                ctx.bulk.retain(|_, building| *building != root);
                rebuild_multilink(conn, root)?;
            }
            Ok(WriteReply::Done)
        }
        WriteCommand::ForgetGrant { grant_id, .. } => {
            let root: Option<i64> = conn
                .query_row("SELECT id FROM root WHERE grant_id=?1", [grant_id], |r| {
                    r.get(0)
                })
                .optional()?;
            if let Some(root) = root {
                purge_root(conn, root)?;
            }
            // Extension IDs are cached across batches; a deleted vocabulary row must not be reused.
            ctx.extensions.clear();
            Ok(WriteReply::Done)
        }
        WriteCommand::Repair { root_id } => {
            match rollup(conn, *root_id, false) {
                Ok(()) => {
                    conn.execute(
                        "UPDATE root SET state='partial' WHERE id=?1 AND state='repairing'",
                        [root_id],
                    )?;
                }
                // Still overdeep (rollup fails before writing): re-quarantine it as restart repair does,
                // so reads of other roots do not retry and fail on it.
                Err(Error::Invalid("tree depth limit")) => {
                    conn.execute("UPDATE root SET state='repairing' WHERE id=?1", [root_id])?;
                }
                Err(e) => return Err(e),
            }
            Ok(WriteReply::Done)
        }
        WriteCommand::ObjectReference { .. } | WriteCommand::ReconcileReference { .. } => {
            Ok(WriteReply::Reference(match prepared {
                Some(Prepared::Id(id)) => id,
                _ => return Err(Error::Invalid("reference not committed")),
            }))
        }
        WriteCommand::Barrier => Ok(WriteReply::Done),
    }
}
fn stage(
    conn: &Connection,
    t: Tables,
    run: i64,
    dir: i64,
    seq: i64,
    files: &Vec<Observation>,
    dirs: &Vec<Observation>,
) -> Result<()> {
    active_run(conn, t, run, Some(dir))?;
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
type DirectoryMatch = (
    i64,
    Option<i64>,
    String,
    Option<Vec<u8>>,
    String,
    Option<i64>,
);
fn old_files(conn: &Connection, t: Tables, dir: i64) -> Result<bool> {
    conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS old_file(id INTEGER PRIMARY KEY,name TEXT,raw BLOB,identity BLOB,created_ft INTEGER,used INTEGER);CREATE INDEX IF NOT EXISTS old_by_name ON old_file(name,raw);CREATE INDEX IF NOT EXISTS old_by_identity ON old_file(identity,used);DELETE FROM old_file")?;
    conn.prepare_cached(&format!(
        "INSERT INTO old_file SELECT id,name,name_utf16,file_id,created_ft,0 FROM {} WHERE dir_id=?1",
        t.file
    ))?
    .execute([dir])?;
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM old_file)", [], |r| r.get(0))?)
}
fn match_file(
    conn: &Connection,
    name: &str,
    raw: Option<&[u8]>,
    identity: Option<&[u8]>,
    created: Option<i64>,
) -> Result<Option<FileMatch>> {
    conn.prepare_cached("UPDATE old_file SET used=-1 WHERE used=0 AND identity IS ?1 AND created_ft IS NOT ?2 AND (identity IS NOT NULL OR (name=?3 AND raw IS ?4))")?.execute(params![identity,created,name,raw])?;
    Ok(conn.prepare_cached("UPDATE old_file SET used=1 WHERE id=coalesce((SELECT id FROM old_file WHERE name=?1 AND raw IS ?2 AND identity IS ?3 AND created_ft IS ?4 AND used=0 ORDER BY id LIMIT 1),(SELECT id FROM old_file WHERE identity=?3 AND created_ft IS ?4 AND used=0 ORDER BY id LIMIT 1)) RETURNING id,name,raw")?
        .query_row(params![name,raw,identity,created], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?)
}
type FileMatch = (i64, String, Option<Vec<u8>>);
fn publish(
    conn: &Connection,
    run: i64,
    dir: i64,
    status: (ListingOutcome, (u64, u64)),
    ctx: &mut Context,
    mut direct: Option<(&[Observation], &[Observation])>,
    prepared: bool,
) -> Result<WriteReply> {
    let (outcome, (skipped, errors)) = status;
    let listing_start = Instant::now();
    let mut file_seconds = 0.0;
    let mut seen_seconds = 0.0;
    let mut names_seconds = 0.0;
    let t = ctx.tables(run);
    let d = t.dir;
    let root = active_run(conn, t, run, Some(dir))?;
    if t == BULK {
        share_sequences(conn)?;
    }
    let (depth,fs):(i64,Option<String>)=conn.query_row(&format!("SELECT d.depth,v.filesystem FROM {d} d JOIN root r ON r.id=d.root_id LEFT JOIN volume v ON v.id=r.volume_id WHERE d.id=?1"),[dir],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let complete = matches!(outcome, ListingOutcome::Complete);
    let coverage = outcome.coverage()?;
    // Direct payloads are prevalidated, but staged entries still need the cross-entry guard.
    let is_direct = direct.is_some()
        && !conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM stage_entry WHERE run_id=?1 AND dir_id=?2)",
            params![run, dir],
            |r| r.get::<_, bool>(0),
        )?;
    let has_old = if prepared {
        conn.query_row("SELECT EXISTS(SELECT 1 FROM old_file)", [], |r| r.get(0))?
    } else {
        old_files(conn, t, dir)?
    };
    conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS listing_seen(kind INTEGER,id INTEGER,PRIMARY KEY(kind,id));DELETE FROM listing_seen;CREATE TEMP TABLE IF NOT EXISTS listing_names(name TEXT,raw BLOB,kind INTEGER);DELETE FROM listing_names")?;
    let mut own = Totals {
        complete,
        ..Totals::default()
    };
    let mut noid = 0i64;
    let mut members_changed = false;
    let mut revealed = Vec::new();
    let mut chunks = conn.prepare_cached(
        "SELECT entries FROM stage_entry WHERE run_id=?1 AND dir_id=?2 ORDER BY seq",
    )?;
    let mut rows = chunks.query(params![run, dir])?;
    let mut continuity = if prepared {
        Some(conn.prepare_cached("SELECT f.id,f.name,f.raw FROM continuity_entry c LEFT JOIN old_file f ON f.id=c.matched_id WHERE c.kind=1 ORDER BY c.rowid")?)
    } else {
        None
    };
    let mut matches = continuity.as_mut().map(|s| s.query([])).transpose()?;
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
        let mut values = Vec::with_capacity(128 * 17);
        let mut tuples = Vec::with_capacity(128);
        for o in files {
            let identity = native_id(o, fs.as_deref());
            // An excluded entry is listed, never counted (the engine's sums agree).
            if o.flags & EXCLUDED_FLAGS == 0 {
                own.files += 1;
                own.logical = checked(own.logical, o.logical)?;
                own.allocated = checked(own.allocated, o.allocated.unwrap_or(0))?;
                own.allocation_unknown += u64::from(o.allocated.is_none());
                own.newest_ft = own.newest_ft.max(o.modified_ft);
                noid += i64::from(identity.is_none());
            }
            // Prepared continuity rows line up one-to-one with every listed file, excluded or not.
            let matched = if let Some(matches) = &mut matches {
                let row = matches.next()?.ok_or(Error::CorruptDatabase)?;
                row.get::<_, Option<i64>>(0)?
                    .map(|id| -> rusqlite::Result<FileMatch> { Ok((id, row.get(1)?, row.get(2)?)) })
                    .transpose()?
            } else if has_old {
                match_file(
                    conn,
                    &o.name,
                    o.name_utf16.as_deref(),
                    identity,
                    o.created_ft,
                )?
            } else {
                None
            };
            let id = if let Some((id, name, raw)) = matched {
                members_changed |= name != o.name || raw != o.name_utf16;
                Some(id)
            } else {
                None
            };
            members_changed |= id.is_none();
            let ext = if let Some(ext) = &o.extension {
                if let Some((id, family)) = ctx.extensions.get(ext) {
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
                    if ctx.extensions.len() < 1024 {
                        ctx.extensions.insert(ext.clone(), (id, family));
                    }
                    Some(id)
                }
            } else {
                None
            };
            values.extend([
                id.map_or(Value::Null, Value::Integer),
                Value::Integer(dir),
                Value::Text(o.name.as_bytes()),
                o.name_utf16.as_deref().map_or(Value::Null, Value::Blob),
                ext.map_or(Value::Null, Value::Integer),
                identity.map_or(Value::Null, Value::Blob),
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
                let (insert, seen) = insert_files(conn, t, &mut values, &mut tuples, has_old)?;
                file_seconds += insert;
                seen_seconds += seen;
            }
        }
        let (insert, seen) = insert_files(conn, t, &mut values, &mut tuples, has_old)?;
        file_seconds += insert;
        seen_seconds += seen;
        for o in dirs {
            let identity = native_id(o, fs.as_deref());
            let excluded = o.flags & EXCLUDED_FLAGS != 0;
            let mut old_dir: Option<DirectoryMatch> = if let Some(id) = identity {
                conn.prepare_cached(&format!("SELECT id,parent_id,name,name_utf16,listing_state,created_ft FROM {d} WHERE root_id=?1 AND file_id=?2"))?.query_row(params![root,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?
            } else {
                conn.prepare_cached(&format!("SELECT id,parent_id,name,name_utf16,listing_state,created_ft FROM {d} WHERE parent_id=?1 AND name=?2 AND name_utf16 IS ?3 AND file_id IS NULL AND listing_state!='absent_pending'"))?.query_row(params![dir,o.name,o.name_utf16],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?
            };
            if let Some((id, parent, _, _, state, created)) = &old_dir {
                if *created == o.created_ft && excluded && state != "excluded" {
                    // Same directory, now excluded: hide its descendants but shield their
                    // identities and references until rediscovery or an observed absent ancestor.
                    conn.execute(&format!("UPDATE {d} SET listing_state='absent_pending' WHERE parent_id=?1 AND listing_state!='absent_pending'"), [id])?;
                    ctx.dirty(*id, run);
                    members_changed = true;
                }
                if *created != o.created_ft {
                    let parent = parent.ok_or(Error::Invalid("cannot replace root"))?;
                    ctx.dirty(parent, run);
                    conn.execute(&format!("WITH RECURSIVE replaced(id) AS (SELECT ?1 UNION ALL SELECT d.id FROM {d} d JOIN replaced x ON d.parent_id=x.id) UPDATE {d} SET listing_state='absent_pending',file_id=NULL,id_quality='path_observation',id_basis='none' WHERE id IN (SELECT id FROM replaced)"), [id])?;
                    conn.execute(
                        &format!("UPDATE {d} SET listing_rev=listing_rev+1 WHERE id=?1"),
                        [parent],
                    )?;
                    old_dir = None;
                }
            }
            let id = if let Some((id, parent, name, utf16, state, _)) = old_dir {
                // Restoring a hidden branch can expose descendants at depths changed by an ancestor move.
                if parent != Some(dir) || state == "absent_pending" {
                    revealed.push(id);
                }
                if parent != Some(dir) {
                    let parent = parent.ok_or(Error::Invalid("cannot reparent root"))?;
                    let cycle:bool=conn.query_row(&format!("WITH RECURSIVE a(id) AS (SELECT ?1 UNION ALL SELECT d.parent_id FROM {d} d JOIN a ON d.id=a.id WHERE d.parent_id IS NOT NULL) SELECT EXISTS(SELECT 1 FROM a WHERE id=?2)"),params![dir,id],|r|r.get(0))?;
                    if cycle {
                        return Err(Error::Invalid("directory identity cycle"));
                    }
                    ctx.dirty(parent, run);
                    ctx.parents.insert(id, Some(dir));
                    ctx.dirty(id, run);
                    members_changed = true;
                    conn.execute(
                        &format!("UPDATE {d} SET listing_rev=listing_rev+1 WHERE id=?1"),
                        [parent],
                    )?;
                }
                members_changed |=
                    name != o.name || utf16 != o.name_utf16 || state == "absent_pending";
                conn.prepare_cached(&format!("UPDATE {d} SET parent_id=?2,name=?3,name_utf16=?4,attrs=?5,reparse_tag=?6,flags=?7,created_ft=?8,modified_ft=?9,changed_ft=?10,listing_outcome=CASE WHEN ?11 OR listing_state IN ('absent_pending','excluded') THEN NULL ELSE listing_outcome END,listing_state=CASE WHEN ?11 THEN 'excluded' WHEN listing_state IN ('absent_pending','excluded') THEN 'unlisted' ELSE listing_state END,seen_run=CASE WHEN ?11 THEN ?12 ELSE seen_run END WHERE id=?1"))?.execute(params![id,dir,o.name,o.name_utf16,o.attrs,o.reparse_tag,o.flags,o.created_ft,o.modified_ft,o.changed_ft,excluded,run])?;
                id
            } else {
                let quality = identity.map_or("path_observation", |id| {
                    if id.len() == 16 {
                        "native_file_id_128"
                    } else {
                        "native_file_id_64"
                    }
                });
                conn.prepare_cached(&format!("INSERT INTO {d}(root_id,parent_id,name,name_utf16,file_id,id_quality,id_basis,depth,attrs,reparse_tag,flags,created_ft,modified_ft,changed_ft,listing_state,born_run,seen_run) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?16,?15,?17)"))?.execute(params![root,dir,o.name,o.name_utf16,identity,quality,if identity.is_some(){o.id_basis.as_str()}else{"none"},depth+1,o.attrs,o.reparse_tag,o.flags,o.created_ft,o.modified_ft,o.changed_ft,run,if excluded {"excluded"} else {"unlisted"},if excluded {run} else {0}])?;
                let id = conn.last_insert_rowid();
                ctx.parents.insert(id, Some(dir));
                ctx.dirty(id, run);
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
    members_changed |= conn.execute(
        &format!(
            "DELETE FROM {} WHERE id IN (SELECT id FROM old_file WHERE used=-1)",
            t.file
        ),
        [],
    )? > 0;
    if complete {
        members_changed |= conn.execute(
            &format!(
                "DELETE FROM {} WHERE id IN (SELECT id FROM old_file WHERE used=0)",
                t.file
            ),
            [],
        )? > 0;
        let missing=conn.execute(&format!("UPDATE {d} SET listing_state='absent_pending' WHERE parent_id=?1 AND listing_state!='absent_pending' AND id NOT IN (SELECT id FROM listing_seen WHERE kind=0)"),[dir])?;
        members_changed |= missing > 0;
    } else {
        let sums:(i64,i64,i64,i64,i64,Option<i64>)=conn.query_row(&format!("SELECT count(*),coalesce(sum(logical),0),coalesce(sum(allocated),0),coalesce(sum(allocated IS NULL),0),coalesce(sum(file_id IS NULL),0),max(modified_ft) FROM {} WHERE dir_id=?1 AND flags & ?2 = 0", t.file),params![dir,EXCLUDED_FLAGS],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
        own.files = sums.0 as u64;
        own.logical = sums.1 as u64;
        own.allocated = sums.2 as u64;
        own.allocation_unknown = sums.3 as u64;
        noid = sums.4;
        own.newest_ft = sums.5;
    }
    conn.execute(&format!("UPDATE {d} SET listing_state=?2,seen_run=?3,listing_rev=listing_rev+?4,own_files=?5,own_logical=?6,own_allocated=?7,own_alloc_unknown=?8,own_noid=?9,own_newest_ft=?10,own_skipped=?11,own_errors=?12,listing_outcome=?13 WHERE id=?1"),params![dir,if complete{"complete"}else{"incomplete"},run,i64::from(members_changed),own.files as i64,own.logical as i64,own.allocated as i64,own.allocation_unknown as i64,noid,own.newest_ft,skipped as i64,errors as i64,coverage])?;
    ctx.dirty(dir, run);
    if !revealed.is_empty() {
        let destination = ctx.chain_length(conn, t, dir, &mut HashMap::new())?;
        // Later moves in this listing can flatten an earlier move's subtree.
        for id in revealed {
            let too_deep: bool = conn.prepare_cached(
                &format!("WITH RECURSIVE subtree(id,height) AS (SELECT ?1,0 UNION ALL SELECT d.id,s.height+1 FROM {d} d JOIN subtree s ON d.parent_id=s.id WHERE s.height<?2 AND d.listing_state!='absent_pending') SELECT EXISTS(SELECT 1 FROM subtree WHERE height=?2)"),
            )?.query_row(params![id, MAX_DIRECTORY_CHAIN_LENGTH as i64 - destination as i64], |r| r.get(0))?;
            if too_deep {
                return Err(Error::Invalid("tree depth limit"));
            }
        }
    }
    conn.execute(
        "DELETE FROM stage_entry WHERE run_id=?1 AND dir_id=?2",
        params![run, dir],
    )?;
    if t == BULK {
        share_sequences(conn)?;
    }
    {
        let mut timing = ctx.timings.lock().expect("timing lock");
        timing.listing_calls += 1;
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
    t: Tables,
    values: &mut Vec<Value<'_>>,
    tuples: &mut Vec<&str>,
    has_old: bool,
) -> Result<(f64, f64)> {
    if tuples.is_empty() {
        return Ok((0.0, 0.0));
    }
    let insert_start = Instant::now();
    let conflict = if has_old {
        " ON CONFLICT(id) DO UPDATE SET name=excluded.name,name_utf16=excluded.name_utf16,ext_id=excluded.ext_id,logical=excluded.logical,allocated=excluded.allocated,created_ft=excluded.created_ft,modified_ft=excluded.modified_ft,changed_ft=excluded.changed_ft,accessed_ft=excluded.accessed_ft,attrs=excluded.attrs,reparse_tag=excluded.reparse_tag,flags=excluded.flags,seen_run=excluded.seen_run"
    } else {
        ""
    };
    let sql=format!("INSERT INTO {}(id,dir_id,name,name_utf16,ext_id,file_id,logical,allocated,created_ft,modified_ft,changed_ft,accessed_ft,attrs,reparse_tag,flags,born_run,seen_run) VALUES {}{conflict}",t.file,tuples.join(","));
    conn.prepare_cached(&sql)?
        .execute(rusqlite::params_from_iter(
            values.iter().map(|v| ToSqlOutput::Borrowed(*v)),
        ))?;
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
/// Live and bulk tables draw IDs from one sequence: neither side reuses the other's IDs, and a
/// lone first scan gets exactly the IDs the incremental path would give it.
fn share_sequences(conn: &Connection) -> Result<()> {
    conn.prepare_cached("INSERT INTO sqlite_sequence(name,seq) SELECT n.name,0 FROM (SELECT 'dir' AS name UNION ALL SELECT 'file' UNION ALL SELECT 'bulk_dir' UNION ALL SELECT 'bulk_file') n WHERE NOT EXISTS(SELECT 1 FROM sqlite_sequence s WHERE s.name=n.name)")?.execute([])?;
    for pair in ["('dir','bulk_dir')", "('file','bulk_file')"] {
        conn.prepare_cached(&format!("UPDATE sqlite_sequence SET seq=(SELECT max(seq) FROM sqlite_sequence WHERE name IN {pair}) WHERE name IN {pair}"))?.execute([])?;
    }
    Ok(())
}
/// Publish a bulk build inside the EndRun savepoint: readers see the root before or after, never
/// in between. The file indexes are built once when the build is at least as large as everything
/// already live; otherwise its rows join the indexed table in one transaction.
fn swap_bulk(conn: &Connection, root: i64) -> Result<()> {
    let root_dir: i64 = conn.query_row(
        "SELECT id FROM bulk_dir WHERE root_id=?1 AND parent_id IS NULL",
        [root],
        |r| r.get(0),
    )?;
    if conn.query_row("SELECT EXISTS(SELECT 1 FROM dir WHERE root_id=?1 AND id!=?2) OR EXISTS(SELECT 1 FROM file WHERE dir_id=?2)", params![root, root_dir], |r| r.get::<_, bool>(0))? {
        return Err(Error::Invalid("bulk root gained live rows"));
    }
    conn.execute("DELETE FROM dir WHERE id=?1", [root_dir])?;
    // The build's rows are new token keys (IDs are never reused): index them in one pass below
    // instead of queueing each through the insert triggers and reading it back (#520).
    let token_triggers = crate::search::suspend_insert_triggers(conn)?;
    conn.execute(
        "INSERT INTO dir SELECT * FROM bulk_dir WHERE root_id=?1 ORDER BY id",
        [root],
    )?;
    // The usual case is the only build in progress: copy and empty the table without a filter.
    let alone = !conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM bulk_dir WHERE root_id!=?1)",
        [root],
        |r| r.get::<_, bool>(0),
    )?;
    let source = if alone {
        "bulk_file WHERE ?1 IS NOT NULL"
    } else {
        "bulk_file WHERE dir_id IN (SELECT id FROM bulk_dir WHERE root_id=?1)"
    };
    let rows: i64 = conn.query_row(&format!("SELECT count(*) FROM {source}"), [root], |r| {
        r.get(0)
    })?;
    let deferred = if conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM file LIMIT 1 OFFSET ?1)",
        [rows],
        |r| r.get::<_, bool>(0),
    )? {
        Vec::new()
    } else {
        let mut s = conn.prepare("SELECT name,sql FROM sqlite_schema WHERE type='index' AND tbl_name='file' AND sql IS NOT NULL")?;
        let indexes = s
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (name, _) in &indexes {
            conn.execute_batch(&format!("DROP INDEX \"{name}\""))?;
        }
        indexes
    };
    conn.execute(
        &format!("INSERT INTO file SELECT * FROM {source} ORDER BY id"),
        [root],
    )?;
    for (_, sql) in deferred {
        conn.execute_batch(&sql)?;
    }
    if !token_triggers.is_empty() {
        crate::search::index_new_tokens(
            conn,
            "SELECT id,name FROM bulk_dir WHERE root_id=?1 AND parent_id IS NOT NULL ORDER BY id",
            root,
        )?;
        crate::search::index_new_tokens(
            conn,
            &format!(
                "SELECT id+{},name FROM {source} ORDER BY id",
                crate::search::FILE_KEY
            ),
            root,
        )?;
        crate::search::restore_insert_triggers(conn, &token_triggers)?;
    }
    if alone {
        crate::db::bulk_tables(conn, true)?;
    }
    // Cascades to the build's files.
    conn.execute("DELETE FROM bulk_dir WHERE root_id=?1", [root])?;
    Ok(())
}
fn rebuild_multilink(conn: &Connection, root: i64) -> Result<()> {
    let volume: i64 = conn.query_row("SELECT volume_id FROM root WHERE id=?1", [root], |r| {
        r.get(0)
    })?;
    conn.execute("DELETE FROM multilink WHERE volume_id=?1", [volume])?;
    // The identity index rejects singletons before the volume/visibility grouping needs scratch.
    conn.execute("INSERT INTO multilink SELECT ?1,f.file_id,count(*),max(f.allocated) FROM file f JOIN dir d ON d.id=f.dir_id JOIN visible_dir v ON v.id=d.id JOIN root r ON r.id=d.root_id WHERE r.volume_id=?1 AND f.file_id IN (SELECT file_id FROM file WHERE file_id IS NOT NULL GROUP BY file_id HAVING count(*)>1) AND d.listing_state!='excluded' GROUP BY f.file_id HAVING count(*)>1",[volume])?;
    conn.execute("UPDATE file SET flags=flags & ~?2 WHERE dir_id IN (SELECT d.id FROM dir d JOIN root r ON r.id=d.root_id WHERE r.volume_id=?1) AND flags & ?2 != 0",params![volume,HARDLINK_SUSPECTED])?;
    // Only visible rows carry the flag; hidden copies were cleared above and are not counted.
    conn.execute("UPDATE file SET flags=flags | ?2 WHERE dir_id IN (SELECT d.id FROM dir d JOIN visible_dir v ON v.id=d.id JOIN root r ON r.id=d.root_id WHERE r.volume_id=?1 AND d.listing_state!='excluded') AND file_id IN (SELECT file_id FROM multilink WHERE volume_id=?1)",params![volume,HARDLINK_SUSPECTED])?;
    Ok(())
}
/// File-backed postorder, with at most one batch of directory IDs on the heap.
pub(crate) fn rollup(conn: &Connection, root: i64, clean: bool) -> Result<()> {
    conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS rollup_work(id INTEGER PRIMARY KEY,depth INTEGER NOT NULL) STRICT;
        CREATE INDEX IF NOT EXISTS rollup_postorder ON rollup_work(depth DESC,id DESC); DELETE FROM rollup_work")?;
    // Child lookups must use parent_id; the root_id index scans the whole root per directory.
    conn.execute("WITH RECURSIVE walk(id,depth) AS (
        SELECT id,0 FROM dir WHERE root_id=?1 AND parent_id IS NULL AND listing_state!='absent_pending'
        UNION ALL SELECT d.id,w.depth+1 FROM walk w JOIN dir d INDEXED BY dir_by_logical ON d.parent_id=w.id
            WHERE d.root_id=?1 AND d.listing_state!='absent_pending' AND w.depth<512)
        INSERT INTO rollup_work SELECT id,depth FROM walk", [root])?;
    let max_depth: Option<i64> =
        conn.query_row("SELECT max(depth) FROM rollup_work", [], |r| r.get(0))?;
    match max_depth {
        None => return Err(Error::NotFound),
        Some(512..) => return Err(Error::Invalid("tree depth limit")),
        _ => {}
    }
    let mut batch = conn.prepare("SELECT id,depth FROM rollup_work WHERE (depth,id)<(?1,?2) ORDER BY depth DESC,id DESC LIMIT 1024")?;
    let mut own = conn.prepare("SELECT own_files,own_logical,own_allocated,own_alloc_unknown,own_skipped,own_errors,own_newest_ft,own_noid,listing_state FROM dir WHERE id=?1")?;
    let mut children = conn.prepare("SELECT sub_files,sub_dirs,sub_logical,sub_allocated,sub_alloc_unknown,sub_skipped,sub_errors,sub_newest_ft,sub_complete,sub_noid,listing_state FROM dir INDEXED BY dir_by_logical WHERE parent_id=?1 AND root_id=?2 AND listing_state!='absent_pending'")?;
    let mut update=conn.prepare_cached("UPDATE dir SET depth=?2,sub_files=?3,sub_dirs=?4,sub_logical=?5,sub_allocated=?6,sub_alloc_unknown=?7,sub_skipped=?8,sub_errors=?9,sub_newest_ft=?10,sub_complete=?11,sub_noid=?12,agg_valid_rev=dirty_rev WHERE id=?1")?;
    let mut after = (512, i64::MAX);
    loop {
        let ids = batch
            .query_map(params![after.0, after.1], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if ids.is_empty() {
            break;
        }
        for (id, depth) in ids {
            let (mut t, mut noid) = own.query_row([id], |r| {
                let state: String = r.get(8)?;
                if state == "excluded" {
                    return Ok((Totals::default(), 0u64));
                }
                Ok((
                    Totals {
                        files: r.get::<_, i64>(0)? as u64,
                        logical: r.get::<_, i64>(1)? as u64,
                        allocated: r.get::<_, i64>(2)? as u64,
                        allocation_unknown: r.get::<_, i64>(3)? as u64,
                        skipped: r.get::<_, i64>(4)? as u64,
                        errors: r.get::<_, i64>(5)? as u64,
                        newest_ft: r.get(6)?,
                        complete: clean
                            && state == "complete"
                            && r.get::<_, i64>(4)? == 0
                            && r.get::<_, i64>(5)? == 0,
                        ..Totals::default()
                    },
                    r.get::<_, i64>(7)? as u64,
                ))
            })?;
            let mut rows = children.query(params![id, root])?;
            while let Some(r) = rows.next()? {
                t.files = checked(t.files, r.get::<_, i64>(0)? as u64)?;
                t.dirs = checked(
                    t.dirs,
                    checked(
                        r.get::<_, i64>(1)? as u64,
                        u64::from(r.get::<_, String>(10)? != "excluded"),
                    )?,
                )?;
                t.logical = checked(t.logical, r.get::<_, i64>(2)? as u64)?;
                t.allocated = checked(t.allocated, r.get::<_, i64>(3)? as u64)?;
                t.allocation_unknown = checked(t.allocation_unknown, r.get::<_, i64>(4)? as u64)?;
                t.skipped = checked(t.skipped, r.get::<_, i64>(5)? as u64)?;
                t.errors = checked(t.errors, r.get::<_, i64>(6)? as u64)?;
                t.newest_ft = t.newest_ft.max(r.get(7)?);
                t.complete &= r.get::<_, bool>(8)?;
                noid = checked(noid, r.get::<_, i64>(9)? as u64)?;
            }
            update.execute(params![
                id,
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
                noid as i64
            ])?;
            after = (depth, id);
        }
    }
    conn.execute_batch("DROP TABLE rollup_work")?;
    Ok(())
}
#[cfg(test)]
mod bulk_tests;
#[cfg(test)]
mod full_disk_tests;
#[cfg(test)]
mod publication_tests;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn end_run_checkpoint_does_not_wait_for_a_reader_and_restores_write_timeout() {
        let temp = tempfile::tempdir().unwrap();
        let conn = crate::db::open_pair(temp.path(), "synthetic").unwrap();
        let writer = Writer::start(conn).unwrap();
        writer
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "fixture".into(),
                display_name: "Fixture".into(),
                display_path: "Fixture root".into(),
                root_file_id: Some(vec![1; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap();
        let WriteReply::Run(run_id) = writer
            .call(WriteCommand::BeginRun {
                grant_id: 1,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 1,
            })
            .unwrap()
        else {
            panic!("run receipt")
        };
        let reader = Connection::open(temp.path().join("catalog.db")).unwrap();
        reader
            .execute_batch("BEGIN; SELECT count(*) FROM file")
            .unwrap();
        writer
            .call(WriteCommand::DirListing(DirListing {
                run_id,
                dir_id: 1,
                files: vec![Observation::file("item", 7, Some(7))],
                dirs: vec![],
                state: "complete".into(),
                skipped: 0,
                errors: 0,
            }))
            .unwrap();
        let end = writer
            .send(WriteCommand::EndRun {
                run_id,
                state: "completed".into(),
                finished_at_ns: 2,
            })
            .unwrap();
        // Keep the snapshot open until the receipt or timeout; release it even on a red run.
        let result = end.rx.recv_timeout(Duration::from_secs(1));
        assert_eq!(
            reader
                .query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        reader.execute_batch("ROLLBACK").unwrap();
        result
            .expect("EndRun waited for a reader instead of returning its committed receipt")
            .unwrap();
        assert!(writer.timings().checkpoint_seconds < 1.0);
        assert_eq!(
            reader
                .query_row("SELECT logical FROM file", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            7
        );
        assert_eq!(
            reader
                .query_row("SELECT state FROM scan_run WHERE id=?1", [run_id], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            "completed"
        );

        reader.execute_batch("BEGIN IMMEDIATE").unwrap();
        let barrier = writer.send(WriteCommand::Barrier).unwrap();
        assert!(matches!(
            barrier.rx.recv_timeout(Duration::from_millis(100)),
            Err(crossbeam_channel::RecvTimeoutError::Timeout)
        ));
        reader.execute_batch("ROLLBACK").unwrap();
        barrier
            .wait()
            .expect("checkpoint must restore the writer's busy timeout");
        drop(writer);
        assert_eq!(
            reader
                .query_row("PRAGMA main.wal_checkpoint(TRUNCATE)", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            std::fs::metadata(temp.path().join("catalog.db-wal"))
                .unwrap()
                .len(),
            0
        );
    }
    /// LW-121: a completed run keeps the root's size and coverage; other endings keep nothing.
    #[test]
    fn completed_run_records_root_size_and_coverage_for_growth_history() {
        let temp = tempfile::tempdir().unwrap();
        let writer =
            Writer::start(crate::db::open_pair(temp.path(), "synthetic").unwrap()).unwrap();
        writer
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "fixture".into(),
                display_name: "Fixture".into(),
                display_path: "Fixture root".into(),
                root_file_id: Some(vec![1; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap();
        let run = |state: &str, skipped: u64| {
            let WriteReply::Run(run_id) = writer
                .call(WriteCommand::BeginRun {
                    grant_id: 1,
                    mode: "full".into(),
                    strategy: "fixture".into(),
                    started_at_ns: 1,
                })
                .unwrap()
            else {
                panic!("run receipt")
            };
            writer
                .call(WriteCommand::DirListing(DirListing {
                    run_id,
                    dir_id: 1,
                    files: vec![
                        Observation::file("a", 7, Some(7)),
                        Observation::file("b", 5, Some(5)),
                    ],
                    dirs: vec![],
                    state: "complete".into(),
                    skipped,
                    errors: 0,
                }))
                .unwrap();
            writer
                .call(WriteCommand::EndRun {
                    run_id,
                    state: state.into(),
                    finished_at_ns: 2,
                })
                .unwrap();
            run_id
        };
        let counters = |id: i64| {
            let conn = Connection::open(temp.path().join("catalog.db")).unwrap();
            let json: String = conn
                .query_row(
                    "SELECT counters_json FROM scan_run WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .unwrap();
            serde_json::from_str::<serde_json::Value>(&json).unwrap()
        };
        let clean = counters(run("completed", 0));
        assert_eq!(clean["logical_bytes"], 12);
        assert_eq!(clean["files"], 2);
        assert_eq!(clean["complete"], true);
        // A listing that skipped entries keeps its size but not a complete-coverage claim.
        assert_eq!(counters(run("completed", 1))["complete"], false);
        assert_eq!(counters(run("cancelled", 0)), serde_json::json!({}));
    }
    #[test]
    fn synchronous_listing_receipts_do_not_wait_for_future_commands() {
        let temp = tempfile::tempdir().unwrap();
        let writer =
            Writer::start(crate::db::open_pair(temp.path(), "synthetic").unwrap()).unwrap();
        writer
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "fixture".into(),
                display_name: "Fixture".into(),
                display_path: "Fixture root".into(),
                root_file_id: Some(vec![1; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap();
        let WriteReply::Run(run_id) = writer
            .call(WriteCommand::BeginRun {
                grant_id: 1,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 1,
            })
            .unwrap()
        else {
            panic!("run receipt")
        };
        let before = writer.timings();
        for logical in 1..=64 {
            writer
                .call(WriteCommand::DirListing(DirListing {
                    run_id,
                    dir_id: 1,
                    files: vec![Observation::file("item", logical, Some(logical))],
                    dirs: vec![],
                    state: "complete".into(),
                    skipped: 0,
                    errors: 0,
                }))
                .unwrap();
            // A receipt still means COMMIT: a separate reader sees each publication immediately.
            let reader = Connection::open(temp.path().join("catalog.db")).unwrap();
            assert_eq!(
                reader
                    .query_row("SELECT logical FROM file", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                logical as i64
            );
        }
        let after = writer.timings();
        assert_eq!(after.listing_calls - before.listing_calls, 64);
        assert_eq!(after.batch_wait_calls - before.batch_wait_calls, 64);
        // Measures only the queue probe, excluding SQLite and shared-host disk scheduling.
        assert!(
            after.batch_wait_seconds - before.batch_wait_seconds < 0.025,
            "idle batching delayed synchronous receipts: {} seconds",
            after.batch_wait_seconds - before.batch_wait_seconds
        );
    }
    #[test]
    fn nested_exclusions_shield_each_directory_once() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE dir(id INTEGER PRIMARY KEY,parent_id INTEGER,root_id INTEGER,listing_state TEXT);
            INSERT INTO dir VALUES(1,NULL,1,'complete'),(2,1,1,'excluded'),(3,2,1,'excluded'),(4,3,1,'complete');")
            .unwrap();
        let count: i64 = conn
            .query_row(
                &format!("WITH RECURSIVE {SHIELDED} SELECT count(*) FROM shielded"),
                [1],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 3);
    }
    #[test]
    fn streamed_transactions_bound_bytes_and_release_applied_payloads() {
        let temp = tempfile::tempdir().unwrap();
        let conn = crate::db::open_pair(temp.path(), "synthetic").unwrap();
        let writer = Writer::start(conn).unwrap();
        writer
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "fixture".into(),
                display_name: "Fixture".into(),
                display_path: "Fixture root".into(),
                root_file_id: Some(vec![1; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap();
        let WriteReply::Run(run_id) = writer
            .call(WriteCommand::BeginRun {
                grant_id: 1,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 1,
            })
            .unwrap()
        else {
            panic!()
        };
        drop(writer);
        let conn = Connection::open(temp.path().join("catalog.db")).unwrap();
        conn.execute(
            "ATTACH DATABASE ?1 AS st",
            [temp.path().join("state.db").to_string_lossy().as_ref()],
        )
        .unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA main.synchronous=NORMAL; PRAGMA st.synchronous=FULL",
        )
        .unwrap();
        let (tx, rx) = bounded(8);
        let reservation = 24 * 1024 * 1024;
        let quota = Arc::new((Mutex::new(3 * reservation), Condvar::new()));
        let mut replies = Vec::new();
        for size in [1, 2, 3] {
            let (reply, result) = bounded(1);
            tx.send(Pending {
                command: WriteCommand::DirListing(DirListing {
                    run_id,
                    dir_id: 1,
                    files: vec![Observation::file("item", size, Some(size))],
                    dirs: vec![],
                    state: "complete".into(),
                    skipped: 0,
                    errors: 0,
                }),
                reply,
                _permit: BytePermit {
                    quota: quota.clone(),
                    bytes: reservation,
                },
            })
            .unwrap();
            replies.push(result);
        }
        drop(tx);
        let timings = Arc::new(Mutex::new(WriterTimings::default()));
        let (alive, _) = bounded(0);
        hold_batch_window_open();
        work(conn, rx, timings.clone(), alive, None);
        for reply in replies {
            reply.recv().unwrap().unwrap();
        }
        let timing = timings.lock().unwrap();
        assert_eq!(timing.transactions, 2);
        assert_eq!(timing.commands, 3);
        assert_eq!(timing.max_transaction_commands, 2);
        assert_eq!(timing.max_transaction_bytes, 2 * reservation);
        assert_eq!(*quota.0.lock().unwrap(), 0);
        let conn = Connection::open(temp.path().join("catalog.db")).unwrap();
        assert_eq!(
            conn.query_row("SELECT logical FROM file", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
    }
    #[test]
    fn two_listings_in_one_writer_batch_reject_the_older_final() {
        let temp = tempfile::tempdir().unwrap();
        let conn = crate::db::open_pair(temp.path(), "synthetic").unwrap();
        let (tx, rx) = bounded(64);
        let quota = Arc::new((Mutex::new(0), Condvar::new()));
        let enqueue = |command| {
            let (reply, result) = bounded(1);
            tx.send(Pending {
                command,
                reply,
                _permit: BytePermit {
                    quota: quota.clone(),
                    bytes: 0,
                },
            })
            .unwrap();
            result
        };
        let root = enqueue(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "fixture".into(),
            display_name: "Fixture".into(),
            display_path: "Fixture root".into(),
            root_file_id: Some(vec![1; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }));
        let run = enqueue(WriteCommand::BeginRun {
            grant_id: 1,
            mode: "full".into(),
            strategy: "fixture".into(),
            started_at_ns: 1,
        });
        let listing = |bytes| {
            WriteCommand::DirListing(DirListing {
                run_id: 1,
                dir_id: 1,
                files: vec![Observation::file("item", bytes, Some(bytes))],
                dirs: vec![],
                state: "complete".into(),
                skipped: 0,
                errors: 0,
            })
        };
        // Both listings are already queued before work starts, so they share one batch.
        let first = enqueue(listing(150));
        let second = enqueue(listing(200));
        let (alive, _) = bounded(0);
        let handle = std::thread::spawn(move || {
            work(
                conn,
                rx,
                Arc::new(Mutex::new(WriterTimings::default())),
                alive,
                None,
            )
        });
        root.recv().unwrap().unwrap();
        run.recv().unwrap().unwrap();
        let WriteReply::Listing { revision: old } = first.recv().unwrap().unwrap() else {
            panic!()
        };
        let WriteReply::Listing { revision: new } = second.recv().unwrap().unwrap() else {
            panic!()
        };
        let final_command = |input_revision, bytes| WriteCommand::DirFinal {
            run_id: 1,
            dir_id: 1,
            input_revision,
            totals: Totals {
                files: 1,
                logical: bytes,
                allocated: bytes,
                complete: true,
                ..Totals::default()
            },
        };
        let stale = enqueue(final_command(old, 150)).recv().unwrap();
        let current = enqueue(final_command(new, 200)).recv().unwrap();
        drop(tx);
        handle.join().unwrap();
        assert!(matches!(stale, Err(Error::StaleGeneration)));
        assert!(new > old);
        current.unwrap();
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT own_logical,sub_logical,agg_valid_rev=dirty_rev FROM dir WHERE id=1",
                [],
                |r| Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, bool>(2)?
                ))
            )
            .unwrap(),
            (200, 200, true)
        );
    }
    #[test]
    fn deep_reparented_chain_rejects_only_its_listing_in_a_shared_batch() {
        let temp = tempfile::tempdir().unwrap();
        let writer =
            Writer::start(crate::db::open_pair(temp.path(), "synthetic").unwrap()).unwrap();
        writer
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "fixture".into(),
                display_name: "Fixture".into(),
                display_path: "Fixture root".into(),
                root_file_id: Some(vec![1; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap();
        writer
            .call(WriteCommand::BeginRun {
                grant_id: 1,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 1,
            })
            .unwrap();
        drop(writer);
        // Reopen without open_pair, which would interrupt the running run.
        let conn = Connection::open(temp.path().join("catalog.db")).unwrap();
        conn.execute(
            "ATTACH DATABASE ?1 AS st",
            [temp.path().join("state.db").to_string_lossy().as_ref()],
        )
        .unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        // Two 300-deep chains under the root (ids 2..=301 and 302..=601) and two leaves.
        // Simulate an overdeep tree left by the legacy reparent bug; publication now rejects it.
        conn.execute_batch(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<300)
             INSERT INTO dir(id,root_id,parent_id,name,file_id,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run)
             SELECT i+1,1,i,'a'||i,NULL,'path_observation','none',i,0,'complete',1,1 FROM n
             UNION ALL
             SELECT i+301,1,CASE i WHEN 1 THEN 1 ELSE i+300 END,'b'||i,
                    CASE i WHEN 1 THEN x'00000000000000000000000000000002' END,
                    CASE i WHEN 1 THEN 'native_file_id_128' ELSE 'path_observation' END,
                    CASE i WHEN 1 THEN 'listed' ELSE 'none' END,i,0,'complete',1,1 FROM n;
             INSERT INTO dir(id,root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run)
             VALUES(602,1,1,'c','path_observation','none',1,0,'complete',1,1),
                   (603,1,1,'d','path_observation','none',1,0,'complete',1,1);",
        )
        .unwrap();
        conn.execute("UPDATE dir SET parent_id=301 WHERE id=302", [])
            .unwrap();
        let (tx, rx) = bounded(64);
        let quota = Arc::new((Mutex::new(0), Condvar::new()));
        let enqueue = |command| {
            let (reply, result) = bounded(1);
            tx.send(Pending {
                command,
                reply,
                _permit: BytePermit {
                    quota: quota.clone(),
                    bytes: 0,
                },
            })
            .unwrap();
            result
        };
        let listing = |dir_id, files, dirs| {
            WriteCommand::DirListing(DirListing {
                run_id: 1,
                dir_id,
                files,
                dirs,
                state: "complete".into(),
                skipped: 0,
                errors: 0,
            })
        };
        let item = || vec![Observation::file("item", 7, Some(7))];
        // All four listings are queued before work starts, so they share one transaction.
        let before = enqueue(listing(602, item(), vec![]));
        let shallow = enqueue(listing(301, vec![], vec![]));
        // b300 still stores depth 300, but its chain is now 601 directories long.
        let deep = enqueue(listing(601, item(), vec![]));
        let after = enqueue(listing(603, item(), vec![]));
        let barrier = enqueue(WriteCommand::Barrier);
        drop(tx);
        let timings = Arc::new(Mutex::new(WriterTimings::default()));
        let (alive, _) = bounded(0);
        hold_batch_window_open();
        work(conn, rx, timings.clone(), alive, None);
        before.recv().unwrap().unwrap();
        shallow.recv().unwrap().unwrap();
        assert!(matches!(
            deep.recv().unwrap(),
            Err(Error::Invalid("tree depth limit"))
        ));
        after.recv().unwrap().unwrap();
        // The writer stayed open: the next command is applied, not dropped.
        barrier.recv().unwrap().unwrap();
        assert_eq!(timings.lock().unwrap().max_transaction_commands, 4);
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        let state = |sql: &str| db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(state("SELECT parent_id FROM dir WHERE id=302"), 301);
        assert_eq!(
            state("SELECT count(*) FROM file WHERE dir_id IN (602,603)"),
            2
        );
        assert_eq!(state("SELECT count(*) FROM file"), 2);
        assert_eq!(state("SELECT own_files FROM dir WHERE id=601"), 0);
    }
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
    #[test]
    fn failed_scrub_marker_clear_is_not_reported_as_complete() {
        let temp = tempfile::tempdir().unwrap();
        let conn = crate::db::open_pair(temp.path(), "synthetic").unwrap();
        conn.execute_batch("INSERT INTO st.meta VALUES('scrub_pending','1'); CREATE TRIGGER st.refuse_clear BEFORE DELETE ON meta WHEN old.key='scrub_pending' BEGIN SELECT RAISE(ABORT,'synthetic marker failure'); END;").unwrap();
        assert!(!wal_cut(&conn, Duration::ZERO, &mut false));
        assert!(scrub_pending(&conn).unwrap());
    }
    #[test]
    fn persistent_scrub_failure_backs_off_and_keeps_the_durable_marker() {
        use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
        let temp = tempfile::tempdir().unwrap();
        let conn = crate::db::open_pair(temp.path(), "synthetic").unwrap();
        conn.execute("INSERT INTO st.meta VALUES('scrub_pending','1')", [])
            .unwrap();
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let observed = attempts.clone();
        conn.authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(context.action, AuthAction::Attach { .. }) {
                observed.lock().unwrap().push(Instant::now());
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }))
        .unwrap();
        let writer = Writer::start(conn).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while attempts.lock().unwrap().len() < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(writer);
        let times = attempts.lock().unwrap();
        assert!(times.len() >= 3, "failure injection did not run");
        assert!(times[1].duration_since(times[0]) >= Duration::from_secs(1));
        assert!(times[2].duration_since(times[1]) >= Duration::from_secs(2));
        let state = Connection::open(temp.path().join("state.db")).unwrap();
        assert_eq!(
            state
                .query_row(
                    "SELECT count(*) FROM meta WHERE key='scrub_pending'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn rewrite_uses_file_temp_store_and_restores_writer_views_even_on_failure() {
        use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
        for (temp_store, fail) in [0, 1, 2]
            .into_iter()
            .flat_map(|mode| [false, true].map(|fail| (mode, fail)))
        {
            let temp = tempfile::tempdir().unwrap();
            let conn = crate::db::open_pair(temp.path(), "synthetic").unwrap();
            if temp_store != 1 {
                conn.pragma_update(None, "temp_store", temp_store).unwrap();
                crate::db::grant_view(&conn).unwrap();
            }
            let mode = Arc::new(Mutex::new(String::new()));
            let observed = mode.clone();
            conn.authorizer(Some(move |context: AuthContext<'_>| {
                match context.action {
                    AuthAction::Pragma {
                        pragma_name: "temp_store",
                        pragma_value: Some(value),
                    } => *observed.lock().unwrap() = value.into(),
                    AuthAction::Attach { .. } => {
                        assert_eq!(*observed.lock().unwrap(), "FILE");
                        if fail {
                            return Authorization::Deny;
                        }
                    }
                    _ => {}
                }
                Authorization::Allow
            }))
            .unwrap();
            assert_eq!(wal_cut(&conn, Duration::ZERO, &mut true), !fail);
            assert_eq!(
                conn.query_row("PRAGMA temp_store", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                temp_store
            );
            conn.execute_batch(
                "SELECT count(*) FROM temp.root_grant; SELECT count(*) FROM temp.visible_dir",
            )
            .unwrap();
        }
    }
}
