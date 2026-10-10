use super::*;
use crate::{Engine, EngineConfig};
use loomward_protocol::{DatasetClass, RootId};
use source::*;
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};
fn identity(id: u64) -> OpenedIdentity {
    OpenedIdentity {
        id: Some(FileIdObs::Id64(id)),
        basis: IdBasis::Listed,
        volume_serial: Some(42),
        attributes: 0x10,
        reparse_tag: None,
    }
}
fn root() -> GrantedRoot {
    GrantedRoot::new(
        RootId::new("rt_test").unwrap(),
        "unused".into(),
        DatasetClass::Synthetic,
        identity(1),
    )
}
#[test]
fn dataset_class_is_checked_before_start_or_refresh() {
    let e = Engine::open(EngineConfig::new("unused".into(), DatasetClass::Personal)).unwrap();
    let root = root();
    for result in [e.scan_start(&root, None), e.scan_refresh(&root, None)] {
        assert_eq!(
            result.unwrap_err().to_body().message.as_str(),
            "dataset_class_mismatch"
        );
    }
}
#[test]
fn same_run_obsolete_final_is_rejected_n1() {
    let sink = MemorySink::default();
    let root = root();
    sink.begin_run(root.root_id(), 7, &RunScope::FullRoot)
        .unwrap();
    let ticket = sink
        .prepare_listing(root.root_id(), 7, None, &[], identity(1))
        .unwrap();
    sink.dirty_again(root.root_id(), ticket.dir);
    assert!(sink
        .consume(
            root.root_id(),
            ScanMessage::DirFinal {
                run: 7,
                ticket,
                sums: Sums::ZERO,
                complete: true
            }
        )
        .is_err());
    assert!(sink.totals(root.root_id()).is_none());
}
#[test]
fn targeted_run_never_sweeps_unrelated_tombstones_n2() {
    let sink = MemorySink::default();
    let root = root();
    let targeted = RunScope::Targeted(vec![2]);
    sink.begin_run(root.root_id(), 7, &targeted).unwrap();
    sink.finish_run(root.root_id(), 7, &targeted, true).unwrap();
    assert!(!sink.sweep_allowed(root.root_id()));
    sink.begin_run(root.root_id(), 8, &RunScope::FullRoot)
        .unwrap();
    sink.finish_run(root.root_id(), 8, &RunScope::FullRoot, true)
        .unwrap();
    assert!(sink.sweep_allowed(root.root_id()));
}
#[test]
fn reparent_invalidates_both_chains_and_restart_repairs() {
    let sink = Arc::new(MemorySink::default());
    let root = root();
    sink.begin_run(root.root_id(), 7, &RunScope::FullRoot)
        .unwrap();
    let r = sink
        .prepare_listing(root.root_id(), 7, None, &[], identity(1))
        .unwrap();
    let a = sink
        .prepare_listing(root.root_id(), 7, Some(r.dir), &[65], identity(2))
        .unwrap();
    let b = sink
        .prepare_listing(root.root_id(), 7, Some(r.dir), &[66], identity(3))
        .unwrap();
    sink.prepare_listing(root.root_id(), 7, Some(a.dir), &[120], identity(4))
        .unwrap();
    let old_totals = Sums {
        files: 99,
        ..Sums::ZERO
    };
    sink.consume(
        root.root_id(),
        ScanMessage::DirFinal {
            run: 7,
            ticket: r,
            sums: old_totals,
            complete: true,
        },
    )
    .unwrap();
    assert_eq!(sink.totals(root.root_id()), Some(old_totals));
    let x = sink
        .prepare_listing(root.root_id(), 7, Some(b.dir), &[120], identity(4))
        .unwrap();
    assert_eq!(x.old_parent, Some(a.dir));
    sink.consume(
        root.root_id(),
        ScanMessage::InvalidateChains {
            run: 7,
            old_parent: a.dir,
            new_parent: b.dir,
        },
    )
    .unwrap();
    for ticket in [a, b, r] {
        assert!(sink.invalidated(root.root_id()).contains(&ticket.dir));
        assert!(matches!(
            sink.consume(
                root.root_id(),
                ScanMessage::DirFinal {
                    run: 7,
                    ticket,
                    sums: Sums {
                        files: 99,
                        ..Sums::ZERO
                    },
                    complete: true,
                },
            ),
            Err(crate::EngineError::StaleGeneration { .. })
        ));
    }
    assert!(sink.totals(root.root_id()).is_none());
    sink.recover_interrupted().unwrap();
    assert!(sink.is_repairing(root.root_id()));
    assert!(!sink.sweep_allowed(root.root_id()));
    let report = run_scan(
        &Tree,
        &root,
        8,
        sink.clone(),
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        Arc::new(AtomicBool::new(false)),
        ScanOptions::default(),
    )
    .unwrap();
    assert!(report.complete);
    assert_eq!(
        report.totals,
        Sums {
            files: 1,
            dirs: 1,
            logical: 123,
            allocated: Some(4096)
        }
    );
    assert_eq!(sink.totals(root.root_id()), Some(report.totals));
    assert!(!sink.is_repairing(root.root_id()));
}
struct Tree;
impl DirSource for Tree {
    type Dir = u64;
    fn strategy(&self) -> Strategy {
        Strategy::Portable
    }
    fn open_root(&self, _: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
        Ok((1, identity(1)))
    }
    fn open_child(&self, _: &u64, e: &RawEntry<'_>) -> Result<(u64, OpenedIdentity), SourceError> {
        let Some(FileIdObs::Id64(id)) = e.file_id else {
            unreachable!()
        };
        Ok((id, identity(id)))
    }
    fn list(&self, dir: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        let (attributes, id, size) = if *dir == 1 { (0x10, 2, 0) } else { (0, 3, 123) };
        let e = RawEntry {
            name: &[120],
            file_id: Some(FileIdObs::Id64(id)),
            attributes,
            reparse_tag: None,
            end_of_file: size,
            allocation_size: Some(4096),
            creation: None,
            last_write: None,
            change: None,
            last_access: None,
        };
        if sink(e) == Flow::Stop {
            ListOutcome::Incomplete(IncompleteReason::Cancelled)
        } else {
            ListOutcome::Complete
        }
    }
}
#[test]
fn staged_pipeline_has_exact_totals_and_releases_bytes() {
    let sink = Arc::new(MemorySink::default());
    let root = root();
    let bytes = Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES));
    let report = run_scan(
        &Tree,
        &root,
        1,
        sink.clone(),
        bytes.clone(),
        Arc::new(AtomicBool::new(false)),
        ScanOptions::default(),
    )
    .unwrap();
    assert!(report.complete);
    assert_eq!(
        report.totals,
        Sums {
            files: 1,
            dirs: 1,
            logical: 123,
            allocated: Some(4096)
        }
    );
    assert_eq!(sink.totals(root.root_id()), Some(report.totals));
    assert_eq!(bytes.used(), 0);
}
/// Records which listings were published before each child ticket was requested.
#[derive(Default)]
struct OrderProbe {
    sink: MemorySink,
    published: std::sync::Mutex<Vec<u64>>,
    early: std::sync::Mutex<Vec<u64>>,
}
impl ScanSink for OrderProbe {
    fn recover_interrupted(&self) -> crate::EngineResult<()> {
        self.sink.recover_interrupted()
    }
    fn begin_run(&self, root: &RootId, run: u64, scope: &RunScope) -> crate::EngineResult<()> {
        self.sink.begin_run(root, run, scope)
    }
    fn prepare_listing(
        &self,
        root: &RootId,
        run: u64,
        parent: Option<u64>,
        name: &[u16],
        identity: OpenedIdentity,
    ) -> crate::EngineResult<ListingTicket> {
        if let Some(p) = parent {
            if !self.published.lock().unwrap().contains(&p) {
                self.early.lock().unwrap().push(p);
            }
        }
        self.sink.prepare_listing(root, run, parent, name, identity)
    }
    fn refresh_listing(
        &self,
        root: &RootId,
        run: u64,
        ticket: ListingTicket,
    ) -> crate::EngineResult<ListingTicket> {
        self.sink.refresh_listing(root, run, ticket)
    }
    fn consume(&self, root: &RootId, message: ScanMessage) -> crate::EngineResult<()> {
        if let ScanMessage::ListingDone { ticket, .. } = &message {
            self.published.lock().unwrap().push(ticket.dir);
        }
        self.sink.consume(root, message)
    }
    fn finish_run(
        &self,
        root: &RootId,
        run: u64,
        scope: &RunScope,
        complete: bool,
    ) -> crate::EngineResult<()> {
        self.sink.finish_run(root, run, scope, complete)
    }
    fn fence_root(&self, root: &RootId) -> crate::EngineResult<()> {
        self.sink.fence_root(root)
    }
}
/// A durable writer creates child rows when the parent listing publishes, so no child ticket may
/// be requested before its parent's ListingDone reached the writer.
#[test]
fn child_tickets_follow_the_parent_publication() {
    let sink = Arc::new(OrderProbe::default());
    let report = run_scan(
        &Tree,
        &root(),
        1,
        sink.clone(),
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        Arc::new(AtomicBool::new(false)),
        ScanOptions::default(),
    )
    .unwrap();
    assert!(report.complete);
    assert_eq!(report.totals.dirs, 1);
    assert_eq!(sink.published.lock().unwrap().len(), 2);
    assert!(sink.early.lock().unwrap().is_empty());
}
#[test]
fn incomplete_listing_does_not_authorize_absence() {
    let root = root();
    let sink = Arc::new(MemorySink::default());
    let report = run_scan(
        &Tree,
        &root,
        1,
        sink.clone(),
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        Arc::new(AtomicBool::new(false)),
        ScanOptions {
            max_entries: 1,
            ..ScanOptions::default()
        },
    )
    .unwrap();
    assert!(!report.complete);
    assert!(!sink.sweep_allowed(root.root_id()));
}
#[test]
fn fence_rejects_late_staging_and_final() {
    let sink = MemorySink::default();
    let root = root();
    sink.begin_run(root.root_id(), 7, &RunScope::FullRoot)
        .unwrap();
    let ticket = sink
        .prepare_listing(root.root_id(), 7, None, &[], identity(1))
        .unwrap();
    sink.fence_root(root.root_id()).unwrap();
    assert!(sink
        .consume(
            root.root_id(),
            ScanMessage::DirListing {
                run: 7,
                ticket,
                entries: vec![]
            }
        )
        .is_err());
    assert!(sink
        .consume(
            root.root_id(),
            ScanMessage::DirFinal {
                run: 7,
                ticket,
                sums: Sums::ZERO,
                complete: true
            }
        )
        .is_err());
}
#[test]
fn checked_sums_report_byte_overflow_detail() {
    let error = Sums {
        logical: u64::MAX,
        ..Sums::ZERO
    }
    .checked_add(Sums {
        logical: 1,
        ..Sums::ZERO
    })
    .unwrap_err();
    assert_eq!(
        error.to_body().detail.unwrap().get("reason").unwrap(),
        "byte_overflow"
    );
}
#[test]
fn cancel_before_admission_returns_quickly() {
    let now = std::time::Instant::now();
    assert!(run_scan(
        &Tree,
        &root(),
        1,
        Arc::new(MemorySink::default()),
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        Arc::new(AtomicBool::new(true)),
        ScanOptions::default()
    )
    .is_err());
    assert!(now.elapsed() < Duration::from_millis(250));
}
#[test]
fn obsolete_run_final_and_nonunique_refs_cannot_reparent() {
    let sink = MemorySink::default();
    let root = root();
    sink.begin_run(root.root_id(), 7, &RunScope::FullRoot)
        .unwrap();
    let r = sink
        .prepare_listing(root.root_id(), 7, None, &[], identity(1))
        .unwrap();
    assert!(sink
        .consume(
            root.root_id(),
            ScanMessage::DirFinal {
                run: 6,
                ticket: r,
                sums: Sums::ZERO,
                complete: true
            }
        )
        .is_err());
    let a = sink
        .prepare_listing(root.root_id(), 7, Some(r.dir), &[65], identity(2))
        .unwrap();
    let b = sink
        .prepare_listing(root.root_id(), 7, Some(r.dir), &[66], identity(3))
        .unwrap();
    let mut observed = identity(4);
    observed.basis = IdBasis::NonUnique;
    let x = sink
        .prepare_listing(root.root_id(), 7, Some(a.dir), &[120], observed)
        .unwrap();
    let y = sink
        .prepare_listing(root.root_id(), 7, Some(b.dir), &[120], observed)
        .unwrap();
    assert_ne!(x.dir, y.dir);
    assert_eq!(y.old_parent, None);
}
#[test]
fn root_identity_mismatch_never_enters_writer() {
    struct NeverList;
    impl DirSource for NeverList {
        type Dir = u64;
        fn strategy(&self) -> Strategy {
            Strategy::Portable
        }
        fn open_root(&self, path: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
            Tree.open_root(path)
        }
        fn open_child(
            &self,
            _: &u64,
            _: &RawEntry<'_>,
        ) -> Result<(u64, OpenedIdentity), SourceError> {
            panic!("a mismatched grant must not open children")
        }
        fn list(&self, _: &u64, _: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
            panic!("a mismatched grant must not list the opened root")
        }
    }
    let mut root = root();
    root.expected = Some(identity(999));
    let sink = Arc::new(MemorySink::default());
    let error = run_scan(
        &NeverList,
        &root,
        1,
        sink.clone(),
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        Arc::new(AtomicBool::new(false)),
        ScanOptions::default(),
    )
    .unwrap_err();
    assert_eq!(error.to_body().message.as_str(), "root_identity_changed");
    assert!(sink.totals(root.root_id()).is_none());
}

