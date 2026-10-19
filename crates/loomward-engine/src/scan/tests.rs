use super::*;
use crate::{Engine, EngineConfig};
use loomward_protocol::{DatasetClass, RootId};

#[path = "followups.rs"]
mod followups;

#[test]
fn refused_scan_begin_does_not_finish_the_existing_run() {
    let root = root();
    let sink = Arc::new(MemorySink::default());
    sink.begin_run(root.root_id(), 7, &RunScope::FullRoot)
        .unwrap();
    assert!(matches!(
        run_scan(
            &Tree,
            &root,
            7,
            sink.clone(),
            Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
            Arc::new(AtomicBool::new(false)),
            ScanOptions::default(),
        ),
        Err(EngineError::Busy { .. })
    ));
    // The accepted run is still active, so it can reserve and finish its own listing.
    let identity = Tree.open_root(root.path()).unwrap().1;
    sink.prepare_listing(root.root_id(), 7, None, &[], identity)
        .unwrap();
    sink.finish_run(root.root_id(), 7, &RunScope::FullRoot, false)
        .unwrap();
}

#[test]
fn granted_root_rejects_foreign_catalogue_identities_before_opening_a_path() {
    let state = tempfile::tempdir().unwrap();
    let grant = root();
    let _conn = duplicate_grant_store(state.path(), &grant);
    rusqlite::Connection::open(state.path().join("catalog.db"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT);
            INSERT INTO meta VALUES('instance_id','fixture-instance');
            CREATE TABLE root(id INTEGER PRIMARY KEY,grant_id INTEGER);
            INSERT INTO root VALUES(7,1);",
        )
        .unwrap();
    let engine = Engine::open(EngineConfig::new(
        state.path().to_owned(),
        DatasetClass::Synthetic,
    ))
    .unwrap();
    for (row, instance) in [
        (7, "foreign-instance"),
        (7, ""),
        (8, "fixture-instance"),
        (0, "fixture-instance"),
        (-7, "fixture-instance"),
    ] {
        assert_eq!(
            engine
                .granted_root(
                    RootId::new("rt_opaque").unwrap(),
                    1,
                    &CatalogRootIdentity {
                        row,
                        instance: instance.into()
                    },
                )
                .unwrap_err(),
            EngineError::PermissionDenied {
                message: "grant_root_mismatch".into()
            }
        );
    }
}

/// q-10: personal content reads are open, but only where the grant, the engine and the durable
/// store all name the same dataset class; a session never reads the other class's roots.
#[test]
fn duplicate_admission_binds_grant_engine_and_store_to_one_class() {
    use DatasetClass::{Personal, Synthetic};
    for (engine_class, grant_class, store_class, refused) in [
        (Personal, Personal, "personal", None),
        (Synthetic, Synthetic, "synthetic", None),
        (
            Personal,
            Synthetic,
            "synthetic",
            Some("dataset_class_mismatch"),
        ),
        (
            Synthetic,
            Personal,
            "personal",
            Some("dataset_class_mismatch"),
        ),
        (
            Personal,
            Personal,
            "synthetic",
            Some("dataset_class_mismatch"),
        ),
        (
            Synthetic,
            Synthetic,
            "personal",
            Some("dataset_class_mismatch"),
        ),
    ] {
        let state = tempfile::tempdir().unwrap();
        let mut grant = root();
        grant.dataset_class = grant_class;
        let _conn = duplicate_grant_store_as(state.path(), &grant, store_class);
        let engine = Arc::new(
            Engine::open(EngineConfig::new(state.path().to_owned(), engine_class)).unwrap(),
        );
        let admitted = engine.duplicate_read_admission(&grant);
        match refused {
            None => assert!(admitted.is_ok(), "{engine_class:?}: {admitted:?}"),
            Some(message) => {
                assert_eq!(admitted.unwrap_err().to_body().message.as_str(), message);
                let budget = serde_json::from_value(
                    serde_json::json!({"max_candidates":200,"max_bytes":"100","max_ms":1000}),
                )
                .unwrap();
                assert_eq!(
                    engine
                        .duplicates_start(grant, Default::default(), budget)
                        .unwrap_err()
                        .to_body()
                        .message
                        .as_str(),
                    message
                );
            }
        }
    }
}

#[cfg(windows)]
#[test]
fn duplicate_grant_returns_evidence_for_its_own_class() {
    // q-10: a personal owner grant reads like a synthetic lab grant, through the same reader.
    for class in [DatasetClass::Synthetic, DatasetClass::Personal] {
        duplicate_grant_returns_evidence(class);
    }
}

#[cfg(windows)]
fn duplicate_grant_returns_evidence(class: DatasetClass) {
    use loomward_windows::{
        duplicate_reader::{Budget, Entry},
        enumerate::{DirSource, NativeSource},
        identity,
    };
    if loomward_windows::enumerate::running_elevated() {
        assert_eq!(
            NativeSource::default()
                .open_root(std::path::Path::new("synthetic"))
                .unwrap_err(),
            source::SourceError::Refused
        );
        return;
    }
    let temp = tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
    for name in ["a", "b"] {
        std::fs::write(temp.path().join(name), b"same").unwrap();
    }
    let expected = NativeSource::default().open_root(temp.path()).unwrap().1;
    let grant = GrantedRoot::new(
        RootId::new("rt_duplicate_fixture").unwrap(),
        temp.path().to_owned(),
        class,
        expected,
    );
    let entries = ["a", "b"].map(|name| Entry {
        relative: name.into(),
        expected: identity::observe(&temp.path().join(name)).unwrap(),
    });
    let state = canonical_tempdir();
    let store_class = match class {
        DatasetClass::Synthetic => "synthetic",
        DatasetClass::Personal => "personal",
    };
    let conn = duplicate_grant_store_as(state.path(), &grant, store_class);
    let engine = Engine::open(EngineConfig::new(state.path().to_owned(), class)).unwrap();
    let report = engine
        .inspect_duplicates(
            &grant,
            &entries,
            Budget {
                bytes: 100,
                time: std::time::Duration::from_secs(10),
            },
        )
        .unwrap();
    assert_eq!(report.groups, vec![vec![0, 1]]);
    assert!(!report.partial);
    // Revocation requested while a job's reader runs on its own cancel flag: the final gate
    // refuses the finished report in its entirety rather than return names from a revoked root.
    let root_cancel = engine
        .scan_root_gate(grant.key())
        .lock()
        .unwrap()
        .duplicate_cancel
        .clone();
    root_cancel.store(true, Ordering::Release);
    let refused = engine
        .inspect_duplicates_cancel(
            &grant,
            &entries,
            duplicate_budget(),
            Some(Arc::new(AtomicBool::new(false))),
        )
        .unwrap_err();
    assert_eq!(refused.to_body().message.as_str(), "cancelled");
    root_cancel.store(false, Ordering::Release);
    conn.execute("UPDATE root_grant SET state='revoked'", [])
        .unwrap();
    let err = engine
        .inspect_duplicates(&grant.clone(), &entries, duplicate_budget())
        .unwrap_err();
    assert_eq!(err.to_body().message.as_str(), "grant_not_active");
}

pub(super) fn duplicate_budget() -> loomward_windows::duplicate_reader::Budget {
    loomward_windows::duplicate_reader::Budget {
        bytes: 100,
        time: std::time::Duration::from_secs(10),
    }
}

