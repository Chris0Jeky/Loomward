//! Does `born_run` survive later scans of the same granted root? (Feasibility of a "new since the
//! last scan" view built on `file.born_run` / `dir.born_run` with no schema change.)
//!
//! Real native engine -> production catalogue sink on a synthetic NTFS tree, three completed scans
//! with real filesystem mutations between them. Nothing is hand-inserted. The assertions pin what
//! the writer does today; where that would make "born_run > last scan" misleading the case says so.
#![cfg(windows)]

use loomward_protocol::{CallContext, DatasetClass, ResponseEnvelope, ViewService};
use loomward_service::{Config, Service};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

fn call(service: &Service, command: &str, payload: Value) -> Value {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let request = serde_json::from_value(json!({
        "protocol": "loomward/3", "request_id": format!("born-run-{id}"), "command": command,
        "payload": payload
    }))
    .unwrap();
    match service.call(request, &CallContext::http()) {
        ResponseEnvelope::Ok(reply) => reply.result.into(),
        ResponseEnvelope::Err(reply) => panic!("{command}: {:?}", reply.error),
    }
}

/// One catalogue row as `born_run` sees it.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    id: i64,
    born: i64,
    seen: i64,
    /// Logical bytes for a file (0 for a directory row, which carries aggregates instead).
    logical: i64,
    /// Hex of the native file identity; the continuity key the writer matches on.
    native: String,
}

type Snapshot = BTreeMap<String, Row>;

struct Lab {
    service: Arc<Service>,
    root_id: String,
    tree: PathBuf,
    state: PathBuf,
    _temp: tempfile::TempDir,
}

impl Lab {
    /// `None` when the host cannot run a native case (elevated token), as the pathological suite does.
    fn new() -> Option<Lab> {
        if loomward_windows::enumerate::skip_elevated_test() {
            if std::env::var("LOOMWARD_REQUIRE_NATIVE").as_deref() == Ok("1") {
                panic!("LOOMWARD_REQUIRE_NATIVE=1 but the native case cannot run: elevated token");
            }
            eprintln!("UNVERIFIED: native scan requires a non-elevated token");
            return None;
        }
        let temp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
        let state = temp.path().join("state");
        let tree = temp.path().join("tree");
        loomward_service::lab::register(
            &state,
            &tree,
            "born-run",
            "rescans",
            &format!("sha256:{}", "00".repeat(32)),
        )
        .unwrap();
        // The grant pins the canonical root; write the fixture through that spelling.
        let tree = tree.canonicalize().unwrap();
        write(&tree, "keep.txt", "unchanged");
        write(&tree, "grow.txt", "before");
        write(&tree, "old-name.txt", "renamed later");
        write(&tree, "recreate.txt", "first incarnation");
        write(&tree, "src/moved.txt", "moved later");
        write(&tree, "stable/inner.txt", "inside an unchanged dir");
        write(&tree, "olddir/inside.txt", "inside a renamed dir");
        write(&tree, "mv/sub/deep.txt", "inside a moved dir");
        fs::create_dir_all(tree.join("dst")).unwrap();
        let service = Arc::new(
            Service::open(Config {
                state_dir: state.clone(),
                dataset: DatasetClass::Synthetic,
                allow_personal: false,
                allow_identity_unverified: false,
                grant_roots: vec![tree.clone()],
            })
            .unwrap(),
        );
        let root_id = call(&service, "roots.list", json!({}))["roots"][0]["root_id"]
            .as_str()
            .unwrap()
            .to_owned();
        Some(Lab {
            service,
            root_id,
            tree,
            state,
            _temp: temp,
        })
    }

    fn db(&self) -> Connection {
        let db =
            Connection::open(loomward_catalog::sqlite_dir(&self.state).join("catalog.db")).unwrap();
        db.busy_timeout(Duration::from_secs(30)).unwrap();
        db
    }