struct BlockingSource {
    entered: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    dropped: Arc<std::sync::atomic::AtomicU64>,
}
struct HeldDir {
    entered: Arc<AtomicBool>,
    dropped: Arc<std::sync::atomic::AtomicU64>,
}
impl Drop for HeldDir {
    fn drop(&mut self) {
        assert!(!self.entered.load(std::sync::atomic::Ordering::Acquire));
        self.dropped
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}
impl DirSource for BlockingSource {
    type Dir = HeldDir;
    fn strategy(&self) -> Strategy {
        Strategy::Portable
    }
    fn open_root(&self, _: &std::path::Path) -> Result<(HeldDir, OpenedIdentity), SourceError> {
        Ok((
            HeldDir {
                entered: self.entered.clone(),
                dropped: self.dropped.clone(),
            },
            identity(1),
        ))
    }
    fn open_child(
        &self,
        _: &HeldDir,
        _: &RawEntry<'_>,
    ) -> Result<(HeldDir, OpenedIdentity), SourceError> {
        unreachable!()
    }
    fn list(&self, _: &HeldDir, _: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        use std::sync::atomic::Ordering;
        self.entered.store(true, Ordering::Release);
        while !self.release.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(2));
        }
        self.entered.store(false, Ordering::Release);
        ListOutcome::Complete
    }
}
#[test]
fn blocked_worker_cancel_ack_does_not_close_live_handle() {
    use loomward_protocol::{JobKind, JobState};
    use std::sync::atomic::{AtomicU64, Ordering};
    let e = Engine::open(EngineConfig::new("unused".into(), DatasetClass::Synthetic)).unwrap();
    let sink = Arc::new(MemorySink::default());
    e.set_scan_sink(sink.clone()).unwrap();
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicU64::new(0));
    let source = BlockingSource {
        entered: entered.clone(),
        release: release.clone(),
        dropped: dropped.clone(),
    };
    let root = root();
    let root_id = root.root_id().clone();
    e.register_job_runner(
        JobKind::Scan,
        Arc::new(move |ctx, _| {
            run_scan(
                &source,
                &root,
                1,
                sink.clone(),
                Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
                ctx.cancel,
                ScanOptions::default(),
            )?;
            Ok(loomward_protocol::CoverageState::Complete)
        }),
    );
    let job = e
        .job_submit(crate::jobs::JobSpec::new(
            JobKind::Scan,
            Some(root_id.clone()),
        ))
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while !entered.load(Ordering::Acquire) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    let start = std::time::Instant::now();
    assert_eq!(
        e.scan_cancel_root(&root_id).unwrap()[0].state,
        JobState::CancelRequested
    );
    assert!(start.elapsed() < Duration::from_millis(250));
    assert_eq!(dropped.load(Ordering::Relaxed), 0);
    assert_eq!(
        e.job_status(&job.job_id).unwrap().state,
        JobState::CancelRequested
    );
    e.scan_writer_fence(&root_id).unwrap();
    release.store(true, Ordering::Release);
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while e.job_status(&job.job_id).unwrap().state != JobState::Cancelled {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
}
#[test]
fn scan_worker_cap_is_shared_across_roots_and_released() {
    let e = Engine::open(EngineConfig::new("unused".into(), DatasetClass::Synthetic)).unwrap();
    let lease = e.budgets.reserve_scan_workers(Some(8)).unwrap();
    assert!(e.budgets.reserve_scan_workers(Some(1)).is_err());
    assert_eq!(e.budgets_get().unwrap().pools[0].current_workers.get(), 8);
    drop(lease);
    assert_eq!(e.budgets_get().unwrap().pools[0].current_workers.get(), 0);
    assert!(e.budgets.reserve_scan_workers(Some(8)).is_ok());
}
#[test]
fn writer_install_is_startup_only() {
    let e = Engine::open(EngineConfig::new("unused".into(), DatasetClass::Synthetic)).unwrap();
    e.set_scan_sink(Arc::new(MemorySink::default())).unwrap();
    assert!(e.set_scan_sink(Arc::new(MemorySink::default())).is_err());
}
#[cfg(windows)]
#[test]
fn native_scan_job_reports_exact_final_progress() {
    if skip_elevated() {
        return;
    }
    let temp = canonical_tempdir();
    std::fs::write(temp.path().join("synthetic"), [0u8; 123]).unwrap();
    let source = loomward_windows::enumerate::NativeSource::default();
    let (_, identity) = source
        .open_root(temp.path())
        .unwrap_or_else(|e| panic!("{e:?}: {}", explain(temp.path())));
    let root = GrantedRoot::new(
        RootId::new("rt_native_test").unwrap(),
        temp.path().to_path_buf(),
        DatasetClass::Synthetic,
        identity,
    );
    let e = Engine::open(EngineConfig::new("unused".into(), DatasetClass::Synthetic)).unwrap();
    e.set_scan_sink(Arc::new(MemorySink::default())).unwrap();
    let job = e.scan_start(&root, None).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    let job = loop {
        let j = e.job_status(&job.job_id).unwrap();
        if j.state == loomward_protocol::JobState::Completed {
            break j;
        }
        assert!(std::time::Instant::now() < deadline, "{j:?}");
        std::thread::sleep(Duration::from_millis(2));
    };
    let p = job.progress.unwrap();
    assert_eq!(p.examined.get(), 1);
    assert_eq!(p.indexed_files.get(), 1);
    assert_eq!(p.logical_bytes.get(), 123);
    assert_eq!(p.skipped.get(), 0);
    assert_eq!(p.failed.get(), 0);
}
struct Oversized;
impl DirSource for Oversized {
    type Dir = u64;
    fn strategy(&self) -> Strategy {
        Strategy::Portable
    }
    fn open_root(&self, _: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
        Ok((1, identity(1)))
    }
    fn open_child(&self, _: &u64, _: &RawEntry<'_>) -> Result<(u64, OpenedIdentity), SourceError> {
        unreachable!()
    }
    fn list(&self, _: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        for i in 0..2_500_000u32 {
            let mut name = [0u16; 6];
            for (j, c) in name.iter_mut().enumerate() {
                *c = u16::from(b"0123456789abcdef"[((i >> (j * 4)) & 15) as usize]);
            }
            if sink(RawEntry {
                name: &name,
                file_id: None,
                attributes: 0,
                reparse_tag: None,
                end_of_file: 1,
                allocation_size: None,
                creation: None,
                last_write: None,
                change: None,
                last_access: None,
            }) == Flow::Stop
            {
                return ListOutcome::Incomplete(IncompleteReason::Cancelled);
            }
        }
        ListOutcome::Complete
    }
}
#[test]
fn oversized_stress_is_separate_with_partial_oracle_n6() {
    let sink = Arc::new(MemorySink::default());
    let root = root();
    let report = run_scan(
        &Oversized,
        &root,
        1,
        sink.clone(),
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        Arc::new(AtomicBool::new(false)),
        ScanOptions {
            workers: 1,
            ..ScanOptions::default()
        },
    )
    .unwrap();
    assert!(!report.complete);
    assert!(report.limit_hit);
    assert_eq!(report.totals.files, 2_000_000);
    assert_eq!(report.totals.logical, 2_000_000);
    assert_eq!(report.totals.allocated, None);
    assert!(!sink.sweep_allowed(root.root_id()));
}
struct FailedWriter(MemorySink);
struct MovingTree {
    x_listings: std::sync::atomic::AtomicUsize,
    unique: bool,
}
impl DirSource for MovingTree {
    type Dir = u64;
    fn strategy(&self) -> Strategy {
        Strategy::Portable
    }
    fn open_root(&self, path: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
        Tree.open_root(path)
    }
    fn open_child(
        &self,
        parent: &u64,
        entry: &RawEntry<'_>,
    ) -> Result<(u64, OpenedIdentity), SourceError> {
        let (dir, mut identity) = Tree.open_child(parent, entry)?;
        if !self.unique && dir == 4 {
            identity.basis = IdBasis::NonUnique;
        }
        Ok((dir, identity))
    }
    fn list(&self, dir: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        let children: &[(u64, u32, u64)] = match dir {
            1 => &[(2, 0x10, 0), (3, 0x10, 0)],
            // x moves after A's listing and before B's: both see the same opened identity.
            2 | 3 => &[(4, 0x10, 0)],
            4 => {
                self.x_listings
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                &[(5, 0, 123)]
            }
            _ => unreachable!(),
        };
        for &(id, attributes, size) in children {
            if sink(RawEntry {
                name: &[id as u16],
                file_id: Some(FileIdObs::Id64(id)),
                attributes,
                reparse_tag: None,
                end_of_file: size,
                allocation_size: Some(4096),
                creation: None,
                last_write: None,
                change: None,
                last_access: None,
            }) == Flow::Stop
            {
                return ListOutcome::Incomplete(IncompleteReason::Cancelled);
            }
        }
        ListOutcome::Complete
    }
}
#[test]
fn moved_directory_is_discovered_once_and_marks_the_run_partial() {
    for workers in [1, 8] {
        for unique in [true, false] {
            let copies = if unique { 1 } else { 2 };
            let source = MovingTree {
                x_listings: Default::default(),
                unique,
            };
            let sink = Arc::new(MemorySink::default());
            let root = root();
            let report = run_scan(
                &source,
                &root,
                1,
                sink.clone(),
                Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
                Arc::new(AtomicBool::new(false)),
                ScanOptions {
                    workers,
                    ..ScanOptions::default()
                },
            )
            .unwrap();
            // A unique identity sighted under a second parent moved during the run: neither
            // parent's aggregate is trustworthy, so the run is partial (no sweep). Name-only
            // identities are indistinguishable from two directories and count twice.
            assert_eq!(report.complete, !unique);
            assert_eq!(sink.sweep_allowed(root.root_id()), !unique);
            assert_eq!(
                report.totals,
                Sums {
                    files: copies,
                    dirs: 2 + copies,
                    logical: 123 * copies,
                    allocated: Some(4096 * copies)
                }
            );
            assert_eq!(
                source.x_listings.load(std::sync::atomic::Ordering::Relaxed),
                copies as usize
            );
            assert_eq!(sink.totals(root.root_id()), Some(report.totals));
        }
    }
}

