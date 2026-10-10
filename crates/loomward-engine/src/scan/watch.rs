//! Watch hints are dirty epochs, never authoritative changes or operation grants.
//! LW-007: the non-elevated directory-handle USN hypothesis was measured in
//! docs/research/change-tracking.md. Names are redacted and records escape the subtree.
//! No USN accelerator is enabled here; watchers plus relists remain the correctness path.
use super::{source::*, *};
use crate::budgets::ByteBudget;
use loomward_windows::watch::Change;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

type RelativeDir = Vec<Vec<u16>>;
const MAX_DIRTY: usize = 1024;
// Four: a directory moved during a full pass is noticed one pass later since #178 (the same-run
// dedupe sends no reparent signal), and a run must still fit the post-publication drain.
const MAX_RELISTS: usize = 4;

#[derive(Default)]
struct Dirty {
    epoch: u64,
    root: Option<u64>,
    dirs: BTreeMap<RelativeDir, u64>,
    stopped: bool,
    failed: bool,
    rename_pending: bool,
}
impl Dirty {
    fn record(&mut self, changes: Vec<Change>) {
        if self.stopped {
            return;
        }
        for change in changes {
            self.epoch += 1;
            match change {
                Change::RootDirty => {
                    self.root = Some(self.epoch);
                    self.dirs.clear();
                    self.rename_pending = false;
                }
                Change::Name { name, action } => {
                    if action == 4 {
                        if self.rename_pending {
                            self.root = Some(self.epoch);
                            self.dirs.clear();
                        }
                        self.rename_pending = true;
                    }
                    if action == 5 {
                        if !self.rename_pending {
                            self.root = Some(self.epoch);
                            self.dirs.clear();
                        }
                        self.rename_pending = false;
                    }
                    let mut path: RelativeDir =
                        name.split(|c| *c == 92).map(<[u16]>::to_vec).collect();
                    path.pop();
                    if self.root.is_some() {
                        self.root = Some(self.epoch);
                    } else if self.dirs.len() == MAX_DIRTY && !self.dirs.contains_key(&path) {
                        self.root = Some(self.epoch);
                        self.dirs.clear();
                    } else {
                        self.dirs.insert(path, self.epoch);
                        if self
                            .dirs
                            .keys()
                            .map(|p| p.iter().map(|n| n.len() * 2).sum::<usize>())
                            .sum::<usize>()
                            > 512 * 1024
                        {
                            self.root = Some(self.epoch);
                            self.dirs.clear();
                        }
                    }
                }
            }
        }
    }
    fn finish_hints(&mut self) {
        if self.rename_pending {
            self.record(vec![Change::RootDirty]);
        }
    }
    fn acknowledge(&mut self, covered_epoch: u64) {
        self.dirs.retain(|_, epoch| *epoch > covered_epoch);
        if self.root.is_some_and(|epoch| epoch <= covered_epoch) {
            self.root = None;
        }
    }
    fn clean(&self) -> bool {
        self.root.is_none() && self.dirs.is_empty() && !self.failed && !self.stopped
    }
}

#[cfg(windows)]
enum Work {
    Hints(Vec<Change>),
    Barrier(std::sync::mpsc::SyncSender<()>),
}

