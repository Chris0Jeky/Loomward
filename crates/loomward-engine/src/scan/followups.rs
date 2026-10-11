//! Synthetic regression oracles for #180 and #269.
use super::*;
use std::sync::{atomic::AtomicUsize, Mutex};

#[cfg(windows)]
#[test]
fn native_scan_completes_with_different_case_root_alias() {
    use loomward_windows::enumerate::NativeSource;
    if skip_elevated() {
        return;
    }
    let temp = canonical_tempdir();
    let path = temp.path().join("synthetic-case-root");
    std::fs::create_dir_all(path.join("child")).unwrap();
    std::fs::write(path.join("child/synthetic-file"), b"synthetic").unwrap();
    let alias = temp.path().join("SYNTHETIC-CASE-ROOT");
    for strategy in [Strategy::Extended, Strategy::Both, Strategy::Find] {
        let source = NativeSource::with_strategy(strategy);
        let (_, id) = source.open_root(&path).unwrap();
        let root = GrantedRoot::new(
            RootId::new("rt_case_alias").unwrap(),
            alias.clone(),
            DatasetClass::Synthetic,
            id,
        );
        let report = run_scan(
            &source,
            &root,
            1,
            Arc::new(MemorySink::default()),
            Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
            Arc::new(AtomicBool::new(false)),
            ScanOptions {
                workers: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(report.complete, "{strategy:?}: {report:?}");
        assert_eq!(
            (report.failed, report.totals.dirs, report.totals.files),
            (0, 1, 1)
        );
    }
}

enum Notice<'a> {
    Message(ScanMessage),
    Prepare(&'a mut OpenedIdentity),
    Refresh,
}
type Hook = dyn Fn(&RootId, Notice<'_>, &MemorySink) -> crate::EngineResult<()> + Send + Sync;
struct ProbeWriter {
    sink: MemorySink,
    hook: Box<Hook>,
}
impl ScanSink for ProbeWriter {
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
        mut identity: OpenedIdentity,
    ) -> crate::EngineResult<ListingTicket> {
        (self.hook)(root, Notice::Prepare(&mut identity), &self.sink)?;
        self.sink.prepare_listing(root, run, parent, name, identity)
    }
    fn refresh_listing(
        &self,
        root: &RootId,
        run: u64,
        ticket: ListingTicket,
    ) -> crate::EngineResult<ListingTicket> {
        (self.hook)(root, Notice::Refresh, &self.sink)?;
        self.sink.refresh_listing(root, run, ticket)
    }
    fn consume(&self, root: &RootId, message: ScanMessage) -> crate::EngineResult<()> {
        let notice = match &message {
            ScanMessage::ListingDone {
                run,
                ticket,
                outcome,
            } => Some(ScanMessage::ListingDone {
                run: *run,
                ticket: *ticket,
                outcome: *outcome,
            }),
            ScanMessage::DirFinal {
                run,
                ticket,
                sums,
                complete,
            } => Some(ScanMessage::DirFinal {
                run: *run,
                ticket: *ticket,
                sums: *sums,
                complete: *complete,
            }),
            _ => None,
        };
        self.sink.consume(root, message)?;
        notice.map_or(Ok(()), |message| {
            (self.hook)(root, Notice::Message(message), &self.sink)
        })
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
fn scan(source: &impl DirSource, sink: Arc<dyn ScanSink>) -> ScanReport {
    run_scan(
        source,
        &root(),
        1,
        sink,
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        Arc::new(AtomicBool::new(false)),
        ScanOptions {
            workers: 1,
            ..Default::default()
        },
    )
    .unwrap()
}
fn obsolete() -> crate::EngineError {
    crate::EngineError::StaleGeneration {
        message: "synthetic dirty epoch".into(),
        detail: Some(
            loomward_protocol::Detail::new(std::collections::BTreeMap::from([(
                "reason".into(),
                serde_json::json!("listing_revision_changed"),
            )]))
            .unwrap(),
        ),
    }
}

#[cfg(windows)]
fn native_relist_containment(mode: u8) {
    use loomward_windows::enumerate::{NativeDir, NativeSource};
    if skip_elevated() {
        return;
    }
    // All effects are confined to this disposable fixture, including the outside sibling.
    let temp = canonical_tempdir();
    for retained in [false, true] {
        for strategy in [Strategy::Extended, Strategy::Both, Strategy::Find] {
            let base = temp.path().join(format!("case-{retained}-{strategy:?}"));
            let path = base.join("root");
            let child = path.join("child");
            std::fs::create_dir_all(&child).unwrap();
            std::fs::write(child.join("first"), b"synthetic").unwrap();
            struct Source {
                native: NativeSource,
                retained: bool,
                child_lists: AtomicUsize,
                relisted_entries: AtomicUsize,
            }
            impl DirSource for Source {
                type Dir = NativeDir;
                fn strategy(&self) -> Strategy {
                    self.native.strategy()
                }
                fn open_root(
                    &self,
                    p: &std::path::Path,
                ) -> Result<(NativeDir, OpenedIdentity), SourceError> {
                    self.native.open_root(p)
                }
                fn open_child(
                    &self,
                    p: &NativeDir,
                    e: &RawEntry<'_>,
                ) -> Result<(NativeDir, OpenedIdentity), SourceError> {
                    self.native.open_child(p, e)
                }
                fn can_reopen(&self, d: &NativeDir, id: OpenedIdentity) -> bool {
                    !self.retained && self.native.can_reopen(d, id)
                }
                fn reopen(
                    &self,
                    p: &NativeDir,
                    root: OpenedIdentity,
                    id: OpenedIdentity,
                ) -> Result<(NativeDir, OpenedIdentity), SourceError> {
                    self.native.reopen(p, root, id)
                }
                fn list(
                    &self,
                    d: &NativeDir,
                    sink: &mut dyn FnMut(RawEntry<'_>) -> Flow,
                ) -> ListOutcome {
                    self.native.list(d, &mut |e| {
                        // Only the child contains files; root contains one directory.
                        if e.attributes & 0x10 == 0 && self.child_lists.load(Ordering::Relaxed) > 0
                        {
                            self.relisted_entries.fetch_add(1, Ordering::Relaxed);
                        }
                        sink(e)
                    })
                }
            }
            let source = Arc::new(Source {
                native: NativeSource::with_strategy(strategy),
                retained,
                child_lists: AtomicUsize::new(0),
                relisted_entries: AtomicUsize::new(0),
            });
            let expected = source.open_root(&path).unwrap().1;
            let grant = GrantedRoot::new(
                RootId::new("rt_containment_fixture").unwrap(),
                path.clone(),
                DatasetClass::Synthetic,
                expected,
            );
            let outcomes = Arc::new(Mutex::new(Vec::new()));
            let observed = outcomes.clone();
            let probe = source.clone();
            let moved = AtomicBool::new(false);
            let sink = Arc::new(ProbeWriter {
                sink: MemorySink::default(),
                hook: Box::new(move |root, notice, sink| {
                    match notice {
                        Notice::Message(ScanMessage::ListingDone {
                            ticket, outcome, ..
                        }) if ticket.dir == 2 => {
                            observed.lock().unwrap().push(outcome);
                            if probe.child_lists.fetch_add(1, Ordering::Relaxed) == 0 {
                                sink.dirty_again(root, ticket.dir);
                                return Err(obsolete());
                            }
                        }
                        // Refresh runs after workers join: the move is exactly between listings.
                        Notice::Refresh if !moved.swap(true, Ordering::Relaxed) => {
                            let destination = match mode {
                                0 => base.join("root-sibling"),
                                1 => path.join("moved"),
                                2 if retained => child.clone(),
                                2 => base.join("renamed-root").join("child"),
                                _ => unreachable!(),
                            };
                            if mode == 2 && retained {
                                // Windows refuses renaming a directory with an open descendant.
                                assert_eq!(
                                    std::fs::rename(&path, base.join("renamed-root"))
                                        .unwrap_err()
                                        .kind(),
                                    std::io::ErrorKind::PermissionDenied
                                );
                            } else if mode == 2 {
                                std::fs::rename(&path, base.join("renamed-root")).map_err(|e| {
                                    EngineError::Internal {
                                        message: format!("fixture root rename: {e}"),
                                        detail: None,
                                    }
                                })?;
                                std::fs::create_dir_all(&child).unwrap();
                                std::fs::write(child.join("ungranted"), b"synthetic").unwrap();
                            } else {
                                std::fs::rename(&child, &destination).unwrap();
                            }
                            std::fs::write(destination.join("late"), b"synthetic").unwrap();
                        }
                        _ => {}
                    }
                    Ok(())
                }),
            });
            let bytes = Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES));
            let report = run_scan(
                source.as_ref(),
                &grant,
                1,
                sink.clone(),
                bytes.clone(),
                Arc::new(AtomicBool::new(false)),
                ScanOptions {
                    workers: 1,
                    ..Default::default()
                },
            )
            .unwrap();
            let allowed = mode != 0;
            assert_eq!(
                report.complete, allowed,
                "mode={mode}, retained={retained}, {report:?}"
            );
            assert_eq!(sink.sink.sweep_allowed(grant.root_id()), allowed);
            assert_eq!(
                source.relisted_entries.load(Ordering::Relaxed),
                if allowed { 2 } else { 0 }
            );
            assert_eq!(
                *outcomes.lock().unwrap(),
                vec![
                    ListOutcome::Complete,
                    if allowed {
                        ListOutcome::Complete
                    } else {
                        ListOutcome::Incomplete(IncompleteReason::ContainmentUnproven)
                    }
                ]
            );
            assert_eq!(report.totals.files, if allowed { 2 } else { 0 });
            assert_eq!(report.failed, if allowed { 0 } else { 1 });
            assert_eq!(bytes.used(), 0);
        }
    }
}

#[cfg(windows)]
#[test]
fn native_relist_containment_refuses_moved_out_directory() {
    native_relist_containment(0);
}
#[cfg(windows)]
#[test]
fn native_relist_containment_allows_move_within_root() {
    native_relist_containment(1);
}
#[cfg(windows)]
#[test]
fn native_relist_containment_follows_renamed_root_handle() {
    native_relist_containment(2);
}

#[test]
fn cancelled_multidirectory_scan_preserves_partial_totals() {
    struct GatedTree {
        entered: std::sync::mpsc::SyncSender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl DirSource for GatedTree {
        type Dir = u64;
        fn strategy(&self) -> Strategy {
            Strategy::Portable
        }
        fn open_root(&self, p: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
            Tree.open_root(p)
        }
        fn open_child(
            &self,
            p: &u64,
            e: &RawEntry<'_>,
        ) -> Result<(u64, OpenedIdentity), SourceError> {
            Tree.open_child(p, e)
        }
        fn list(&self, dir: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
            if *dir == 2 {
                self.entered.send(()).unwrap();
                self.release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap();
            } else if Tree.list(&2, sink) != ListOutcome::Complete {
                return ListOutcome::Incomplete(IncompleteReason::Cancelled);
            }
            Tree.list(dir, sink)
        }
    }
    let (entered, waiting) = std::sync::mpsc::sync_channel(1);
    let (release, released) = std::sync::mpsc::sync_channel(1);
    let source = GatedTree {
        entered,
        release: Mutex::new(released),
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let bytes = Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES));
    let sink = Arc::new(MemorySink::default());
    let report = std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            run_scan(
                &source,
                &root(),
                1,
                sink.clone(),
                bytes.clone(),
                cancel.clone(),
                ScanOptions {
                    workers: 1,
                    ..Default::default()
                },
            )
        });
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        cancel.store(true, Ordering::Release);
        release.send(()).unwrap();
        worker.join().unwrap().unwrap()
    });
    assert_eq!(
        report.totals,
        Sums {
            files: 1,
            dirs: 1,
            logical: 123,
            allocated: Some(4096)
        }
    );
    assert!(!report.complete);
    assert!(!sink.sweep_allowed(root().root_id()));
    assert_eq!(bytes.used(), 0);
}