#[cfg(windows)]
#[test]
fn tree_comparison_requires_both_durable_grants_and_final_cancellation_gate() {
    use loomward_windows::enumerate::{DirSource, NativeSource};
    if skip_elevated() {
        return;
    }
    let a = canonical_tempdir();
    let b = canonical_tempdir();
    let state = canonical_tempdir();
    for root in [a.path(), b.path()] {
        std::fs::write(root.join("fixture"), b"same").unwrap();
    }
    let grant = |name, path: &std::path::Path| {
        GrantedRoot::new(
            RootId::new(name).unwrap(),
            path.into(),
            DatasetClass::Synthetic,
            NativeSource::default().open_root(path).unwrap().1,
        )
    };
    let left = grant("rt_tree_left", a.path());
    let right = grant("rt_tree_right", b.path());
    let conn = duplicate_grant_store(state.path(), &left);
    let engine = Engine::open(EngineConfig::new(
        state.path().into(),
        DatasetClass::Synthetic,
    ))
    .unwrap();
    assert_eq!(
        engine
            .compare_trees(&left, &right, duplicate_budget(), 100)
            .unwrap_err()
            .to_body()
            .message
            .as_str(),
        "grant_not_active"
    );
    let identity = right.expected.unwrap();
    let FileIdObs::Id128(id) = identity.id.unwrap() else {
        panic!("fixture needs native identity")
    };
    conn.execute("INSERT INTO root_grant(id,display_path,volume_key,root_file_id,state,volume_guid) VALUES(2,?1,?2,?3,'active',?4)",
        rusqlite::params![b.path().to_str().unwrap(), format!("vsn_{}", identity.volume_serial.unwrap()), id.as_slice(), identity.volume_guid.map(|g| format!("{g:032x}"))]).unwrap();
    let report = engine
        .compare_trees(&left, &right, duplicate_budget(), 100)
        .unwrap();
    assert!(!report.differences.iter().any(|d| matches!(
        d.kind,
        loomward_operations::tree::DifferenceKind::Content
            | loomward_operations::tree::DifferenceKind::Missing
            | loomward_operations::tree::DifferenceKind::Extra
    )));
    for root in [&left, &right] {
        let cancel = engine.duplicate_read_admission(root).unwrap();
        cancel.store(true, Ordering::Release);
        assert_eq!(
            engine
                .compare_trees(&left, &right, duplicate_budget(), 100)
                .unwrap_err()
                .to_body()
                .message
                .as_str(),
            "cancelled"
        );
        cancel.store(false, Ordering::Release);
    }
    conn.execute("UPDATE root_grant SET state='revoked' WHERE id=1", [])
        .unwrap();
    assert_eq!(
        engine
            .compare_trees(&left, &right, duplicate_budget(), 100)
            .unwrap_err()
            .to_body()
            .message
            .as_str(),
        "grant_not_active"
    );
}

pub(super) fn duplicate_grant_store(
    state: &std::path::Path,
    root: &GrantedRoot,
) -> rusqlite::Connection {
    duplicate_grant_store_as(state, root, "synthetic")
}

fn duplicate_grant_store_as(
    state: &std::path::Path,
    root: &GrantedRoot,
    class: &str,
) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(state.join("state.db")).unwrap();
    conn.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT);
        CREATE TABLE root_grant(id INTEGER PRIMARY KEY,display_path TEXT,volume_key TEXT,root_file_id BLOB,state TEXT,identity_mode TEXT NOT NULL DEFAULT 'verified',volume_guid TEXT);").unwrap();
    conn.execute("INSERT INTO meta VALUES('dataset_class',?1)", [class])
        .unwrap();
    let expected = root.expected.unwrap();
    let id = match expected.id.unwrap() {
        FileIdObs::Id128(id) => id.to_vec(),
        FileIdObs::Id64(id) => id.to_le_bytes().to_vec(),
    };
    conn.execute(
        "INSERT INTO root_grant(id,display_path,volume_key,root_file_id,state,volume_guid) VALUES(1,?1,?2,?3,'active',?4)",
        rusqlite::params![
            root.path.to_str().unwrap(),
            format!("vsn_{}", expected.volume_serial.unwrap()),
            id,
            expected.volume_guid.map(|g| format!("{g:032x}")),
        ],
    )
    .unwrap();
    conn
}

/// #219: a scan's native source lists identity-unverified only for a root allowed to: a
/// synthetic fixture root, or a granted root whose grant record says so; personal otherwise not.
#[cfg(windows)]
#[test]
fn scan_source_allows_identity_unverified_only_when_the_root_does() {
    use loomward_windows::enumerate::NativeSource;
    let temp = canonical_tempdir();
    if skip_elevated() {
        return;
    }
    let (_, id) = NativeSource::default().open_root(temp.path()).unwrap();
    for (class, allowed) in [
        (DatasetClass::Synthetic, true),
        (DatasetClass::Personal, false),
    ] {
        let root = GrantedRoot::new(
            RootId::new("rt_unverified_fixture").unwrap(),
            temp.path().into(),
            class,
            id,
        );
        assert_eq!(root.allows_identity_unverified(), allowed);
        let opened = super::native_source(&root, Default::default())
            .with_filesystem("exFAT")
            .open_root(temp.path());
        assert_eq!(
            opened.map(|(_, id)| id.basis).ok(),
            allowed.then_some(IdBasis::Unverified),
            "{class:?}"
        );
    }
}

/// #219: an identity-unverified root is never watched (the watch reopens by file ID); a
/// verified root on the same watch-capable sink is watched. On this NTFS host the unverified
/// grant is forged, so its scan also ends `root_identity_changed` (h2-scan finding 2): the
/// opened root's basis is not the grant's. It is still refused before any watch starts.
#[cfg(windows)]
#[test]
fn identity_unverified_roots_are_never_watched() {
    use loomward_protocol::JobState;
    use loomward_windows::enumerate::NativeSource;
    let temp = canonical_tempdir();
    if skip_elevated() {
        return;
    }
    let (_, id) = NativeSource::default().open_root(temp.path()).unwrap();
    for (name, basis, watched, end) in [
        (
            "rt_watch_verified",
            IdBasis::PostOpen,
            true,
            JobState::Completed,
        ),
        (
            "rt_watch_unverified",
            IdBasis::Unverified,
            false,
            JobState::Failed,
        ),
    ] {
        let root = GrantedRoot::new(
            RootId::new(name).unwrap(),
            temp.path().into(),
            DatasetClass::Synthetic,
            OpenedIdentity { basis, ..id },
        );
        let e = Engine::open(EngineConfig::new(
            std::env::temp_dir(),
            DatasetClass::Synthetic,
        ))
        .unwrap();
        e.set_scan_sink(Arc::new(MemorySink::default())).unwrap();
        let job = e.scan_start(&root, None).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while e.job_status(&job.job_id).unwrap().state != end {
            assert!(std::time::Instant::now() < deadline, "{name}");
            std::thread::sleep(Duration::from_millis(2));
        }
        let watch = e.scan_watches.lock().unwrap().remove(root.root_id());
        assert_eq!(watch.is_some(), watched, "{name}");
        if let Some(watch) = watch {
            watch.stop();
        }
    }
}

/// #219: an identity-unverified root is refused at content-read admission, before the grant
/// store is even consulted.
#[test]
fn duplicate_admission_refuses_an_identity_unverified_root() {
    let state = tempfile::tempdir().unwrap();
    let mut grant = root();
    let _conn = duplicate_grant_store(state.path(), &grant);
    let engine = Engine::open(EngineConfig::new(
        state.path().to_owned(),
        DatasetClass::Synthetic,
    ))
    .unwrap();
    assert!(engine.duplicate_read_admission(&grant).is_ok());
    grant.expected.as_mut().unwrap().basis = IdBasis::Unverified;
    for err in [
        engine.duplicate_read_admission(&grant).unwrap_err(),
        engine
            .inspect_duplicates(&grant, &[], duplicate_budget())
            .unwrap_err(),
    ] {
        assert_eq!(err.to_body().message.as_str(), "identity_unverified");
    }
}

#[test]
fn duplicate_inspection_checks_engine_dataset_class() {
    let engine = Engine::open(EngineConfig::new("unused".into(), DatasetClass::Personal)).unwrap();
    let err = engine
        .inspect_duplicates(&root(), &[], duplicate_budget())
        .unwrap_err();
    assert_eq!(err.to_body().message.as_str(), "dataset_class_mismatch");
}

#[test]
fn duplicate_inspection_refuses_revoked_root_before_open() {
    let engine = Engine::open(EngineConfig::new("unused".into(), DatasetClass::Synthetic)).unwrap();
    let grant = root();
    engine
        .scan_cancel_root(grant.key(), grant.root_id())
        .unwrap();
    let err = engine
        .inspect_duplicates(&grant, &[], duplicate_budget())
        .unwrap_err();
    assert_eq!(err.to_body().message.as_str(), "root revoked");
}

