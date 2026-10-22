use loomward_catalog::*;

#[cfg(feature = "test-hooks")]
thread_local! {
    static INTERRUPT: std::cell::RefCell<Option<rusqlite::InterruptHandle>> = const { std::cell::RefCell::new(None) };
}

#[cfg(feature = "test-hooks")]
#[test]
fn cloud_summary_external_interrupt_is_not_budget_exhaustion() {
    fn interrupt_walk(event: rusqlite::trace::TraceEvent<'_>) {
        if let rusqlite::trace::TraceEvent::Stmt(_, sql) = event {
            if sql.starts_with("WITH RECURSIVE walk(") {
                INTERRUPT.with(|handle| handle.borrow().as_ref().unwrap().interrupt());
            }
        }
    }
    let (_tmp, c, _, dir) = fixture_with_skipped(0);
    let mut reader = c.reader().unwrap();
    INTERRUPT.with(|handle| *handle.borrow_mut() = Some(reader.interrupt_handle()));
    reader.trace_statements_for_test(Some(interrupt_walk));
    let result = reader.cloud_summary(dir, Reader::DEFAULT_CLOUD_SUMMARY_STEPS);
    reader.trace_statements_for_test(None);
    INTERRUPT.with(|handle| handle.borrow_mut().take());
    assert!(matches!(result, Err(Error::Sql(_))), "{result:?}");
    assert!(
        !reader
            .cloud_summary(dir, Reader::DEFAULT_CLOUD_SUMMARY_STEPS)
            .unwrap()
            .partial
    );
}

fn fixture() -> (tempfile::TempDir, Catalog, i64, i64) {
    fixture_with_skipped(1)
}

fn fixture_with_skipped(skipped: u64) -> (tempfile::TempDir, Catalog, i64, i64) {
    let tmp = tempfile::tempdir().unwrap();
    let c = Catalog::open(tmp.path(), "synthetic").unwrap();
    let WriteReply::Root {
        grant_id, dir_id, ..
    } = c
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "fixture".into(),
            display_name: "Synthetic".into(),
            display_path: "Synthetic root".into(),
            root_file_id: Some(vec![0; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))
        .unwrap()
    else {
        panic!()
    };
    let WriteReply::Run(run) = c
        .writer()
        .call(WriteCommand::BeginRun {
            grant_id,
            mode: "full".into(),
            strategy: "fixture".into(),
            started_at_ns: 0,
        })
        .unwrap()
    else {
        panic!()
    };
    let files = [
        ("remote", 900, Some(16), 0x400000, 1 << 3),
        ("local", 100, Some(4096), 0x80, 0),
        ("pinned", 200, Some(4096), 0x80000, 0),
        ("missing", 50, None, 0, 0),
    ]
    .into_iter()
    .map(|(name, size, allocated, attrs, flags)| {
        let mut o = Observation::file(name, size, allocated);
        o.attrs = attrs;
        o.flags = flags;
        o
    })
    .collect();
    c.writer()
        .call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id,
            files,
            dirs: vec![],
            state: "complete".into(),
            skipped,
            errors: 0,
        }))
        .unwrap();
    c.writer()
        .call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 1,
        })
        .unwrap();
    (tmp, c, grant_id, dir_id)
}

#[test]
fn cloud_summary_clean_completed_scan_is_not_partial() {
    let (_tmp, c, _, dir) = fixture_with_skipped(0);
    assert!(
        !c.reader()
            .unwrap()
            .cloud_summary(dir, Reader::DEFAULT_CLOUD_SUMMARY_STEPS)
            .unwrap()
            .partial
    );
}

#[test]
fn cloud_summary_running_scan_is_partial() {
    let (_tmp, c, grant, dir) = fixture_with_skipped(0);
    c.writer()
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "refresh".into(),
            strategy: "fixture".into(),
            started_at_ns: 2,
        })
        .unwrap();
    assert!(
        c.reader()
            .unwrap()
            .cloud_summary(dir, Reader::DEFAULT_CLOUD_SUMMARY_STEPS)
            .unwrap()
            .partial
    );
}

