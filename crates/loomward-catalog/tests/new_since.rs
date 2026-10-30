//! #1092 `Reader::new_since`: synthetic catalogue rows only (no scan, no filesystem). The `born_run`
//! semantics themselves (what survives a rescan) are pinned against real scans by the engine's
//! `born_run_rescans` test; this file pins the query over rows whose `born_run` is set by hand.
use loomward_catalog::*;
use rusqlite::{params, Connection};

struct Lab {
    _temp: tempfile::TempDir,
    catalog: Catalog,
    db: Connection,
    root: i64,
    dir: i64,
}

fn register(catalog: &Catalog, key: &str, file_id: u8) -> (i64, i64) {
    let WriteReply::Root {
        root_id, dir_id, ..
    } = catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: key.into(),
            display_name: "Lab".into(),
            display_path: "Synthetic lab".into(),
            root_file_id: Some(vec![file_id; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))
        .unwrap()
    else {
        panic!()
    };
    (root_id, dir_id)
}

impl Lab {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(temp.path(), "synthetic").unwrap();
        let (root, dir) = register(&catalog, "synthetic-lab", 1);
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        db.execute(
            "UPDATE dir SET listing_state='complete',sub_complete=1 WHERE id=?1",
            [dir],
        )
        .unwrap();
        Self {
            _temp: temp,
            catalog,
            db,
            root,
            dir,
        }
    }
    /// A `scan_run` row; `complete` is the counter a finished full traversal records.
    fn run(&self, mode: &str, state: &str, complete: Option<bool>) -> i64 {
        self.run_of(self.root, mode, state, complete)
    }
    fn run_of(&self, root: i64, mode: &str, state: &str, complete: Option<bool>) -> i64 {
        let counters = match complete {
            Some(c) => format!("{{\"complete\":{c}}}"),
            None => "{}".into(),
        };
        self.db
            .execute(
                "INSERT INTO scan_run(root_id,mode,strategy,state,started_at_ns,finished_at_ns,counters_json,budget_json) VALUES(?1,?2,'fixture',?3,1,?4,?5,'{}')",
                params![root, mode, state, if state == "running" { None } else { Some(1_000_000_000_i64) }, counters],
            )
            .unwrap();
        self.db.last_insert_rowid()
    }
    fn directory(&self, parent: i64, name: &str, born: i64, bytes: i64) -> i64 {
        self.directory_in(self.root, parent, name, born, bytes, "complete", 0)
    }
    #[allow(clippy::too_many_arguments)]
    fn directory_in(
        &self,
        root: i64,
        parent: i64,
        name: &str,
        born: i64,
        bytes: i64,
        state: &str,
        flags: u32,
    ) -> i64 {
        self.db
            .execute(
                "INSERT INTO dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,flags,listing_state,born_run,seen_run,sub_complete,sub_logical,sub_allocated) VALUES(?1,?2,?3,'path_observation','none',1,16,?7,?6,?4,?4,1,?5,?5)",
                params![root, parent, name, born, bytes, state, flags],
            )
            .unwrap();
        self.db.last_insert_rowid()
    }
    fn file(&self, dir: i64, name: &str, born: i64, logical: i64, allocated: Option<i64>) -> i64 {
        self.file_flagged(dir, name, born, logical, allocated, 0)
    }
    fn file_flagged(
        &self,
        dir: i64,
        name: &str,
        born: i64,
        logical: i64,
        allocated: Option<i64>,
        flags: u32,
    ) -> i64 {
        self.db
            .execute(
                "INSERT INTO file(dir_id,name,logical,allocated,attrs,flags,born_run,seen_run) VALUES(?1,?2,?3,?4,32,?6,?5,?5)",
                params![dir, name, logical, allocated, born, flags],
            )
            .unwrap();
        self.db.last_insert_rowid()
    }
    fn since(&self, baseline: Option<i64>, cap: usize) -> Result<NewSince> {
        self.catalog
            .reader()
            .unwrap()
            .new_since(self.root, baseline, cap)
    }
    fn totals(&self, baseline: Option<i64>) -> NewSinceTotals {
        self.since(baseline, 50).unwrap().totals.unwrap()
    }
}

