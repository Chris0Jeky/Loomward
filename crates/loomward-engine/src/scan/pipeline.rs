use super::{sink::*, source::*, GrantedRoot};
#[cfg(test)]
#[path = "publication_tests.rs"]
mod publication_tests;
#[cfg(test)]
#[path = "targeted_tests.rs"]
mod targeted_tests;
#[cfg(test)]
#[path = "w2_tests.rs"]
mod w2_tests;
use crate::{
    budgets::{ByteBudget, BytePermit},
    EngineError, EngineResult,
};
use loomward_catalog::MAX_DIRECTORY_CHAIN_LENGTH;
use loomward_protocol::{RootId, ScanLimitReason};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
const CHUNK_BYTES: usize = 128 * 1024;
const CHUNK_ENTRIES: usize = 256;
const CHUNK_ENTRY_BYTES: usize = CHUNK_ENTRIES * std::mem::size_of::<Entry>();
const TASK_CAPACITY: usize = 1024;
/// Finite scan caps; the memory budget additionally bounds the directory arena.
#[derive(Clone, Debug)]
pub struct ScanOptions {
    /// Own enumeration worker count, 1..=32.
    pub workers: usize,
    /// Entire-run enumeration-attempt limit, including relists.
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
    /// Coverage describes this run's observations; changes after its final drain are unknown.
    pub complete: bool,
    /// Least capable actual strategy observed across this traversal.
    pub strategy: Strategy,
    /// A finite entry, depth or directory limit was reached.
    pub limit_hit: bool,
    /// The named cause when one applies (depth, or the directory arena); the entry budget retains
    /// `limit_hit` only.
    pub limit_reason: Option<ScanLimitReason>,
    /// Entries in each directory's latest listing, including excluded metadata.
    pub examined: u64,
    /// Enumeration attempts across all listings, including relists; used for the run limit.
    pub enumeration_attempts: u64,
    /// Deliberately skipped entries.
    pub skipped: u64,
    /// Failed child opens or listings.
    pub failed: u64,
    /// Unique directory identities sighted twice in one listing or under another parent.
    pub duplicate_identity: u64,
    /// Wall-clock elapsed seconds, including parallel overlap.
    pub elapsed_seconds: f64,
    /// Sum of worker time spent enumerating and handing off chunks (not exclusive CPU time).
    pub enumeration_worker_seconds: f64,
    /// Wall time through the final drained ListingDone (overlaps writer staging).
    pub enumeration_wall_seconds: f64,
    /// Writer time in listing preparation/staging/publication, excluding DirFinal and barriers.
    pub staging_seconds: f64,
    /// Coordinator time in bottom-up finalization and root finish, including writer waits.
    pub aggregation_seconds: f64,
    /// Sum of writer time inside sink staging/publication methods.
    pub persistence_seconds: f64,
}
impl ScanReport {
    /// Job coverage, distinct from the traversal EOF used for reconciliation.
    pub fn coverage_complete(&self) -> bool {
        self.complete && self.skipped == 0
    }
}
/// `discovered` value while a sighted directory's slot is still being created.
const PENDING: usize = usize::MAX;