#[test]
fn cloud_summary_retains_observed_bytes_across_reopen() {
    let (tmp, c, _, dir) = fixture();
    drop(c);
    let c = Catalog::open(tmp.path(), "synthetic").unwrap();
    let report = c
        .reader()
        .unwrap()
        .cloud_summary(dir, Reader::DEFAULT_CLOUD_SUMMARY_STEPS)
        .unwrap();
    #[cfg(windows)]
    {
        assert_eq!(
            report.cloud_only,
            CloudBytes {
                files: 1,
                logical_bytes: 900,
                allocated_bytes: Some(16)
            }
        );
        assert_eq!(
            report.local,
            CloudBytes {
                files: 1,
                logical_bytes: 100,
                allocated_bytes: Some(4096)
            }
        );
        assert_eq!(report.pinned.logical_bytes, 200);
        assert_eq!(report.unknown.logical_bytes, 50);
    }
    #[cfg(not(windows))]
    assert_eq!(report.unknown.logical_bytes, 1250);
    assert_eq!(report.unknown.allocated_bytes, None);
    assert!(report.partial);
}

#[test]
fn cloud_summary_refuses_revoked_and_missing_directories_and_deadlines() {
    let (_tmp, c, grant_id, dir) = fixture();
    let mut reader = c.reader().unwrap();
    assert!(matches!(
        reader.cloud_summary(i64::MAX, Reader::DEFAULT_CLOUD_SUMMARY_STEPS),
        Err(Error::NotFound)
    ));
    reader.set_deadline(std::time::Instant::now());
    assert!(matches!(
        reader.cloud_summary(dir, Reader::DEFAULT_CLOUD_SUMMARY_STEPS),
        Err(Error::Cancelled)
    ));
    c.writer()
        .call(WriteCommand::RevokeGrant {
            grant_id,
            revoked_at_ns: 2,
        })
        .unwrap();
    assert!(matches!(
        c.reader()
            .unwrap()
            .cloud_summary(dir, Reader::DEFAULT_CLOUD_SUMMARY_STEPS),
        Err(Error::NotFound)
    ));
}

#[test]
fn cloud_summary_recurses_but_hides_absent_and_excluded_subtrees() {
    let (tmp, c, grant, root) = fixture();
    let WriteReply::Run(run) = c
        .writer()
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "refresh".into(),
            strategy: "fixture".into(),
            started_at_ns: 2,
        })
        .unwrap()
    else {
        panic!()
    };
    let child = Observation::file("child", 0, None);
    c.writer()
        .call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id: root,
            files: vec![],
            dirs: vec![child.clone()],
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        }))
        .unwrap();
    let db = rusqlite::Connection::open(tmp.path().join("catalog.db")).unwrap();
    let dir = db
        .query_row("SELECT id FROM dir WHERE name='child'", [], |r| r.get(0))
        .unwrap();
    let mut remote = Observation::file("remote-child", 800, Some(32));
    remote.attrs = 0x1000;
    remote.flags = 1 << 3;
    c.writer()
        .call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id: dir,
            files: vec![remote],
            dirs: vec![Observation::file("grandchild", 0, None)],
            state: "complete".into(),
            skipped: 1,
            errors: 0,
        }))
        .unwrap();
    let grandchild = db
        .query_row("SELECT id FROM dir WHERE name='grandchild'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mut remote = Observation::file("remote-grandchild", 1200, Some(64));
    remote.attrs = 0x1000;
    remote.flags = 1 << 3;
    c.writer()
        .call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id: grandchild,
            files: vec![remote],
            dirs: vec![],
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        }))
        .unwrap();
    c.writer()
        .call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 3,
        })
        .unwrap();
    let mut reader = c.reader().unwrap();
    let report = reader
        .cloud_summary(root, Reader::DEFAULT_CLOUD_SUMMARY_STEPS)
        .unwrap();
    let bucket = if cfg!(windows) {
        &report.cloud_only
    } else {
        &report.unknown
    };
    assert_eq!(
        bucket,
        &CloudBytes {
            files: 2,
            logical_bytes: 2000,
            allocated_bytes: Some(96)
        }
    );
    assert!(report.partial);
    // Simulate retained observations beneath a directory that is no longer traversable.
    // Only synthetic catalogue rows are changed, never a scanned path.
    let mut counts = Vec::new();
    for (state, flags) in [
        ("excluded", 0),
        ("denied", 0),
        ("complete", EXCLUDED_FLAGS),
        ("absent_pending", 0),
    ] {
        db.execute(
            "UPDATE dir SET listing_state=?1,flags=?2 WHERE id=?3",
            rusqlite::params![state, flags, dir],
        )
        .unwrap();
        let report = reader
            .cloud_summary(root, Reader::DEFAULT_CLOUD_SUMMARY_STEPS)
            .unwrap();
        counts.push(report.cloud_only.files + report.unknown.files);
        if state == "absent_pending" {
            assert!(matches!(
                reader.cloud_summary(dir, Reader::DEFAULT_CLOUD_SUMMARY_STEPS),
                Err(Error::NotFound)
            ));
        }
    }
    assert_eq!(counts, [0, 0, 0, 0], "excluded, denied, flagged, absent");
}