#[test]
fn no_earlier_completed_scan_is_no_baseline_never_zero() {
    let lab = Lab::new();
    let none = lab.since(None, 5).unwrap();
    assert!(none.baseline.is_none() && none.latest.is_none() && none.totals.is_none());
    // One completed scan is still not "since the previous scan"; a running one and a failed one do not count.
    lab.run("full", "failed", None);
    lab.run("full", "completed", Some(true));
    lab.run("full", "running", None);
    let one = lab.since(None, 5).unwrap();
    assert!(one.baseline.is_none() && one.totals.is_none() && one.largest_files.is_empty());
    assert_eq!(one.latest.unwrap().run, 2);
}

#[test]
fn default_baseline_is_the_scan_before_the_newest_completed_full_scan() {
    let lab = Lab::new();
    let (first, second) = (
        lab.run("full", "completed", Some(true)),
        lab.run("full", "completed", Some(true)),
    );
    let old = lab.file(lab.dir, "old.bin", first, 100, Some(4096));
    lab.file(lab.dir, "new.bin", second, 7, Some(4096));
    let _ = old;
    let r = lab.since(None, 5).unwrap();
    assert_eq!(r.baseline.as_ref().unwrap().run, first);
    assert_eq!(r.latest.as_ref().unwrap().run, second);
    assert!(r.baseline.unwrap().complete);
    assert!(r.partial_reasons.is_empty(), "{:?}", r.partial_reasons);
    let t = r.totals.unwrap();
    assert_eq!(
        (t.new_files, t.new_dirs, t.logical_bytes, t.allocated_bytes),
        (1, 0, 7, Some(4096))
    );
    // A third full scan moves the baseline up: what arrived in the second scan is no longer new.
    lab.run("full", "completed", Some(true));
    assert_eq!(lab.totals(None).new_files, 0);
    assert_eq!(lab.since(None, 5).unwrap().baseline.unwrap().run, second);
}

#[test]
fn the_cut_is_greater_than_baseline_so_watcher_and_targeted_runs_count() {
    let lab = Lab::new();
    let first = lab.run("full", "completed", Some(true));
    let watcher = lab.run("full", "completed", Some(true)); // the watcher's own full-root run
    let targeted = lab.run("targeted", "completed", Some(true));
    let last = lab.run("full", "completed", Some(true));
    lab.file(lab.dir, "a", watcher, 1, Some(1));
    lab.file(lab.dir, "b", targeted, 2, Some(2));
    lab.file(lab.dir, "c", last, 4, Some(4));
    lab.file(lab.dir, "kept", first, 8, Some(8));
    // Previous full scan before the newest one is the watcher's run 2 (full-root, completed): only run 3+ count.
    let r = lab.since(None, 5).unwrap();
    assert_eq!(r.baseline.as_ref().unwrap().run, watcher);
    assert_eq!(r.totals.unwrap().logical_bytes, 2 + 4);
    // Asking from the first scan sees everything born after it, whichever run stamped it.
    let t = lab.totals(Some(first));
    assert_eq!((t.new_files, t.logical_bytes), (3, 1 + 2 + 4));
    // The targeted run is a valid baseline too; the newest completed run of any mode is the latest considered.
    let from_targeted = lab.since(Some(targeted), 5).unwrap();
    assert_eq!(from_targeted.totals.unwrap().logical_bytes, 4);
    assert_eq!(from_targeted.latest.unwrap().run, last);
}