#[test]
fn cancellation_stops_reconciliation_before_the_next_refresh() {
    let cancel = Arc::new(AtomicBool::new(false));
    let refreshes = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(ProbeWriter {
        sink: MemorySink::default(),
        hook: Box::new({
            let cancel = cancel.clone();
            let refreshes = refreshes.clone();
            move |root, notice, sink| {
                match notice {
                    Notice::Message(ScanMessage::ListingDone { ticket, .. }) => {
                        sink.dirty_again(root, ticket.dir);
                        return Err(obsolete());
                    }
                    Notice::Refresh => {
                        refreshes.fetch_add(1, Ordering::Relaxed);
                        cancel.store(true, Ordering::Release);
                    }
                    _ => {}
                }
                Ok(())
            }
        }),
    });
    let report = run_scan(
        &Tree,
        &root(),
        1,
        sink.clone(),
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        cancel,
        ScanOptions {
            workers: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(refreshes.load(Ordering::Relaxed), 1);
    assert_eq!(report.totals.files, 1);
    assert!(!report.complete);
    assert!(!sink.sink.sweep_allowed(root().root_id()));
}

#[test]
fn released_slot_losing_identity_stays_partial_without_panicking() {
    struct Source {
        opens: AtomicUsize,
        root_lists: AtomicUsize,
        child_releases: AtomicUsize,
        finished: (Mutex<bool>, std::sync::Condvar),
    }
    impl DirSource for Source {
        type Dir = u64;
        fn strategy(&self) -> Strategy {
            Strategy::Portable
        }
        fn open_root(&self, p: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
            Tree.open_root(p)
        }
        fn open_child(
            &self,
            p: &u64,
            e: &RawEntry<'_>,
        ) -> Result<(u64, OpenedIdentity), SourceError> {
            let (dir, mut id) = Tree.open_child(p, e)?;
            if self.opens.fetch_add(1, Ordering::Relaxed) > 0 {
                id.id = None;
                id.basis = IdBasis::None;
            }
            Ok((dir, id))
        }
        fn list(&self, dir: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
            if *dir == 1 && self.root_lists.fetch_add(1, Ordering::Relaxed) == 1 {
                let (done, _) = self
                    .finished
                    .1
                    .wait_timeout_while(
                        self.finished.0.lock().unwrap(),
                        Duration::from_secs(2),
                        |done| !*done,
                    )
                    .unwrap();
                assert!(
                    *done,
                    "child must release its relist handle before parent adoption"
                );
            }
            Tree.list(dir, sink)
        }
        fn can_reopen(&self, dir: &Self::Dir, id: OpenedIdentity) -> bool {
            if *dir == 2
                && id.id == Some(FileIdObs::Id64(2))
                && self.child_releases.fetch_add(1, Ordering::Relaxed) == 1
            {
                *self.finished.0.lock().unwrap() = true;
                self.finished.1.notify_all();
            }
            true
        }
        fn reopen(
            &self,
            _: &u64,
            _: OpenedIdentity,
            id: OpenedIdentity,
        ) -> Result<(u64, OpenedIdentity), SourceError> {
            let Some(FileIdObs::Id64(dir)) = id.id else {
                unreachable!()
            };
            Ok((dir, id))
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let source = Source {
            opens: AtomicUsize::new(0),
            root_lists: AtomicUsize::new(0),
            child_releases: AtomicUsize::new(0),
            finished: Default::default(),
        };
        let listings = Mutex::new(std::collections::HashMap::new());
        let sink = Arc::new(ProbeWriter {
            sink: MemorySink::default(),
            hook: Box::new(move |root, notice, sink| {
                match notice {
                    // Model a catalogue resolving the same slot after native identity is lost.
                    Notice::Prepare(id) if id.id.is_none() => *id = identity(2),
                    Notice::Message(ScanMessage::ListingDone { ticket, .. }) => {
                        let mut listings = listings.lock().unwrap();
                        let count = listings.entry(ticket.dir).or_insert(0);
                        *count += 1;
                        if *count == 1 || (ticket.dir == 2 && *count == 2) {
                            sink.dirty_again(root, ticket.dir);
                            return Err(obsolete());
                        }
                    }
                    _ => {}
                }
                Ok(())
            }),
        });
        let bytes = Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES));
        let result = run_scan(
            &source,
            &root(),
            1,
            sink.clone(),
            bytes.clone(),
            Arc::new(AtomicBool::new(false)),
            ScanOptions {
                workers: 2,
                ..Default::default()
            },
        );
        let _ = tx.send((
            result,
            sink.sink.sweep_allowed(root().root_id()),
            bytes.used(),
        ));
    });
    let (result, sweep, used) = rx
        .recv_timeout(Duration::from_secs(4))
        .expect("a released slot with no identity must terminate without a coordinator panic");
    let report = result.unwrap();
    assert_eq!(report.totals.files, 1);
    assert_eq!(report.failed, 1);
    assert!(!report.complete);
    assert!(!sweep);
    assert_eq!(used, 0);
}