#[test]
fn cloud_summary_empty_and_unknown_allocation_are_distinct() {
    let (tmp, c, _, dir) = fixture();
    let db = rusqlite::Connection::open(tmp.path().join("catalog.db")).unwrap();
    // All recall variants, including a pin request, remain placeholders. No provider is needed.
    db.execute(
        "UPDATE file SET attrs=4194304 | 524288, allocated=NULL WHERE name='remote'",
        [],
    )
    .unwrap();
    let report = c
        .reader()
        .unwrap()
        .cloud_summary(dir, Reader::DEFAULT_CLOUD_SUMMARY_STEPS)
        .unwrap();
    if cfg!(windows) {
        assert_eq!(report.cloud_only.logical_bytes, 900);
        assert_eq!(report.cloud_only.allocated_bytes, None);
    }
    db.execute("DELETE FROM file", []).unwrap();
    let report = c
        .reader()
        .unwrap()
        .cloud_summary(dir, Reader::DEFAULT_CLOUD_SUMMARY_STEPS)
        .unwrap();
    for bucket in [
        report.cloud_only,
        report.local,
        report.pinned,
        report.unknown,
    ] {
        assert_eq!(
            bucket,
            CloudBytes {
                files: 0,
                logical_bytes: 0,
                allocated_bytes: Some(0)
            }
        );
    }
}

#[test]
fn cloud_summary_bounds_work_instead_of_publishing_truncated_totals() {
    let (tmp, c, _, dir) = fixture();
    let db = rusqlite::Connection::open(tmp.path().join("catalog.db")).unwrap();
    db.execute(
        "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000)
        INSERT INTO file(dir_id,name,logical,allocated,attrs,flags,born_run,seen_run)
        SELECT ?1,'synthetic-'||x,1,0,4194304,8,1,1 FROM n",
        [dir],
    )
    .unwrap();
    assert!(matches!(
        c.reader().unwrap().cloud_summary(dir, 2_000_000),
        Err(Error::ResourceBudget)
    ));
    let mut reader = c.reader().unwrap();
    for budget in [0, 1] {
        assert!(matches!(
            reader.cloud_summary(dir, budget),
            Err(Error::ResourceBudget)
        ));
    }
    let report = reader
        .cloud_summary(dir, Reader::DEFAULT_CLOUD_SUMMARY_STEPS)
        .unwrap();
    assert_eq!(
        report.cloud_only.files + report.local.files + report.pinned.files + report.unknown.files,
        100_004
    );
}