/// A granted-root watcher retained between scans, stopped on revocation and engine shutdown.
#[cfg(windows)]
pub(crate) struct RootWatch {
    root: GrantedRoot,
    native: loomward_windows::watch::Watcher,
    dirty: Arc<Mutex<Dirty>>,
    stop: Arc<AtomicBool>,
    sender: std::sync::mpsc::SyncSender<Work>,
    dispatcher: Mutex<Option<std::thread::JoinHandle<()>>>,
}
#[cfg(windows)]
impl RootWatch {
    pub(crate) fn start(root: &GrantedRoot, sink: Arc<dyn ScanSink>) -> EngineResult<Arc<Self>> {
        use std::{sync::mpsc, time::Duration};
        let dirty = Arc::new(Mutex::new(Dirty::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let overflow = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::sync_channel(16);
        let hints = sender.clone();
        let lost = overflow.clone();
        let native =
            loomward_windows::watch::Watcher::start(root.path(), root.expected, move |batch| {
                if hints.try_send(Work::Hints(batch)).is_err() {
                    lost.store(true, Ordering::Release);
                }
            })
            .map_err(|_| EngineError::PermissionDenied {
                message: "watch root open refused".into(),
            })?;
        sink.watch_dirty(root.root_id())?;
        let state = dirty.clone();
        let closed = stop.clone();
        let root_id = root.root_id().clone();
        let dispatcher = std::thread::spawn(move || {
            while !closed.load(Ordering::Acquire) {
                let work = receiver.recv_timeout(Duration::from_millis(10));
                let mut state = state.lock().unwrap();
                let mut changed = false;
                if overflow.swap(false, Ordering::AcqRel) {
                    state.record(vec![Change::RootDirty]);
                    changed = true;
                }
                match work {
                    Ok(Work::Hints(batch)) => {
                        state.record(batch);
                        changed = true;
                    }
                    Ok(Work::Barrier(reply)) => {
                        if state.rename_pending {
                            state.finish_hints();
                            changed = true;
                        }
                        if changed && sink.watch_dirty(&root_id).is_err() {
                            state.failed = true;
                        }
                        let _ = reply.send(());
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => (),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
                if changed && sink.watch_dirty(&root_id).is_err() {
                    state.failed = true;
                }
            }
        });
        Ok(Arc::new(Self {
            root: root.clone(),
            native,
            dirty,
            stop,
            sender,
            dispatcher: Mutex::new(Some(dispatcher)),
        }))
    }
    pub(crate) fn validate(&self, root: &GrantedRoot) -> EngineResult<()> {
        if self.root != *root {
            return Err(EngineError::PermissionDenied {
                message: "watch grant changed or revoked".into(),
            });
        }
        Ok(())
    }
    pub(crate) fn healthy(&self) -> bool {
        !self.stop.load(Ordering::Acquire)
            && self.checkpoint()
            && !self.dirty.lock().unwrap().failed
    }
    fn checkpoint(&self) -> bool {
        use std::{
            sync::mpsc,
            time::{Duration, Instant},
        };
        if self.native.checkpoint().is_err() {
            self.dirty.lock().unwrap().failed = true;
            return false;
        }
        let (reply, ack) = mpsc::sync_channel(1);
        let mut work = Work::Barrier(reply);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.sender.try_send(work) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Full(item)) if Instant::now() < deadline => {
                    work = item;
                    std::thread::sleep(Duration::from_millis(2));
                }
                _ => {
                    self.dirty.lock().unwrap().failed = true;
                    return false;
                }
            }
        }
        if ack.recv_timeout(Duration::from_secs(2)).is_err() {
            self.dirty.lock().unwrap().failed = true;
            return false;
        }
        true
    }
    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        self.native.stop();
        if let Some(thread) = self.dispatcher.lock().unwrap().take() {
            let _ = thread.join();
        }
        self.dirty.lock().unwrap().stopped = true;
    }
}
#[cfg(windows)]
impl Drop for RootWatch {
    fn drop(&mut self) {
        self.stop();
    }
}