struct CounterTree(AtomicUsize);
impl DirSource for CounterTree {
    type Dir = u64;
    fn strategy(&self) -> Strategy {
        Strategy::Portable
    }
    fn open_root(&self, p: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
        Tree.open_root(p)
    }
    fn open_child(&self, p: &u64, e: &RawEntry<'_>) -> Result<(u64, OpenedIdentity), SourceError> {
        if e.file_id == Some(FileIdObs::Id64(4)) {
            return Err(SourceError::AccessDenied);
        }
        Tree.open_child(p, e)
    }
    fn list(&self, dir: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        if *dir == 1 {
            self.0.fetch_add(1, Ordering::Relaxed);
            for (id, attributes) in [(4, 0x10), (5, 0x410)] {
                let e = RawEntry {
                    name: &[id as u16],
                    file_id: Some(FileIdObs::Id64(id)),
                    attributes,
                    reparse_tag: None,
                    end_of_file: 0,
                    allocation_size: Some(0),
                    creation: None,
                    last_write: None,
                    change: None,
                    last_access: None,
                };
                if sink(e) == Flow::Stop {
                    return ListOutcome::Incomplete(IncompleteReason::Cancelled);
                }
            }
        }
        Tree.list(dir, sink)
    }
}
#[test]
fn relists_replace_examined_skipped_and_failed_counts() {
    let source = CounterTree(Default::default());
    let sink = Arc::new(DirtyWriter {
        sink: MemorySink::default(),
        dirtied: AtomicBool::new(false),
        on_final: false,
        always: false,
    });
    let report = scan(&source, sink);
    assert!(source.0.load(Ordering::Relaxed) > 1);
    assert_eq!((report.examined, report.skipped, report.failed), (4, 1, 1));
    assert!(!report.complete);
}