#[test]
fn unknown_allocation_stays_unknown_not_zero() {
    let lab = Lab::new();
    let first = lab.run("full", "completed", Some(true));
    let second = lab.run("full", "completed", Some(true));
    lab.file(lab.dir, "known", second, 10, Some(4096));
    lab.file(lab.dir, "unknown", second, 20, None);
    let t = lab.totals(Some(first));
    assert_eq!((t.new_files, t.logical_bytes), (2, 30));
    assert_eq!((t.allocated_bytes, t.allocation_unknown_files), (None, 1));
    let r = lab.since(Some(first), 5).unwrap();
    assert_eq!(r.largest_files[0].name, "unknown");
    assert_eq!(r.largest_files[0].allocated_bytes, None);
    assert_eq!(r.largest_files[1].allocated_bytes, Some(4096));
}

#[test]
fn largest_files_are_capped_ordered_and_tie_broken_by_row() {
    let lab = Lab::new();
    let first = lab.run("full", "completed", Some(true));
    let second = lab.run("full", "completed", Some(true));
    let sub = lab.directory(lab.dir, "sub", first, 0);
    let ids: Vec<i64> = [("a", 5), ("b", 9), ("c", 9), ("d", 1), ("e", 7)]
        .iter()
        .map(|(n, s)| {
            lab.file(
                if *n == "e" { sub } else { lab.dir },
                n,
                second,
                *s,
                Some(*s),
            )
        })
        .collect();
    let r = lab.since(Some(first), 3).unwrap();
    assert_eq!(
        r.totals.as_ref().unwrap().new_files,
        5,
        "the count is not capped"
    );
    let got: Vec<_> = r
        .largest_files
        .iter()
        .map(|f| (f.name.as_str(), f.logical_bytes))
        .collect();
    assert_eq!(got, [("b", 9), ("c", 9), ("e", 7)]);
    assert_eq!(r.largest_files[0].node, NodeKey::File(ids[1]));
    assert!(
        r.largest_files[2].path.text.ends_with("sub / e"),
        "{}",
        r.largest_files[2].path.text
    );
    assert_eq!(lab.since(Some(first), 1).unwrap().largest_files.len(), 1);
    assert!(lab.since(Some(first), 0).is_err() && lab.since(Some(first), 51).is_err());
}

#[test]
fn new_folders_are_tops_of_new_subtrees_and_old_folders_with_new_files_are_not_folders() {
    let lab = Lab::new();
    let first = lab.run("full", "completed", Some(true));
    let second = lab.run("full", "completed", Some(true));
    let old = lab.directory(lab.dir, "old", first, 500);
    lab.file(old, "added.bin", second, 50, Some(50)); // a new file in an old folder
    let big = lab.directory(lab.dir, "big", second, 1000);
    let inner = lab.directory(big, "inner", second, 900); // new, but under a new parent: not a top
    lab.file(inner, "x", second, 900, None);
    let small = lab.directory(lab.dir, "small", second, 10);
    let r = lab.since(Some(first), 10).unwrap();
    let names: Vec<_> = r.largest_folders.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["big", "small"]);
    assert_eq!(r.largest_folders[0].logical_bytes, Some(1000));
    assert_eq!(r.largest_folders[0].node, NodeKey::Dir(big));
    assert_eq!(r.largest_folders[1].node, NodeKey::Dir(small));
    // Three new directories, two new files; the root directory (born in run 0) is never new.
    let t = r.totals.unwrap();
    assert_eq!((t.new_dirs, t.new_files, t.logical_bytes), (3, 2, 950));
    assert_eq!(lab.since(Some(first), 1).unwrap().largest_folders.len(), 1);
}

