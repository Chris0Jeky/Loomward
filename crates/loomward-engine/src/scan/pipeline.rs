use super::{sink::*, source::*, GrantedRoot};
use crate::{
    budgets::{ByteBudget, BytePermit},
    EngineError, EngineResult,
};
use loomward_protocol::RootId;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
const CHUNK_BYTES: usize = 128 * 1024;
const CHUNK_ENTRIES: usize = 256;
const CHUNK_ENTRY_BYTES: usize = CHUNK_ENTRIES * std::mem::size_of::<Entry>();
const MAX_DEPTH: usize = 128;
const TASK_CAPACITY: usize = 1024;
/// Finite scan caps; the memory budget additionally bounds the directory arena.
#[derive(Clone, Debug)]
pub struct ScanOptions {
    /// Own enumeration worker count, 1..=32.
    pub workers: usize,
    /// Entire-run examined-entry limit.
    pub max_entries: u64,
    /// Maximum directories, also clamped by the arena byte reservation.
    pub max_dirs: usize,
    /// Per-directory listing limit, including excluded entries.
    pub max_listing_entries: u64,
    /// Full-root or explicitly targeted reconciliation.
    pub scope: RunScope,
    /// Loomward state path, opened metadata-only to obtain its exclusion identity.
    pub excluded_path: Option<PathBuf>,
}
impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            workers: 8,
            max_entries: 50_000_000,
            max_dirs: 8_000_000,
            max_listing_entries: 2_000_000,
            scope: RunScope::FullRoot,
            excluded_path: None,
        }
    }
}
/// Per-run observations. Enumeration time includes source callbacks; staging time measures sink calls.
#[derive(Clone, Debug)]
pub struct ScanReport {
    /// Final root totals (allocation can remain unknown).
    pub totals: Sums,
    /// Every traversed listing reached native EOF, no errors or limits occurred.
    pub complete: bool,
    /// Least capable actual strategy observed across this traversal.
    pub strategy: Strategy,
    /// A finite entry, depth or directory limit was reached.
    pub limit_hit: bool,
    /// Entries examined, including excluded metadata.
    pub examined: u64,
    /// Deliberately skipped entries.
    pub skipped: u64,
    /// Failed child opens or listings.
    pub failed: u64,
    /// Wall-clock elapsed seconds, including parallel overlap.
    pub elapsed_seconds: f64,
    /// Sum of worker time spent enumerating and handing off chunks (not exclusive CPU time).
    pub enumeration_worker_seconds: f64,
    /// Sum of writer time inside sink staging/publication methods.
    pub persistence_seconds: f64,
}
struct Slot<D> {
    parent: Option<usize>,
    dir: Arc<D>,
    depth: usize,
    own: Sums,
    listed: bool,
    sums: Sums,
    complete: bool,
    ticket: ListingTicket,
}
struct Task<D> {
    dir: Arc<D>,
    slot: usize,
    depth: usize,
}
struct Tasks<D> {
    queue: VecDeque<Task<D>>,
    active: usize,
    done: bool,
}
enum Command {
    Stop,
    Prepare {
        parent: Option<u64>,
        name: Vec<u16>,
        identity: OpenedIdentity,
        reply: SyncSender<EngineResult<ListingTicket>>,
    },
    Message(ScanMessage),
    Barrier(SyncSender<Vec<u64>>),
}
struct Envelope {
    command: Command,
    _permit: Option<BytePermit>,
}
struct Shared<'a, S: DirSource> {
    source: &'a S,
    run: u64,
    options: ScanOptions,
    tasks: Mutex<Tasks<S::Dir>>,
    ready: Condvar,
    arena: Mutex<Vec<Slot<S::Dir>>>,
    discovered: Mutex<HashSet<(Option<u64>, FileIdObs)>>,
    catalog_dirs: Mutex<HashMap<u64, usize>>,
    tx: SyncSender<Envelope>,
    bytes: Arc<ByteBudget>,
    cancel: Arc<AtomicBool>,
    writer_failed: Arc<AtomicBool>,
    examined: AtomicU64,
    skipped: AtomicU64,
    failures: AtomicU64,
    enumeration_ns: AtomicU64,
    strategy: Mutex<Strategy>,
    limit_hit: AtomicBool,
    excluded: Vec<OpenedIdentity>,
    max_dirs: usize,
}
impl<S: DirSource> Shared<'_, S> {
    fn stopping(&self) -> bool {
        self.cancel.load(Ordering::Acquire) || self.writer_failed.load(Ordering::Acquire)
    }
    fn send(&self, mut envelope: Envelope) -> bool {
        loop {
            if self.stopping() {
                return false;
            }
            match self.tx.try_send(envelope) {
                Ok(()) => return true,
                Err(TrySendError::Full(e)) => {
                    envelope = e;
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.writer_failed.store(true, Ordering::Release);
                    return false;
                }
            }
        }
    }
    fn prepare(
        &self,
        parent: Option<u64>,
        name: &[u16],
        identity: OpenedIdentity,
    ) -> EngineResult<ListingTicket> {
        let (tx, rx) = mpsc::sync_channel(1);
        if !self.send(Envelope {
            command: Command::Prepare {
                parent,
                name: name.to_vec(),
                identity,
                reply: tx,
            },
            _permit: None,
        }) {
            return Err(cancelled());
        }
        loop {
            if self.stopping() {
                return Err(cancelled());
            }
            match rx.recv_timeout(Duration::from_millis(5)) {
                Ok(r) => return r,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err(cancelled()),
            }
        }
    }
    fn message(&self, m: ScanMessage) -> bool {
        self.send(Envelope {
            command: Command::Message(m),
            _permit: None,
        })
    }
    fn finish_slot(&self, index: usize, own: Sums, complete: bool) -> EngineResult<()> {
        let mut arena = self.arena.lock().unwrap();
        arena[index].own = own;
        arena[index].listed = complete;
        Ok(())
    }
    fn finalize(&self) -> EngineResult<()> {
        let mut arena = self.arena.lock().unwrap();
        for slot in arena.iter_mut() {
            slot.sums = slot.own;
            slot.complete = slot.listed;
        }
        for index in (0..arena.len()).rev() {
            let slot = &arena[index];
            let (parent, ticket, sums, complete) =
                (slot.parent, slot.ticket, slot.sums, slot.complete);
            if !self.message(ScanMessage::DirFinal {
                run: self.run,
                ticket,
                sums,
                complete,
            }) {
                return Err(cancelled());
            }
            if let Some(parent) = parent {
                let p = &mut arena[parent];
                p.sums = p.sums.checked_add(sums.checked_add(Sums {
                    dirs: 1,
                    ..Sums::ZERO
                })?)?;
                p.complete &= complete;
            }
        }
        Ok(())
    }
    fn dirty_directories(&self) -> EngineResult<Vec<u64>> {
        let (tx, rx) = mpsc::sync_channel(1);
        if !self.send(Envelope {
            command: Command::Barrier(tx),
            _permit: None,
        }) {
            return Err(cancelled());
        }
        loop {
            if self.stopping() {
                return Err(cancelled());
            }
            match rx.recv_timeout(Duration::from_millis(5)) {
                Ok(dirs) => return Ok(dirs),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err(cancelled()),
            }
        }
    }
    fn flush(
        &self,
        ticket: ListingTicket,
        entries: &mut Vec<Entry>,
        permit: &mut Option<BytePermit>,
    ) -> bool {
        if entries.is_empty() {
            permit.take();
            return true;
        }
        self.send(Envelope {
            command: Command::Message(ScanMessage::DirListing {
                run: self.run,
                ticket,
                entries: std::mem::take(entries),
            }),
            _permit: permit.take(),
        })
    }
    fn visit(&self, task: Task<S::Dir>) -> EngineResult<()> {
        let ticket = self.arena.lock().unwrap()[task.slot].ticket;
        let mut own = Sums::ZERO;
        let mut complete = true;
        let mut entries = Vec::new();
        let mut permit = None;
        let mut chunk_bytes = CHUNK_ENTRY_BYTES;
        let mut count = 0u64;
        let mut stopped = None;
        let mut error = None;
        let start = Instant::now();

        let outcome = self.source.list(&task.dir, &mut |raw| {
            if self.stopping() {
                return Flow::Stop;
            }
            if raw.name.len() > 32767 || raw.name.is_empty() {
                stopped = Some(IncompleteReason::Malformed);
                return Flow::Stop;
            }
            count += 1;
            if count > self.options.max_listing_entries
                || self.examined.fetch_add(1, Ordering::Relaxed) >= self.options.max_entries
            {
                stopped = Some(IncompleteReason::Limit);
                return Flow::Stop;
            }
            let mut excluded = refused_attributes(raw.attributes)
                || raw.name == [46, 108, 111, 111, 109, 119, 97, 114, 100];
            let mut child = None;
            let mut traversal_error = None;
            if !excluded && raw.attributes & 0x10 != 0 {
                if task.depth >= MAX_DEPTH {
                    complete = false;
                    stopped = Some(IncompleteReason::Limit);
                    excluded = true;
                } else {
                    match self.source.open_child(&task.dir, &raw) {
                        Ok((dir, identity)) => {
                            excluded = self.excluded.iter().any(|e| {
                                e.id.is_some()
                                    && e.id == identity.id
                                    && e.volume_serial == identity.volume_serial
                            });
                            if !excluded {
                                child = Some((dir, identity));
                            }
                        }
                        Err(reason) => {
                            traversal_error = Some(reason);
                            complete = false;
                            self.failures.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
            if excluded {
                self.skipped.fetch_add(1, Ordering::Relaxed);
            }
            let entry = Entry {
                name: raw.name.to_vec(),
                file_id: raw.file_id,
                identity_eligible: raw.file_id.is_some()
                    && !raw.file_id.is_some_and(|id| {
                        matches!(id, FileIdObs::Id64(_)) && self.source.is_refs(&task.dir)
                    }),
                attributes: raw.attributes,
                reparse_tag: raw.reparse_tag,
                logical: raw.end_of_file,
                allocated: raw.allocation_size,
                creation: raw.creation,
                last_write: raw.last_write,
                change: raw.change,
                last_access: raw.last_access,
                excluded,
                traversal_error,
            };
            let size = entry.name.capacity() * 2;
            if chunk_bytes + size > CHUNK_BYTES || entries.len() == CHUNK_ENTRIES {
                if !self.flush(ticket, &mut entries, &mut permit) {
                    return Flow::Stop;
                }
                chunk_bytes = CHUNK_ENTRY_BYTES;
            }
            if permit.is_none() {
                permit = self
                    .bytes
                    .acquire(CHUNK_BYTES, &self.cancel, &self.writer_failed);
                if permit.is_none() {
                    return Flow::Stop;
                }
                entries = Vec::with_capacity(CHUNK_ENTRIES);
            }
            if !excluded && entry.attributes & 0x10 == 0 {
                match own.checked_add(Sums {
                    files: 1,
                    dirs: 0,
                    logical: entry.logical,
                    allocated: entry.allocated,
                }) {
                    Ok(s) => own = s,
                    Err(e) => {
                        error = Some(e);
                        return Flow::Stop;
                    }
                }
            }
            chunk_bytes += size;
            entries.push(entry);
            if let Some((dir, identity)) = child {
                if let Some(key) = directory_key(identity) {
                    if !self.discovered.lock().unwrap().insert(key) {
                        return Flow::Continue;
                    }
                }
                // Drop the parent's chunk reservation before waiting for a writer reply.
                if !self.flush(ticket, &mut entries, &mut permit) {
                    return Flow::Stop;
                }
                chunk_bytes = CHUNK_ENTRY_BYTES;
                let available = self.arena.lock().unwrap().len() < self.max_dirs;
                if !available {
                    complete = false;
                    stopped = Some(IncompleteReason::Limit);
                    return Flow::Stop;
                }
                let child_ticket = match self.prepare(Some(ticket.dir), raw.name, identity) {
                    Ok(t) => t,
                    Err(e) => {
                        error = Some(e);
                        return Flow::Stop;
                    }
                };
                if let Some(old_parent) = child_ticket.old_parent {
                    if !self.message(ScanMessage::InvalidateChains {
                        run: self.run,
                        old_parent,
                        new_parent: ticket.dir,
                    }) {
                        return Flow::Stop;
                    }
                }
                let slot = {
                    let mut arena = self.arena.lock().unwrap();
                    if let Some(&index) = self.catalog_dirs.lock().unwrap().get(&child_ticket.dir) {
                        // Name-only identities can recur when their parent is relisted.
                        arena[index].ticket = child_ticket;
                        self.tasks.lock().unwrap().queue.push_back(Task {
                            dir: arena[index].dir.clone(),
                            slot: index,
                            depth: arena[index].depth,
                        });
                        self.ready.notify_one();
                        return Flow::Continue;
                    }
                    if arena.len() >= self.max_dirs {
                        complete = false;
                        stopped = Some(IncompleteReason::Limit);
                        return Flow::Stop;
                    }
                    let index = arena.len();
                    arena.push(Slot {
                        parent: Some(task.slot),
                        dir: Arc::new(dir),
                        depth: task.depth + 1,
                        own: Sums::ZERO,
                        listed: false,
                        sums: Sums::ZERO,
                        complete: false,
                        ticket: child_ticket,
                    });
                    self.catalog_dirs
                        .lock()
                        .unwrap()
                        .insert(child_ticket.dir, index);
                    index
                };
                // The arena reservation also covers every queued handle/task: no recursive
                // enumeration retains another native buffer or parent callback on the stack.
                let dir = self.arena.lock().unwrap()[slot].dir.clone();
                self.tasks.lock().unwrap().queue.push_back(Task {
                    dir,
                    slot,
                    depth: task.depth + 1,
                });
                self.ready.notify_one();
            }
            Flow::Continue
        });
        self.enumeration_ns.fetch_add(
            start.elapsed().as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
        if let Some(e) = error {
            return Err(e);
        }
        let mut outcome = stopped.map_or(outcome, ListOutcome::Incomplete);
        if outcome == ListOutcome::Complete
            && matches!(&self.options.scope,RunScope::Targeted(dirs) if !dirs.contains(&ticket.dir))
        {
            outcome = ListOutcome::Incomplete(IncompleteReason::OutsideScope);
        }

        if matches!(outcome, ListOutcome::Incomplete(IncompleteReason::Limit)) {
            self.limit_hit.store(true, Ordering::Release);
        }
        {
            let mut actual = self.strategy.lock().unwrap();
            let observed = self.source.strategy_for(&task.dir);
            if (observed as u8) > (*actual as u8) {
                *actual = observed;
            }
        }
        if outcome != ListOutcome::Complete {
            complete = false;
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
        if !self.flush(ticket, &mut entries, &mut permit) {
            return Err(cancelled());
        }
        if !self.message(ScanMessage::ListingDone {
            run: self.run,
            ticket,
            outcome,
        }) {
            return Err(cancelled());
        }
        self.finish_slot(task.slot, own, complete)
    }
    fn worker(&self) -> EngineResult<()> {
        loop {
            let task = {
                let mut tasks = self.tasks.lock().unwrap();
                loop {
                    if self.stopping() || tasks.done {
                        return Ok(());
                    }
                    if let Some(t) = tasks.queue.pop_front() {
                        tasks.active += 1;
                        break t;
                    }
                    if tasks.active == 0 {
                        tasks.done = true;
                        self.ready.notify_all();
                        return Ok(());
                    }
                    tasks = self
                        .ready
                        .wait_timeout(tasks, Duration::from_millis(5))
                        .unwrap()
                        .0;
                }
            };
            let result = self.visit(task);
            {
                let mut tasks = self.tasks.lock().unwrap();
                tasks.active -= 1;
                if result.is_err() {
                    self.writer_failed.store(true, Ordering::Release);
                }
                self.ready.notify_all();
            }
            result?;
        }
    }
}
fn cancelled() -> EngineError {
    EngineError::Cancelled {
        message: "scan cancelled or writer stopped".into(),
    }
}
fn directory_key(identity: OpenedIdentity) -> Option<(Option<u64>, FileIdObs)> {
    if matches!(identity.basis, IdBasis::Listed | IdBasis::PostOpen) {
        identity
            .id
            .and_then(FileIdObs::nonzero)
            .map(|id| (identity.volume_serial, id))
    } else {
        None
    }
}
fn writer(
    root: &RootId,
    run: u64,
    sink: &dyn ScanSink,
    rx: Receiver<Envelope>,
    failed: &AtomicBool,
) -> EngineResult<f64> {
    let mut seconds = 0.;
    let mut first_error = None;
    let mut dirty = HashSet::new();
    for e in rx {
        if matches!(e.command, Command::Stop) {
            break;
        }
        if first_error.is_some() {
            continue;
        }
        let start = Instant::now();
        let result = match e.command {
            Command::Prepare {
                parent,
                name,
                identity,
                reply,
            } => {
                let r = sink.prepare_listing(root, run, parent, &name, identity);
                let failed = r.as_ref().err().cloned();
                let _ = reply.send(r);
                failed.map_or(Ok(()), Err)
            }
            Command::Message(m) => {
                let ticket = match &m {
                    ScanMessage::DirListing { ticket, .. }
                    | ScanMessage::ListingDone { ticket, .. }
                    | ScanMessage::DirFinal { ticket, .. } => Some(*ticket),
                    _ => None,
                };
                // Once a final is rejected, later ancestor finals belong to the same dirty epoch.
                if !dirty.is_empty() && matches!(m, ScanMessage::DirFinal { .. }) {
                    continue;
                }
                match sink.consume(root, m) {
                    Err(EngineError::StaleGeneration {
                        detail: Some(detail),
                        ..
                    }) if ticket.is_some()
                        && detail.get("reason")
                            == Some(&serde_json::json!("listing_revision_changed")) =>
                    {
                        dirty.insert(ticket.unwrap().dir);
                        Ok(())
                    }
                    result => result,
                }
            }
            Command::Barrier(reply) => {
                let _ = reply.send(dirty.drain().collect());
                Ok(())
            }
            Command::Stop => unreachable!(),
        };
        seconds += start.elapsed().as_secs_f64();
        if let Err(error) = result {
            failed.store(true, Ordering::Release);
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(seconds), Err)
}
/// Scan one granted root into a bounded sink. No real metadata is exported or retained by this API.
pub fn run_scan<S: DirSource>(
    source: &S,
    root: &GrantedRoot,
    run: u64,
    sink: Arc<dyn ScanSink>,
    bytes: Arc<ByteBudget>,
    cancel: Arc<AtomicBool>,
    options: ScanOptions,
) -> EngineResult<ScanReport> {
    if !(1..=32).contains(&options.workers)
        || options.max_dirs == 0
        || options.max_entries == 0
        || options.max_listing_entries == 0
    {
        return Err(EngineError::InvalidRequest {
            message: "invalid scan caps".into(),
        });
    }
    let start = Instant::now();
    let failed = Arc::new(AtomicBool::new(false));
    let arena_bytes = (bytes.capacity() / 4).min(16 * 1024 * 1024);
    let reservation = arena_bytes
        + TASK_CAPACITY * std::mem::size_of::<Task<S::Dir>>()
        + options.workers * 1280 * 1024;
    if reservation + CHUNK_BYTES > bytes.capacity() {
        return Err(EngineError::ResourceBudget {
            message: "scan byte budget too small".into(),
        });
    }
    let _arena_permit = bytes
        .acquire(reservation, &cancel, &failed)
        .ok_or_else(cancelled)?;
    let node_bytes = std::mem::size_of::<Slot<S::Dir>>()
        + std::mem::size_of::<Task<S::Dir>>()
        + std::mem::size_of::<S::Dir>()
        + 2 * std::mem::size_of::<usize>()
        + 128; // Identity-set buckets and the retained handle's Arc allocation.
    let max_dirs = options.max_dirs.min(arena_bytes / node_bytes);
    let (dir, identity) =
        source
            .open_root(root.path())
            .map_err(|_| EngineError::PermissionDenied {
                message: "root open refused".into(),
            })?;
    if root
        .expected
        .is_some_and(|e| e.id != identity.id || e.volume_serial != identity.volume_serial)
    {
        return Err(EngineError::PermissionDenied {
            message: "root_identity_changed".into(),
        });
    }
    let excluded = match &options.excluded_path {
        Some(path) if path.exists() => vec![
            source
                .open_root(path)
                .map_err(|_| EngineError::PermissionDenied {
                    message: "state exclusion identity unavailable".into(),
                })?
                .1,
        ],
        _ => Vec::new(),
    };
    if excluded
        .iter()
        .any(|e| e.id.is_some() && e.id == identity.id && e.volume_serial == identity.volume_serial)
    {
        return Err(EngineError::PermissionDenied {
            message: "root_is_state_directory".into(),
        });
    }
    sink.begin_run(root.root_id(), run, &options.scope)?;
    let ticket = match sink.prepare_listing(root.root_id(), run, None, &[], identity) {
        Ok(t) => t,
        Err(e) => {
            let _ = sink.finish_run(root.root_id(), run, &options.scope, false);
            return Err(e);
        }
    };
    let (tx, rx) = mpsc::sync_channel(32);
    let mut queue = VecDeque::with_capacity(max_dirs);
    let dir = Arc::new(dir);
    queue.push_back(Task {
        dir: dir.clone(),
        slot: 0,
        depth: 0,
    });
    let shared = Shared {
        source,
        run,
        options: options.clone(),
        tasks: Mutex::new(Tasks {
            queue,
            active: 0,
            done: false,
        }),
        ready: Condvar::new(),
        arena: Mutex::new(Vec::with_capacity(max_dirs)),
        discovered: Mutex::new(directory_key(identity).into_iter().collect()),
        catalog_dirs: Mutex::new(HashMap::from([(ticket.dir, 0)])),
        tx,
        bytes: bytes.clone(),
        cancel: cancel.clone(),
        writer_failed: failed.clone(),
        examined: AtomicU64::new(0),
        skipped: AtomicU64::new(0),
        failures: AtomicU64::new(0),
        enumeration_ns: AtomicU64::new(0),
        strategy: Mutex::new(source.strategy()),
        limit_hit: AtomicBool::new(false),
        excluded,
        max_dirs,
    };
    shared.arena.lock().unwrap().push(Slot {
        parent: None,
        dir,
        depth: 0,
        own: Sums::ZERO,
        listed: false,
        sums: Sums::ZERO,
        complete: false,
        ticket,
    });
    let (worker_result, writer_result) = std::thread::scope(|scope| {
        let write = scope.spawn(|| writer(root.root_id(), run, sink.as_ref(), rx, &failed));
        let mut result = Ok(());
        let mut reconciled = false;
        for attempt in 0..3 {
            let workers: Vec<_> = (0..options.workers)
                .map(|_| {
                    scope.spawn(|| {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            shared.worker()
                        }))
                        .unwrap_or_else(|_| {
                            Err(EngineError::Internal {
                                message: "scan worker panicked".into(),
                                detail: None,
                            })
                        });
                        if result.is_err() {
                            failed.store(true, Ordering::Release);
                            shared.ready.notify_all();
                        }
                        result
                    })
                })
                .collect();
            for w in workers {
                if let Err(e) = w.join().expect("worker catches panic") {
                    if result.is_ok() {
                        result = Err(e);
                    }
                }
            }
            if result.is_err() || shared.stopping() {
                break;
            }
            let dirty = match (|| {
                let mut dirty = shared.dirty_directories()?;
                if dirty.is_empty() {
                    shared.finalize()?;
                    dirty = shared.dirty_directories()?;
                }
                Ok::<_, EngineError>(dirty)
            })() {
                Ok(dirs) => dirs,
                Err(e) => {
                    result = Err(e);
                    break;
                }
            };
            if dirty.is_empty() {
                reconciled = true;
                break;
            }
            if attempt == 2 {
                break;
            }
            let mut arena = shared.arena.lock().unwrap();
            let mut tasks = shared.tasks.lock().unwrap();
            tasks.done = false;
            for dir in dirty {
                let Some(&index) = shared.catalog_dirs.lock().unwrap().get(&dir) else {
                    result = Err(EngineError::Internal {
                        message: "unknown dirty directory".into(),
                        detail: None,
                    });
                    break;
                };
                let slot = &mut arena[index];
                slot.ticket = match sink.refresh_listing(root.root_id(), run, slot.ticket) {
                    Ok(ticket) => ticket,
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                };
                tasks.queue.push_back(Task {
                    dir: slot.dir.clone(),
                    slot: index,
                    depth: slot.depth,
                });
            }
            if result.is_err() {
                break;
            }
        }
        if !reconciled {
            shared.arena.lock().unwrap()[0].complete = false;
        }
        let _ = shared.tx.send(Envelope {
            command: Command::Stop,
            _permit: None,
        });
        (
            result,
            write.join().unwrap_or_else(|_| {
                Err(EngineError::Internal {
                    message: "scan writer panicked".into(),
                    detail: None,
                })
            }),
        )
    });
    let slot = &shared.arena.lock().unwrap()[0];
    let complete = slot.complete
        && !cancel.load(Ordering::Acquire)
        && worker_result.is_ok()
        && writer_result.is_ok();
    let finish = sink.finish_run(root.root_id(), run, &options.scope, complete);
    let persistence_seconds = writer_result?;
    if !cancel.load(Ordering::Acquire) {
        worker_result?;
    }
    finish?;
    let strategy = *shared.strategy.lock().unwrap();
    Ok(ScanReport {
        totals: slot.sums,
        complete,
        strategy,
        limit_hit: shared.limit_hit.load(Ordering::Acquire),
        examined: shared
            .examined
            .load(Ordering::Relaxed)
            .min(options.max_entries),
        skipped: shared.skipped.load(Ordering::Relaxed),
        failed: shared.failures.load(Ordering::Relaxed),
        elapsed_seconds: start.elapsed().as_secs_f64(),
        enumeration_worker_seconds: shared.enumeration_ns.load(Ordering::Relaxed) as f64 / 1e9,
        persistence_seconds,
    })
}