struct CollisionTree;
impl DirSource for CollisionTree {
    type Dir = u64;
    fn strategy(&self) -> Strategy {
        Strategy::Portable
    }
    fn open_root(&self, p: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
        Tree.open_root(p)
    }
    fn open_child(&self, p: &u64, e: &RawEntry<'_>) -> Result<(u64, OpenedIdentity), SourceError> {
        Tree.open_child(p, e)
    }
    fn list(&self, dir: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        Tree.list(dir, &mut |mut e| {
            if *dir == 1 {
                let flow = sink(e.clone());
                if flow == Flow::Stop {
                    return flow;
                }
                e.name = &[121];
            }
            sink(e)
        })
    }
}
#[test]
fn duplicate_identity_is_counted_and_never_complete() {
    let report = scan(&CollisionTree, Arc::new(MemorySink::default()));
    assert_eq!(report.duplicate_identity, 1);
    assert!(!report.complete);
    let report = scan(&Tree, Arc::new(MemorySink::default()));
    assert_eq!(report.duplicate_identity, 0);
    assert!(report.complete);
    let source = MovingTree {
        x_listings: Default::default(),
        unique: true,
    };
    let report = scan(&source, Arc::new(MemorySink::default()));
    assert_eq!(report.duplicate_identity, 1);
    assert!(!report.complete);
}