struct DirtyWriter {
    sink: MemorySink,
    dirtied: AtomicBool,
    on_final: bool,
    always: bool,
}
impl ScanSink for DirtyWriter {
    fn refresh_listing(
        &self,
        root: &RootId,
        run: u64,
        ticket: ListingTicket,
    ) -> crate::EngineResult<ListingTicket> {
        self.sink.refresh_listing(root, run, ticket)
    }

    fn recover_interrupted(&self) -> crate::EngineResult<()> {
        self.sink.recover_interrupted()
    }
    fn begin_run(&self, root: &RootId, run: u64, scope: &RunScope) -> crate::EngineResult<()> {
        self.sink.begin_run(root, run, scope)
    }
    fn prepare_listing(
        &self,
        root: &RootId,
        run: u64,
        parent: Option<u64>,
        name: &[u16],
        identity: OpenedIdentity,
    ) -> crate::EngineResult<ListingTicket> {
        self.sink.prepare_listing(root, run, parent, name, identity)
    }
    fn consume(&self, root: &RootId, message: ScanMessage) -> crate::EngineResult<()> {
        let ticket = match &message {
            ScanMessage::DirListing { ticket, .. } if !self.on_final => Some(*ticket),
            ScanMessage::DirFinal { ticket, .. } if self.on_final => Some(*ticket),
            _ => None,
        };
        if let Some(ticket) = ticket {
            if !self.dirtied.swap(true, std::sync::atomic::Ordering::AcqRel) || self.always {
                self.sink.dirty_again(root, ticket.dir);
            }
        }
        self.sink.consume(root, message)
    }
    fn finish_run(
        &self,
        root: &RootId,
        run: u64,
        scope: &RunScope,
        complete: bool,
    ) -> crate::EngineResult<()> {
        self.sink.finish_run(root, run, scope, complete)
    }
    fn fence_root(&self, root: &RootId) -> crate::EngineResult<()> {
        self.sink.fence_root(root)
    }
}
#[test]
fn stale_listing_and_final_relist_the_dirty_directory() {
    for (on_final, always) in [(false, false), (true, false), (false, true), (true, true)] {
        let sink = Arc::new(DirtyWriter {
            sink: MemorySink::default(),
            dirtied: AtomicBool::new(false),
            on_final,
            always,
        });
        let root = root();
        let bytes = Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES));
        let report = run_scan(
            &Tree,
            &root,
            1,
            sink.clone(),
            bytes.clone(),
            Arc::new(AtomicBool::new(false)),
            ScanOptions {
                workers: 1,
                ..ScanOptions::default()
            },
        )
        .unwrap();
        if always {
            assert!(!report.complete);
            assert!(!sink.sink.sweep_allowed(root.root_id()));
            assert!(sink.sink.totals(root.root_id()).is_none());
            // No final was ever accepted, yet the report keeps the partial sums of what was
            // listed instead of a zero standing in for unknown.
            assert_eq!(
                report.totals,
                Sums {
                    files: 1,
                    dirs: 1,
                    logical: 123,
                    allocated: Some(4096)
                }
            );
            assert!(report.examined <= 6, "at most three dirty epochs");
            assert_eq!(bytes.used(), 0);
            continue;
        }
        assert!(report.complete);
        assert_eq!(
            report.totals,
            Sums {
                files: 1,
                dirs: 1,
                logical: 123,
                allocated: Some(4096)
            }
        );
        assert_eq!(sink.sink.totals(root.root_id()), Some(report.totals));
        assert!(report.examined > 2, "dirty directory must be relisted");
        assert_eq!(bytes.used(), 0);
    }
}
/// `Tree`, except that the root's subdirectory is gone from every listing after the first.
struct VanishingChild(std::sync::atomic::AtomicUsize);
impl DirSource for VanishingChild {
    type Dir = u64;
    fn strategy(&self) -> Strategy {
        Strategy::Portable
    }
    fn open_root(&self, path: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
        Tree.open_root(path)
    }
    fn open_child(&self, p: &u64, e: &RawEntry<'_>) -> Result<(u64, OpenedIdentity), SourceError> {
        Tree.open_child(p, e)
    }
    fn list(&self, dir: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        if *dir == 1 && self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) > 0 {
            return ListOutcome::Complete;
        }
        Tree.list(dir, sink)
    }
}
#[test]
fn relist_without_a_child_directory_marks_the_run_partial() {
    for workers in [1, 8] {
        let sink = Arc::new(DirtyWriter {
            sink: MemorySink::default(),
            dirtied: AtomicBool::new(false),
            on_final: false,
            always: false,
        });
        let source = VanishingChild(Default::default());
        let root = root();
        let report = run_scan(
            &source,
            &root,
            1,
            sink.clone(),
            Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
            Arc::new(AtomicBool::new(false)),
            ScanOptions {
                workers,
                ..ScanOptions::default()
            },
        )
        .unwrap();
        assert!(
            source.0.load(std::sync::atomic::Ordering::Relaxed) > 1,
            "root relisted"
        );
        assert!(
            !report.complete,
            "a vanished child cannot leave the root complete"
        );
        assert!(!sink.sink.sweep_allowed(root.root_id()));
    }
}
impl ScanSink for FailedWriter {
    fn refresh_listing(
        &self,
        root: &RootId,
        run: u64,
        ticket: ListingTicket,
    ) -> crate::EngineResult<ListingTicket> {
        self.0.refresh_listing(root, run, ticket)
    }