// Baseline and relists publish entries, but only the final quiet epoch publishes coverage.
struct RepairSink {
    inner: Arc<dyn ScanSink>,
    valid: Mutex<bool>,
    reparented: AtomicBool,
    outside_scope: AtomicU64,
}
impl ScanSink for RepairSink {
    fn recover_interrupted(&self) -> EngineResult<()> {
        self.inner.recover_interrupted()
    }
    fn begin_run(&self, root: &RootId, run: u64, scope: &RunScope) -> EngineResult<()> {
        self.inner.begin_run(root, run, scope)
    }
    fn prepare_listing(
        &self,
        root: &RootId,
        run: u64,
        parent: Option<u64>,
        name: &[u16],
        identity: OpenedIdentity,
    ) -> EngineResult<ListingTicket> {
        self.inner
            .prepare_listing(root, run, parent, name, identity)
    }
    fn refresh_listing(
        &self,
        root: &RootId,
        run: u64,
        ticket: ListingTicket,
    ) -> EngineResult<ListingTicket> {
        self.inner.refresh_listing(root, run, ticket)
    }
    fn consume(&self, root: &RootId, message: ScanMessage) -> EngineResult<()> {
        match &message {
            ScanMessage::ListingDone {
                outcome: ListOutcome::Incomplete(IncompleteReason::OutsideScope),
                ..
            } => {
                self.outside_scope.fetch_add(1, Ordering::Relaxed);
            }
            ScanMessage::ListingDone {
                outcome: ListOutcome::Incomplete(reason),
                ..
            } if *reason != IncompleteReason::OutsideScope => *self.valid.lock().unwrap() = false,
            ScanMessage::DirListing { entries, .. }
                if entries.iter().any(|e| e.traversal_error.is_some()) =>
            {
                *self.valid.lock().unwrap() = false
            }
            _ => (),
        }
        if matches!(&message, ScanMessage::InvalidateChains { .. }) {
            self.reparented.store(true, Ordering::Release);
        }
        match self.inner.consume(root, message) {
            // Reparenting invalidates tickets already in flight; discard them and relist the root.
            Err(EngineError::StaleGeneration { .. }) if self.reparented.load(Ordering::Acquire) => {
                Ok(())
            }
            result => result,
        }
    }
    fn finish_run(
        &self,
        root: &RootId,
        run: u64,
        scope: &RunScope,
        _complete: bool,
    ) -> EngineResult<()> {
        self.inner.finish_run(root, run, scope, false)
    }
    fn fence_root(&self, root: &RootId) -> EngineResult<()> {
        self.inner.fence_root(root)
    }
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_watched<S: DirSource>(
    source: &S,
    root: &GrantedRoot,
    run: u64,
    sink: Arc<dyn ScanSink>,
    bytes: Arc<ByteBudget>,
    cancel: Arc<AtomicBool>,
    options: ScanOptions,
    watch: &RootWatch,
) -> EngineResult<ScanReport> {
    watch.validate(root)?;
    if watch.stop.load(Ordering::Acquire) {
        return Err(EngineError::PermissionDenied {
            message: "watch grant revoked".into(),
        });
    }
    reconcile(
        source,
        root,
        run,
        sink,
        bytes,
        cancel,
        options,
        &watch.dirty,
        || watch.checkpoint(),
    )
}

#[allow(clippy::too_many_arguments)]
fn reconcile<S: DirSource>(
    source: &S,
    root: &GrantedRoot,
    run: u64,
    sink: Arc<dyn ScanSink>,
    bytes: Arc<ByteBudget>,
    cancel: Arc<AtomicBool>,
    mut options: ScanOptions,
    dirty: &Mutex<Dirty>,
    checkpoint: impl Fn() -> bool,
) -> EngineResult<ScanReport> {
    let start = std::time::Instant::now();
    let repair = Arc::new(RepairSink {
        inner: sink.clone(),
        valid: Mutex::new(true),
        reparented: AtomicBool::new(false),
        outside_scope: AtomicU64::new(0),
    });
    // Restart/first scan always needs a full baseline: watcher history does not survive exit.
    options.scope = RunScope::FullRoot;
    checkpoint();
    let mut covered = dirty.lock().unwrap().epoch;
    let mut report = run_scan(
        source,
        root,
        run,
        repair.clone(),
        bytes.clone(),
        cancel.clone(),
        options.clone(),
    )?;
    let mut valid = *repair.valid.lock().unwrap() && !report.limit_hit;
    // A full pass can also be partial without any listing failure: a directory moved or vanished
    // while it was listed (#178). Its changes are in the watcher's epochs, but coverage is only
    // claimed after a pass that settled; until then the next pass is a full one.
    let mut settled = report.complete;
    let mut examined = report.examined;
    for attempt in 0..=MAX_RELISTS {
        checkpoint();
        let mut state = dirty.lock().unwrap();
        if repair.reparented.swap(false, Ordering::AcqRel) {
            state.record(vec![Change::RootDirty]);
        }
        if valid {
            state.acknowledge(covered);
        }
        if !valid || state.failed || state.stopped || cancel.load(Ordering::Acquire) {
            break;
        }
        if state.clean() && settled {
            // finish_run owns the repair rollup and only now may finalise absence/generation.
            sink.begin_run(root.root_id(), run, &RunScope::FullRoot)?;
            sink.finish_run(root.root_id(), run, &RunScope::FullRoot, true)?;
            drop(state);
            checkpoint();
            state = dirty.lock().unwrap();
            if state.clean() && !cancel.load(Ordering::Acquire) {
                report.complete = true;
                report.examined = examined;
                report.elapsed_seconds = start.elapsed().as_secs_f64();
                return Ok(report);
            }
            if state.failed || state.stopped || cancel.load(Ordering::Acquire) {
                break;
            }
        }
        if attempt == MAX_RELISTS || examined >= options.max_entries {
            break;
        }
        covered = state.epoch;
        // Clean here means the last full pass did not settle and the watcher holds nothing to target.
        options.scope = if state.root.is_some() || state.clean() {
            RunScope::FullRoot
        } else {
            let ids: Option<Vec<u64>> = state
                .dirs
                .keys()
                .map(|path| sink.resolve_watch_directory(root.root_id(), path))
                .collect();
            ids.map(RunScope::Targeted).unwrap_or(RunScope::FullRoot)
        };
        drop(state);
        let full = matches!(options.scope, RunScope::FullRoot);
        let mut pass_options = options.clone();
        pass_options.max_entries -= examined;
        *repair.valid.lock().unwrap() = true;
        repair.outside_scope.store(0, Ordering::Relaxed);
        let next = run_scan(
            source,
            root,
            run,
            repair.clone(),
            bytes.clone(),
            cancel.clone(),
            pass_options,
        )?;
        valid = *repair.valid.lock().unwrap() && !next.limit_hit;
        // A targeted pass is partial by construction (outside-scope directories); only a full
        // pass reports whether the tree settled.
        settled = !full || next.complete;
        examined += next.examined;
        report.enumeration_worker_seconds += next.enumeration_worker_seconds;
        report.persistence_seconds += next.persistence_seconds;
        report.totals = next.totals;
        report.skipped += next.skipped;
        report.failed += next
            .failed
            .saturating_sub(repair.outside_scope.load(Ordering::Relaxed));
        report.limit_hit |= next.limit_hit;
        report.strategy = next.strategy;
    }
    sink.watch_dirty(root.root_id())?;
    report.complete = false;
    report.examined = examined;
    report.elapsed_seconds = start.elapsed().as_secs_f64();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overflow_and_rename_parents_have_epochs() {
        let mut state = Dirty::default();
        for error in [Some(1022), None] {
            assert!(loomward_windows::watch::overflow(error, 0));
            state.record(vec![Change::RootDirty]);
            assert!(state.root.is_some());
            state.acknowledge(state.epoch);
            assert!(state.clean());
        }
        let hint = |action, name: &str| Change::Name {
            action,
            name: name.encode_utf16().collect(),
        };
        state.record(vec![hint(4, "a\\x"), hint(5, "b\\x")]);
        assert_eq!(state.dirs.len(), 2);
        let captured = state.epoch;
        state.record(vec![hint(3, "a\\x")]);
        state.acknowledge(captured);
        assert_eq!(
            state.dirs.len(),
            1,
            "a notification during a listing must survive its acknowledgement"
        );
        assert!(state.dirs.contains_key(&vec![vec![97]]));
        state.record(vec![hint(4, "a\\unpaired")]);
        state.finish_hints();
        assert!(
            state.root.is_some(),
            "an unmatched rename cannot establish absence"
        );
        state.acknowledge(state.epoch);
        state.stopped = true;
        state.record(vec![Change::RootDirty]);
        assert!(state.root.is_none());
    }
}

#[cfg(all(test, windows))]
mod native_tests {
    use super::*;
    use loomward_windows::enumerate::{NativeDir, NativeSource};
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    type DirectoryRows = BTreeMap<Vec<u16>, (Option<FileIdObs>, u32, u64)>;
    type Rows = HashMap<FileIdObs, DirectoryRows>;
    #[derive(Default)]
    struct RecordingSink {
        memory: MemorySink,
        identities: Mutex<HashMap<u64, FileIdObs>>,
        rows: Mutex<Rows>,
        stages: Mutex<HashMap<u64, DirectoryRows>>,
        scopes: Mutex<Vec<RunScope>>,
        finish_change: Mutex<Option<PathBuf>>,
    }
    impl ScanSink for RecordingSink {
        fn resolve_watch_directory(&self, root: &RootId, path: &[Vec<u16>]) -> Option<u64> {
            self.memory.resolve_watch_directory(root, path)
        }
        fn watch_dirty(&self, root: &RootId) -> EngineResult<()> {
            self.memory.watch_dirty(root)
        }
        fn recover_interrupted(&self) -> EngineResult<()> {
            self.memory.recover_interrupted()
        }
        fn begin_run(&self, root: &RootId, run: u64, scope: &RunScope) -> EngineResult<()> {
            self.scopes.lock().unwrap().push(scope.clone());
            self.memory.begin_run(root, run, scope)
        }
        fn prepare_listing(
            &self,
            root: &RootId,
            run: u64,
            parent: Option<u64>,
            name: &[u16],
            id: OpenedIdentity,
        ) -> EngineResult<ListingTicket> {
            let ticket = self.memory.prepare_listing(root, run, parent, name, id)?;
            self.identities
                .lock()
                .unwrap()
                .insert(ticket.dir, id.id.unwrap());
            Ok(ticket)
        }
        fn refresh_listing(
            &self,
            root: &RootId,
            run: u64,
            ticket: ListingTicket,
        ) -> EngineResult<ListingTicket> {
            // A refreshed listing re-sends every entry; drop what the rejected one staged.
            self.stages.lock().unwrap().remove(&ticket.dir);
            self.memory.refresh_listing(root, run, ticket)
        }
        fn consume(&self, root: &RootId, message: ScanMessage) -> EngineResult<()> {
            match &message {
                ScanMessage::DirListing {
                    ticket, entries, ..
                } => {
                    let mut stages = self.stages.lock().unwrap();
                    let stage = stages.entry(ticket.dir).or_default();
                    for e in entries {
                        stage.insert(e.name.clone(), (e.file_id, e.attributes, e.logical));
                    }
                }
                ScanMessage::ListingDone {
                    ticket, outcome, ..
                } => {
                    let stage = self
                        .stages
                        .lock()
                        .unwrap()
                        .remove(&ticket.dir)
                        .unwrap_or_default();
                    let id = self.identities.lock().unwrap()[&ticket.dir];
                    let mut rows = self.rows.lock().unwrap();
                    if *outcome == ListOutcome::Complete {
                        rows.insert(id, stage);
                    } else {
                        rows.entry(id).or_default().extend(stage);
                    }
                }
                _ => (),
            }
            self.memory.consume(root, message)
        }
        fn finish_run(
            &self,
            root: &RootId,
            run: u64,
            scope: &RunScope,
            complete: bool,
        ) -> EngineResult<()> {
            if complete {
                if let Some(path) = self.finish_change.lock().unwrap().take() {
                    std::fs::write(path, b"change during final publication").unwrap();
                }
            }
            self.stages.lock().unwrap().clear();
            self.memory.finish_run(root, run, scope, complete)
        }
        fn fence_root(&self, root: &RootId) -> EngineResult<()> {
            self.memory.fence_root(root)
        }
    }
    struct MarkedDir {
        native: NativeDir,
        name: Vec<u16>,
    }
    struct BurstSource {
        native: NativeSource,
        path: PathBuf,
        mutated: AtomicBool,
        watch: Arc<RootWatch>,
        move_sub: bool,
    }
    impl DirSource for BurstSource {
        type Dir = MarkedDir;
        fn strategy(&self) -> Strategy {
            self.native.strategy()
        }
        fn strategy_for(&self, dir: &MarkedDir) -> Strategy {
            self.native.strategy_for(&dir.native)
        }
        fn is_refs(&self, dir: &MarkedDir) -> bool {
            self.native.is_refs(&dir.native)
        }
        fn open_root(&self, path: &Path) -> Result<(MarkedDir, OpenedIdentity), SourceError> {
            let (native, id) = self.native.open_root(path)?;
            Ok((
                MarkedDir {
                    native,
                    name: vec![],
                },
                id,
            ))
        }
        fn open_child(
            &self,
            dir: &MarkedDir,
            entry: &RawEntry<'_>,
        ) -> Result<(MarkedDir, OpenedIdentity), SourceError> {
            let (native, id) = self.native.open_child(&dir.native, entry)?;
            Ok((
                MarkedDir {
                    native,
                    name: entry.name.to_vec(),
                },
                id,
            ))
        }
        fn list(&self, dir: &MarkedDir, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
            let result = self.native.list(&dir.native, sink);
            if dir.name == [97] && !self.mutated.swap(true, Ordering::AcqRel) {
                // These changes happen AFTER a's entries were handed to the scan, not before it.
                for i in 0..200 {
                    let old = self.path.join(format!("a/new-{i}"));
                    let new = self.path.join(format!("b/new-{i}"));
                    std::fs::write(&old, b"fixture").unwrap();
                    std::fs::rename(&old, &new).unwrap();
                    if i % 2 == 0 {
                        std::fs::remove_file(new).unwrap();
                    }
                }
                std::fs::remove_file(self.path.join("a/delete-me")).unwrap();
                if self.move_sub {
                    std::fs::rename(self.path.join("a/sub"), self.path.join("b/sub")).unwrap();
                }
                std::fs::write(self.path.join("a/retained"), b"longer retained fixture").unwrap();
                assert!(self.watch.checkpoint());
            }
            result
        }
    }
    fn fixture() -> tempfile::TempDir {
        tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
    }
    fn grant(path: &Path) -> Option<GrantedRoot> {
        let source = NativeSource::default();
        if loomward_windows::enumerate::running_elevated() {
            assert!(source.open_root(path).is_err());
            assert!(loomward_windows::watch::Watcher::start(path, None, |_| ()).is_err());
            return None;
        }
        let (_, id) = source.open_root(path).unwrap();
        Some(GrantedRoot::new(
            RootId::new("rt_watch").unwrap(),
            path.to_owned(),
            loomward_protocol::DatasetClass::Synthetic,
            id,
        ))
    }
    fn bytes() -> Arc<ByteBudget> {
        Arc::new(ByteBudget::new(crate::budgets::SCAN_BYTES))
    }
    fn options() -> ScanOptions {
        ScanOptions {
            workers: 1,
            ..ScanOptions::default()
        }
    }