#[test]
fn a_clean_relist_does_not_erase_duplicate_identity_evidence() {
    struct Source(AtomicUsize);
    impl DirSource for Source {
        type Dir = u64;
        fn strategy(&self) -> Strategy {
            Strategy::Portable
        }
        fn open_root(&self, p: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
            Tree.open_root(p)
        }
        fn open_child(
            &self,
            p: &u64,
            e: &RawEntry<'_>,
        ) -> Result<(u64, OpenedIdentity), SourceError> {
            Tree.open_child(p, e)
        }
        fn list(&self, d: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
            if *d == 1 && self.0.fetch_add(1, Ordering::Relaxed) == 0 {
                CollisionTree.list(d, sink)
            } else {
                Tree.list(d, sink)
            }
        }
    }
    let sink = Arc::new(DirtyWriter {
        sink: MemorySink::default(),
        dirtied: AtomicBool::new(false),
        on_final: false,
        always: false,
    });
    let report = scan(&Source(AtomicUsize::new(0)), sink.clone());
    assert_eq!(report.duplicate_identity, 1);
    assert!(!report.complete);
    assert!(!sink.sink.sweep_allowed(root().root_id()));
}

#[derive(Default)]
struct NameOnlyTree {
    opens: AtomicUsize,
    listings: Mutex<Vec<usize>>,
}
impl DirSource for NameOnlyTree {
    type Dir = (u64, usize);
    fn strategy(&self) -> Strategy {
        Strategy::Portable
    }
    fn open_root(&self, _: &std::path::Path) -> Result<(Self::Dir, OpenedIdentity), SourceError> {
        Ok(((1, 0), identity(1)))
    }
    fn open_child(
        &self,
        _: &Self::Dir,
        _: &RawEntry<'_>,
    ) -> Result<(Self::Dir, OpenedIdentity), SourceError> {
        let mut id = identity(2);
        id.id = None;
        id.basis = IdBasis::None;
        Ok(((2, self.opens.fetch_add(1, Ordering::Relaxed) + 1), id))
    }
    fn list(&self, dir: &Self::Dir, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        if dir.0 == 2 {
            self.listings.lock().unwrap().push(dir.1);
        }
        Tree.list(&dir.0, &mut |mut e| {
            if dir.0 == 1 {
                e.file_id = None;
            }
            sink(e)
        })
    }
}
#[test]
fn dirty_name_only_parent_and_child_are_listed_once_per_round() {
    let seen = Mutex::new(std::collections::HashSet::new());
    let sink = Arc::new(ProbeWriter {
        sink: MemorySink::default(),
        hook: Box::new(move |root, message, sink| {
            if let Notice::Message(ScanMessage::ListingDone { ticket, .. }) = message {
                if seen.lock().unwrap().insert(ticket.dir) {
                    sink.dirty_again(root, ticket.dir);
                    return Err(obsolete());
                }
            }
            Ok(())
        }),
    });
    let source = NameOnlyTree::default();
    let report = scan(&source, sink);
    assert!(report.complete, "{report:?}");
    assert_eq!(source.listings.lock().unwrap().len(), 2);
}
#[test]
fn name_only_relisting_uses_the_freshly_opened_handle() {
    let dirtied = AtomicBool::new(false);
    let sink = Arc::new(ProbeWriter {
        sink: MemorySink::default(),
        hook: Box::new(move |root, message, sink| {
            if let Notice::Message(ScanMessage::ListingDone { ticket, .. }) = message {
                if !dirtied.swap(true, Ordering::Relaxed) {
                    sink.dirty_again(root, ticket.dir);
                    return Err(obsolete());
                }
            }
            Ok(())
        }),
    });
    let source = NameOnlyTree::default();
    let report = scan(&source, sink);
    assert!(report.complete, "{report:?}");
    assert_eq!(*source.listings.lock().unwrap(), [1, 2]);
}