    /// Start a scan and wait for it; then wait until no job (a watcher's targeted pass included)
    /// is queued or running and the run list has stopped growing.
    fn scan(&self, mode: &str) {
        let job = call(
            &self.service,
            "scan.start",
            json!({"root_id": self.root_id, "mode": mode}),
        )["job"]["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let done = call(&self.service, "jobs.get", json!({"job_id": job}))["job"].clone();
            match done["state"].as_str().unwrap() {
                "completed" => {
                    assert_eq!(done["coverage"], "complete", "{done}");
                    break;
                }
                "failed" | "cancelled" => panic!("scan ended {done}"),
                _ => {}
            }
            assert!(Instant::now() < deadline, "scan timed out: {done}");
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut last = (self.runs().len(), Instant::now());
        loop {
            let jobs = call(&self.service, "jobs.list", json!({"limit": 50}));
            let idle = jobs["jobs"].as_array().unwrap().iter().all(|j| {
                !matches!(
                    j["state"].as_str(),
                    Some("queued" | "running" | "cancel_requested")
                )
            });
            let runs = self.runs().len();
            if runs != last.0 {
                last = (runs, Instant::now());
            }
            if idle && last.1.elapsed() > Duration::from_millis(1500) {
                return;
            }
            assert!(Instant::now() < deadline, "catalogue never went idle");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// (run id, mode, state) of every scan run, oldest first.
    fn runs(&self) -> Vec<(i64, String, String)> {
        let db = self.db();
        let mut s = db
            .prepare("SELECT id,mode,state FROM scan_run ORDER BY id")
            .unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// Every live file and directory row keyed by `file:/path` or `dir:/path`.
    fn snapshot(&self) -> Snapshot {
        let db = self.db();
        let mut s = db
            .prepare(
                "WITH RECURSIVE paths(id,path) AS (
                   SELECT id,'' FROM dir WHERE parent_id IS NULL
                   UNION ALL SELECT d.id,p.path||'/'||d.name FROM dir d JOIN paths p ON d.parent_id=p.id
                   WHERE d.listing_state!='absent_pending')
                 SELECT 'dir:'||p.path,d.id,d.born_run,d.seen_run,hex(d.file_id),0 FROM dir d JOIN paths p ON p.id=d.id
                 UNION ALL
                 SELECT 'file:'||p.path||'/'||f.name,f.id,f.born_run,f.seen_run,hex(f.file_id),f.logical
                 FROM file f JOIN paths p ON p.id=f.dir_id",
            )
            .unwrap();
        s.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Row {
                    id: r.get(1)?,
                    born: r.get(2)?,
                    seen: r.get(3)?,
                    native: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    logical: r.get(5)?,
                },
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    }
}

fn write(tree: &Path, relative: &str, text: &str) {
    let path = tree.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn row<'a>(s: &'a Snapshot, key: &str) -> &'a Row {
    s.get(key)
        .unwrap_or_else(|| panic!("{key} missing; have {:?}", s.keys().collect::<Vec<_>>()))
}

fn table(label: &str, s: &Snapshot) {
    eprintln!("--- {label}");
    for (key, r) in s {
        eprintln!(
            "{key:<34} id={:<3} born={:<3} seen={:<3} native={}",
            r.id, r.born, r.seen, r.native
        );
    }
}

#[test]
fn born_run_across_rescans() {
    let Some(lab) = Lab::new() else {
        return;
    };

    // Scan 1: the first scan of a fresh root (the bulk-build path).
    lab.scan("full");
    let one = lab.snapshot();
    table("after scan 1", &one);
    let r1 = lab.runs().first().unwrap().0;
    // The root directory row is created with the grant, before any run: born_run 0 for good.
    assert_eq!(row(&one, "dir:").born, 0);
    for (key, r) in one.iter().filter(|(key, _)| *key != "dir:") {
        assert_eq!(
            r.born, r1,
            "{key}: every row of the first scan is born in it"
        );
    }

    // Mutations between scans 1 and 2.
    let t = &lab.tree;
    write(t, "added.txt", "new file");
    write(t, "newdir/child.txt", "inside a new dir");
    fs::OpenOptions::new()
        .write(true)
        .open(t.join("grow.txt"))
        .unwrap()
        .set_len(4096)
        .unwrap();
    fs::rename(t.join("old-name.txt"), t.join("renamed.txt")).unwrap();
    fs::rename(t.join("src/moved.txt"), t.join("dst/moved.txt")).unwrap();
    fs::remove_file(t.join("recreate.txt")).unwrap();
    write(t, "recreate.txt", "second incarnation, different size");
    fs::rename(t.join("olddir"), t.join("renameddir")).unwrap();
    fs::rename(t.join("mv/sub"), t.join("dst/sub")).unwrap();

    // Scan 2: a refresh of the same root.
    lab.scan("refresh");
    let two = lab.snapshot();
    table("after scan 2", &two);
    let runs2 = lab.runs();
    eprintln!("runs after scan 2: {runs2:?}");
    // Runs after scan 1 (the refresh plus any watcher pass that raced it): rows a rescan
    // creates are born in one of these, never in run 1.
    let later: Vec<i64> = runs2.iter().map(|r| r.0).filter(|id| *id > r1).collect();
    assert!(!later.is_empty());
    let born_later = |key: &str| {
        let r = row(&two, key);
        assert!(
            later.contains(&r.born),
            "{key}: born {} not in {later:?}",
            r.born
        );
        r.clone()
    };
    let kept = |key: &str| {
        let (before, after) = (row(&one, key), row(&two, key));
        assert_eq!(after.id, before.id, "{key}: same row id");
        assert_eq!(
            after.born, before.born,
            "{key}: born_run survives the rescan"
        );
        assert_eq!(after.native, before.native, "{key}: same native identity");
        after.clone()
    };

    // (a) unchanged file: first born_run kept, seen_run advances.
    let keep = kept("file:/keep.txt");
    assert_eq!(keep.born, r1);
    assert!(later.contains(&keep.seen), "seen_run moved to a later run");

    // (b) file added before scan 2: born in a later run, not run 1. WHICH later run is a race with
    // the root's watcher: it answers the mutations with its own full-root run, and in every local
    // execution that run (id 2) listed the new rows before the explicit refresh (id 3) did. So
    // born_run names the first run that listed a row, explicit or watcher-driven; the explicit
    // scan only advances seen_run. MISLEADING if "since the last scan" means `born_run = <last
    // explicit run>`: a cut must compare `born_run > <run id at the user's baseline>`, which
    // also counts rows a watcher run found in between.
    born_later("file:/added.txt");

    // (c) file changed in place (same identity, size and mtime change): keeps its born_run.
    let grow = kept("file:/grow.txt");
    assert_eq!(grow.born, r1);
    assert_ne!(row(&one, "file:/grow.txt").seen, grow.seen);
    assert_eq!(
        (row(&one, "file:/grow.txt").logical, grow.logical),
        (6, 4096)
    );

    // (d) renamed in the same folder: the writer matches by native identity, so it is the same
    // row with its first born_run. "New since last scan" does NOT list a plain rename.
    let renamed = row(&two, "file:/renamed.txt");
    let old = row(&one, "file:/old-name.txt");
    assert!(!two.contains_key("file:/old-name.txt"));
    assert_eq!((renamed.id, renamed.born), (old.id, r1));

    // (e) moved between folders: matching is per directory (`old_files` reads one dir_id), so the
    // destination listing finds nothing to match, inserts a new row and the source listing deletes
    // the old one. MISLEADING: a moved file looks new (born in a later run, new row id), although
    // its native identity is unchanged.
    let moved = born_later("file:/dst/moved.txt");
    let before_move = row(&one, "file:/src/moved.txt");
    assert!(!two.contains_key("file:/src/moved.txt"));
    assert_ne!(moved.id, before_move.id, "a moved file is a new row");
    assert_eq!(moved.native, before_move.native, "yet the same native file");

    // (f) deleted and recreated under the same name: a new file, born in a later run.
    let again = born_later("file:/recreate.txt");
    let first = row(&one, "file:/recreate.txt");
    assert_ne!(again.id, first.id, "the recreated file is a new row");
    // NTFS reuses the MFT record but bumps its sequence number, so the 128-bit identity differs
    // and the writer cannot pair the two incarnations.
    assert_ne!(again.native, first.native);

    // (g) directories: unchanged keeps its first born_run, a new one is born later; a renamed or
    // moved directory is matched by identity and keeps its born_run, and so do the files in it.
    assert_eq!(kept("dir:/stable").born, r1);
    assert_eq!(kept("file:/stable/inner.txt").born, r1);
    born_later("dir:/newdir");
    born_later("file:/newdir/child.txt");
    let renamed_dir = row(&two, "dir:/renameddir");
    assert_eq!(
        (renamed_dir.id, renamed_dir.born),
        (row(&one, "dir:/olddir").id, r1)
    );
    assert_eq!(row(&two, "file:/renameddir/inside.txt").born, r1);
    let moved_dir = row(&two, "dir:/dst/sub");
    assert_eq!(
        (moved_dir.id, moved_dir.born),
        (row(&one, "dir:/mv/sub").id, r1)
    );
    assert_eq!(row(&two, "file:/dst/sub/deep.txt").born, r1);

    // Scan 3: nothing changed; a full scan of an already-scanned root must not restamp anything.
    lab.scan("full");
    let three = lab.snapshot();
    table("after scan 3", &three);
    assert_eq!(three.len(), two.len());
    for (key, r) in &two {
        let after = row(&three, key);
        assert_eq!(
            after.id, r.id,
            "{key}: same row id after an unchanged full scan"
        );
        assert_eq!(
            after.born, r.born,
            "{key}: born_run unchanged by a third scan"
        );
    }
}