#[test]
fn duplicate_admission_uses_the_root_gate_and_cancellation() {
    let state = tempfile::tempdir().unwrap();
    let grant = root();
    let _conn = duplicate_grant_store(state.path(), &grant);
    let engine = Engine::open(EngineConfig::new(
        state.path().to_owned(),
        DatasetClass::Synthetic,
    ))
    .unwrap();
    let gate = engine.scan_root_gate(grant.key());
    let held = gate.lock().unwrap();
    assert!(matches!(
        engine.duplicate_read_admission(&grant),
        Err(EngineError::Busy { .. })
    ));
    drop(held);
    let cancel = engine.duplicate_read_admission(&grant).unwrap();
    assert!(!cancel.load(Ordering::Acquire));
    assert!(Arc::ptr_eq(
        &cancel,
        &engine.duplicate_read_admission(&grant.clone()).unwrap()
    ));
    engine
        .scan_cancel_root(grant.key(), grant.root_id())
        .unwrap();
    assert!(cancel.load(Ordering::Acquire));
    assert_eq!(
        engine
            .duplicate_read_admission(&grant)
            .unwrap_err()
            .to_body()
            .message
            .as_str(),
        "root revoked"
    );
}

#[test]
fn duplicate_admission_rechecks_durable_class_identity_and_ambiguity() {
    let state = tempfile::tempdir().unwrap();
    let grant = root();
    let conn = duplicate_grant_store(state.path(), &grant);
    let engine = Engine::open(EngineConfig::new(
        state.path().to_owned(),
        DatasetClass::Synthetic,
    ))
    .unwrap();
    engine.duplicate_read_admission(&grant).unwrap();
    conn.execute("UPDATE meta SET value='personal'", [])
        .unwrap();
    assert_eq!(
        engine
            .duplicate_read_admission(&grant)
            .unwrap_err()
            .to_body()
            .message
            .as_str(),
        "dataset_class_mismatch"
    );
    conn.execute("UPDATE meta SET value='synthetic'", [])
        .unwrap();
    for field in ["path", "volume", "identity"] {
        let mut stale = grant.clone();
        match field {
            "path" => stale.path = "other-synthetic".into(),
            "volume" => stale.expected.as_mut().unwrap().volume_serial = Some(43),
            _ => stale.expected.as_mut().unwrap().id = Some(FileIdObs::Id64(2)),
        }
        assert_eq!(
            engine
                .duplicate_read_admission(&stale)
                .unwrap_err()
                .to_body()
                .message
                .as_str(),
            "grant_not_active"
        );
    }
    conn.execute("INSERT INTO root_grant(id,display_path,volume_key,root_file_id,state,volume_guid) SELECT 2,display_path,volume_key,root_file_id,'active',volume_guid FROM root_grant WHERE id=1", []).unwrap();
    assert_eq!(
        engine
            .duplicate_read_admission(&grant)
            .unwrap_err()
            .to_body()
            .message
            .as_str(),
        "grant_not_active"
    );
    // A revoked row beside one active grant for the same folder is history, not ambiguity: the
    // folder was revoked and granted again, and reads follow the one active grant (review on #376).
    conn.execute("UPDATE root_grant SET state='revoked' WHERE id=1", [])
        .unwrap();
    assert!(engine.duplicate_read_admission(&grant.clone()).is_ok());
}