struct PanicUnderArena;
impl DirSource for PanicUnderArena {
    type Dir = u64;
    fn strategy(&self) -> Strategy {
        Strategy::Portable
    }
    fn open_root(&self, p: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
        Tree.open_root(p)
    }
    fn open_child(&self, p: &u64, e: &RawEntry<'_>) -> Result<(u64, OpenedIdentity), SourceError> {
        Tree.open_child(p, e)
    }
    fn list(&self, dir: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        Tree.list(dir, sink)
    }
    fn can_reopen(&self, _: &Self::Dir, _: OpenedIdentity) -> bool {
        panic!("synthetic panic under arena lock")
    }
}
#[test]
fn poisoned_arena_fails_the_run_and_releases_bytes() {
    let sink = Arc::new(MemorySink::default());
    let bytes = Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES));
    let (tx, rx) = std::sync::mpsc::channel();
    let writer = sink.clone();
    let budget = bytes.clone();
    std::thread::spawn(move || {
        let report = run_scan(
            &PanicUnderArena,
            &root(),
            1,
            writer,
            budget,
            Arc::new(AtomicBool::new(false)),
            ScanOptions {
                workers: 1,
                ..Default::default()
            },
        );
        let _ = tx.send(report);
    });
    let report = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("a poisoned arena must terminate the run");
    assert_eq!(
        report.unwrap_err().to_body().message.as_str(),
        "scan worker panicked"
    );
    assert!(!sink.sweep_allowed(root().root_id()));
    assert_eq!(bytes.used(), 0);
}

