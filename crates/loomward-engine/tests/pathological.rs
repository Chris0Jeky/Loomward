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
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let request = serde_json::from_value(json!({
        "protocol": "loomward/3", "request_id": format!("pathological-{id}"), "command": command,
        "payload": payload
    }))
    .unwrap();
    match service.call(request, &CallContext::http()) {
        ResponseEnvelope::Ok(reply) => reply.result.into(),
        ResponseEnvelope::Err(reply) => panic!("{command}: {:?}", reply.error),
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

fn lab(profile: Profile) -> Option<Lab> {
    if loomward_windows::enumerate::running_elevated() {
        eprintln!("UNVERIFIED: native pathological scan requires a non-elevated token; no elevation attempted");
        return None;
    }
    let base = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("pathological-fixtures");
    fs::create_dir_all(&base).unwrap();
    let temp = tempfile::tempdir_in(base.canonicalize().unwrap()).unwrap();
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
            eprintln!("UNVERIFIED: symlink cycle requires Developer Mode or symlink privilege (1314); no elevation attempted");
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
#[ignore = "#483: pipeline MAX_DEPTH=128 truncates the writer's 512-directory chain; raising the scan safety cap needs its own fix"]
fn writer_depth_limit_512_scanned_end_to_end() {
    let Some(lab) = lab(Profile::DepthLimit) else {
        return;
    };
    let done = lab.scan("full");
    assert_eq!(done["coverage"], "complete", "{done}");
    let depth: i64 = lab
        .db()
        .query_row("SELECT max(depth) FROM dir", [], |r| r.get(0))
        .unwrap();
    assert_eq!(depth, 511); // The writer's 512-directory chain includes the root.
    assert_eq!(lab.slice()["nodes"][0]["files"], 1);
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
    assert_eq!(lab.scan("full")["coverage"], "complete");
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
    let (overlap, batches, done) = std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            let deadline = Instant::now() + Duration::from_secs(30);
            while call(&lab.service, "jobs.get", json!({"job_id":job}))["job"]["state"] != "running"
            {
                assert!(
                    Instant::now() < deadline,
                    "scan never ran while writer was ready"
                );
                std::thread::yield_now();
            }
            let mut overlap = 0;
            for batch in 0..128 {
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
                if call(&lab.service, "jobs.get", json!({"job_id":job}))["job"]["state"]
                    == "running"
                {
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
    assert!(overlap > 0, "no mutation batch overlapped a running scan");
    assert_eq!(done["state"], "completed", "{done}");
    assert_eq!(lab.scan("refresh")["coverage"], "complete");
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
    let expected = observations(&fresh_db);
    assert_eq!(actual.len(), expected.len(), "catalogue row count differs");
    for (actual, expected) in actual.iter().zip(&expected) {
        assert_eq!(actual, expected, "catalogue observation differs");
    }
    eprintln!("CHURN VERIFIED: {files} initial files, {batches} mutation batches, {overlap} batches during scan, {} exact catalogue observations",actual.len());
}
