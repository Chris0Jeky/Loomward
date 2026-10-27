//! #782/#789: a targeted refresh lists its hinted directories and settles their freshness,
//! through the same tracker -> `scan_refresh_scopes` path the freshness pump uses. Synthetic
//! fixtures only, created outside the checkout.
use super::*;
use crate::tests::{f3_wait_live, lab, settle, start, Lab};
use loomward_windows::watch::Change;
use std::collections::BTreeMap;
use std::path::Path;

type Payload = serde_json::Map<String, serde_json::Value>;
/// (name, filesystem change, watcher hints, entries-examined bound).
type Case<'a> = (&'a str, &'a dyn Fn(), &'a [&'a str], u64);

/// Extra structure beside the lab's own tree: `c/sub1`, `c/sub2` and a bulk `big/` that a
/// targeted refresh of another directory must never enumerate.
fn grow(path: &Path) {
    for sub in ["sub1", "sub2"] {
        let dir = path.join("c").join(sub);
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        for n in 0..3 {
            std::fs::write(dir.join(format!("{n}.bin")), vec![1u8; n + 1]).unwrap();
        }
        std::fs::write(dir.join("inner").join("i.bin"), [2u8; 4]).unwrap();
    }
    for d in 0..20 {
        let dir = path.join("big").join(format!("d{d}"));
        std::fs::create_dir_all(&dir).unwrap();
        for f in 0..10 {
            std::fs::write(dir.join(format!("f{f}.bin")), [3u8; 2]).unwrap();
        }
    }
}

fn ready() -> Option<Lab> {
    let lab = lab()?;
    grow(&lab.path);
    // Keep fixture creation outside the scan-read ChangeTime slack (#717).
    std::thread::sleep(Duration::from_millis(400));
    settle(&lab.svc, &start(&lab.svc, &lab.root, "full"));
    f3_wait_live(&lab.svc);
    lab.svc.inner.freshness.pause_pump();
    Some(lab)
}

/// Hints as the watcher reports them (root-relative, backslash-separated names), then the
/// tracker's own refresh. Returns the finished job and whether it ran targeted.
fn refresh_with(svc: &crate::Service, hints: &[&str], overflow: bool) -> (Payload, String) {
    {
        let mut roots = svc.inner.freshness.roots.lock().unwrap();
        // Each case is a fresh automatic attempt with exactly these hints: drop real watcher
        // hints flushed by earlier runs' checkpoints (a parent's own timestamp change, say).
        let root = roots.get_mut(&1).unwrap();
        root.attempts = 0;
        root.paths.clear();
        root.targets.clear();
    }
    svc.inner.freshness.ready_targeted_hint(1);
    let mut changes: Vec<Change> = hints
        .iter()
        .map(|h| Change::Name {
            action: 3,
            name: h.encode_utf16().collect(),
        })
        .collect();
    if overflow {
        changes.push(Change::RootDirty);
    }
    svc.inner.freshness.record(1, changes);
    // Admission can answer Busy while the previous job releases its permits: retry, keeping
    // the recorded hints and mode.
    let deadline = Instant::now() + Duration::from_secs(30);
    let job = loop {
        {
            let mut roots = svc.inner.freshness.roots.lock().unwrap();
            let root = roots.get_mut(&1).unwrap();
            root.changed = Instant::now() - Duration::from_secs(1);
            root.attempts = 0;
        }
        refresh(&svc.inner);
        let job = svc.inner.freshness.roots.lock().unwrap()[&1].job.clone();
        if let Some(job) = job {
            break job;
        }
        assert!(
            Instant::now() < deadline,
            "no refresh: {}",
            svc.inner.freshness.debug(1)
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let result = settle(svc, job.as_str());
    assert_eq!(result["state"], "completed", "{result:?}");
    let mode: String = svc
        .inner
        .db()
        .query_row(
            "SELECT mode FROM main.scan_run ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    (result, mode)
}

/// The catalogue row of a root-relative directory (`/`-separated; empty for the root).
fn dir_id(lab: &Lab, path: &str) -> u64 {
    let parts: Vec<Vec<u16>> = path
        .split('/')
        .filter(|p| !p.is_empty())
        .map(|p| p.encode_utf16().collect())
        .collect();
    resolve(&lab.svc.inner, 1, &parts).unwrap_or_else(|| panic!("no row for {path}"))
}

/// The engine call the tracker makes for resolved hints, without the live watcher's own hints.
fn direct(lab: &Lab, dirs: Vec<u64>) -> Payload {
    let inner = &lab.svc.inner;
    let row = db::roots(&inner.db(), Some(1)).unwrap().remove(0);
    let grant = granted(inner, &row).unwrap().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let job = loop {
        match inner.engine.scan_refresh_scopes(&grant, dirs.clone()) {
            Ok(job) => break job,
            // Admission can answer Busy while the previous job releases its permits.
            Err(loomward_engine::EngineError::Busy { .. }) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(e) => panic!("{e:?}"),
        }
    };
    let job = settle(&lab.svc, job.job_id.as_str());
    assert_eq!(job["state"], "completed", "{job:?}");
    job
}

/// Evidence in `--nocapture` output that the fixture ran rather than skipped.
fn ran(test: &str) {
    eprintln!("#782 lab ran: {test}");
}

fn examined(job: &Payload) -> u64 {
    job["progress"]["examined"].as_u64().unwrap()
}

/// Visible catalogue tree: `path -> Some(bytes)` for files, `None` for directories.
fn catalogued(svc: &crate::Service) -> BTreeMap<String, Option<i64>> {
    let conn = svc.inner.db();
    let mut stmt = conn
        .prepare(
            "WITH RECURSIVE t(id,path) AS (
               SELECT id,'' FROM main.dir WHERE root_id=1 AND parent_id IS NULL
               UNION ALL SELECT d.id,t.path||'/'||d.name FROM main.dir d JOIN t ON d.parent_id=t.id
               WHERE d.listing_state!='absent_pending')
             SELECT path,NULL FROM t WHERE path!=''
             UNION ALL SELECT t.path||'/'||f.name,f.logical FROM main.file f JOIN t ON f.dir_id=t.id",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
        })
        .unwrap();
    rows.map(Result::unwrap).collect()
}

/// An independent walk of the same tree, in the same shape.
fn walked(root: &Path) -> BTreeMap<String, Option<i64>> {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Option<i64>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = format!("{prefix}/{}", entry.file_name().to_string_lossy());
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                out.insert(path.clone(), None);
                walk(&entry.path(), &path, out);
            } else {
                out.insert(path, Some(meta.len() as i64));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out);
    out
}

fn assert_matches_walk(lab: &Lab, case: &str) {
    assert_eq!(catalogued(&lab.svc), walked(&lab.path), "{case}");
}

#[test]
fn f3_issue789_one_file_settles_without_a_following_full_job() {
    let Some(lab) = ready() else { return };
    let bookmark = || -> (Option<Vec<u8>>, Option<i64>, i64) {
        lab.svc
            .inner
            .db()
            .query_row(
                "SELECT journal_id,next_usn,at_ns FROM st.root_freshness WHERE grant_id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    };
    let full_witness = bookmark();
    std::fs::write(lab.path.join("a/2.bin"), [9u8; 21]).unwrap();
    let (job, mode) = refresh_with(&lab.svc, &["a\\2.bin"], false);
    assert_eq!(mode, "targeted");
    assert_eq!(examined(&job), 4);
    assert_eq!(job["coverage"], "partial");
    assert_eq!(
        lab.svc.inner.hit_freshness(1).state,
        SearchFreshnessState::Live
    );
    assert_eq!(
        bookmark(),
        full_witness,
        "targeted run replaced the full-root witness"
    );
    let page = f3_wait_live(&lab.svc);
    assert_eq!(page["index_state"], "current");
    assert_eq!(page["partial"], true);
    assert_eq!(page["roots"][0]["coverage"], "partial");
    assert!(page["hits"]
        .as_array()
        .unwrap()
        .iter()
        .all(|h| h["freshness"]["state"] == "live"));
    let runs = || -> i64 {
        lab.svc
            .inner
            .db()
            .query_row("SELECT count(*) FROM main.scan_run", [], |r| r.get(0))
            .unwrap()
    };
    let before = runs();
    lab.svc
        .inner
        .freshness
        .roots
        .lock()
        .unwrap()
        .get_mut(&1)
        .unwrap()
        .changed = Instant::now() - Duration::from_secs(1);
    refresh(&lab.svc.inner);
    assert_eq!(
        runs(),
        before,
        "settled targeted run scheduled another scan"
    );
    let roots = lab.svc.inner.freshness.roots.lock().unwrap();
    assert!(roots[&1].job.is_none());
    assert!(!roots[&1].full, "the next hint must remain targeted");
    drop(roots);
    assert_eq!(
        lab.svc.inner.hit_freshness(1).state,
        SearchFreshnessState::Live
    );
    assert_matches_walk(&lab, "settled one-file change");
    ran("issue789_one_file_settles_without_a_following_full_job");
}

#[test]
fn f3_issue789_new_nested_subtree_targets_the_nearest_catalogued_ancestor() {
    let Some(lab) = ready() else { return };
    let b = dir_id(&lab, "b");
    std::fs::create_dir_all(lab.path.join("b/new/n1/n2")).unwrap();
    std::fs::write(lab.path.join("b/new/n1/n2/leaf.bin"), [4u8; 6]).unwrap();
    std::fs::write(lab.path.join("b/new/top.bin"), [4u8; 7]).unwrap();
    // The pump normally debounces for 500 ms; keep creation outside #717's access-only slack.
    std::thread::sleep(Duration::from_millis(400));
    // Flush the real notifications before selecting exactly the below-catalogue hint.
    lab.svc.inner.hit_freshness(1);
    let path: Vec<Vec<u16>> = ["b", "new", "n1", "n2"]
        .iter()
        .map(|part| part.encode_utf16().collect())
        .collect();
    assert_eq!(resolve(&lab.svc.inner, 1, &path), Some(b));
    assert_eq!(resolve_hint(&lab.svc.inner, 1, &path), Some((b, false)));
    let (job, mode) = refresh_with(&lab.svc, &["b\\new\\n1\\n2\\leaf.bin"], false);
    assert_eq!(mode, "targeted");
    // Five entries under b; the native watcher may also hint the root's five entries.
    assert!(
        examined(&job) <= 10,
        "only b, its new subtree and the hinted parent: {job:?}"
    );
    assert_eq!(job["coverage"], "partial");
    assert_matches_walk(&lab, "new nested subtree");
    assert_eq!(
        lab.svc.inner.hit_freshness(1).state,
        SearchFreshnessState::Stale
    );
    let (full, mode) = next_automatic(&lab.svc);
    assert_eq!(mode, "full", "{full:?}");
    assert_eq!(full["coverage"], "complete");
    assert_eq!(
        lab.svc.inner.hit_freshness(1).state,
        SearchFreshnessState::Live
    );
    assert_matches_walk(&lab, "new nested subtree after full recovery");
    ran("issue789_new_nested_subtree_targets_the_nearest_catalogued_ancestor");
}

fn renamed_directory_hint_requires_a_full_refresh(destination: &str, case: &str) {
    let Some(lab) = ready() else { return };
    let source = lab.path.join("c/sub1");
    let destination_path = lab.path.join(destination);
    std::fs::rename(source, &destination_path).unwrap();
    std::fs::write(destination_path.join("inner/new.bin"), [7u8; 23]).unwrap();
    // Drain real rename/write notifications before selecting the below-catalogue hint.
    std::thread::sleep(Duration::from_millis(400));
    lab.svc.inner.hit_freshness(1);
    let hint = format!("{}\\inner\\new.bin", destination.replace('/', "\\"));
    let (targeted, mode) = refresh_with(&lab.svc, &[hint.as_str()], false);
    assert_eq!(mode, "targeted", "{case}");
    assert_eq!(targeted["coverage"], "partial");
    assert_eq!(
        lab.svc.inner.hit_freshness(1).state,
        SearchFreshnessState::Stale,
        "an ancestor listing did not establish the nested write: {case}"
    );
    let (full, mode) = next_automatic(&lab.svc);
    assert_eq!(mode, "full", "{case}: {full:?}");
    assert_eq!(full["coverage"], "complete");
    assert_eq!(
        lab.svc.inner.hit_freshness(1).state,
        SearchFreshnessState::Live
    );
    assert_matches_walk(&lab, case);
    ran(case);
}

#[test]
fn f3_issue789_rename_then_nested_write_requires_full_recovery() {
    renamed_directory_hint_requires_a_full_refresh(
        "c/sub1-renamed",
        "issue789_rename_then_nested_write",
    );
}

#[test]
fn f3_issue789_move_then_nested_write_requires_full_recovery() {
    renamed_directory_hint_requires_a_full_refresh("b/sub1", "issue789_move_then_nested_write");
}

#[test]
fn f3_issue789_case_rename_then_nested_write_requires_full_recovery() {
    renamed_directory_hint_requires_a_full_refresh(
        "c/SUB1",
        "issue789_case_rename_then_nested_write",
    );
}

#[test]
fn f3_issue789_ancestor_resolution_preserves_depth_and_invalid_hint_bounds() {
    let Some(lab) = ready() else { return };
    for path in [
        vec![vec![b'z' as u16]; loomward_catalog::MAX_DIRECTORY_CHAIN_LENGTH],
        vec![vec![b'z' as u16], vec![0xd800]],
        vec!["z".encode_utf16().collect(), "..".encode_utf16().collect()],
    ] {
        assert_eq!(resolve(&lab.svc.inner, 1, &path), None);
    }
    ran("issue789_ancestor_resolution_preserves_depth_and_invalid_hint_bounds");
}

/// Advance the real automatic scheduler, preserving full mode, pending hints and retry state.
fn next_automatic(svc: &crate::Service) -> (Payload, String) {
    let previous = svc.inner.freshness.roots.lock().unwrap()[&1].job.clone();
    let deadline = Instant::now() + Duration::from_secs(30);
    let job = loop {
        svc.inner
            .freshness
            .roots
            .lock()
            .unwrap()
            .get_mut(&1)
            .unwrap()
            .changed = Instant::now() - Duration::from_secs(1);
        refresh(&svc.inner);
        let job = svc.inner.freshness.roots.lock().unwrap()[&1].job.clone();
        if let Some(job) = job.filter(|job| Some(job) != previous.as_ref()) {
            break job;
        }
        assert!(
            Instant::now() < deadline,
            "no fallback: {}",
            svc.inner.freshness.debug(1)
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let result = settle(svc, job.as_str());
    assert_eq!(result["state"], "completed");
    let mode = svc
        .inner
        .db()
        .query_row(
            "SELECT mode FROM main.scan_run ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    (result, mode)
}

#[test]
fn f3_issue789_overflow_during_targeted_run_requires_a_full_fallback() {
    for change in [
        Change::RootDirty,
        Change::Name {
            action: 3,
            name: "b\\later.bin".encode_utf16().collect(),
        },
    ] {
        let Some(lab) = ready() else { return };
        let weak = Arc::downgrade(&lab.svc.inner);
        *lab.svc.inner.hooks.before_freshness_finish.lock().unwrap() = Some(Box::new(move || {
            weak.upgrade().unwrap().freshness.record(1, vec![change]);
        }));
        // Own the initial scope: real watcher hints may arrive after refresh_with clears them.
        let job = direct(&lab, vec![dir_id(&lab, "a")]);
        assert_eq!(examined(&job), 4, "only the selected directory: {job:?}");
        assert_eq!(job["coverage"], "partial");
        assert!(lab.svc.inner.freshness.stale(1));
        let (fallback, mode) = next_automatic(&lab.svc);
        assert_eq!(mode, "full");
        assert_eq!(examined(&fallback), walked(&lab.path).len() as u64);
        assert!(!lab.svc.inner.freshness.stale(1));
        ran("issue789_overflow_during_targeted_run_requires_a_full_fallback");
    }
}

#[test]
fn f3_issue789_consumed_hints_and_checkpoint_loss_require_full_recovery() {
    let Some(lab) = ready() else { return };
    let svc = &lab.svc;
    svc.inner.freshness.record(
        1,
        vec![Change::Name {
            action: 3,
            name: "a\\2.bin".encode_utf16().collect(),
        }],
    );
    let row = db::roots(&svc.inner.db(), Some(1)).unwrap().remove(0);
    svc.inner.freshness.ready_targeted_hint(1);
    // The sink begins before writer admission; Busy can leave its consumed hints without a job.
    svc.inner
        .freshness_begin(row.grant, &RunScope::Targeted(vec![dir_id(&lab, "a")]));
    assert!(svc.inner.freshness.roots.lock().unwrap()[&1]
        .paths
        .is_empty());
    assert!(svc.inner.freshness.roots.lock().unwrap()[&1].full);
    assert_eq!(next_automatic(svc).1, "full");

    for intervening_search in [false, true] {
        let weak = Arc::downgrade(&svc.inner);
        *svc.inner.hooks.before_freshness_finish.lock().unwrap() = Some(Box::new(move || {
            let inner = weak.upgrade().unwrap();
            inner
                .hooks
                .fail_freshness_checkpoint
                .store(true, Ordering::SeqCst);
            if intervening_search {
                assert_eq!(inner.hit_freshness(1).state, SearchFreshnessState::Stale);
            }
        }));
        let (_, mode) = refresh_with(svc, &["a\\2.bin"], false);
        assert_eq!(mode, "targeted");
        assert!(svc.inner.freshness.stale(1));
        assert_eq!(next_automatic(svc).1, "full");
        assert!(!svc.inner.freshness.stale(1));
    }
    svc.inner
        .freshness
        .roots
        .lock()
        .unwrap()
        .get_mut(&1)
        .unwrap()
        .restart
        .recovering = true;
    let (_, mode) = refresh_with(svc, &["a\\2.bin"], false);
    assert_eq!(mode, "targeted");
    assert!(
        svc.inner.freshness.stale(1),
        "watcher replacement is lost coverage"
    );
    assert_eq!(next_automatic(svc).1, "full");
    assert!(!svc.inner.freshness.stale(1));
    ran("issue789_consumed_hints_and_checkpoint_loss_require_full_recovery");
}

#[test]
fn f3_issue789_failed_target_listing_requires_a_full_fallback() {
    let Some(lab) = ready() else { return };
    let outside = lab.path.parent().unwrap().join("outside-789");
    std::fs::create_dir(&outside).unwrap();
    std::fs::rename(lab.path.join("c/sub1"), outside.join("sub1")).unwrap();
    std::thread::sleep(Duration::from_millis(400));
    lab.svc.inner.hit_freshness(1);
    let (job, mode) = refresh_with(&lab.svc, &["c\\sub1\\0.bin"], false);
    assert_eq!(mode, "targeted");
    assert_eq!(job["progress"]["failed"], 1, "{job:?}");
    assert!(lab.svc.inner.freshness.stale(1));
    let (_, mode) = next_automatic(&lab.svc);
    assert_eq!(mode, "full");
    assert_matches_walk(&lab, "failed target recovered by full fallback");
    assert!(!lab.svc.inner.freshness.stale(1));
    ran("issue789_failed_target_listing_requires_a_full_fallback");
}

#[test]
fn f3_issue782_one_file_change_examines_only_its_directory() {
    let Some(lab) = ready() else {
        return;
    };
    std::fs::write(lab.path.join("a").join("2.bin"), [9u8; 21]).unwrap();
    let (job, mode) = refresh_with(&lab.svc, &["a\\2.bin"], false);
    assert_eq!(mode, "targeted");
    // `a` holds 1.bin, 2.bin, 3.bin and deep/: four entries, never the ~240 elsewhere.
    assert_eq!(examined(&job), 4, "{job:?}");
    assert_eq!(
        job["coverage"], "partial",
        "a targeted run never claims the root"
    );
    assert_matches_walk(&lab, "one-file change");
    ran("one_file_change_examines_only_its_directory");
}

#[test]
fn f3_issue782_targeted_refresh_matches_an_independent_walk() {
    let Some(lab) = ready() else {
        return;
    };
    let p = &lab.path;
    // The real watcher also reports the parent's own timestamp change, which the tracker flushes
    // in as a hint on the grandparent: each bound allows that listing too (the root's five
    // entries here), far below the ~240 entries a whole-root traversal examines.
    let cases: [Case; 4] = [
        (
            "file rename",
            &|| std::fs::rename(p.join("a/1.bin"), p.join("a/1-renamed.bin")).unwrap(),
            &["a\\1.bin", "a\\1-renamed.bin"],
            4 + 5,
        ),
        (
            "directory rename",
            &|| std::fs::rename(p.join("c/sub1"), p.join("c/sub1-renamed")).unwrap(),
            &["c\\sub1", "c\\sub1-renamed"],
            2 + 5,
        ),
        (
            "directory move",
            &|| std::fs::rename(p.join("c/sub2"), p.join("b/sub2")).unwrap(),
            &["c\\sub2", "b\\sub2"],
            // One entry in each parent, plus sub2's own four when `c` publishes first and the
            // move reveals it from a pending absence (state `unlisted`, so it is relisted).
            6 + 5,
        ),
        (
            "subtree delete",
            &|| std::fs::remove_dir_all(p.join("a/deep")).unwrap(),
            &["a\\deep\\x.bin", "a\\deep"],
            3 + 5,
        ),
    ];
    for (case, change, hints, bound) in cases {
        change();
        let (job, mode) = refresh_with(&lab.svc, hints, false);
        assert_eq!(mode, "targeted", "{case}");
        assert!(examined(&job) <= bound, "{case}: {job:?}");
        assert_matches_walk(&lab, case);
    }
    // Keep direct engine coverage for discovering new descendants; #789 above separately tests
    // the watcher hint -> nearest catalogued ancestor -> automatic targeted path.
    std::fs::create_dir_all(p.join("b/new/n1/n2")).unwrap();
    std::fs::write(p.join("b/new/n1/n2/leaf.bin"), [4u8; 6]).unwrap();
    std::fs::write(p.join("b/new/top.bin"), [4u8; 7]).unwrap();
    let job = direct(&lab, vec![dir_id(&lab, "b")]);
    // b: sub2, new; new: n1, top.bin; n1: n2; n2: leaf.bin (sub2 was already relisted).
    assert!(examined(&job) <= 6 + 4, "new subtree: {job:?}");
    assert_matches_walk(&lab, "new subtree");
    ran("targeted_refresh_matches_an_independent_walk");
}

#[test]
fn f3_issue782_coverage_loss_still_takes_the_full_fallback() {
    let Some(lab) = ready() else {
        return;
    };
    let total = walked(&lab.path).len() as u64;
    std::fs::write(lab.path.join("a").join("3.bin"), [9u8; 31]).unwrap();
    // Overflow: the watcher lost events.
    let (job, mode) = refresh_with(&lab.svc, &["a\\3.bin"], true);
    assert_eq!(mode, "full");
    assert_eq!(examined(&job), total, "{job:?}");
    // A new directory now resolves to its nearest known ancestor (#789); overflow still won above.
    std::fs::create_dir_all(lab.path.join("zz").join("unknown")).unwrap();
    let (job, mode) = refresh_with(&lab.svc, &["zz\\unknown\\f.bin"], false);
    assert_eq!(mode, "targeted");
    assert!(examined(&job) < total, "{job:?}");
    assert_matches_walk(&lab, "full fallback");
    ran("coverage_loss_still_takes_the_full_fallback");
}

/// Delete a hinted directory and recreate it under the same name (a new native ID). Its old ID
/// cannot be reopened; the named parent's complete listing finds the new directory instead.
#[test]
fn f3_issue782_recreated_directory_is_relisted_through_its_named_parent() {
    let Some(lab) = ready() else {
        return;
    };
    let (a, deep) = (dir_id(&lab, "a"), dir_id(&lab, "a/deep"));
    std::fs::remove_dir_all(lab.path.join("a/deep")).unwrap();
    std::fs::create_dir(lab.path.join("a/deep")).unwrap();
    std::fs::write(lab.path.join("a/deep/y.bin"), [8u8; 11]).unwrap();
    let job = direct(&lab, vec![deep, a]);
    // a: 1.bin, 2.bin, 3.bin, deep; the new deep: y.bin. Never the root walk.
    assert_eq!(examined(&job), 5, "{job:?}");
    assert_eq!(job["progress"]["failed"], 0, "{job:?}");
    assert_matches_walk(&lab, "recreated directory");
    let old: String = lab
        .svc
        .inner
        .db()
        .query_row(
            "SELECT listing_state FROM main.dir WHERE id=?1",
            [deep as i64],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        old, "absent_pending",
        "the replaced row must not stay visible"
    );
    ran("recreated_directory_is_relisted_through_its_named_parent");
}

/// The same unreachable directory with no named parent to settle it: the run cannot be bounded
/// and walks the whole root.
#[test]
fn f3_issue782_unreachable_target_without_named_parent_walks_the_whole_root() {
    let Some(lab) = ready() else {
        return;
    };
    let deep = dir_id(&lab, "a/deep");
    std::fs::remove_dir_all(lab.path.join("a/deep")).unwrap();
    std::fs::create_dir(lab.path.join("a/deep")).unwrap();
    std::fs::write(lab.path.join("a/deep/y.bin"), [8u8; 11]).unwrap();
    let job = direct(&lab, vec![deep]);
    assert_eq!(examined(&job), walked(&lab.path).len() as u64, "{job:?}");
    assert_eq!(job["coverage"], "partial", "{job:?}");
    // Listings outside the named set publish upsert-only, so every walked entry is present
    // (the replaced row may remain until a full refresh): nothing is lost or invented.
    let rows = catalogued(&lab.svc);
    for (path, size) in walked(&lab.path) {
        assert_eq!(
            rows.get(&path),
            Some(&size),
            "{path} missing after the fallback"
        );
    }
    ran("unreachable_target_without_named_parent_walks_the_whole_root");
}

/// A hinted directory moved out of the granted root (same volume) still reopens by ID, but its
/// listing cannot prove containment: nothing it holds may be published as inside the root.
#[test]
fn f3_issue782_directory_moved_out_of_the_root_publishes_nothing_inside() {
    let Some(lab) = ready() else {
        return;
    };
    let (c, sub1) = (dir_id(&lab, "c"), dir_id(&lab, "c/sub1"));
    let outside = lab.path.parent().unwrap().join("outside-782");
    std::fs::create_dir(&outside).unwrap();
    std::fs::rename(lab.path.join("c/sub1"), outside.join("sub1")).unwrap();
    std::fs::write(outside.join("sub1/escaped.bin"), [6u8; 13]).unwrap();
    let escaped = |svc: &crate::Service| -> i64 {
        svc.inner
            .db()
            .query_row(
                "SELECT count(*) FROM main.file WHERE name='escaped.bin'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    let job = direct(&lab, vec![sub1]);
    assert_eq!(job["coverage"], "partial", "{job:?}");
    assert_eq!(
        escaped(&lab.svc),
        0,
        "an outside listing was published: {job:?}"
    );
    // Bounded: the reopened handle lists nothing (containment unproven) and counts one failure.
    assert_eq!(
        (examined(&job), &job["progress"]["failed"]),
        (0, &serde_json::json!(1)),
        "{job:?}"
    );
    // The parent's complete listing then settles the move.
    direct(&lab, vec![c]);
    assert_eq!(escaped(&lab.svc), 0);
    assert_matches_walk(&lab, "moved out of the root");
    ran("directory_moved_out_of_the_root_publishes_nothing_inside");
}

/// A named directory the sink cannot reopen by identity (no verified file ID on its row):
/// `prepare_target` answers `None` and the run walks the whole root.
#[test]
fn f3_issue782_target_without_a_reopenable_identity_walks_the_whole_root() {
    let Some(lab) = ready() else {
        return;
    };
    let a = dir_id(&lab, "a");
    lab.svc
        .inner
        .db()
        .execute(
            "UPDATE main.dir SET file_id=NULL,id_basis='none',id_quality='path_observation' WHERE id=?1",
            [a as i64],
        )
        .unwrap();
    std::fs::write(lab.path.join("a/2.bin"), [9u8; 23]).unwrap();
    let job = direct(&lab, vec![a]);
    assert_eq!(examined(&job), walked(&lab.path).len() as u64, "{job:?}");
    assert_eq!(job["coverage"], "partial", "{job:?}");
    assert_eq!(
        catalogued(&lab.svc).get("/a/2.bin"),
        Some(&Some(23)),
        "the named directory's change must still publish"
    );
    ran("target_without_a_reopenable_identity_walks_the_whole_root");
}

/// STEP 0 measurement for #782 (run with `--ignored --nocapture`): a full refresh versus a
/// targeted refresh after one file change, on a ~50k-entry synthetic tree.
#[test]
#[ignore = "measurement"]
fn f3_issue782_measure_full_versus_targeted() {
    let Some(lab) = lab() else { return };
    for d in 0..50 {
        for s in 0..10 {
            let dir = lab.path.join(format!("m{d}")).join(format!("s{s}"));
            std::fs::create_dir_all(&dir).unwrap();
            for f in 0..100 {
                std::fs::write(dir.join(format!("f{f}.bin")), [5u8; 3]).unwrap();
            }
        }
    }
    std::thread::sleep(Duration::from_millis(400));
    let svc = &lab.svc;
    let started = Instant::now();
    let full = settle(svc, &start(svc, &lab.root, "full"));
    let full_wall = started.elapsed();
    f3_wait_live(svc);
    svc.inner.freshness.pause_pump();
    let started = Instant::now();
    let (refreshed, _) = refresh_with(svc, &["m7\\s3\\f1.bin"], true);
    let refresh_wall = started.elapsed();
    std::fs::write(lab.path.join("m7/s3/f1.bin"), [6u8; 9]).unwrap();
    let started = Instant::now();
    let (targeted, mode) = refresh_with(svc, &["m7\\s3\\f1.bin"], false);
    let targeted_wall = started.elapsed();
    eprintln!(
        "#782 measure: full examined={} wall={:?}; full refresh examined={} wall={:?}; {mode} examined={} wall={:?}",
        examined(&full),
        full_wall,
        examined(&refreshed),
        refresh_wall,
        examined(&targeted),
        targeted_wall
    );
}