#[test]
fn reopen_failure_or_changed_identity_keeps_the_run_partial() {
    struct Source(u8, AtomicUsize);
    impl DirSource for Source {
        type Dir = u64;
        fn strategy(&self) -> Strategy {
            Strategy::Portable
        }
        fn open_root(&self, p: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
            Tree.open_root(p)
        }
        fn open_child(
            &self,
            p: &u64,
            e: &RawEntry<'_>,
        ) -> Result<(u64, OpenedIdentity), SourceError> {
            Tree.open_child(p, e)
        }
        fn list(&self, dir: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
            self.1.fetch_add(1, Ordering::Relaxed);
            let outcome = Tree.list(dir, sink);
            if *dir == 2 && self.0 >= 3 {
                ListOutcome::Incomplete(IncompleteReason::Io(5))
            } else {
                outcome
            }
        }
        fn can_reopen(&self, _: &Self::Dir, _: OpenedIdentity) -> bool {
            true
        }
        fn reopen(
            &self,
            _: &u64,
            _: OpenedIdentity,
            expected: OpenedIdentity,
        ) -> Result<(u64, OpenedIdentity), SourceError> {
            if self.0 == 2 || self.0 == 3 {
                return Err(SourceError::AccessDenied);
            }
            let Some(FileIdObs::Id64(id)) = expected.id else {
                unreachable!()
            };
            Ok((
                id,
                if self.0 == 1 || self.0 == 4 {
                    identity(99)
                } else {
                    expected
                },
            ))
        }
    }
    for mode in [0, 1, 2, 3, 4] {
        let source = Source(mode, AtomicUsize::new(0));
        let sink = Arc::new(DirtyWriter {
            sink: MemorySink::default(),
            dirtied: AtomicBool::new(false),
            on_final: true,
            always: false,
        });
        let report = scan(&source, sink.clone());
        assert_eq!(report.complete, mode == 0);
        assert_eq!(sink.sink.sweep_allowed(root().root_id()), mode == 0);
        assert_eq!(report.failed, u64::from(mode != 0) + u64::from(mode >= 3));
        assert_eq!(
            report.totals.files, 1,
            "unknown remains the prior observation"
        );
        assert_eq!(
            source.1.load(Ordering::Relaxed),
            if mode == 0 { 3 } else { 2 }
        );
    }
}

