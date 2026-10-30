//! #414: native engine -> production catalogue sink -> query -> forget, synthetic trees only.
#![cfg(windows)]

use loomward_lab::pathological::{generate, GeneratedTree, Profile};
use loomward_protocol::{CallContext, DatasetClass, ResponseEnvelope, ViewService};
use loomward_service::{Config, Service};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

struct Lab {
    service: Arc<Service>,
    tree: GeneratedTree,
    state: PathBuf,
    root: String,
    _temp: tempfile::TempDir,
}

fn call(service: &Service, command: &str, payload: Value) -> Value {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut retries = 0;
    loop {
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let request = serde_json::from_value(json!({
            "protocol": "loomward/3", "request_id": format!("pathological-{id}"), "command": command,
            "payload": payload
        }))
        .unwrap();
        match service.call(request, &CallContext::http()) {
            ResponseEnvelope::Ok(reply) => return reply.result.into(),
            // Aggregate repair can overlap a completed scan (as in the lab soak, #605); only this
            // read waits for it, and only while the service says it is retryable.
            ResponseEnvelope::Err(reply)
                if command == "tree.slice"
                    && reply.error.code == loomward_protocol::ErrorCode::PartialCoverage
                    && reply.error.retryable
                    && Instant::now() < deadline =>
            {
                retries += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            ResponseEnvelope::Err(reply) => {
                panic!(
                    "{command} after {retries} repair retries: {:?}",
                    reply.error
                )
            }
        }
    }
}

fn service(state: PathBuf, path: PathBuf) -> Arc<Service> {
    Arc::new(
        Service::open(Config {
            state_dir: state,
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![path],
        })
        .unwrap(),
    )
}

// A host that cannot run a native case reports it UNVERIFIED, unless LOOMWARD_REQUIRE_NATIVE=1 turns
// that skip into a failure (the native gate must not pass vacuously on a host that cannot run it).
fn unverified(reason: &str) {
    if std::env::var("LOOMWARD_REQUIRE_NATIVE").as_deref() == Ok("1") {
        panic!("LOOMWARD_REQUIRE_NATIVE=1 but the native case cannot run: {reason}");
    }
    eprintln!("UNVERIFIED: {reason}");
}

fn lab(profile: Profile) -> Option<Lab> {
    if loomward_windows::enumerate::skip_elevated_test() {
        unverified(
            "native pathological scan requires a non-elevated token; no elevation attempted",
        );
        return None;
    }
    let temp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
    let state = temp.path().join("state");
    let path = temp.path().join("tree");
    loomward_service::lab::register(
        &state,
        &path,
        "414",
        "pathological",
        &format!("sha256:{}", "00".repeat(32)),
    )
    .unwrap();
    if matches!(profile, Profile::Churn { .. }) {
        loomward_service::lab::register(
            &temp.path().join("fresh-state"),
            &path,
            "414",
            "pathological",
            &format!("sha256:{}", "00".repeat(32)),
        )
        .unwrap();
    }
    let tree = match generate(&path, profile) {
        Ok(tree) => tree,
        Err(error) if profile == Profile::SymlinkCycle && error.raw_os_error() == Some(1314) => {
            unverified("symlink cycle requires Developer Mode or symlink privilege (1314); no elevation attempted");
            return None;
        }
        Err(error) => panic!("{profile:?} generation: {error}"),
    };
    let service = service(state.clone(), path);
    let root = call(&service, "roots.list", json!({}))["roots"][0]["root_id"]
        .as_str()
        .unwrap()
        .to_owned();
    Some(Lab {
        service,
        tree,
        state,
        root,
        _temp: temp,
    })
}

impl Lab {
    fn db(&self) -> Connection {
        let db =
            Connection::open(loomward_catalog::sqlite_dir(&self.state).join("catalog.db")).unwrap();
        db.busy_timeout(Duration::from_secs(30)).unwrap();
        db
    }
    fn start(&self, mode: &str) -> String {
        call(
            &self.service,
            "scan.start",
            json!({"root_id": self.root, "mode": mode}),
        )["job"]["job_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn settle(&self, job: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            let result = call(&self.service, "jobs.get", json!({"job_id": job}));
            if matches!(
                result["job"]["state"].as_str().unwrap(),
                "completed" | "failed" | "cancelled"
            ) {
                return result["job"].clone();
            }
            assert!(Instant::now() < deadline, "scan timed out: {result}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn scan(&self, mode: &str) -> Value {
        let done = self.settle(&self.start(mode));
        assert_eq!(done["state"], "completed", "{done}");
        done
    }
    fn slice(&self) -> Value {
        call(
            &self.service,
            "tree.slice",
            json!({"anchor": {"kind":"root", "root_id":self.root},
            "depth":8, "max_nodes":256, "min_share":0.0, "basis":"logical", "include_files":true}),
        )
    }
    fn forget(&self) {
        call(
            &self.service,
            "roots.revoke",
            json!({"root_id":self.root, "purge_catalog":false}),
        );
        call(&self.service, "roots.forget", json!({"root_id":self.root}));
        let db = self.db();
        for table in ["root", "dir", "file", "stage_entry", "scan_run"] {
            let count: i64 = db
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table} survived forget");
        }
        assert!(
            self.tree.leaf.exists(),
            "forget changed the fixture on disk"
        );
    }
}

#[test]
fn writer_depth_limit_512_scanned_end_to_end() {
    let Some(lab) = lab(Profile::DepthLimit) else {
        return;
    };
    let done = lab.scan("full");
    assert_eq!(done["coverage"], "complete", "{done}");
    assert_eq!(done["progress"]["limit_hit"], false, "{done}");
    eprintln!(
        "DEPTH 512: scan elapsed_ms={}",
        done["progress"]["elapsed_ms"]
    );
    let depth: i64 = lab
        .db()
        .query_row("SELECT max(depth) FROM dir", [], |r| r.get(0))
        .unwrap();
    assert_eq!(depth, 511); // The writer's 512-directory chain includes the root.
    assert_eq!(lab.slice()["nodes"][0]["files"], 1);
    lab.forget();
}

#[test]
fn past_writer_depth_limit_513_is_partial_with_depth_limit_reason() {
    let Some(lab) = lab(Profile::DepthLimit) else {
        return;
    };
    let extra = lab.tree.leaf.parent().unwrap().join("beyond-limit");
    fs::create_dir(&extra).unwrap();
    fs::write(extra.join("unscanned.bin"), b"synthetic").unwrap();
    let done = lab.scan("full");
    assert_eq!(done["coverage"], "partial", "{done}");
    assert_eq!(done["progress"]["limit_hit"], true, "{done}");
    assert_eq!(done["progress"]["limit_reason"], "depth_limit", "{done}");
    let db = lab.db();
    let (count, depth): (i64, i64) = db
        .query_row("SELECT count(*),max(depth) FROM dir", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!((count, depth), (512, 511));
    assert_eq!(lab.slice()["nodes"][0]["files"], 1);
    assert!(extra.join("unscanned.bin").exists());
    drop(db);
    lab.forget();
}

#[test]
fn past_max_path_scanned_queried_and_forgotten() {
    use std::os::windows::ffi::OsStrExt;
    let Some(lab) = lab(Profile::LongPath) else {
        return;
    };
    assert!(lab.tree.leaf.as_os_str().encode_wide().count() > 260);
    assert_eq!(lab.scan("full")["coverage"], "complete");
    let slice = lab.slice();
    assert_eq!(slice["nodes"][0]["files"], 1);
    let file = slice["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "file")
        .unwrap();
    let detail = call(
        &lab.service,
        "node.inspect",
        json!({"node_id":file["node_id"]}),
    );
    assert_eq!(detail["row"]["name"], "synthetic.bin");
    let name: String = lab
        .db()
        .query_row("SELECT name FROM file", [], |r| r.get(0))
        .unwrap();
    assert_eq!(name, "synthetic.bin");
    lab.forget();
}

#[test]
fn unpaired_surrogate_scanned_queried_and_forgotten() {
    use std::os::windows::ffi::OsStrExt;
    let Some(lab) = lab(Profile::UnpairedSurrogate) else {
        return;
    };
    assert_eq!(lab.scan("full")["coverage"], "complete");
    let raw: Vec<u8> = lab
        .db()
        .query_row("SELECT name_utf16 FROM file", [], |r| r.get(0))
        .unwrap();
    let expected: Vec<u8> = lab
        .tree
        .leaf
        .file_name()
        .unwrap()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect();
    assert_eq!(raw, expected);
    let slice = lab.slice();
    let file = slice["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "file")
        .unwrap_or_else(|| panic!("file absent from slice: {slice}"));
    assert_eq!(file["name"], "synthetic-\u{fffd}.bin");
    let detail = call(
        &lab.service,
        "node.inspect",
        json!({"node_id":file["node_id"]}),
    );
    assert!(detail["row"]["flags"]
        .as_array()
        .unwrap()
        .contains(&json!("name_lossy")));
    lab.forget();
}

fn cycle(profile: Profile) {
    let Some(lab) = lab(profile) else { return };
    assert_eq!(lab.scan("full")["coverage"], "partial");
    let db = lab.db();
    let excluded: i64 = db
        .query_row(
            "SELECT count(*) FROM dir WHERE listing_state='excluded' AND flags & 16 != 0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(excluded, 1);
    assert_eq!(lab.slice()["nodes"][0]["files"], 1);
    drop(db);
    lab.forget();
}

#[test]
fn junction_cycle_is_visible_but_never_followed() {
    cycle(Profile::JunctionCycle);
}

#[test]
fn symlink_cycle_is_visible_but_never_followed() {
    cycle(Profile::SymlinkCycle);
}

#[test]
fn multi_thousand_hardlinks_keep_every_name_and_shared_identity() {
    let Some(lab) = lab(Profile::HardLinks) else {
        return;
    };
    assert_eq!(lab.scan("full")["coverage"], "complete");
    let db = lab.db();
    let counts: (i64, i64) = db
        .query_row(
            "SELECT count(*),count(DISTINCT hex(file_id)) FROM file",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(counts, (4096, 8));
    let groups: (i64, i64) = db
        .query_row("SELECT count(*),sum(names) FROM multilink", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(groups, (8, 4096));
    assert_eq!(lab.slice()["nodes"][0]["files"], 4096);
    drop(db);
    lab.forget();
}

#[test]
fn real_acl_denied_directory_stays_visible_and_partial() {
    let Some(lab) = lab(Profile::DeniedDirectory) else {
        return;
    };
    assert_eq!(
        fs::read_dir(lab.tree.leaf.parent().unwrap())
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let done = lab.scan("full");
    assert_ne!(done["coverage"], "complete", "{done}");
    let flags: i64 = lab
        .db()
        .query_row("SELECT flags FROM dir WHERE name='denied'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_ne!(flags & 64, 0);
    let slice = lab.slice();
    let denied = slice["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "denied")
        .unwrap();
    assert_eq!(denied["coverage"], "denied");
    assert_eq!(slice["nodes"][0]["files"], 1);
    lab.forget();
}

// Compare every stable observation and rollup, excluding catalogue IDs, run/revision counters and
// access times (enumeration itself may update them). Relative names retain topology, not just totals.
fn observations(db: &Connection) -> Vec<Vec<rusqlite::types::Value>> {
    let mut statement = db
        .prepare(
            "WITH RECURSIVE paths(id,path) AS (
        SELECT id,'' FROM dir WHERE parent_id IS NULL UNION ALL
        SELECT d.id,p.path||'/'||d.name FROM dir d JOIN paths p ON d.parent_id=p.id)
        SELECT 'dir',p.path,d.file_id,d.attrs,d.reparse_tag,d.flags,d.listing_state,
            d.own_files,d.own_logical,d.own_allocated,d.own_alloc_unknown,
            d.sub_files,d.sub_dirs,d.sub_logical,d.sub_allocated,d.sub_alloc_unknown,d.sub_complete
        FROM dir d JOIN paths p ON d.id=p.id
        UNION ALL SELECT 'file',p.path||'/'||f.name,f.file_id,f.attrs,f.reparse_tag,f.flags,
            hex(f.name_utf16),f.logical,f.allocated,f.created_ft,f.modified_ft,
            f.changed_ft,f.link_count,NULL,NULL,NULL,NULL
        FROM file f JOIN paths p ON f.dir_id=p.id ORDER BY 1,2",
        )
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| (0..columns).map(|i| row.get(i)).collect())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

// A batch overlaps enumeration only while the job is running and the catalogue has published no file
// rows. No live entry counter is exposed (progress is set only at completion), and staged rows only
// cover large listings, so they cannot serve as the signal. A fresh full scan publishes its files in
// one commit, so an empty file table marks enumeration. Pre-commit publish work shares the running
// state and cannot be separated from enumeration here; the overlap floor in the test bounds that.
fn enumerating(lab: &Lab, db: &Connection, job: &str) -> bool {
    let running =
        call(&lab.service, "jobs.get", json!({"job_id":job}))["job"]["state"] == "running";
    let published: i64 = db
        .query_row("SELECT count(*) FROM file", [], |r| r.get(0))
        .unwrap();
    running && published == 0
}

// The filesystem's own truth: every regular file under `root` with its logical length, reached with
// std::fs and never following a reparse point (the churn fixture has none, asserted below).
fn walk_files(root: &std::path::Path) -> Vec<(String, i64)> {
    use std::os::windows::fs::MetadataExt;
    const REPARSE_POINT: u32 = 0x400;
    let mut files = Vec::new();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let name = entry
                .file_name()
                .into_string()
                .expect("synthetic names are UTF-8");
            let relative = format!("{prefix}/{name}");
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert_eq!(
                metadata.file_attributes() & REPARSE_POINT,
                0,
                "unexpected reparse point in the churn fixture: {relative}"
            );
            if metadata.is_dir() {
                pending.push((path, relative));
            } else if metadata.is_file() {
                files.push((relative, metadata.len() as i64));
            }
        }
    }
    files.sort();
    files
}

// The catalogue's view of the same files: (relative path, logical bytes), sorted.
fn catalogue_files(db: &Connection) -> Vec<(String, i64)> {
    let mut statement = db
        .prepare(
            "WITH RECURSIVE paths(id,path) AS (
        SELECT id,'' FROM dir WHERE parent_id IS NULL UNION ALL
        SELECT d.id,p.path||'/'||d.name FROM dir d JOIN paths p ON d.parent_id=p.id)
        SELECT p.path||'/'||f.name,f.logical FROM file f JOIN paths p ON f.dir_id=p.id",
        )
        .unwrap();
    let mut files: Vec<(String, i64)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    files.sort();
    files
}

fn wait_quiescent(service: &Service, root: &str) {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let page = call(
            service,
            "search.query",
            json!({
                "root_id":root,"text":"bin","extension":null,"min_bytes":null,"max_bytes":null,
                "modified_after":null,"modified_before":null,"kind":"file","limit":1,"cursor":null
            }),
        );
        let jobs = call(service, "jobs.list", json!({"limit":50}));
        let idle = jobs["jobs"].as_array().unwrap().iter().all(|job| {
            !matches!(
                job["state"].as_str(),
                Some("queued" | "running" | "cancel_requested")
            )
        });
        // Foreground completion can precede a watcher's targeted pass and full-root fallback.
        if idle
            && !page["partial"].as_bool().unwrap()
            && page["hits"].as_array().unwrap().iter().any(|hit| {
                matches!(
                    hit["freshness"]["state"].as_str(),
                    Some("live" | "caught_up")
                )
            })
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "catalogue never became fresh and idle: search={page}; jobs={jobs}"
        );
        std::thread::yield_now();
    }
}

#[test]
fn concurrent_create_delete_rename_grow_reconciles_exactly_to_fresh_scan() {
    let files = std::env::var("LOOMWARD_CHURN_FILES")
        .map(|v| v.parse::<usize>().unwrap())
        .unwrap_or(4000);
    assert!((1000..=100_000).contains(&files));
    let Some(lab) = lab(Profile::Churn { files }) else {
        return;
    };
    let job = lab.start("full");
    // The writer starts only after the job is observed running, so it cannot race a scan that has not
    // begun. A scan that reaches a terminal state before that observation fails here, explicitly.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let state = call(&lab.service, "jobs.get", json!({"job_id":job}))["job"]["state"].clone();
        if state == "running" {
            break;
        }
        assert_eq!(
            state, "queued",
            "scan reached a terminal state before it was observed running"
        );
        assert!(
            Instant::now() < deadline,
            "scan never ran while the writer was ready"
        );
        std::thread::yield_now();
    }
    // The default size runs in CI on hosts of any speed, so it only requires real overlap; the
    // recorded evidence run (100k files, docs/45 row 4) requires at least 8 overlapping batches.
    let floor = if files >= 100_000 { 8 } else { 1 };
    let (overlap, batches, done) = std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            let db = lab.db();
            let mut overlap = 0;
            for batch in 0..128 {
                let before = enumerating(&lab, &db, &job);
                let directory = lab
                    .tree
                    .root
                    .join(format!("bucket-{:04}", batch % files.div_ceil(100)));
                let index = (batch * 7) % 100;
                let original = directory.join(format!("file-{index:04}.bin"));
                let renamed = directory.join(format!("renamed-{index:04}.bin"));
                fs::write(
                    directory.join(format!("created-{batch:04}.bin")),
                    b"created",
                )
                .unwrap();
                if original.exists() {
                    fs::rename(&original, &renamed).unwrap();
                    fs::OpenOptions::new()
                        .write(true)
                        .open(&renamed)
                        .unwrap()
                        .set_len(8193 + batch as u64)
                        .unwrap();
                }
                let deleted = directory.join(format!("file-{:04}.bin", (index + 1) % 100));
                if deleted.exists() {
                    fs::remove_file(deleted).unwrap();
                }
                let after = enumerating(&lab, &db, &job);
                if before && after {
                    overlap += 1;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            (overlap, 128)
        });
        let done = lab.settle(&job);
        let (overlap, batches) = writer.join().unwrap();
        (overlap, batches, done)
    });
    assert!(
        overlap >= floor,
        "only {overlap} of {batches} mutation batches overlapped enumeration (floor {floor})"
    );
    assert_eq!(done["state"], "completed", "{done}");
    assert_eq!(lab.scan("refresh")["coverage"], "complete");
    wait_quiescent(&lab.service, &lab.root);
    // Filesystem truth first: the catalogue must hold exactly the files on disk, with their lengths.
    let walked = walk_files(&lab.tree.root);
    let catalogued = catalogue_files(&lab.db());
    assert_eq!(
        catalogued.len(),
        walked.len(),
        "catalogue file count differs from the filesystem walk"
    );
    assert_eq!(
        catalogued, walked,
        "catalogue (path, logical bytes) differs from the filesystem walk"
    );
    let actual = observations(&lab.db());
    let fresh_state = lab._temp.path().join("fresh-state");
    let fresh = service(fresh_state.clone(), lab.tree.root.clone());
    let fresh_root = call(&fresh, "roots.list", json!({}))["roots"][0]["root_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let fresh_job = call(
        &fresh,
        "scan.start",
        json!({"root_id":fresh_root,"mode":"full"}),
    )["job"]["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let result = call(&fresh, "jobs.get", json!({"job_id":fresh_job}));
        if result["job"]["state"] == "completed" {
            assert_eq!(result["job"]["coverage"], "complete");
            break;
        }
        assert_ne!(result["job"]["state"], "failed", "{result}");
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let fresh_db =
        Connection::open(loomward_catalog::sqlite_dir(&fresh_state).join("catalog.db")).unwrap();
    wait_quiescent(&fresh, &fresh_root);
    let expected = observations(&fresh_db);
    assert_eq!(actual.len(), expected.len(), "catalogue row count differs");
    for (actual, expected) in actual.iter().zip(&expected) {
        assert_eq!(actual, expected, "catalogue observation differs");
    }
    eprintln!("CHURN VERIFIED: {files} initial files, {batches} mutation batches, {overlap} batches during enumeration, {} exact catalogue observations, {} files equal to the filesystem walk", actual.len(), walked.len());
}