#[test]
fn duplicate_worker_admission_waits_for_same_root_gate() {
    let state = tempfile::tempdir().unwrap();
    let grant = root();
    let _conn = duplicate_grant_store(state.path(), &grant);
    let engine = Arc::new(
        Engine::open(EngineConfig::new(
            state.path().to_owned(),
            DatasetClass::Synthetic,
        ))
        .unwrap(),
    );
    for revoke in [false, true] {
        let gate = engine.scan_root_gate(grant.key());
        let mut held = gate.lock().unwrap();
        let (entered, started) = std::sync::mpsc::channel();
        let (result, outcome) = std::sync::mpsc::channel();
        let worker_engine = engine.clone();
        let worker_grant = grant.clone();
        let worker = std::thread::spawn(move || {
            entered.send(()).unwrap();
            result
                .send(worker_engine.duplicate_read_admission_worker(&worker_grant))
                .unwrap();
        });
        started
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let premature = outcome.recv_timeout(std::time::Duration::from_millis(50));
        held.revoked = revoke;
        drop(held);
        worker.join().unwrap();
        assert!(matches!(
            premature,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        let admitted = outcome.recv().unwrap();
        if revoke {
            assert_eq!(
                admitted.unwrap_err().to_body().message.as_str(),
                "root revoked"
            );
        } else {
            assert!(admitted.is_ok());
        }
    }
}
use source::*;
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};
pub(super) fn identity(id: u64) -> OpenedIdentity {
    OpenedIdentity {
        id: Some(FileIdObs::Id64(id)),
        basis: IdBasis::Listed,
        volume_guid: None,
        volume_serial: Some(42),
        attributes: 0x10,
        reparse_tag: None,
    }
}
pub(super) fn root() -> GrantedRoot {
    GrantedRoot::new(
        RootId::new("rt_test").unwrap(),
        "unused".into(),
        DatasetClass::Synthetic,
        identity(1),
    )
}
#[test]
fn dataset_class_is_checked_before_start_or_refresh() {
    for dataset in [DatasetClass::Synthetic, DatasetClass::Personal] {
        let engine = Engine::open(EngineConfig::new("unused".into(), dataset)).unwrap();
        let mut grant = root();
        grant.dataset_class = dataset;
        assert!(engine.check_root(&grant).is_ok());
        // Matching roots reach the (uninstalled) sink, rather than the class refusal.
        for result in [
            engine.scan_start(&grant, None),
            engine.scan_refresh(&grant, None),
        ] {
            assert_eq!(
                result.unwrap_err(),
                EngineError::unavailable(Component::Scan)
            );
        }
        grant.dataset_class = match dataset {
            DatasetClass::Synthetic => DatasetClass::Personal,
            DatasetClass::Personal => DatasetClass::Synthetic,
        };
        for result in [
            engine.scan_start(&grant, None),
            engine.scan_refresh(&grant, None),
        ] {
            assert_eq!(
                result.unwrap_err(),
                EngineError::PermissionDenied {
                    message: "dataset_class_mismatch".into(),
                }
            );
        }
        assert!(engine.jobs.root_jobs(grant.root_id()).is_empty());
        assert!(engine.scan_roots.lock().unwrap().is_empty());
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
pub(super) struct Tree;

#[test]
fn iterative_depth_cap_preserves_boundary_files_for_every_strategy() {
    struct Chain(Strategy, u64);
    impl DirSource for Chain {
        type Dir = u64;
        fn strategy(&self) -> Strategy {
            self.0
        }
        fn open_root(&self, path: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
            Tree.open_root(path)
        }
        fn open_child(
            &self,
            _: &u64,
            entry: &RawEntry<'_>,
        ) -> Result<(u64, OpenedIdentity), SourceError> {
            let Some(FileIdObs::Id64(id)) = entry.file_id else {
                unreachable!()
            };
            assert!(id <= 512, "opened a directory beyond the writer limit");
            Ok((id, identity(id)))
        }
        fn list(&self, dir: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
            Tree.list(&2, &mut |mut entry| {
                // A file precedes any child, including at the deepest accepted directory.
                entry.file_id = None;
                let flow = sink(entry.clone());
                if flow == Flow::Stop || *dir == self.1 {
                    return flow;
                }
                entry.name = &[100];
                entry.file_id = Some(FileIdObs::Id64(dir + 1));
                entry.attributes = 0x10;
                sink(entry)
            })
        }
    }
    for strategy in [
        Strategy::Extended,
        Strategy::Both,
        Strategy::Find,
        Strategy::Portable,
    ] {
        for levels in [512, 513] {
            let bytes = Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES));
            let sink = Arc::new(MemorySink::default());
            let root = root();
            let report = run_scan(
                &Chain(strategy, levels),
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
            assert_eq!(report.complete, levels == 512, "{strategy:?}: {report:?}");
            assert_eq!(report.limit_hit, levels == 513);
            assert_eq!(
                report.limit_reason,
                (levels == 513).then_some(loomward_protocol::ScanLimitReason::DepthLimit)
            );
            assert_eq!((report.totals.dirs, report.totals.files), (511, 512));
            assert_eq!(sink.totals(root.root_id()), Some(report.totals));
            assert_eq!(bytes.used(), 0);
        }
    }
}

struct ReplacementTree {
    id: u64,
    attributes: u32,
    listed_id: bool,
    name_only: bool,
}
impl DirSource for ReplacementTree {
    type Dir = u64;
    fn strategy(&self) -> Strategy {
        Strategy::Portable
    }
    fn open_root(&self, path: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
        Tree.open_root(path)
    }
    fn open_child(&self, _: &u64, _: &RawEntry<'_>) -> Result<(u64, OpenedIdentity), SourceError> {
        let mut id = identity(self.id);
        if self.name_only {
            if self.listed_id {
                id.basis = IdBasis::NonUnique;
            } else {
                id.id = None;
                id.basis = IdBasis::None;
            }
        } else if !self.listed_id {
            id.basis = IdBasis::PostOpen;
        }
        Ok((self.id, id))
    }
    fn list(&self, dir: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        Tree.list(dir, &mut |mut e| {
            if *dir == 1 {
                e.file_id = self.listed_id.then_some(FileIdObs::Id64(self.id));
                e.attributes = self.attributes;
            }
            sink(e)
        })
    }
}
fn replacement_matches_fresh(attributes: u32, listed_id: bool, name_only: bool) {
    let root = root();
    let sink = Arc::new(MemorySink::default());
    let scan = |source: &ReplacementTree, run, sink| {
        run_scan(
            source,
            &root,
            run,
            sink,
            Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
            Arc::new(AtomicBool::new(false)),
            ScanOptions {
                workers: 1,
                ..ScanOptions::default()
            },
        )
        .unwrap()
    };
    let first = ReplacementTree {
        id: 2,
        attributes: 0x10,
        listed_id,
        name_only,
    };
    assert!(scan(&first, 1, sink.clone()).complete);
    let next = ReplacementTree {
        id: if refused_attributes(attributes) { 2 } else { 4 },
        attributes,
        listed_id,
        name_only,
    };
    assert!(scan(&next, 2, sink.clone()).complete);
    let fresh = Arc::new(MemorySink::default());
    let expected = scan(&next, 3, fresh.clone());
    assert_eq!(
        sink.totals(root.root_id()),
        Some(expected.totals),
        "retained totals must equal a fresh scan"
    );
    assert_eq!(sink.totals(root.root_id()), fresh.totals(root.root_id()));
}
#[test]
fn same_name_new_directory_identity_matches_fresh_scan() {
    replacement_matches_fresh(0x10, true, false);
}
#[test]
fn directory_becoming_reparse_or_offline_matches_fresh_scan() {
    for refused in [0x400, 0x1000, 0x40000, 0x400000] {
        replacement_matches_fresh(0x10 | refused, true, false);
    }
}
#[test]
fn post_open_and_name_only_directory_presence_matches_fresh_scan() {
    replacement_matches_fresh(0x10, false, false);
    replacement_matches_fresh(0x10, false, true);
    replacement_matches_fresh(0x10, true, true);
}
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
fn skipped_subtree_keeps_listing_eof_but_makes_run_partial() {
    struct Junction;
    impl DirSource for Junction {
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
            panic!("a junction must never be opened")
        }
        fn list(&self, dir: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
            let files = Tree.list(&2, &mut |mut entry| {
                entry.name = &[102];
                sink(entry)
            });
            if files != ListOutcome::Complete {
                return files;
            }
            Tree.list(dir, &mut |mut entry| {
                entry.attributes |= 0x400;
                sink(entry)
            })
        }
    }
    let sink = Arc::new(OrderProbe::default());
    let root = root();
    // First observe an ordinary child, then replace it with an excluded junction at EOF.
    run_scan(
        &Tree,
        &root,
        1,
        sink.clone(),
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        Default::default(),
        ScanOptions::default(),
    )
    .unwrap();
    let report = run_scan(
        &Junction,
        &root,
        2,
        sink.clone(),
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        Default::default(),
        ScanOptions::default(),
    )
    .unwrap();
    assert_eq!(report.skipped, 1);
    assert_eq!(report.failed, 0);
    assert_eq!(
        sink.outcomes.lock().unwrap().last(),
        Some(&ListOutcome::Complete)
    );
    assert!(
        !report.coverage_complete(),
        "a skipped subtree is not complete coverage"
    );
    assert!(report.complete, "every traversed directory reached EOF");
    assert!(sink.sink.sweep_allowed(root.root_id()));
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
    assert!(report.enumeration_wall_seconds > 0.);
    assert!(report.enumeration_wall_seconds <= report.elapsed_seconds);
    assert!(report.staging_seconds > 0.);
    assert!(report.staging_seconds <= report.persistence_seconds);
    assert!(report.aggregation_seconds > 0.);
    assert!(report.aggregation_seconds <= report.elapsed_seconds);
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
    outcomes: std::sync::Mutex<Vec<ListOutcome>>,
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
        if let ScanMessage::ListingDone {
            ticket, outcome, ..
        } = &message
        {
            self.outcomes.lock().unwrap().push(*outcome);
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
fn directory_limit_partial_sink_oracle() {
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
            max_dirs: 1,
            ..ScanOptions::default()
        },
    )
    .unwrap();
    assert!(!report.complete);
    assert!(report.limit_hit);
    assert_eq!(report.skipped, 0);
    assert_eq!(report.failed, 0);
    assert!(!sink.sweep_allowed(root.root_id()));
    assert!(sink.is_repairing(root.root_id()));
    assert_eq!(sink.totals(root.root_id()), Some(report.totals));
}
#[test]
fn directory_limit_does_not_hide_child_io_errors() {
    struct DeniedThenLimit;
    impl DirSource for DeniedThenLimit {
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
            if entry.file_id == Some(FileIdObs::Id64(2)) {
                Err(SourceError::AccessDenied)
            } else {
                Tree.open_child(parent, entry)
            }
        }
        fn list(&self, dir: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
            let first = Tree.list(dir, &mut |mut entry| {
                entry.name = &[98];
                sink(entry)
            });
            assert_eq!(first, ListOutcome::Complete);
            Tree.list(dir, &mut |mut entry| {
                entry.name = &[97];
                entry.file_id = Some(FileIdObs::Id64(3));
                sink(entry)
            })
        }
    }
    let root = root();
    let sink = Arc::new(MemorySink::default());
    let report = run_scan(
        &DeniedThenLimit,
        &root,
        1,
        sink.clone(),
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        Arc::new(AtomicBool::new(false)),
        ScanOptions {
            max_dirs: 1,
            ..ScanOptions::default()
        },
    )
    .unwrap();
    assert!(!report.complete);
    assert!(report.limit_hit);
    assert_eq!(report.failed, 1);
    assert!(!sink.sweep_allowed(root.root_id()));
}
/// The directory-slot cap names itself (`directory_limit`), so a client never reads a capped scan
/// as a generic stop with no cause.
#[test]
fn directory_cap_names_its_limit() {
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
            max_dirs: 1,
            ..ScanOptions::default()
        },
    )
    .unwrap();
    assert!(report.limit_hit);
    assert!(!report.complete);
    assert_eq!(
        report.limit_reason,
        Some(loomward_protocol::ScanLimitReason::DirectoryLimit)
    );
}