// A large owned handle makes the arena's retained-versus-transient cost observable in a small tree.
struct LargeDir {
    id: u64,
    _storage: [u8; 65536],
}
struct ReopenTree;
impl DirSource for ReopenTree {
    type Dir = LargeDir;
    fn strategy(&self) -> Strategy {
        Strategy::Portable
    }
    fn open_root(&self, _: &std::path::Path) -> Result<(LargeDir, OpenedIdentity), SourceError> {
        Ok((
            LargeDir {
                id: 1,
                _storage: [0; 65536],
            },
            identity(1),
        ))
    }
    fn open_child(
        &self,
        _: &LargeDir,
        e: &RawEntry<'_>,
    ) -> Result<(LargeDir, OpenedIdentity), SourceError> {
        let Some(FileIdObs::Id64(id)) = e.file_id else {
            unreachable!()
        };
        Ok((
            LargeDir {
                id,
                _storage: [0; 65536],
            },
            identity(id),
        ))
    }
    fn list(&self, dir: &LargeDir, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        if dir.id == 96 {
            return ListOutcome::Complete;
        }
        Tree.list(&1, &mut |mut e| {
            e.file_id = Some(FileIdObs::Id64(dir.id + 1));
            sink(e)
        })
    }
    fn can_reopen(&self, _: &Self::Dir, _: OpenedIdentity) -> bool {
        true
    }
    fn reopen(
        &self,
        _: &LargeDir,
        _: OpenedIdentity,
        expected: OpenedIdentity,
    ) -> Result<(LargeDir, OpenedIdentity), SourceError> {
        let Some(FileIdObs::Id64(id)) = expected.id else {
            unreachable!()
        };
        Ok((
            LargeDir {
                id,
                _storage: [0; 65536],
            },
            expected,
        ))
    }
}
#[test]
fn listed_handles_release_arena_capacity_for_later_directories() {
    let bytes = Arc::new(crate::budgets::ByteBudget::new(4 * 1024 * 1024));
    let report = run_scan(
        &ReopenTree,
        &root(),
        1,
        Arc::new(MemorySink::default()),
        bytes.clone(),
        Arc::new(AtomicBool::new(false)),
        ScanOptions {
            workers: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(report.complete, "{report:?}");
    assert_eq!(report.totals.dirs, 95);
    assert_eq!(bytes.used(), 0);
}

#[test]
fn reopened_handles_remain_bounded_by_the_arena_reservation() {
    let seen = Mutex::new(std::collections::HashSet::new());
    let sink = Arc::new(ProbeWriter {
        sink: MemorySink::default(),
        hook: Box::new(move |root, message, sink| {
            if let Notice::Message(ScanMessage::ListingDone { ticket, .. }) = message {
                if seen.lock().unwrap().insert(ticket.dir) {
                    sink.dirty_again(root, ticket.dir);
                    return Err(obsolete());
                }
            }
            Ok(())
        }),
    });
    let bytes = Arc::new(crate::budgets::ByteBudget::new(4 * 1024 * 1024));
    let report = run_scan(
        &ReopenTree,
        &root(),
        1,
        sink.clone(),
        bytes.clone(),
        Arc::new(AtomicBool::new(false)),
        ScanOptions {
            workers: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        report.limit_hit,
        "reopening every large handle cannot fit the arena"
    );
    assert!(!report.complete);
    assert_eq!(report.failed, 0, "a budget limit is not an I/O error");
    assert!(!sink.sink.sweep_allowed(root().root_id()));
    assert_eq!(bytes.used(), 0);
}

#[cfg(windows)]
#[test]
fn native_child_handles_close_but_root_stays_pinned_until_finalization() {
    use loomward_windows::enumerate::{NativeDir, NativeSource};
    if skip_elevated() {
        return;
    }
    struct TrackedDir(NativeDir, Arc<AtomicUsize>);
    impl Drop for TrackedDir {
        fn drop(&mut self) {
            self.1.fetch_sub(1, Ordering::Relaxed);
        }
    }
    struct Source(NativeSource, Arc<AtomicUsize>);
    impl DirSource for Source {
        type Dir = TrackedDir;
        fn strategy(&self) -> Strategy {
            self.0.strategy()
        }
        fn open_root(
            &self,
            p: &std::path::Path,
        ) -> Result<(TrackedDir, OpenedIdentity), SourceError> {
            let (d, id) = self.0.open_root(p)?;
            self.1.fetch_add(1, Ordering::Relaxed);
            Ok((TrackedDir(d, self.1.clone()), id))
        }
        fn open_child(
            &self,
            p: &TrackedDir,
            e: &RawEntry<'_>,
        ) -> Result<(TrackedDir, OpenedIdentity), SourceError> {
            let (d, id) = self.0.open_child(&p.0, e)?;
            self.1.fetch_add(1, Ordering::Relaxed);
            Ok((TrackedDir(d, self.1.clone()), id))
        }
        fn list(&self, d: &TrackedDir, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
            self.0.list(&d.0, sink)
        }
        fn can_reopen(&self, dir: &Self::Dir, id: OpenedIdentity) -> bool {
            self.0.can_reopen(&dir.0, id)
        }
    }
    let temp = canonical_tempdir();
    std::fs::create_dir(temp.path().join("child")).unwrap();
    let live = Arc::new(AtomicUsize::new(0));
    let source = Source(NativeSource::default(), live.clone());
    let expected = source.open_root(temp.path()).unwrap().1;
    let grant = GrantedRoot::new(
        RootId::new("rt_handle_fixture").unwrap(),
        temp.path().to_owned(),
        DatasetClass::Synthetic,
        expected,
    );
    let sink = Arc::new(ProbeWriter {
        sink: MemorySink::default(),
        hook: Box::new(move |_, message, _| {
            if matches!(message, Notice::Message(ScanMessage::DirFinal { .. })) {
                assert_eq!(live.load(Ordering::Relaxed), 1);
            }
            Ok(())
        }),
    });
    let report = run_scan(
        &source,
        &grant,
        1,
        sink,
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        Arc::new(AtomicBool::new(false)),
        ScanOptions::default(),
    )
    .unwrap();
    assert!(report.complete);
}