    #[test]
    fn armed_before_scan_burst_reconciles_names_moves_and_sums_like_a_fresh_scan() {
        for move_sub in [false, true] {
            let tree = fixture();
            let Some(root) = grant(tree.path()) else {
                return;
            };
            std::fs::create_dir_all(tree.path().join("a/sub")).unwrap();
            std::fs::create_dir(tree.path().join("b")).unwrap();
            std::fs::write(tree.path().join("a/sub/keep"), b"keep").unwrap();
            std::fs::write(tree.path().join("a/delete-me"), b"delete").unwrap();
            std::fs::write(tree.path().join("a/retained"), b"short").unwrap();
            let sink = Arc::new(RecordingSink::default());
            *sink.finish_change.lock().unwrap() = Some(tree.path().join("a/commit-change"));
            let watch = RootWatch::start(&root, sink.clone()).unwrap();
            assert!(watch.checkpoint());
            let source = BurstSource {
                native: NativeSource::default(),
                path: tree.path().to_owned(),
                mutated: AtomicBool::new(false),
                watch: watch.clone(),
                move_sub,
            };
            let report = run_watched(
                &source,
                &root,
                1,
                sink.clone(),
                bytes(),
                Arc::new(AtomicBool::new(false)),
                options(),
                &watch,
            )
            .unwrap();
            assert!(source.mutated.load(Ordering::Acquire));
            assert!(
                report.complete,
                "must reconcile notifications that arrive during a listing"
            );
            assert!(
                move_sub
                    || sink
                        .scopes
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|scope| matches!(scope, RunScope::Targeted(ids) if ids.len() >= 2)),
                "old and new rename parents must both be relisted"
            );
            let fresh = Arc::new(RecordingSink::default());
            let expected = run_scan(
                &NativeSource::default(),
                &root,
                2,
                fresh.clone(),
                bytes(),
                Arc::new(AtomicBool::new(false)),
                options(),
            )
            .unwrap();
            assert_eq!(*sink.rows.lock().unwrap(), *fresh.rows.lock().unwrap());
            assert_eq!(report.totals, expected.totals);
            assert_eq!(
                sink.memory.totals(root.root_id()),
                Some(expected.totals),
                "ancestor rollup"
            );
            assert!(sink.memory.sweep_allowed(root.root_id()));
            assert!(!sink.memory.is_repairing(root.root_id()));
            // Notifications between runs keep catalogue coverage visibly stale.
            std::fs::write(tree.path().join("a/idle-change"), b"idle").unwrap();
            assert!(watch.checkpoint());
            assert!(sink.memory.is_repairing(root.root_id()));
            assert!(!sink.memory.sweep_allowed(root.root_id()));
            drop(source);
            watch.stop();
        }
    }