#[test]
fn excluded_entries_other_roots_and_hidden_directories_are_not_counted() {
    let lab = Lab::new();
    let first = lab.run("full", "completed", Some(true));
    let second = lab.run("full", "completed", Some(true));
    lab.file(lab.dir, "counted", second, 1, Some(1));
    lab.file_flagged(lab.dir, "cloud", second, 100, Some(100), 1 << 3); // seen, not followed: never in totals
    let excluded = lab.directory_in(lab.root, lab.dir, "state", second, 0, "excluded", 1 << 5);
    lab.file(excluded, "inside-excluded", second, 100, Some(100));
    let gone = lab.directory_in(lab.root, lab.dir, "gone", second, 0, "absent_pending", 0);
    lab.file(gone, "inside-gone", second, 100, Some(100));
    let (other, other_dir) = register(&lab.catalog, "synthetic-other", 2);
    lab.run_of(other, "full", "completed", Some(true));
    lab.run_of(other, "full", "completed", Some(true));
    lab.db
        .execute(
            "UPDATE dir SET listing_state='complete' WHERE id=?1",
            [other_dir],
        )
        .unwrap();
    lab.db
        .execute(
            "INSERT INTO file(dir_id,name,logical,allocated,attrs,flags,born_run,seen_run) VALUES(?1,'elsewhere',999,999,32,0,99,99)",
            [other_dir],
        )
        .unwrap();
    let t = lab.totals(Some(first));
    assert_eq!((t.new_files, t.logical_bytes, t.new_dirs), (1, 1, 0));
    // The other root answers for itself only, from its own runs.
    let theirs = lab
        .catalog
        .reader()
        .unwrap()
        .new_since(other, None, 5)
        .unwrap();
    assert_eq!(theirs.totals.unwrap().logical_bytes, 999);
    assert!(
        lab.since(Some(lab.run_of(other, "full", "completed", Some(true))), 5)
            .is_err(),
        "another root's run is not a baseline"
    );
}

#[test]
fn an_incomplete_or_failed_baseline_makes_the_answer_partial() {
    let lab = Lab::new();
    let failed = lab.run("full", "failed", None);
    let partial = lab.run("full", "completed", Some(false));
    let unknown = lab.run("full", "completed", None); // an older row without counters
    let clean = lab.run("full", "completed", Some(true));
    let cancelled = lab.run("full", "cancelled", None);
    let newest = lab.run("full", "completed", Some(true));
    lab.file(lab.dir, "later", newest, 3, Some(3));
    for (baseline, reason) in [
        (failed, "baseline_not_completed"),
        (cancelled, "baseline_not_completed"),
        (partial, "baseline_incomplete"),
        (unknown, "baseline_incomplete"),
    ] {
        let r = lab.since(Some(baseline), 5).unwrap();
        assert_eq!(r.partial_reasons, [reason], "baseline {baseline}");
        assert!(!r.baseline.as_ref().unwrap().complete);
        assert_eq!(
            r.totals.unwrap().new_files,
            1,
            "the figure is still reported, as partial"
        );
    }
    assert!(lab
        .since(Some(clean), 5)
        .unwrap()
        .partial_reasons
        .is_empty());
    // Default selection takes the previous completed full scan as it is, and says when it was incomplete.
    let default = lab.since(None, 5).unwrap();
    assert_eq!(default.baseline.unwrap().run, clean);
    assert!(default.partial_reasons.is_empty());
    lab.run("full", "completed", Some(false)); // newest is now a partial scan; the baseline is the clean `newest`
    assert_eq!(lab.since(None, 5).unwrap().baseline.unwrap().run, newest);
}