#[cfg(test)]
type PublishHook = Box<dyn FnMut(&str)>;
#[cfg(test)]
thread_local! {
    /// Test hook run on the scan thread where coverage is about to be published.
    pub(super) static BEFORE_PUBLISH: std::cell::RefCell<Option<PublishHook>> =
        const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(super) fn before_publish(at: &str) {
    let taken = BEFORE_PUBLISH.with_borrow_mut(Option::take);
    if let Some(mut hook) = taken {
        hook(at);
        BEFORE_PUBLISH.with_borrow_mut(|slot| *slot = slot.take().or(Some(hook)));
    }
}

#[cfg(test)]
#[test]
fn directory_limit_releases_pending_identity_before_parent_relist() {
    use super::tests::{identity, Tree};
    let ticket = ListingTicket {
        dir: 1,
        input_revision: 1,
        old_parent: None,
    };
    let (tx, rx) = mpsc::sync_channel::<Envelope>(32);
    let shared = Shared {
        source: &Tree,
        root_identity: identity(1),
        round: AtomicU32::new(1),
        run: 1,
        options: ScanOptions::default(),
        tasks: Mutex::new(Tasks {
            queue: VecDeque::new(),
            active: 0,
            done: false,
        }),
        ready: Condvar::new(),
        arena: Mutex::new(vec![Slot {
            parent: None,
            listings: 0,
            seen: 0,
            dir: Some(Arc::new(1)),
            identity: reopen_key(identity(1)),
            round: 1,
            counts: [0; 3],
            depth: 0,
            own: Sums::ZERO,
            listed: false,
            sums: Sums::ZERO,
            complete: false,
            ticket,
        }]),
        discovered: Mutex::new(HashMap::new()),
        catalog_dirs: Mutex::new(HashMap::from([(1, 0)])),
        tx,
        bytes: Arc::new(ByteBudget::new(crate::budgets::SCAN_BYTES)),
        cancel: Default::default(),
        writer_failed: Default::default(),
        attempted: Default::default(),
        duplicate_identity: Default::default(),
        enumeration_ns: Default::default(),
        strategy: Mutex::new(Strategy::Portable),
        limit_hit: Default::default(),
        depth_limit_hit: Default::default(),
        directory_limit_hit: Default::default(),
        excluded: vec![],
        max_dirs: 1,
        held: Default::default(),
        arena_bytes: crate::budgets::SCAN_BYTES,
        node_bytes: 1,
        handle_bytes: 1,
        handles: AtomicUsize::new(1),
        bounded: None,
    };
    std::thread::scope(|threads| {
        let writer = threads.spawn(move || {
            while let Ok(envelope) = rx.recv_timeout(Duration::from_secs(2)) {
                match envelope.command {
                    Command::Prepare { reply, .. } => {
                        reply.send(Ok(ListingTicket { dir: 2, ..ticket })).unwrap()
                    }
                    Command::Stop => break,
                    _ => (),
                }
            }
        });
        let key = directory_key(identity(2)).unwrap();
        shared.discovered.lock().unwrap().insert(key, PENDING);
        let task = || Task {
            dir: Arc::new(1),
            slot: 0,
            depth: 0,
        };
        assert!(!shared
            .adopt(&task(), ticket, 1, &[120], 2, identity(2), Some(key))
            .unwrap());
        let adopted_marker = shared.discovered.lock().unwrap().get(&key).copied();
        // Exercise the earlier reservation limit on two successive parent listings as well.
        shared.discovered.lock().unwrap().remove(&key);
        shared.visit(task()).unwrap();
        shared.visit(task()).unwrap();
        shared
            .tx
            .send(Envelope {
                command: Command::Stop,
                _permit: None,
            })
            .unwrap();
        writer.join().unwrap();
        assert_eq!(
            adopted_marker, None,
            "arena rejection must release the pending identity"
        );
        assert_eq!(
            shared.duplicate_identity.load(Ordering::Relaxed),
            0,
            "a rejected child is not a duplicate on relist"
        );
        assert!(shared.limit_hit.load(Ordering::Acquire));
    });
}
struct Slot<D> {
    parent: Option<usize>,
    /// Listings of this directory started so far.
    listings: u32,
    /// The parent's listing number in which this directory was last sighted.
    seen: u32,
    dir: Option<Arc<D>>,
    identity: Option<(u64, FileIdObs)>,
    round: u32,
    /// Latest listing's examined, skipped and failed counts.
    counts: [u64; 3],
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
    root_identity: OpenedIdentity,
    round: AtomicU32,
    run: u64,
    options: ScanOptions,
    tasks: Mutex<Tasks<S::Dir>>,
    ready: Condvar,
    arena: Mutex<Vec<Slot<S::Dir>>>,
    discovered: Mutex<HashMap<(Option<u64>, FileIdObs), usize>>,
    catalog_dirs: Mutex<HashMap<u64, usize>>,
    tx: SyncSender<Envelope>,
    bytes: Arc<ByteBudget>,
    cancel: Arc<AtomicBool>,
    writer_failed: Arc<AtomicBool>,
    attempted: AtomicU64,
    duplicate_identity: AtomicU64,
    enumeration_ns: AtomicU64,
    strategy: Mutex<Strategy>,
    limit_hit: AtomicBool,
    depth_limit_hit: AtomicBool,
    /// Set only by the directory-slot cap, which names its cause.
    directory_limit_hit: AtomicBool,
    excluded: Vec<OpenedIdentity>,
    max_dirs: usize,
    /// Child directories sighted by a listing but not yet adopted, across all workers:
    /// `(count, name bytes)`. Their names are charged to the arena reservation.
    held: Mutex<(usize, usize)>,
    arena_bytes: usize,
    node_bytes: usize,
    handle_bytes: usize,
    handles: AtomicUsize,
    /// A bounded targeted run (#782): its sink, root and named directories. Children outside the
    /// named set are listed only when the catalogue holds no complete listing of them.
    bounded: Option<(&'a dyn ScanSink, &'a RootId, HashSet<u64>)>,
}
/// One held child's share of [`Shared::held`], returned when the child is adopted or dropped.
struct Held<'a>(&'a Mutex<(usize, usize)>, usize);
impl Drop for Held<'_> {
    fn drop(&mut self) {
        let mut held = self.0.lock().unwrap();
        held.0 -= 1;
        held.1 -= self.1;
    }
}
impl<S: DirSource> Shared<'_, S> {
    /// Hold one more sighted child if the arena reservation still covers every slot and held
    /// child at the per-node estimate plus every held name, and the directory cap allows it.
    fn hold(&self, name_bytes: usize) -> Option<Held<'_>> {
        let arena = self.arena.lock().unwrap();
        let slots = arena.len();
        let handles = self.handles.load(Ordering::Relaxed);
        let mut held = self.held.lock().unwrap();
        let fits = slots + held.0 < self.max_dirs
            && (slots + held.0 + 1) * self.node_bytes
                + (handles + held.0 + 1) * self.handle_bytes
                + held.1
                + name_bytes
                <= self.arena_bytes;
        if !fits {
            // Refusing a sighted child is the directory arena's cap, whether by count or bytes.
            self.directory_limit_hit.store(true, Ordering::Release);
            return None;
        }
        held.0 += 1;
        held.1 += name_bytes;
        Some(Held(&self.held, name_bytes))
    }
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
    fn finish_slot(
        &self,
        index: usize,
        own: Sums,
        complete: bool,
        counts: [u64; 3],
    ) -> EngineResult<()> {
        let mut arena = self.arena.lock().unwrap();
        arena[index].own = own;
        arena[index].listed = complete;
        arena[index].counts = counts;
        if arena[index]
            .dir
            .as_ref()
            .zip(arena[index].identity)
            .is_some_and(|(dir, key)| self.source.can_reopen(dir, reopen_identity(key)))
            // The root is the grant anchor, not a pathname to reopen after a rename.
            && index != 0
            && arena[index].dir.take().is_some()
        {
            self.handles.fetch_sub(1, Ordering::Relaxed);
        }
        Ok(())
    }
    /// Aggregate every slot bottom-up; `send` also publishes each directory's final.
    fn finalize(&self, send: bool) -> EngineResult<()> {
        let mut arena = self.arena.lock().unwrap();
        for slot in arena.iter_mut() {
            slot.sums = slot.own;
            slot.complete = slot.listed;
        }
        for index in (0..arena.len()).rev() {
            let slot = &arena[index];
            let (parent, seen, ticket, sums, complete) = (
                slot.parent,
                slot.seen,
                slot.ticket,
                slot.sums,
                slot.complete,
            );
            if send
                && !self.message(ScanMessage::DirFinal {
                    run: self.run,
                    ticket,
                    sums,
                    complete,
                })
            {
                return Err(cancelled());
            }
            if let Some(parent) = parent {
                let p = &mut arena[parent];
                p.sums = p.sums.checked_add(sums.checked_add(Sums {
                    dirs: 1,
                    ..Sums::ZERO
                })?)?;
                // A child missing from its parent's latest listing was deleted or moved during
                // the run: the parent's aggregate cannot claim to be complete.
                p.complete &= complete && seen == p.listings;
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
        let (ticket, listing) = {
            let mut arena = self.arena.lock().unwrap();
            let slot = &mut arena[task.slot];
            slot.listings += 1;
            (slot.ticket, slot.listings)
        };
        let mut counts = [0u64; 3];
        let mut own = Sums::ZERO;
        let mut complete = true;
        let mut entries = Vec::new();
        let mut permit = None;
        let mut chunk_bytes = CHUNK_ENTRY_BYTES;
        let mut count = 0u64;
        let mut stopped = None;
        let mut error = None;
        let mut children = Vec::new();
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
                || self.attempted.fetch_add(1, Ordering::Relaxed) >= self.options.max_entries
            {
                stopped = Some(IncompleteReason::Limit);
                return Flow::Stop;
            }
            counts[0] += 1;
            if raw.attributes & 0x10 != 0 && task.depth + 1 >= MAX_DIRECTORY_CHAIN_LENGTH {
                // The writer cannot represent another directory, even an excluded/unlisted one.
                complete = false;
                stopped = Some(IncompleteReason::Limit);
                self.limit_hit.store(true, Ordering::Release);
                self.depth_limit_hit.store(true, Ordering::Release);
                counts[1] += 1;
                return Flow::Continue;
            }
            let mut excluded = refused_attributes(raw.attributes)
                || raw.name == [46, 108, 111, 111, 109, 119, 97, 114, 100];
            let mut child = None;
            let mut traversal_error = None;
            if !excluded && raw.attributes & 0x10 != 0 {
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
                        counts[2] += 1;
                    }
                }
            }
            if excluded {
                counts[1] += 1;
            }
            let entry = Entry {
                name: raw.name.to_vec(),
                // Name-only listings can still obtain a verified identity from the child open.
                file_id: child.as_ref().map_or(raw.file_id, |(_, id)| id.id),
                identity_eligible: child.as_ref().map_or_else(
                    || {
                        raw.file_id.is_some()
                            && !raw.file_id.is_some_and(|id| {
                                matches!(id, FileIdObs::Id64(_)) && self.source.is_refs(&task.dir)
                            })
                    },
                    |(_, id)| matches!(id.basis, IdBasis::Listed | IdBasis::PostOpen),
                ),
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
                let key = directory_key(identity);
                if let Some(key) = key {
                    let known = {
                        let mut discovered = self.discovered.lock().unwrap();
                        let known = discovered.get(&key).copied();
                        discovered.entry(key).or_insert(PENDING);
                        known
                    };
                    if let Some(index) = known {
                        let mut arena = self.arena.lock().unwrap();
                        if index != PENDING
                            && arena[index].parent == Some(task.slot)
                            && arena[index].seen != listing
                        {
                            arena[index].seen = listing;
                        } else if self.bounded.is_some()
                            && index != PENDING
                            && index != 0
                            && arena[index].parent.is_none()
                        {
                            // A named directory listed on its own: this aggregate cannot claim it.
                            complete = false;
                        } else {
                            // Already sighted under another parent: it moved during the run,
                            // and this listing's aggregate cannot attribute it.
                            complete = false;
                            self.duplicate_identity.fetch_add(1, Ordering::Relaxed);
                        }
                        return Flow::Continue;
                    }
                }
                // Children are prepared only after this listing's ListingDone: a durable writer
                // creates child rows when the parent listing publishes, so a child ticket must not
                // be requested before then. Held handles and names, across all workers, count
                // against the arena reservation.
                let Some(held) = self.hold(raw.name.len() * 2) else {
                    if let Some(key) = key {
                        self.discovered.lock().unwrap().remove(&key);
                    }
                    complete = false;
                    stopped = Some(IncompleteReason::Limit);
                    return Flow::Stop;
                };
                children.push((raw.name.to_vec(), dir, identity, key, held));
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
        // A bounded run lists only named directories and children it verified as unlisted.
        if outcome == ListOutcome::Complete
            && self.bounded.is_none()
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
            if !matches!(
                outcome,
                ListOutcome::Incomplete(IncompleteReason::Limit | IncompleteReason::OutsideScope)
            ) {
                counts[2] += 1;
            }
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
        for (name, dir, identity, key, _held) in children {
            if !self.adopt(&task, ticket, listing, &name, dir, identity, key)? {
                complete = false;
            }
        }
        self.finish_slot(task.slot, own, complete, counts)
    }
    /// Prepare one child sighted by a published listing and queue it. `Ok(false)` means the
    /// parent's aggregate cannot claim this child (moved, unpublished or over the arena cap).
    #[allow(clippy::too_many_arguments)]
    fn adopt(
        &self,
        task: &Task<S::Dir>,
        ticket: ListingTicket,
        listing: u32,
        name: &[u16],
        dir: S::Dir,
        identity: OpenedIdentity,
        key: Option<(Option<u64>, FileIdObs)>,
    ) -> EngineResult<bool> {
        let child_ticket = match self.prepare(Some(ticket.dir), name, identity) {
            Ok(t) => t,
            // The parent's publication was rejected as obsolete: the child is unknown to the
            // writer this epoch, and the writer marked the parent dirty for a relist. Release
            // the sighting so the relist adopts the child instead of seeing it as moved.
            Err(e) if listing_revision_changed(&e) => {
                if let Some(key) = key {
                    let mut discovered = self.discovered.lock().unwrap();
                    if discovered.get(&key) == Some(&PENDING) {
                        discovered.remove(&key);
                    }
                }
                return Ok(false);
            }
            Err(e) => return Err(e),
        };
        if let Some(old_parent) = child_ticket.old_parent {
            if !self.message(ScanMessage::InvalidateChains {
                run: self.run,
                old_parent,
                new_parent: ticket.dir,
            }) {
                return Err(cancelled());
            }
        }
        let slot = {
            let mut arena = self.arena.lock().unwrap();
            if let Some(&index) = self.catalog_dirs.lock().unwrap().get(&child_ticket.dir) {
                // Name-only identities can recur when their parent is relisted.
                let claimed = arena[index].parent == Some(task.slot);
                if claimed {
                    arena[index].seen = listing;
                }
                arena[index].ticket = child_ticket;
                arena[index].identity = reopen_key(identity);
                if let Some(key) = key {
                    self.discovered.lock().unwrap().insert(key, index);
                }
                if arena[index].round != self.round.load(Ordering::Relaxed) {
                    // Use the freshly validated sighting, never the slot's old name-only handle.
                    let dir = Arc::new(dir);
                    if arena[index].dir.is_none() {
                        self.handles.fetch_add(1, Ordering::Relaxed);
                    }
                    arena[index].dir = Some(dir.clone());
                    arena[index].round = self.round.load(Ordering::Relaxed);
                    self.tasks.lock().unwrap().queue.push_back(Task {
                        dir,
                        slot: index,
                        depth: arena[index].depth,
                    });
                    self.ready.notify_one();
                }
                return Ok(claimed);
            }
            if let Some((sink, root, named)) = &self.bounded {
                if !named.contains(&child_ticket.dir)
                    && !sink.needs_listing(root, self.run, child_ticket.dir)
                {
                    // Kept unchanged by the published parent listing; never claimed as covered.
                    if let Some(key) = key {
                        self.discovered.lock().unwrap().remove(&key);
                    }
                    return Ok(false);
                }
            }
            if arena.len() >= self.max_dirs {
                if let Some(key) = key {
                    self.discovered.lock().unwrap().remove(&key);
                }
                self.limit_hit.store(true, Ordering::Release);
                self.directory_limit_hit.store(true, Ordering::Release);
                return Ok(false);
            }
            let index = arena.len();
            self.handles.fetch_add(1, Ordering::Relaxed);
            arena.push(Slot {
                parent: Some(task.slot),
                listings: 0,
                seen: listing,
                dir: Some(Arc::new(dir)),
                identity: reopen_key(identity),
                round: self.round.load(Ordering::Relaxed),
                counts: [0; 3],
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
        if let Some(key) = key {
            self.discovered.lock().unwrap().insert(key, slot);
        }
        // The arena reservation also covers every queued handle/task: no recursive
        // enumeration retains another native buffer or parent callback on the stack.
        let dir = self.arena.lock().unwrap()[slot]
            .dir
            .as_ref()
            .unwrap()
            .clone();
        self.tasks.lock().unwrap().queue.push_back(Task {
            dir,
            slot,
            depth: task.depth + 1,
        });
        self.ready.notify_one();
        Ok(true)
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
fn reopen_key(identity: OpenedIdentity) -> Option<(u64, FileIdObs)> {
    directory_key(identity).and_then(|(volume, id)| volume.map(|volume| (volume, id)))
}
fn reopen_identity((volume, id): (u64, FileIdObs)) -> OpenedIdentity {
    OpenedIdentity {
        id: Some(id),
        volume_guid: None,
        volume_serial: Some(volume),
        basis: IdBasis::PostOpen,
        attributes: 0,
        reparse_tag: None,
    }
}
fn listing_revision_changed(e: &EngineError) -> bool {
    matches!(e, EngineError::StaleGeneration { detail: Some(detail), .. }
        if detail.get("reason") == Some(&serde_json::json!("listing_revision_changed")))
}
fn writer(
    root: &RootId,
    run: u64,
    sink: &dyn ScanSink,
    rx: Receiver<Envelope>,
    failed: &AtomicBool,
) -> EngineResult<(f64, f64)> {
    let mut seconds = 0.;
    let mut staging_seconds = 0.;
    let mut first_error = None;
    let mut dirty = HashSet::new();
    // Prepares, barriers and the stop that a prepare's drain overtook, in arrival order.
    let mut held: VecDeque<Envelope> = VecDeque::new();
    let mut handle = |e: Envelope| -> bool {
        if matches!(e.command, Command::Stop) {
            return false;
        }
        if first_error.is_some() {
            return true;
        }
        let start = Instant::now();
        let staging = matches!(
            &e.command,
            Command::Prepare { .. }
                | Command::Message(
                    ScanMessage::DirListing { .. } | ScanMessage::ListingDone { .. }
                )
        );
        let result = match e.command {
            Command::Prepare {
                parent,
                name,
                identity,
                reply,
            } => {
                let r = sink.prepare_listing(root, run, parent, &name, identity);
                let failed = match (&r, parent) {
                    // The parent's publication was obsolete, so the writer does not know this
                    // child yet: relist the parent; the worker leaves the child unclaimed.
                    (Err(e), Some(parent)) if listing_revision_changed(e) => {
                        dirty.insert(parent);
                        None
                    }
                    (r, _) => r.as_ref().err().cloned(),
                };
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
                    return true;
                }
                match sink.consume(root, m) {
                    Err(e) if ticket.is_some() && listing_revision_changed(&e) => {
                        dirty.insert(ticket.unwrap().dir);
                        Ok(())
                    }
                    result => result,
                }
            }
            Command::Barrier(reply) => {
                // Deferred steps settle here, so an obsolete one is relisted like a refusal.
                let settled = sink.settle(root).map(|stale| dirty.extend(stale));
                let _ = reply.send(dirty.drain().collect());
                settled
            }
            Command::Stop => unreachable!(),
        };
        let elapsed = start.elapsed().as_secs_f64();
        seconds += elapsed;
        if staging {
            staging_seconds += elapsed;
        }
        if let Err(error) = result {
            failed.store(true, Ordering::Release);
            first_error = Some(error);
        }
        true
    };
    loop {
        let Some(e) = held.pop_front().or_else(|| rx.recv().ok()) else {
            break;
        };
        // A child prepare can wait for its parent's publication to commit (#148). Messages
        // already queued behind it belong to other workers (the preparing worker is blocked), so
        // they reach the sink first and an asynchronous sink can commit them together. Messages
        // keep their order; nothing overtakes a barrier or the stop.
        if matches!(e.command, Command::Prepare { .. }) {
            while held
                .iter()
                .all(|h| matches!(h.command, Command::Prepare { .. }))
            {
                let Ok(next) = rx.try_recv() else { break };
                if matches!(next.command, Command::Message(_)) {
                    handle(next);
                } else {
                    held.push_back(next);
                }
            }
        }
        if !handle(e) {
            break;
        }
    }
    first_error.map_or(Ok((seconds, staging_seconds)), Err)
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
    run_scan_inner(source, root, run, sink, bytes, cancel, options, None)
}

/// The plain job path; watched repair passes retain their separate publication boundary.
#[cfg(any(windows, test))]
pub(super) fn run_job_scan<S: DirSource>(
    source: &S,
    root: &GrantedRoot,
    run: u64,
    sink: Arc<dyn ScanSink>,
    bytes: Arc<ByteBudget>,
    job: &crate::jobs::JobContext,
    options: ScanOptions,
) -> EngineResult<ScanReport> {
    run_scan_inner(
        source,
        root,
        run,
        sink,
        bytes,
        job.cancel.clone(),
        options,
        Some(job),
    )
}

/// Claim the terminal publication outcome without keeping a cancellation gate across sink I/O.
/// This commits the decision to finish, not a successful write: a later sink error still fails
/// the job. Once claimed, ordinary cancellation returns the current job state without acknowledging
/// cancellation that the catalogue can no longer honor. Root revocation still withdraws the job's
/// coverage. All workers are drained before this call.
fn admit_final_publication(job: &crate::jobs::JobContext) -> EngineResult<bool> {
    job.commit_if_not_cancelled(|| Ok(()))
}

#[allow(clippy::too_many_arguments)]
fn run_scan_inner<S: DirSource>(
    source: &S,
    root: &GrantedRoot,
    run: u64,
    sink: Arc<dyn ScanSink>,
    bytes: Arc<ByteBudget>,
    cancel: Arc<AtomicBool>,
    options: ScanOptions,
    job: Option<&crate::jobs::JobContext>,
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
    let node_bytes = std::mem::size_of::<Slot<S::Dir>>() + 128; // Identity and catalogue map buckets.
    let handle_bytes = std::mem::size_of::<Task<S::Dir>>()
        + std::mem::size_of::<S::Dir>()
        + 2 * std::mem::size_of::<usize>();
    let max_dirs = options.max_dirs.min(arena_bytes / node_bytes);
    let (dir, identity) =
        source
            .open_root(root.path())
            .map_err(|_| EngineError::PermissionDenied {
                message: "root open refused".into(),
            })?;
    // The basis is part of the identity: the same IDs under another basis no longer bind what
    // the grant bound (a verified root reading unverified, or the reverse), docs/41 admission.
    if root.expected.is_some_and(|e| {
        e.id != identity.id
            || e.volume_serial != identity.volume_serial
            || e.volume_guid != identity.volume_guid
            || e.basis != identity.basis
    }) {
        sink.root_identity_changed(root.root_id());
        return Err(EngineError::PermissionDenied {
            message: "root_identity_changed".into(),
        });
    }
    // A state path that cannot be opened or carries no ID fails closed: excluding nothing would
    // count Loomward's own state as data and still call the coverage complete.
    let excluded = match &options.excluded_path {
        Some(path) => vec![source
            .open_root(path)
            .ok()
            .map(|(_, state)| state)
            .filter(|state| state.id.is_some())
            .ok_or_else(|| EngineError::PermissionDenied {
                message: "state exclusion identity unavailable".into(),
            })?],
        None => Vec::new(),
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
    let dir = Arc::new(dir);
    // #782: a targeted run lists only its named directories when every one of them can be
    // reopened below the pinned root, or is verified by a named parent's listing. Otherwise it
    // traverses the whole root as before.
    let mut targets = Vec::new();
    let mut named = HashSet::new();
    if let RunScope::Targeted(dirs) = &options.scope {
        named.extend(dirs.iter().copied());
        let mut bounded = identity.volume_serial.is_some();
        for &target in named.iter().filter(|&&d| d != ticket.dir) {
            // The root plus every retained target is a slot and a held handle: the same
            // node-plus-handle reservation normal traversal checks before holding a child.
            let slots = targets.len() + 2;
            if !bounded || slots > max_dirs || slots * (node_bytes + handle_bytes) > arena_bytes {
                bounded = false;
                break;
            }
            let reserved = match sink.prepare_target(root.root_id(), run, target) {
                Ok(t) => t,
                Err(e) => {
                    let _ = sink.finish_run(root.root_id(), run, &options.scope, false);
                    return Err(e);
                }
            };
            let Some(t) = reserved else {
                bounded = false;
                break;
            };
            let expected = reopen_identity((identity.volume_serial.unwrap(), t.id));
            match source.reopen(&dir, identity, expected) {
                Ok((handle, opened))
                    if opened.id == expected.id
                        && opened.volume_serial == expected.volume_serial =>
                {
                    targets.push((t, handle, opened));
                }
                // Gone or unreachable: a named parent's complete listing settles it.
                _ if t.parent.is_some_and(|p| named.contains(&p)) => {}
                _ => bounded = false,
            }
        }
        if !bounded {
            targets.clear();
            named.clear();
        }
    }
    let bounded = !named.is_empty();
    // Parents before children, so the bottom-up finalisation sees every child first.
    targets.sort_by_key(|(t, _, _)| t.depth);
    let (tx, rx) = mpsc::sync_channel(32);
    let mut queue = VecDeque::with_capacity(max_dirs);
    if !bounded || named.contains(&ticket.dir) {
        queue.push_back(Task {
            dir: dir.clone(),
            slot: 0,
            depth: 0,
        });
    }
    let shared = Shared {
        root_identity: identity,
        round: AtomicU32::new(1),
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
        discovered: Mutex::new(
            directory_key(identity)
                .into_iter()
                .map(|k| (k, 0))
                .collect(),
        ),
        catalog_dirs: Mutex::new(HashMap::from([(ticket.dir, 0)])),
        tx,
        bytes: bytes.clone(),
        cancel: cancel.clone(),
        writer_failed: failed.clone(),
        attempted: AtomicU64::new(0),
        duplicate_identity: AtomicU64::new(0),
        enumeration_ns: AtomicU64::new(0),
        strategy: Mutex::new(source.strategy()),
        limit_hit: AtomicBool::new(false),
        depth_limit_hit: AtomicBool::new(false),
        directory_limit_hit: AtomicBool::new(false),
        excluded,
        max_dirs,
        held: Mutex::new((0, 0)),
        arena_bytes,
        node_bytes,
        handle_bytes,
        handles: AtomicUsize::new(1),
        bounded: bounded.then(|| (sink.as_ref(), root.root_id(), named)),
    };
    shared.arena.lock().unwrap().push(Slot {
        parent: None,
        listings: 0,
        seen: 0,
        dir: Some(dir),
        identity: reopen_key(identity),
        round: 1,
        counts: [0; 3],
        depth: 0,
        own: Sums::ZERO,
        listed: false,
        sums: Sums::ZERO,
        complete: false,
        ticket,
    });
    {
        let mut arena = shared.arena.lock().unwrap();
        let mut catalog_dirs = shared.catalog_dirs.lock().unwrap();
        let mut discovered = shared.discovered.lock().unwrap();
        let mut tasks = shared.tasks.lock().unwrap();
        for (t, handle, opened) in targets {
            let index = arena.len();
            let handle = Arc::new(handle);
            arena.push(Slot {
                // Linked only to an earlier slot: an unlisted root, or a named parent.
                parent: t.parent.and_then(|p| catalog_dirs.get(&p).copied()),
                listings: 0,
                seen: 0,
                dir: Some(handle.clone()),
                identity: reopen_key(opened),
                round: 1,
                counts: [0; 3],
                depth: t.depth,
                own: Sums::ZERO,
                listed: false,
                sums: Sums::ZERO,
                complete: false,
                ticket: t.ticket,
            });
            catalog_dirs.insert(t.ticket.dir, index);
            if let Some(key) = directory_key(opened) {
                discovered.insert(key, index);
            }
            shared.handles.fetch_add(1, Ordering::Relaxed);
            tasks.queue.push_back(Task {
                dir: handle,
                slot: index,
                depth: t.depth,
            });
        }
    }
    let mut enumeration_wall_seconds = 0.;
    let mut aggregation_seconds = 0.;
    let (worker_result, writer_result, reconciled) = std::thread::scope(|scope| {
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
                enumeration_wall_seconds = start.elapsed().as_secs_f64();
                if dirty.is_empty() {
                    let aggregation = Instant::now();
                    let finalization = shared.finalize(true);
                    finalization?;
                    dirty = shared.dirty_directories()?;
                    aggregation_seconds += aggregation.elapsed().as_secs_f64();
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
            // Workers are joined before the coordinator takes arena -> tasks -> catalog_dirs;
            // workers take arena -> catalog_dirs -> discovered -> tasks and never reverse it.
            shared.round.fetch_add(1, Ordering::Relaxed);
            let mut arena = shared.arena.lock().unwrap();
            let root_dir = arena[0].dir.as_ref().unwrap().clone();
            let mut tasks = shared.tasks.lock().unwrap();
            tasks.done = false;
            let slot_bytes = arena.len() * shared.node_bytes;
            for dir in dirty {
                if shared.stopping() {
                    break;
                }
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
                slot.round = shared.round.load(Ordering::Relaxed);
                let dir = match &slot.dir {
                    Some(dir) => dir.clone(),
                    None => {
                        if slot_bytes
                            + (shared.handles.load(Ordering::Relaxed) + 1) * shared.handle_bytes
                            > shared.arena_bytes
                        {
                            shared.limit_hit.store(true, Ordering::Release);
                            shared.directory_limit_hit.store(true, Ordering::Release);
                            slot.listed = false;
                            continue;
                        }
                        let Some(identity) = slot.identity else {
                            slot.listed = false;
                            slot.counts[2] += 1;
                            continue;
                        };
                        let expected = reopen_identity(identity);
                        match source.reopen(&root_dir, shared.root_identity, expected) {
                            Ok((dir, identity))
                                if identity.id == expected.id
                                    && identity.volume_serial == expected.volume_serial =>
                            {
                                let dir = Arc::new(dir);
                                shared.handles.fetch_add(1, Ordering::Relaxed);
                                slot.dir = Some(dir.clone());
                                dir
                            }
                            _ => {
                                slot.listed = false;
                                slot.counts[2] += 1;
                                continue;
                            }
                        }
                    }
                };
                tasks.queue.push_back(Task {
                    dir,
                    slot: index,
                    depth: slot.depth,
                });
            }
            if result.is_err() {
                break;
            }
        }
        if !reconciled && !shared.arena.is_poisoned() {
            // Finals were never accepted: still report the partial sums of what was listed,
            // never a zero standing in for unknown.
            let aggregation = Instant::now();
            let finalization = shared.finalize(false);
            aggregation_seconds += aggregation.elapsed().as_secs_f64();
            if let Err(e) = finalization {
                if result.is_ok() {
                    result = Err(e);
                }
            }
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
            reconciled,
        )
    });
    // A worker panic fails the run; recover only to finish it as incomplete, never to resume work.
    let arena = shared
        .arena
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let slot = &arena[0];
    let counts = arena.iter().fold([0u64; 3], |mut totals, slot| {
        for (total, count) in totals.iter_mut().zip(slot.counts) {
            *total += count;
        }
        totals
    });
    #[cfg(test)]
    before_publish("run_scan");
    let drained = reconciled
        && shared.duplicate_identity.load(Ordering::Relaxed) == 0
        && !cancel.load(Ordering::Acquire)
        && worker_result.is_ok()
        && writer_result.is_ok();
    let limit_hit = shared.limit_hit.load(Ordering::Acquire);
    let settled = drained
        // Targeted freshness needs every visited listing, not full-root aggregate coverage.
        && (!matches!(options.scope, RunScope::Targeted(_))
            || (bounded && counts[1] == 0 && counts[2] == 0))
        && !limit_hit;
    let complete = slot.complete && settled;
    let limited = drained && limit_hit;
    #[cfg(test)]
    if job.is_some() {
        before_publish("run_scan_finish");
    }
    // The flag snapshot is only evidence. A job must also win terminal admission against
    // scan.cancel before it can publish completion, freshness, or a finished partial sample.
    let admitted = if settled || limited {
        match job {
            Some(job) => admit_final_publication(job)?,
            None => true,
        }
    } else {
        false
    };
    let complete = complete && admitted;
    let settled = settled && admitted;
    let limited = limited && admitted;
    let aggregation = Instant::now();
    let finish = sink.finish_traversal(
        root.root_id(),
        run,
        &options.scope,
        complete,
        settled,
        limited,
    );
    aggregation_seconds += aggregation.elapsed().as_secs_f64();
    let (persistence_seconds, staging_seconds) = writer_result?;
    if !cancel.load(Ordering::Acquire) {
        worker_result?;
    }
    finish?;
    let strategy = *shared.strategy.lock().unwrap();
    if std::env::var_os("LOOMWARD_SCAN_PROFILE").is_some() {
        let listings: u64 = arena.iter().map(|slot| u64::from(slot.listings)).sum();
        eprintln!(
            "scan-profile pipeline {}",
            serde_json::json!({
                "listing_calls": listings, "relist_calls": listings.saturating_sub(arena.len() as u64),
                "enumeration_attempts": shared.attempted.load(Ordering::Relaxed),
                "enumeration_worker_seconds": shared.enumeration_ns.load(Ordering::Relaxed) as f64 / 1e9,
                "enumeration_wall_seconds": enumeration_wall_seconds, "staging_seconds": staging_seconds,
                "persistence_seconds": persistence_seconds, "aggregation_seconds": aggregation_seconds,
                "final_directory_calls": arena.len(), "finish_run_calls": 1
            })
        );
    }
    let mut totals = slot.sums;
    for other in arena.iter().skip(1).filter(|s| s.parent.is_none()) {
        totals = totals.checked_add(other.sums.checked_add(Sums {
            dirs: 1,
            ..Sums::ZERO
        })?)?;
    }
    Ok(ScanReport {
        totals,
        complete,
        strategy,
        limit_hit,
        // Depth is the more specific cause when both caps were reached.
        limit_reason: if shared.depth_limit_hit.load(Ordering::Acquire) {
            Some(ScanLimitReason::DepthLimit)
        } else if shared.directory_limit_hit.load(Ordering::Acquire) {
            Some(ScanLimitReason::DirectoryLimit)
        } else {
            None
        },
        examined: counts[0],
        enumeration_attempts: shared
            .attempted
            .load(Ordering::Relaxed)
            .min(options.max_entries),
        skipped: counts[1],
        failed: counts[2],
        duplicate_identity: shared.duplicate_identity.load(Ordering::Relaxed),
        elapsed_seconds: start.elapsed().as_secs_f64(),
        enumeration_worker_seconds: shared.enumeration_ns.load(Ordering::Relaxed) as f64 / 1e9,
        enumeration_wall_seconds,
        staging_seconds,
        aggregation_seconds,
        persistence_seconds,
    })
}