    #[test]
    fn revocation_stops_native_io_and_late_hints_cannot_reactivate_it() {
        let tree = fixture();
        let Some(root) = grant(tree.path()) else {
            return;
        };
        let engine = Engine::open(crate::EngineConfig::new(
            tree.path().join("state"),
            root.dataset_class(),
        ))
        .unwrap();
        let sink = Arc::new(MemorySink::default());
        engine.set_scan_sink(sink.clone()).unwrap();
        let watch = RootWatch::start(&root, sink.clone()).unwrap();
        engine
            .scan_watches
            .lock()
            .unwrap()
            .insert(root.root_id().clone(), watch.clone());
        assert!(watch.checkpoint());
        let start = Instant::now();
        engine.scan_cancel_root(root.root_id()).unwrap();
        engine.scan_writer_fence(root.root_id()).unwrap();
        let epoch = watch.dirty.lock().unwrap().epoch;
        std::fs::write(tree.path().join("after-revocation"), b"fixture").unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(watch.dirty.lock().unwrap().epoch, epoch);
        assert!(watch.dirty.lock().unwrap().stopped);
        assert!(watch.native.checkpoint().is_err());
        assert!(engine.scan_watches.lock().unwrap().is_empty());
        // The watcher released its root/ancestor pins before the revoke acknowledgement.
        let moved = tree.path().with_extension("moved-fixture");
        std::fs::rename(tree.path(), &moved).unwrap();
        std::fs::rename(&moved, tree.path()).unwrap();
        eprintln!(
            "fixture revoke stop + join ms: {:.3}",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }

    #[test]
    fn forced_overflow_schedules_full_relist_and_dirty_queue_is_bounded() {
        let tree = fixture();
        let Some(root) = grant(tree.path()) else {
            return;
        };
        let sink = Arc::new(RecordingSink::default());
        let dirty = Mutex::new(Dirty::default());
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let checkpoint = || {
            let n = calls.fetch_add(1, Ordering::AcqRel);
            if n == 1 || n == 2 {
                let error = if n == 1 { Some(1022) } else { None };
                assert!(loomward_windows::watch::overflow(error, 0));
                dirty.lock().unwrap().record(vec![Change::RootDirty]);
                std::fs::write(tree.path().join(format!("overflow-{n}")), b"fixture").unwrap();
            }
            true
        };
        let report = reconcile(
            &NativeSource::default(),
            &root,
            3,
            sink.clone(),
            bytes(),
            Arc::new(AtomicBool::new(false)),
            options(),
            &dirty,
            checkpoint,
        )
        .unwrap();
        assert!(report.complete);
        assert_eq!(report.totals.files, 2);
        assert!(sink
            .scopes
            .lock()
            .unwrap()
            .iter()
            .all(|s| *s == RunScope::FullRoot));
        assert!(dirty.lock().unwrap().clean());
        let mut state = Dirty::default();
        for i in 0..=MAX_DIRTY {
            state.record(vec![Change::Name {
                action: 1,
                name: format!("d{i}\\file").encode_utf16().collect(),
            }]);
        }
        assert!(state.root.is_some());
        assert!(state.dirs.is_empty());
    }
}
