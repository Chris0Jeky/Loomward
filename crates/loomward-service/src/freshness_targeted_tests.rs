//! #782: a targeted refresh lists only its hinted directories (and the new subtrees under them),
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
    // New subtree. The real watcher also names directories below `b` that the catalogue has never
    // seen, and the tracker sends those unresolved hints to a full refresh; drive the targeted
    // engine call the tracker makes for a resolved hint on `b` directly.
    std::fs::create_dir_all(p.join("b/new/n1/n2")).unwrap();
    std::fs::write(p.join("b/new/n1/n2/leaf.bin"), [4u8; 6]).unwrap();
    std::fs::write(p.join("b/new/top.bin"), [4u8; 7]).unwrap();
    let inner = &lab.svc.inner;
    let row = db::roots(&inner.db(), Some(1)).unwrap().remove(0);
    let b = resolve(inner, 1, &["b".encode_utf16().collect()]).unwrap();
    let grant = granted(inner, &row).unwrap().unwrap();
    let job = inner.engine.scan_refresh_scopes(&grant, vec![b]).unwrap();
    let job = settle(&lab.svc, job.job_id.as_str());
    assert_eq!(job["state"], "completed", "{job:?}");
    // b: sub2, new; new: n1, top.bin; n1: n2; n2: leaf.bin (sub2 was already relisted).
    assert!(examined(&job) <= 6 + 4, "new subtree: {job:?}");
    assert_matches_walk(&lab, "new subtree");
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
    // An unresolved hint (no catalogue record for its directory) cannot be bounded either.
    std::fs::create_dir_all(lab.path.join("zz").join("unknown")).unwrap();
    let (job, mode) = refresh_with(&lab.svc, &["zz\\unknown\\f.bin"], false);
    assert_eq!(mode, "full");
    assert_eq!(examined(&job), total + 2, "{job:?}");
    assert_matches_walk(&lab, "full fallback");
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