#[test]
fn a_running_scan_partial_coverage_and_repairs_are_named() {
    let lab = Lab::new();
    let first = lab.run("full", "completed", Some(true));
    lab.run("full", "completed", Some(true));
    assert!(lab
        .since(Some(first), 5)
        .unwrap()
        .partial_reasons
        .is_empty());
    let running = lab.run("full", "running", None);
    assert_eq!(
        lab.since(Some(first), 5).unwrap().partial_reasons,
        ["scan_running"]
    );
    assert!(
        lab.since(Some(running), 5).is_err(),
        "a running run is not a baseline"
    );
    lab.db
        .execute(
            "UPDATE scan_run SET state='cancelled' WHERE id=?1",
            [running],
        )
        .unwrap();
    lab.db
        .execute(
            "UPDATE dir SET listing_state='incomplete' WHERE id=?1",
            [lab.dir],
        )
        .unwrap();
    assert_eq!(
        lab.since(Some(first), 5).unwrap().partial_reasons,
        ["coverage_partial"]
    );
    lab.db
        .execute(
            "UPDATE dir SET listing_state='complete',dirty_rev=agg_valid_rev+1 WHERE id=?1",
            [lab.dir],
        )
        .unwrap();
    assert_eq!(
        lab.since(Some(first), 5).unwrap().partial_reasons,
        ["catalogue_repairing"]
    );
    lab.db
        .execute(
            "UPDATE dir SET dirty_rev=agg_valid_rev WHERE id=?1",
            [lab.dir],
        )
        .unwrap();
    lab.db
        .execute("UPDATE root SET state='repairing' WHERE id=?1", [lab.root])
        .unwrap();
    assert_eq!(
        lab.since(Some(first), 5).unwrap().partial_reasons,
        ["catalogue_repairing"]
    );
}

#[test]
fn unknown_root_and_unknown_baseline_are_refused() {
    let lab = Lab::new();
    lab.run("full", "completed", Some(true));
    lab.run("full", "completed", Some(true));
    assert!(matches!(
        lab.catalog
            .reader()
            .unwrap()
            .new_since(lab.root + 99, None, 5),
        Err(Error::NotFound)
    ));
    assert!(matches!(lab.since(Some(99), 5), Err(Error::NotFound)));
    assert!(matches!(lab.since(Some(0), 5), Err(Error::Invalid(_))));
}

#[test]
fn a_read_changes_nothing() {
    let lab = Lab::new();
    let first = lab.run("full", "completed", Some(true));
    let second = lab.run("full", "completed", Some(true));
    lab.file(lab.dir, "f", second, 1, Some(1));
    let before: i64 = lab
        .db
        .query_row("SELECT catalog_rev FROM revision", [], |r| r.get(0))
        .unwrap();
    lab.db
        .execute("UPDATE dir SET dirty_rev=agg_valid_rev+1", [])
        .unwrap();
    lab.since(Some(first), 5).unwrap();
    let dirty: bool = lab
        .db
        .query_row(
            "SELECT count(*)>0 FROM dir WHERE dirty_rev>agg_valid_rev",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let after: i64 = lab
        .db
        .query_row("SELECT catalog_rev FROM revision", [], |r| r.get(0))
        .unwrap();
    assert!(
        dirty && after == before,
        "new_since must not repair aggregates"
    );
}

#[test]
fn a_budget_hit_or_an_expired_deadline_is_an_error_never_a_short_count() {
    let lab = Lab::new();
    let first = lab.run("full", "completed", Some(true));
    let second = lab.run("full", "completed", Some(true));
    lab.db.execute_batch("BEGIN").unwrap();
    for i in 0..3000 {
        lab.file(lab.dir, &format!("f{i}"), second, 1, Some(1));
    }
    lab.db.execute_batch("COMMIT").unwrap();
    // Plenty of budget: all 3000 are counted.
    let mut reader = lab.catalog.reader().unwrap();
    let whole = reader
        .new_since_within(lab.root, Some(first), 5, 32_000_000)
        .unwrap();
    assert_eq!(whole.totals.unwrap().new_files, 3000);
    // A budget far below the pass: ResourceBudget, not partial totals.
    assert!(matches!(
        reader.new_since_within(lab.root, Some(first), 5, 1000),
        Err(Error::ResourceBudget)
    ));
    // The reader is still usable afterwards (the progress handler was removed).
    assert_eq!(lab.totals(Some(first)).new_files, 3000);
    // An expired deadline is Cancelled.
    let mut late = lab.catalog.reader().unwrap();
    late.set_deadline(std::time::Instant::now());
    assert!(matches!(
        late.new_since(lab.root, Some(first), 5),
        Err(Error::Cancelled)
    ));
}