    fn recover_interrupted(&self) -> crate::EngineResult<()> {
        self.0.recover_interrupted()
    }
    fn begin_run(&self, root: &RootId, run: u64, scope: &RunScope) -> crate::EngineResult<()> {
        self.0.begin_run(root, run, scope)
    }
    fn prepare_listing(
        &self,
        root: &RootId,
        run: u64,
        parent: Option<u64>,
        name: &[u16],
        identity: OpenedIdentity,
    ) -> crate::EngineResult<ListingTicket> {
        self.0.prepare_listing(root, run, parent, name, identity)
    }
    fn consume(&self, _: &RootId, message: ScanMessage) -> crate::EngineResult<()> {
        if let ScanMessage::DirListing { entries, .. } = message {
            assert!(entries
                .iter()
                .all(|e| e.file_id.is_some() || !e.identity_eligible));
        }
        Err(crate::EngineError::Internal {
            message: "fixture writer failure".into(),
            detail: None,
        })
    }
    fn finish_run(
        &self,
        root: &RootId,
        run: u64,
        scope: &RunScope,
        complete: bool,
    ) -> crate::EngineResult<()> {
        self.0.finish_run(root, run, scope, complete)
    }
    fn fence_root(&self, root: &RootId) -> crate::EngineResult<()> {
        self.0.fence_root(root)
    }
}
#[cfg(windows)]
#[test]
fn native_dirty_epoch_relists_the_retained_handle() {
    if skip_elevated() {
        return;
    }
    let temp = canonical_tempdir();
    std::fs::create_dir(temp.path().join("child")).unwrap();
    std::fs::write(temp.path().join("child").join("synthetic"), [0u8; 123]).unwrap();
    let source = loomward_windows::enumerate::NativeSource::default();
    let (_, identity) = source.open_root(temp.path()).unwrap();
    let root = GrantedRoot::new(
        RootId::new("rt_native_relist").unwrap(),
        temp.path().to_path_buf(),
        DatasetClass::Synthetic,
        identity,
    );
    let sink = Arc::new(DirtyWriter {
        sink: MemorySink::default(),
        dirtied: AtomicBool::new(false),
        on_final: true,
        always: false,
    });
    let bytes = Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES));
    let report = run_scan(
        &source,
        &root,
        1,
        sink.clone(),
        bytes.clone(),
        Arc::new(AtomicBool::new(false)),
        ScanOptions::default(),
    )
    .unwrap();
    assert!(report.complete);
    assert_eq!(report.totals.files, 1);
    assert_eq!(report.totals.dirs, 1);
    assert_eq!(report.totals.logical, 123);
    assert_eq!(report.examined, 3);
    assert_eq!(sink.sink.totals(root.root_id()), Some(report.totals));
    assert_eq!(bytes.used(), 0);
}
#[test]
fn writer_failure_unblocks_producers_and_releases_shared_bytes() {
    let bytes = Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES));
    let start = std::time::Instant::now();
    let result = run_scan(
        &Oversized,
        &root(),
        1,
        Arc::new(FailedWriter(MemorySink::default())),
        bytes.clone(),
        Arc::new(AtomicBool::new(false)),
        ScanOptions::default(),
    );
    assert_eq!(
        result.unwrap_err().to_body().message.as_str(),
        "fixture writer failure"
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(bytes.used(), 0);
}
#[test]
fn busy_scan_preserves_job_id_wire_detail() {
    let e = Engine::open(EngineConfig::new("unused".into(), DatasetClass::Synthetic)).unwrap();
    e.set_scan_sink(Arc::new(MemorySink::default())).unwrap();
    e.register_job_runner(
        loomward_protocol::JobKind::Scan,
        Arc::new(|ctx, _| {
            while !ctx.is_cancelled() {
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(loomward_protocol::CoverageState::Cancelled)
        }),
    );
    let root = root();
    let job = e
        .job_submit(crate::jobs::JobSpec::new(
            loomward_protocol::JobKind::Scan,
            Some(root.root_id().clone()),
        ))
        .unwrap();
    let error = e.scan_start(&root, None).unwrap_err();
    e.job_cancel(&job.job_id).unwrap();
    assert_eq!(
        error.to_body().detail.unwrap().get("job_id").unwrap(),
        job.job_id.as_str()
    );
}
#[cfg(windows)]
#[test]
fn native_state_root_is_excluded_by_handle_identity() {
    if skip_elevated() {
        return;
    }
    let temp = canonical_tempdir();
    let source = loomward_windows::enumerate::NativeSource::default();
    let (_, identity) = source
        .open_root(temp.path())
        .unwrap_or_else(|e| panic!("{e:?}: {}", explain(temp.path())));
    let root = GrantedRoot::new(
        RootId::new("rt_state_exclusion").unwrap(),
        temp.path().to_path_buf(),
        DatasetClass::Synthetic,
        identity,
    );
    let error = run_scan(
        &source,
        &root,
        1,
        Arc::new(MemorySink::default()),
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        Arc::new(AtomicBool::new(false)),
        ScanOptions {
            excluded_path: Some(temp.path().to_path_buf()),
            ..ScanOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.to_body().message.as_str(), "root_is_state_directory");
}
#[test]
fn cross_run_ticket_cannot_finalize_an_unlisted_directory() {
    let sink = MemorySink::default();
    let root = root();
    sink.begin_run(root.root_id(), 7, &RunScope::FullRoot)
        .unwrap();
    let ticket = sink
        .prepare_listing(root.root_id(), 7, None, &[], identity(1))
        .unwrap();
    sink.finish_run(root.root_id(), 7, &RunScope::FullRoot, false)
        .unwrap();
    sink.begin_run(root.root_id(), 8, &RunScope::Targeted(vec![]))
        .unwrap();
    assert!(sink
        .consume(
            root.root_id(),
            ScanMessage::DirFinal {
                run: 8,
                ticket,
                sums: Sums::ZERO,
                complete: true
            }
        )
        .is_err());
}
#[test]
fn targeted_pipeline_never_establishes_absence_outside_scope() {
    let sink = Arc::new(MemorySink::default());
    let root = root();
    let report = run_scan(
        &Tree,
        &root,
        1,
        sink.clone(),
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        Arc::new(AtomicBool::new(false)),
        ScanOptions {
            scope: RunScope::Targeted(vec![2]),
            ..ScanOptions::default()
        },
    )
    .unwrap();
    assert!(!report.complete);
    assert!(!sink.sweep_allowed(root.root_id()));
    assert_eq!(report.totals.files, 1);
}

/// Native roots are opened component by component and refuse reparse ancestors, so tests grant a
/// canonical temp base (hosted runners may junction the temp folder), as a real grant would.
#[cfg(windows)]
fn canonical_tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
}

/// Each ancestor's attributes, for a refused root on an unfamiliar host.
#[cfg(windows)]
fn explain(path: &std::path::Path) -> String {
    use std::os::windows::fs::MetadataExt;
    path.ancestors()
        .map(|a| {
            let attrs = std::fs::symlink_metadata(a)
                .map(|m| m.file_attributes())
                .unwrap_or(0);
            format!("{}={attrs:#x}", a.display())
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Hosted CI runners hold an elevated token, and the native scan refuses elevation by design
/// (AGENTS.md invariant 2). There, pin the refusal and skip; elsewhere run the native test.
#[cfg(windows)]
fn skip_elevated() -> bool {
    if !loomward_windows::enumerate::running_elevated() {
        return false;
    }
    let temp = canonical_tempdir();
    assert!(
        matches!(
            loomward_windows::enumerate::NativeSource::default().open_root(temp.path()),
            Err(loomward_windows::enumerate::SourceError::Refused)
        ),
        "an elevated token must be refused before any open"
    );
    eprintln!("skipped: elevated token; the native scan refuses elevation (invariant 2)");
    true
}

/// #144: a root is minted only from an active grant row in the engine's own `state.db`, with the
/// row's path, identity and dataset class; a mismatched class is refused at start and refresh.
#[cfg(windows)]
#[test]
fn granted_root_comes_only_from_an_active_durable_grant() {
    if skip_elevated() {
        return;
    }
    let state = canonical_tempdir();
    let root_dir = canonical_tempdir();
    let (_, identity) = loomward_windows::enumerate::NativeSource::default()
        .open_root(root_dir.path())
        .unwrap();
    let file_id = match identity.id.unwrap() {
        FileIdObs::Id128(id) => id.to_vec(),
        FileIdObs::Id64(id) => id.to_le_bytes().to_vec(),
    };
    let volume = format!("vsn_{}", identity.volume_serial.unwrap());
    let conn = rusqlite::Connection::open(state.path().join("state.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT);
         INSERT INTO meta VALUES('dataset_class','synthetic');
         CREATE TABLE root_grant(id INTEGER PRIMARY KEY,display_path TEXT,volume_key TEXT,root_file_id BLOB,state TEXT);",
    )
    .unwrap();
    let path = root_dir.path().to_string_lossy().into_owned();
    let insert = |id: i64, file_id: &[u8], state: &str| {
        conn.execute(
            "INSERT INTO root_grant VALUES(?1,?2,?3,?4,?5)",
            rusqlite::params![id, path, volume, file_id, state],
        )
        .unwrap();
    };
    insert(1, &file_id, "active");
    insert(2, &file_id, "revoked");
    insert(3, &[9; 16], "active");
    let engine = Engine::open(EngineConfig::new(
        state.path().to_path_buf(),
        DatasetClass::Synthetic,
    ))
    .unwrap();
    let id = || RootId::new("rt_granted").unwrap();
    let message =
        |r: EngineResult<GrantedRoot>| r.unwrap_err().to_body().message.as_str().to_owned();
    let root = engine.granted_root(id(), 1).unwrap();
    assert_eq!(root.path(), root_dir.path());
    assert_eq!(root.dataset_class(), DatasetClass::Synthetic);
    assert_eq!(message(engine.granted_root(id(), 2)), "grant_not_active");
    assert_eq!(message(engine.granted_root(id(), 4)), "grant_not_active");
    assert_eq!(
        message(engine.granted_root(id(), 3)),
        "root_identity_changed"
    );
    // The row's class, not the caller's, reaches the engine's class check.
    conn.execute("UPDATE meta SET value='personal'", [])
        .unwrap();
    let personal = engine.granted_root(id(), 1).unwrap();
    assert_eq!(personal.dataset_class(), DatasetClass::Personal);
    engine
        .set_scan_sink(Arc::new(MemorySink::default()))
        .unwrap();
    for result in [
        engine.scan_start(&personal, None),
        engine.scan_refresh(&personal, None),
    ] {
        assert_eq!(
            result.unwrap_err().to_body().message.as_str(),
            "dataset_class_mismatch"
        );
    }
}

/// A sink without watch support (the trait default) scans with no watcher and no lasting pins.
struct Unwatched(MemorySink);
impl ScanSink for Unwatched {
    fn recover_interrupted(&self) -> crate::EngineResult<()> {
        self.0.recover_interrupted()
    }
    fn begin_run(&self, root: &RootId, run: u64, scope: &RunScope) -> crate::EngineResult<()> {
        self.0.begin_run(root, run, scope)
    }
    fn prepare_listing(
        &self,
        root: &RootId,
        run: u64,
        parent: Option<u64>,
        name: &[u16],
        identity: OpenedIdentity,
    ) -> crate::EngineResult<ListingTicket> {
        self.0.prepare_listing(root, run, parent, name, identity)
    }
    fn refresh_listing(
        &self,
        root: &RootId,
        run: u64,
        ticket: ListingTicket,
    ) -> crate::EngineResult<ListingTicket> {
        self.0.refresh_listing(root, run, ticket)
    }
    fn consume(&self, root: &RootId, message: ScanMessage) -> crate::EngineResult<()> {
        self.0.consume(root, message)
    }
    fn finish_run(
        &self,
        root: &RootId,
        run: u64,
        scope: &RunScope,
        complete: bool,
    ) -> crate::EngineResult<()> {
        self.0.finish_run(root, run, scope, complete)
    }
    fn fence_root(&self, root: &RootId) -> crate::EngineResult<()> {
        self.0.fence_root(root)
    }
}
#[cfg(windows)]
#[test]
fn a_sink_without_watch_support_scans_unwatched() {
    if skip_elevated() {
        return;
    }
    let temp = canonical_tempdir();
    std::fs::create_dir(temp.path().join("child")).unwrap();
    std::fs::write(temp.path().join("child").join("synthetic"), [0u8; 7]).unwrap();
    let (_, identity) = loomward_windows::enumerate::NativeSource::default()
        .open_root(temp.path())
        .unwrap();
    let root = GrantedRoot::new(
        RootId::new("rt_unwatched").unwrap(),
        temp.path().to_path_buf(),
        DatasetClass::Synthetic,
        identity,
    );
    let e = Engine::open(EngineConfig::new("unused".into(), DatasetClass::Synthetic)).unwrap();
    let sink = Arc::new(Unwatched(MemorySink::default()));
    e.set_scan_sink(sink.clone()).unwrap();
    let job = e.scan_start(&root, None).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while e.job_status(&job.job_id).unwrap().state != loomward_protocol::JobState::Completed {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(e.scan_watches.lock().unwrap().is_empty());
    assert_eq!(sink.0.totals(root.root_id()).unwrap().logical, 7);
    // No watch pin outlives the scan: the user can rename the scanned folder.
    std::fs::rename(temp.path().join("child"), temp.path().join("renamed")).unwrap();
}