/// A directory whose open is refused is catalogued by name but never indexed, so it is not counted
/// in the scan's indexed directories (`ScanProgress.indexed_dirs`).
#[test]
fn denied_directory_is_not_an_indexed_directory() {
    struct DeniedChildren;
    impl DirSource for DeniedChildren {
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
            Err(SourceError::AccessDenied)
        }
        fn list(&self, dir: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
            Tree.list(dir, sink)
        }
    }
    let root = root();
    let sink = Arc::new(MemorySink::default());
    let report = run_scan(
        &DeniedChildren,
        &root,
        1,
        sink.clone(),
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        Arc::new(AtomicBool::new(false)),
        ScanOptions::default(),
    )
    .unwrap();
    assert_eq!(report.failed, 1);
    assert!(!report.complete);
    assert_eq!(report.totals.dirs, 0);
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
    let mut changed_guid = identity(1);
    changed_guid.volume_guid = Some(1);
    for expected in [identity(999), changed_guid] {
        let mut root = root();
        root.expected = Some(expected);
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
        assert!(
            sink.identity_changed(root.root_id()),
            "the writer is told, so it can record the transition (#206)"
        );
    }
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
        e.scan_cancel_root(&RootKey::Fixture(root_id.clone()), &root_id)
            .unwrap()[0]
            .state,
        JobState::CancelRequested
    );
    assert!(start.elapsed() < Duration::from_millis(250));
    assert_eq!(dropped.load(Ordering::Relaxed), 0);
    assert_eq!(
        e.job_status(&job.job_id).unwrap().state,
        JobState::CancelRequested
    );
    e.scan_writer_fence(&RootKey::Fixture(root_id.clone()), &root_id)
        .unwrap();
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
    let e = Engine::open(EngineConfig::new(
        std::env::temp_dir(),
        DatasetClass::Synthetic,
    ))
    .unwrap();
    e.set_scan_sink(Arc::new(MemorySink::default())).unwrap();
    let job = e.scan_start(&root, None).unwrap();
    // A liveness bound only: the claim is the exact final counts, not how fast a loaded host scans.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
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
    assert_eq!(report.failed, 0);
    assert_eq!(report.totals.files, 2_000_000);
    assert_eq!(report.totals.logical, 2_000_000);
    assert_eq!(report.totals.allocated, None);
    assert!(!sink.sweep_allowed(root.root_id()));
}
struct FailedWriter(MemorySink);
pub(super) struct MovingTree {
    pub(super) x_listings: std::sync::atomic::AtomicUsize,
    pub(super) unique: bool,
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

pub(super) struct DirtyWriter {
    pub(super) sink: MemorySink,
    pub(super) dirtied: AtomicBool,
    pub(super) on_final: bool,
    pub(super) always: bool,
}
impl ScanSink for DirtyWriter {
    fn watch_dirty(&self, root: &RootId) -> crate::EngineResult<()> {
        self.sink.watch_dirty(root)
    }
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
        assert_eq!(report.examined, 2, "relists replace report counters");
        assert!(
            report.enumeration_attempts > report.examined,
            "relists still consume the run limit"
        );
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
/// A sink whose `consume` never refuses: the obsolete publication is reported only through
/// `settle`, as the service sink's deferred writes do (#148).
struct DeferredObsolete {
    sink: MemorySink,
    /// Report the listed directory on every settle, so the refusal persists across relists.
    always: bool,
    reported: AtomicBool,
    listings: std::sync::Mutex<Vec<u64>>,
}
impl ScanSink for DeferredObsolete {
    fn watch_dirty(&self, root: &RootId) -> crate::EngineResult<()> {
        self.sink.watch_dirty(root)
    }
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
        if let ScanMessage::DirListing { ticket, .. } = &message {
            self.listings.lock().unwrap().push(ticket.dir);
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
    fn settle(&self, _root: &RootId) -> crate::EngineResult<Vec<u64>> {
        let listed = self.listings.lock().unwrap().first().copied();
        let first = !self
            .reported
            .swap(true, std::sync::atomic::Ordering::AcqRel);
        Ok(listed
            .filter(|_| first || self.always)
            .into_iter()
            .collect())
    }
}
/// #148: a directory only `settle` reports obsolete is relisted by the engine, and the run stays
/// incomplete while the refusal persists.
#[test]
fn a_directory_settle_reports_obsolete_is_relisted_and_a_persistent_one_stays_partial() {
    for always in [false, true] {
        let sink = Arc::new(DeferredObsolete {
            sink: MemorySink::default(),
            always,
            reported: AtomicBool::new(false),
            listings: Default::default(),
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
        let listings = sink.listings.lock().unwrap().clone();
        let first = listings[0];
        let relists = listings.iter().filter(|d| **d == first).count();
        assert!(
            relists >= 2,
            "the settled directory was relisted: {listings:?}"
        );
        assert_eq!(report.complete, !always, "always={always}");
        if always {
            assert!(!sink.sink.sweep_allowed(root.root_id()));
        }
        assert_eq!(bytes.used(), 0);
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
fn native_dirty_epoch_relists_by_verified_identity() {
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
    assert_eq!(report.examined, 2);
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
    assert_eq!(
        report.failed, 0,
        "outside-scope publication is not an I/O failure"
    );
    assert!(!sink.sweep_allowed(root.root_id()));
    assert_eq!(report.totals.files, 1);
}

/// Native roots are opened component by component and refuse reparse ancestors, so tests grant a
/// canonical temp base (hosted runners may junction the temp folder), as a real grant would.
#[cfg(windows)]
pub(super) fn canonical_tempdir() -> tempfile::TempDir {
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
pub(super) fn skip_elevated() -> bool {
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
fn grant_rejects_same_serial_and_file_id_with_a_different_volume_guid() {
    if skip_elevated() {
        return;
    }
    let state = canonical_tempdir();
    let directory = canonical_tempdir();
    let (_, identity) = loomward_windows::enumerate::NativeSource::default()
        .open_root(directory.path())
        .unwrap();
    let grant = GrantedRoot::new(
        RootId::new("rt_guid_fixture").unwrap(),
        directory.path().into(),
        DatasetClass::Synthetic,
        identity,
    );
    let conn = duplicate_grant_store(state.path(), &grant);
    conn.execute(
        "UPDATE root_grant SET volume_guid='00000000000000000000000000000001' WHERE id=1",
        [],
    )
    .unwrap();
    rusqlite::Connection::open(state.path().join("catalog.db")).unwrap().execute_batch(
        "CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT); INSERT INTO meta VALUES('instance_id','fixture-instance');
         CREATE TABLE root(id INTEGER PRIMARY KEY,grant_id INTEGER); INSERT INTO root VALUES(1,1);"
    ).unwrap();
    let engine = Engine::open(EngineConfig::new(
        state.path().into(),
        DatasetClass::Synthetic,
    ))
    .unwrap();
    assert!(
        matches!(
            engine.granted_root(grant.root_id().clone(), 1, &CatalogRootIdentity { row: 1, instance: "fixture-instance".into() }),
            Err(EngineError::PermissionDenied { message }) if message == "root_identity_changed"
        ),
        "same serial and file ID must not authorise a different volume GUID"
    );
}

#[cfg(windows)]
#[test]
fn migrated_legacy_grant_adopts_guid_only_after_matching_identity() {
    use loomward_catalog::{Catalog, RootObservation, WriteCommand, WriteReply};
    if skip_elevated() {
        return;
    }
    let state = canonical_tempdir();
    let directory = canonical_tempdir();
    let (_, identity) = loomward_windows::enumerate::NativeSource::default()
        .open_root(directory.path())
        .unwrap();
    let volume = format!("vsn_{}", identity.volume_serial.unwrap());
    let file_id = match identity.id.unwrap() {
        FileIdObs::Id128(id) => id.to_vec(),
        FileIdObs::Id64(id) => id.to_le_bytes().to_vec(),
    };
    let catalog = Catalog::open(state.path(), "synthetic").unwrap();
    let WriteReply::Root {
        grant_id, dir_id, ..
    } = catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: volume.clone(),
            display_name: "Synthetic legacy root".into(),
            display_path: directory.path().to_str().unwrap().into(),
            root_file_id: Some(file_id.clone()),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))
        .unwrap()
    else {
        panic!("expected registered root")
    };
    drop(catalog);
    let conn = rusqlite::Connection::open(state.path().join("state.db")).unwrap();
    conn.execute_batch("ALTER TABLE root_grant DROP COLUMN volume_guid; PRAGMA user_version=8;")
        .unwrap();
    drop(conn);
    drop(Catalog::open(state.path(), "synthetic").unwrap());
    let conn = rusqlite::Connection::open(state.path().join("state.db")).unwrap();
    let stored_guid = || {
        conn.query_row(
            "SELECT volume_guid FROM root_grant WHERE id=?1",
            [grant_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .unwrap()
    };
    let catalog_root = CatalogRootIdentity {
        row: dir_id,
        instance: rusqlite::Connection::open(state.path().join("catalog.db"))
            .unwrap()
            .query_row("SELECT value FROM meta WHERE key='instance_id'", [], |r| {
                r.get(0)
            })
            .unwrap(),
    };
    let admit = || {
        Engine::open(EngineConfig::new(
            state.path().into(),
            DatasetClass::Synthetic,
        ))
        .unwrap()
        .granted_root(
            RootId::new("rt_legacy_fixture").unwrap(),
            grant_id,
            &catalog_root,
        )
    };
    let revision = || {
        conn.query_row("SELECT state_rev FROM revision WHERE id=1", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    };
    let before = revision();
    assert!(stored_guid().is_none());
    for (column, bad) in [
        (
            "volume_key",
            rusqlite::types::Value::Text("vsn_wrong".into()),
        ),
        ("root_file_id", rusqlite::types::Value::Blob(vec![0; 16])),
    ] {
        conn.execute(
            &format!("UPDATE root_grant SET {column}=?1 WHERE id=?2"),
            rusqlite::params![bad, grant_id],
        )
        .unwrap();
        assert_eq!(
            admit().unwrap_err().to_body().message.as_str(),
            "root_identity_changed"
        );
        assert!(
            stored_guid().is_none(),
            "a mismatched {column} cannot bind a GUID"
        );
        conn.execute(
            "UPDATE root_grant SET volume_key=?1,root_file_id=?2 WHERE id=?3",
            rusqlite::params![volume, file_id, grant_id],
        )
        .unwrap();
    }
    // A failed admission transaction must roll back even an already-written GUID.
    conn.execute_batch("CREATE TRIGGER refuse_adoption_revision BEFORE UPDATE ON revision BEGIN SELECT RAISE(ABORT,'fixture write failure'); END;").unwrap();
    assert_eq!(
        admit().unwrap_err().to_body().message.as_str(),
        "grant store unavailable"
    );
    assert!(stored_guid().is_none());
    assert_eq!(revision(), before);
    conn.execute_batch("DROP TRIGGER refuse_adoption_revision")
        .unwrap();
    assert_eq!(admit().unwrap().expected_identity(), Some(identity));
    assert_eq!(revision(), before + 1);
    let guid = format!("{:032x}", identity.volume_guid.unwrap());
    assert_eq!(stored_guid(), Some(guid));
    // A new engine and catalogue open must retain the first admission's binding.
    drop(Catalog::open(state.path(), "synthetic").unwrap());
    assert!(admit().is_ok());
    assert_eq!(revision(), before + 1, "only the first admission adopts");
    let other_guid = format!("{:032x}", identity.volume_guid.unwrap() ^ 1);
    conn.execute(
        "UPDATE root_grant SET volume_guid=?1 WHERE id=?2",
        rusqlite::params![other_guid, grant_id],
    )
    .unwrap();
    assert_eq!(
        admit().unwrap_err().to_body().message.as_str(),
        "root_identity_changed"
    );
    assert_eq!(
        stored_guid(),
        Some(other_guid),
        "a known GUID must never be rebound"
    );
}

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
         CREATE TABLE root_grant(id INTEGER PRIMARY KEY,display_path TEXT,volume_key TEXT,root_file_id BLOB,state TEXT,identity_mode TEXT NOT NULL DEFAULT 'verified',volume_guid TEXT);",
    )
    .unwrap();
    let path = root_dir.path().to_string_lossy().into_owned();
    let insert = |id: i64, file_id: &[u8], state: &str| {
        conn.execute(
            "INSERT INTO root_grant(id,display_path,volume_key,root_file_id,state,volume_guid) VALUES(?1,?2,?3,?4,?5,?6)",
            rusqlite::params![id, path, volume, file_id, state, identity.volume_guid.map(|g| format!("{g:032x}"))],
        )
        .unwrap();
    };
    insert(1, &file_id, "active");
    insert(2, &file_id, "revoked");
    insert(3, &[9; 16], "active");
    // Catalogue root row `g` observes grant `g`.
    rusqlite::Connection::open(state.path().join("catalog.db"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT);
             INSERT INTO meta VALUES('instance_id','fixture-instance');
             CREATE TABLE root(id INTEGER PRIMARY KEY,grant_id INTEGER);
             INSERT INTO root VALUES(1,1),(2,2),(3,3),(4,4);",
        )
        .unwrap();
    let engine = Engine::open(EngineConfig::new(
        state.path().to_path_buf(),
        DatasetClass::Synthetic,
    ))
    .unwrap();
    let rt = |row: i64| RootId::new(format!("rt_opaque_{row}")).unwrap();
    let catalog_root = |row| CatalogRootIdentity {
        row,
        instance: "fixture-instance".into(),
    };
    let message =
        |r: EngineResult<GrantedRoot>| r.unwrap_err().to_body().message.as_str().to_owned();
    let root = engine.granted_root(rt(1), 1, &catalog_root(1)).unwrap();
    assert_eq!(root.root_id(), &rt(1));
    assert_eq!(root.path(), root_dir.path());
    assert_eq!(root.dataset_class(), DatasetClass::Synthetic);
    // #219: only the grant record allows identity-unverified listing, and on NTFS the root
    // keeps its verified identity even then.
    assert!(!root.allows_identity_unverified());
    conn.execute(
        "UPDATE root_grant SET identity_mode='unverified' WHERE id=1",
        [],
    )
    .unwrap();
    let unverified = engine.granted_root(rt(1), 1, &catalog_root(1)).unwrap();
    assert!(unverified.allows_identity_unverified());
    assert_eq!(unverified.expected.unwrap().basis, IdBasis::PostOpen);
    conn.execute(
        "UPDATE root_grant SET identity_mode='verified' WHERE id=1",
        [],
    )
    .unwrap();
    assert_eq!(
        message(engine.granted_root(rt(2), 2, &catalog_root(2))),
        "grant_not_active"
    );
    assert_eq!(
        message(engine.granted_root(rt(4), 4, &catalog_root(4))),
        "grant_not_active"
    );
    assert_eq!(
        message(engine.granted_root(rt(3), 3, &catalog_root(3))),
        "root_identity_changed"
    );
    // #206: a catalogue root and an active grant that do not belong together are refused.
    assert_eq!(
        message(engine.granted_root(rt(3), 1, &catalog_root(3))),
        "grant_root_mismatch"
    );
    assert_eq!(
        message(engine.granted_root(
            rt(1),
            1,
            &CatalogRootIdentity {
                row: 1,
                instance: "foreign-instance".into(),
            }
        )),
        "grant_root_mismatch"
    );
    // The row's class, not the caller's, reaches the engine's class check.
    conn.execute("UPDATE meta SET value='personal'", [])
        .unwrap();
    let personal = engine.granted_root(rt(1), 1, &catalog_root(1)).unwrap();
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
#[cfg(windows)]
struct Unwatched(MemorySink);
#[cfg(windows)]
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
    let e = Engine::open(EngineConfig::new(
        std::env::temp_dir(),
        DatasetClass::Synthetic,
    ))
    .unwrap();
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
/// One wide directory of near-limit (32,767-unit) child names: the names its listing holds until
/// the children are adopted are charged to the arena reservation, so the held set stops at a
/// limit instead of growing past the scan's byte budget (#206).
struct WideNames;
impl DirSource for WideNames {
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
        if *dir != 1 {
            return ListOutcome::Complete;
        }
        let mut name = vec![u16::from(b'n'); 32767];
        for i in 0..400u64 {
            name[..3].copy_from_slice(&[
                0x41 + (i / 26 / 26) as u16,
                0x41 + (i / 26 % 26) as u16,
                0x41 + (i % 26) as u16,
            ]);
            if sink(RawEntry {
                name: &name,
                file_id: Some(FileIdObs::Id64(i + 2)),
                attributes: 0x10,
                reparse_tag: None,
                end_of_file: 0,
                allocation_size: Some(0),
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
fn held_child_names_are_charged_to_the_scan_byte_budget() {
    let bytes = Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES));
    let arena = (bytes.capacity() / 4).min(16 * 1024 * 1024);
    let report = run_scan(
        &WideNames,
        &root(),
        1,
        Arc::new(MemorySink::default()),
        bytes,
        Arc::new(AtomicBool::new(false)),
        ScanOptions::default(),
    )
    .unwrap();
    assert!(report.limit_hit && !report.complete, "{report:?}");
    let held = report.totals.dirs as usize * 32767 * 2;
    assert!(
        report.totals.dirs > 0 && held <= arena,
        "{} held names ({held} bytes) exceed the {arena}-byte arena",
        report.totals.dirs
    );
}
/// A writer whose first child prepare is obsolete (the parent's publication was rejected).
struct StalePrepare {
    sink: MemorySink,
    refused: AtomicBool,
}
impl ScanSink for StalePrepare {
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
        if parent.is_some() && !self.refused.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return Err(crate::EngineError::StaleGeneration {
                message: "obsolete scan writer message".into(),
                detail: loomward_protocol::Detail::new(std::collections::BTreeMap::from([(
                    "reason".to_string(),
                    serde_json::json!("listing_revision_changed"),
                )]))
                .ok(),
            });
        }
        self.sink.prepare_listing(root, run, parent, name, identity)
    }
    fn consume(&self, root: &RootId, message: ScanMessage) -> crate::EngineResult<()> {
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
/// #206 MEDIUM-2: an obsolete child prepare relists its parent instead of failing the run.
#[test]
fn stale_child_prepare_relists_the_parent() {
    let sink = Arc::new(StalePrepare {
        sink: MemorySink::default(),
        refused: AtomicBool::new(false),
    });
    let root = root();
    let report = run_scan(
        &Tree,
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
    .expect("a stale prepare is not a run failure");
    assert!(sink.refused.load(std::sync::atomic::Ordering::Acquire));
    assert!(report.complete, "{report:?}");
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
}

#[cfg(windows)]
#[test]
fn followup_grant_gate_cancels_and_fences_every_wire_alias() {
    use loomward_windows::enumerate::{DirSource, NativeSource};
    if loomward_windows::enumerate::running_elevated() {
        assert_eq!(
            NativeSource::default()
                .open_root(std::path::Path::new("synthetic"))
                .unwrap_err(),
            source::SourceError::Refused
        );
        return;
    }
    let temp = canonical_tempdir();
    let expected = NativeSource::default().open_root(temp.path()).unwrap().1;
    let seed = GrantedRoot::new(
        RootId::new("rt_b").unwrap(),
        temp.path().to_owned(),
        DatasetClass::Synthetic,
        expected,
    );
    let state = canonical_tempdir();
    let _conn = duplicate_grant_store(state.path(), &seed);
    rusqlite::Connection::open(state.path().join("catalog.db"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT);
         INSERT INTO meta VALUES('instance_id','fixture-instance');
         CREATE TABLE root(id INTEGER PRIMARY KEY,grant_id INTEGER);
         INSERT INTO root VALUES(7,1);",
        )
        .unwrap();
    let engine = Engine::open(EngineConfig::new(
        state.path().to_owned(),
        DatasetClass::Synthetic,
    ))
    .unwrap();
    let identity = CatalogRootIdentity {
        row: 7,
        instance: "fixture-instance".into(),
    };
    let a = engine
        .granted_root(RootId::new("rt_a").unwrap(), 1, &identity)
        .unwrap();
    let b = engine
        .granted_root(RootId::new("rt_b").unwrap(), 1, &identity)
        .unwrap();
    let sink = Arc::new(MemorySink::default());
    engine.set_scan_sink(sink.clone()).unwrap();
    engine.register_job_runner(
        JobKind::Scan,
        Arc::new(|ctx, _| {
            while !ctx.is_cancelled() {
                std::thread::yield_now();
            }
            Ok(loomward_protocol::CoverageState::Cancelled)
        }),
    );
    let job = engine
        .job_submit(crate::jobs::JobSpec::new(
            JobKind::Scan,
            Some(a.root_id().clone()),
        ))
        .unwrap();
    let cancelled = engine
        .scan_cancel_root(&RootKey::Grant(1), b.root_id())
        .unwrap();
    assert_eq!(cancelled.len(), 1);
    assert_eq!(cancelled[0].job_id, job.job_id);
    engine
        .scan_writer_fence(&RootKey::Grant(1), b.root_id())
        .unwrap();
    for root in [&a, &b] {
        assert!(matches!(
            engine.scan_start(root, None),
            Err(EngineError::PermissionDenied { .. })
        ));
        assert!(matches!(
            engine.duplicate_read_admission(root),
            Err(EngineError::PermissionDenied { .. })
        ));
        assert!(matches!(
            sink.begin_run(root.root_id(), 1, &RunScope::FullRoot),
            Err(EngineError::PermissionDenied { .. })
        ));
    }
}

#[test]
fn pre_epoch_clock_fails_closed_to_epoch_zero() {
    use std::time::{Duration, UNIX_EPOCH};
    // Simulate a SystemTime before UNIX_EPOCH: duration_since errs.
    let pre_epoch = UNIX_EPOCH.checked_sub(Duration::from_secs(1)).unwrap();
    let seconds = pre_epoch
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    assert_eq!(seconds, 0);
    // Same epoch-zero fallback as timestamp_ns, which clamps negatives.
    assert_eq!(
        crate::events::timestamp_ns(-1).as_str(),
        "1970-01-01T00:00:00Z"
    );
    assert_eq!(
        crate::events::timestamp_ns(0).as_str(),
        "1970-01-01T00:00:00Z"
    );
    // The fail-closed timestamp() constructor must not unwrap a pre-epoch duration.
    let src = include_str!("../events.rs");
    let start = src
        .find("pub(crate) fn timestamp()")
        .expect("timestamp constructor");
    let rest = &src[start..];
    let end = rest.find("\n}\n").expect("timestamp body end") + 3;
    let body = &rest[..end];
    assert!(
        !body.contains(".unwrap()"),
        "timestamp() must not unwrap pre-epoch duration"
    );
    // Normal wall-clock output still yields a valid timestamp without panicking.
    let now = crate::events::timestamp();
    assert!(now.as_str().contains('T'));
    assert!(now.as_str().ends_with('Z'));
}

#[cfg(windows)]
thread_local! {
    pub(super) static GRANT_OPEN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(windows)]
#[test]
fn slow_grant_open_allows_state_writes_and_rechecks_the_grant() {
    use loomward_catalog::{Catalog, RootObservation, WriteCommand, WriteReply};
    if skip_elevated() {
        return;
    }
    for legacy in [false, true] {
        for mutation in ["revoke", "same_guid", "other_guid", "other_id"] {
            let state = canonical_tempdir();
            let directory = canonical_tempdir();
            let (_, identity) = loomward_windows::enumerate::NativeSource::default()
                .open_root(directory.path())
                .unwrap();
            let catalog = Catalog::open(state.path(), "synthetic").unwrap();
            let WriteReply::Root {
                grant_id, dir_id, ..
            } = catalog
                .writer()
                .call(WriteCommand::RegisterRoot(RootObservation {
                    volume_guid: if legacy {
                        None
                    } else {
                        identity.volume_guid.map(|g| format!("{g:032x}"))
                    },
                    volume_key: format!("vsn_{}", identity.volume_serial.unwrap()),
                    display_name: "Synthetic volume".into(),
                    display_path: directory.path().to_str().unwrap().into(),
                    root_file_id: Some(match identity.id.unwrap() {
                        FileIdObs::Id128(id) => id.to_vec(),
                        FileIdObs::Id64(id) => id.to_le_bytes().to_vec(),
                    }),
                    filesystem: Some("NTFS".into()),
                    origin: "fixture".into(),
                    granted_via: "fixture".into(),
                    observed_at_ns: 0,
                }))
                .unwrap()
            else {
                panic!("root expected")
            };
            let catalog_root = CatalogRootIdentity {
                row: dir_id,
                instance: rusqlite::Connection::open(state.path().join("catalog.db"))
                    .unwrap()
                    .query_row("SELECT value FROM meta WHERE key='instance_id'", [], |r| {
                        r.get(0)
                    })
                    .unwrap(),
            };
            let engine = Engine::open(EngineConfig::new(
                state.path().into(),
                DatasetClass::Synthetic,
            ))
            .unwrap();
            let (entered, wait_entered) = std::sync::mpsc::channel();
            let (release, wait_release) = std::sync::mpsc::channel();
            let admission = std::thread::spawn(move || {
                GRANT_OPEN_HOOK.with_borrow_mut(|hook| {
                    *hook = Some(Box::new(move || {
                        entered.send(()).unwrap();
                        wait_release.recv().unwrap();
                    }))
                });
                engine.granted_root(
                    RootId::new("rt_slow_open").unwrap(),
                    grant_id,
                    &catalog_root,
                )
            });
            wait_entered
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            // A short SQLite timeout makes lock ownership, rather than disk speed, the assertion.
            let state_conn = rusqlite::Connection::open(state.path().join("state.db")).unwrap();
            state_conn
                .busy_timeout(std::time::Duration::from_millis(100))
                .unwrap();
            let before: i64 = state_conn
                .query_row("SELECT state_rev FROM revision", [], |r| r.get(0))
                .unwrap();
            let write = match mutation {
                "revoke" => state_conn.execute(
                    "UPDATE root_grant SET state='revoked' WHERE id=?1",
                    [grant_id],
                ),
                "same_guid" | "other_guid" => state_conn.execute(
                    "UPDATE root_grant SET volume_guid=?1 WHERE id=?2",
                    rusqlite::params![
                        format!(
                            "{:032x}",
                            identity.volume_guid.unwrap() ^ u128::from(mutation == "other_guid")
                        ),
                        grant_id
                    ],
                ),
                "other_id" => state_conn.execute(
                    "UPDATE root_grant SET root_file_id=?1 WHERE id=?2",
                    rusqlite::params![vec![9u8; 16], grant_id],
                ),
                _ => unreachable!(),
            };
            release.send(()).unwrap();
            let result = admission.join().unwrap();
            assert_eq!(
                write.unwrap(),
                1,
                "native admission must not own the state writer lock (legacy={legacy})"
            );
            match mutation {
                "same_guid" => assert!(
                    result.is_ok(),
                    "concurrent adoption of the same identity is valid"
                ),
                "revoke" => assert_eq!(
                    result.unwrap_err().to_body().message.as_str(),
                    "grant_not_active"
                ),
                _ => assert_eq!(
                    result.unwrap_err().to_body().message.as_str(),
                    "root_identity_changed"
                ),
            }
            assert_eq!(
                state_conn
                    .query_row("SELECT state_rev FROM revision", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                before,
                "no admission write on a changed or already-adopted grant"
            );
            if legacy && matches!(mutation, "revoke" | "other_id") {
                assert_eq!(
                    state_conn
                        .query_row(
                            "SELECT volume_guid FROM root_grant WHERE id=?1",
                            [grant_id],
                            |r| r.get::<_, Option<String>>(0)
                        )
                        .unwrap(),
                    None
                );
            }
        }
    }
}
/// h2-scan finding 1, as far as it reaches the engine: a revocation that lands after coverage
/// was computed but before it was published still ends the job `Cancelled`, never `Complete`.
/// The durable writer separately refuses that late finish (`EndRun` requires an active grant,
/// and revocation commits durably before it cancels); the memory sink here has no grant.
pub(super) fn revoke_at_publication(
    at: &'static str,
    scan: impl Fn(&GrantedRoot, Arc<dyn ScanSink>, Arc<AtomicBool>) -> EngineResult<ScanReport>
        + Send
        + Sync
        + 'static,
) {
    use loomward_protocol::{CoverageState, JobKind, JobState};
    use std::sync::{mpsc, Mutex};
    let e = Engine::open(EngineConfig::new("unused".into(), DatasetClass::Synthetic)).unwrap();
    let root = root();
    let root_id = root.root_id().clone();
    let (reached, at_publication) = mpsc::channel();
    let (go, wait) = mpsc::channel::<()>();
    let wait = Arc::new(Mutex::new(wait));
    e.register_job_runner(
        JobKind::Scan,
        Arc::new(move |ctx, _| {
            let (reached, wait) = (reached.clone(), wait.clone());
            super::pipeline::BEFORE_PUBLISH.with_borrow_mut(|hook| {
                *hook = Some(Box::new(move |point: &str| {
                    if point == at {
                        reached.send(()).unwrap();
                        wait.lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(5))
                            .unwrap();
                    }
                }))
            });
            // As the production runners do.
            let report = scan(&root, Arc::new(MemorySink::default()), ctx.cancel.clone())?;
            ctx.set_scan_report(&report)?;
            Ok(if report.coverage_complete() {
                CoverageState::Complete
            } else {
                CoverageState::Partial
            })
        }),
    );
    let job = e
        .job_submit(crate::jobs::JobSpec::new(
            JobKind::Scan,
            Some(root_id.clone()),
        ))
        .unwrap();
    at_publication.recv_timeout(Duration::from_secs(5)).unwrap();
    e.scan_cancel_root(&RootKey::Fixture(root_id.clone()), &root_id)
        .unwrap();
    go.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let job = loop {
        let job = e.job_status(&job.job_id).unwrap();
        if matches!(
            job.state,
            JobState::Cancelled | JobState::Completed | JobState::Failed
        ) {
            break job;
        }
        assert!(std::time::Instant::now() < deadline, "{at}: {job:?}");
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(
        (job.state, job.coverage),
        (JobState::Cancelled, Some(CoverageState::Cancelled)),
        "{at}"
    );
}
#[test]
fn revocation_before_publication_ends_the_job_cancelled() {
    revoke_at_publication("run_scan", |root, sink, cancel| {
        run_scan(
            &Tree,
            root,
            1,
            sink,
            Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
            cancel,
            ScanOptions {
                workers: 1,
                ..Default::default()
            },
        )
    });
}
/// Delegates to [`Tree`] but opens its root as `root`, and any other path as `state`.
struct OpenedAs {
    root: OpenedIdentity,
    state: Option<OpenedIdentity>,
}
impl DirSource for OpenedAs {
    type Dir = u64;
    fn strategy(&self) -> Strategy {
        Tree.strategy()
    }
    fn open_root(&self, path: &std::path::Path) -> Result<(u64, OpenedIdentity), SourceError> {
        if path == root().path() {
            return Ok((1, self.root));
        }
        self.state.map(|id| (9, id)).ok_or(SourceError::Io(2))
    }
    fn open_child(
        &self,
        parent: &u64,
        e: &RawEntry<'_>,
    ) -> Result<(u64, OpenedIdentity), SourceError> {
        Tree.open_child(parent, e)
    }
    fn list(&self, dir: &u64, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        Tree.list(dir, sink)
    }
}
fn scan_opened_as(
    source: OpenedAs,
    options: ScanOptions,
) -> (EngineResult<ScanReport>, Arc<MemorySink>) {
    let sink = Arc::new(MemorySink::default());
    let report = run_scan(
        &source,
        &root(),
        1,
        sink.clone(),
        Arc::new(crate::budgets::ByteBudget::new(crate::budgets::SCAN_BYTES)),
        Arc::new(AtomicBool::new(false)),
        ScanOptions {
            workers: 1,
            ..options
        },
    );
    (report, sink)
}
/// h2-scan finding 2: the opened root keeps the grant's IDs but its identity basis changed, so
/// the IDs no longer bind what the grant bound (a verified root now unverified, or the reverse).
#[test]
fn identity_basis_change_is_root_identity_changed() {
    for basis in [IdBasis::Unverified, IdBasis::PostOpen, IdBasis::None] {
        let (report, sink) = scan_opened_as(
            OpenedAs {
                root: OpenedIdentity {
                    basis,
                    ..identity(1)
                },
                state: None,
            },
            ScanOptions::default(),
        );
        assert_eq!(
            report.unwrap_err().to_body().message.as_str(),
            "root_identity_changed",
            "{basis:?}"
        );
        assert!(!sink.sweep_allowed(root().root_id()));
    }
}
/// h2-scan finding 3: a state exclusion that cannot be identified fails closed instead of
/// counting Loomward's own state as data.
#[test]
fn unidentifiable_state_exclusion_refuses_the_scan() {
    let without_id = OpenedIdentity {
        id: None,
        basis: IdBasis::None,
        ..identity(7)
    };
    for state in [None, Some(without_id)] {
        let (report, sink) = scan_opened_as(
            OpenedAs {
                root: identity(1),
                state,
            },
            ScanOptions {
                excluded_path: Some("missing-state".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            report.unwrap_err().to_body().message.as_str(),
            "state exclusion identity unavailable",
            "{state:?}"
        );
        assert!(!sink.sweep_allowed(root().root_id()));
    }
    // A state directory elsewhere, with its own identity, still scans complete.
    let (report, _) = scan_opened_as(
        OpenedAs {
            root: identity(1),
            state: Some(identity(7)),
        },
        ScanOptions {
            excluded_path: Some("elsewhere-state".into()),
            ..Default::default()
        },
    );
    assert!(report.unwrap().complete);
}
