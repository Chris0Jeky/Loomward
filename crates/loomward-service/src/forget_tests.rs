//! `roots.forget` (#288, owner decision q-9): a revoked root leaves Loomward's own state, never the disk.
use crate::tests::*;
use loomward_catalog::{NodeKey, RootObservation, WriteCommand, WriteReply};

const PATH_A: &str = "ForgetServicePathAlpha";
const NAME_A: &str = "ForgetServiceFolderAlpha";
const PATH_B: &str = "KeepServicePathBeta";

/// A fixture root with three listed files and a name of its own.
fn named(svc: &Service, id: u8, name: &str, path: &str) -> (i64, i64) {
    let WriteReply::Root {
        grant_id,
        root_id,
        dir_id,
    } = svc
        .inner
        .catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_key: "fixture".into(),
            display_name: name.into(),
            display_path: path.into(),
            root_file_id: Some(vec![id; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))
        .unwrap()
    else {
        panic!()
    };
    relist(svc, grant_id, dir_id, 3);
    (root_id, grant_id)
}

/// The first file of `root` as a sealed node ID.
fn first_node(svc: &Service, root: i64) -> String {
    let (id, born): (i64, i64) = svc
        .inner
        .db()
        .query_row(
            "SELECT f.id,f.born_run FROM main.file f JOIN main.dir d ON d.id=f.dir_id WHERE d.root_id=?1 ORDER BY f.id LIMIT 1",
            [root],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    svc.inner
        .ids
        .seal(NodeKey::File(id), born, &svc.inner.instance)
}

fn forget(svc: &Service, root: i64, expected: Option<u64>) -> ResponseEnvelope {
    let mut r = request("roots.forget", json!({"root_id": svc.inner.root_id(root)}));
    r.expected_state_rev = expected.map(Generation);
    svc.call(r, &CallContext::http())
}

fn state_rev(svc: &Service) -> u64 {
    svc.inner.revisions().unwrap().1 as u64
}

fn count(svc: &Service, sql: &str) -> i64 {
    svc.inner
        .db()
        .query_row(sql, [], |r| r.get::<_, i64>(0))
        .unwrap()
}

/// Whether the database files or their WALs still hold `needle`.
fn on_disk(dir: &Path, needle: &str) -> bool {
    ["state.db", "state.db-wal", "catalog.db", "catalog.db-wal"]
        .iter()
        .filter_map(|n| std::fs::read(dir.join(n)).ok())
        .any(|bytes| bytes.windows(needle.len()).any(|w| w == needle.as_bytes()))
}

#[test]
fn forgetting_a_revoked_root_purges_its_names_and_keeps_an_anonymised_record() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (a, grant_a) = named(&svc, 1, NAME_A, PATH_A);
    let (b, grant_b) = named(&svc, 2, "KeepServiceFolderBeta", PATH_B);
    svc.inner
        .db()
        .execute_batch("UPDATE main.file SET file_id=cast(printf('%016d',id) AS BLOB),created_ft=1")
        .unwrap();
    let (node_a, node_b) = (first_node(&svc, a), first_node(&svc, b));
    for (n, node) in [&node_a, &node_b].into_iter().enumerate() {
        ok(
            "feedback.record",
            call(
                &svc,
                "feedback.record",
                json!({"node_id": node, "client_event_id": format!("ce_forget_{n}"), "label": "Documents", "retract": false}),
            ),
        );
    }
    let (made, _) = ok(
        "collections.create",
        call(&svc, "collections.create", json!({"name": "Mixed"})),
    );
    let collection = made["collection_id"].as_str().unwrap().to_owned();
    let mut add = request(
        "collections.update_members",
        json!({"collection_id": collection, "add": [node_a, node_b], "remove": []}),
    );
    add.expected_state_rev = Some(Generation(state_rev(&svc)));
    ok(
        "collections.update_members",
        svc.call(add, &CallContext::http()),
    );
    let root_b_before = ok("roots.list", call(&svc, "roots.list", json!({}))).0["roots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["root_id"] == json!(svc.inner.root_id(b)))
        .cloned()
        .unwrap();
    // The names are on disk before the forget, in the file or its WAL.
    assert!(on_disk(dir.path(), PATH_A) && on_disk(dir.path(), NAME_A));

    // An active root is refused, with a reason, and nothing changes.
    let rev = state_rev(&svc);
    let refused = err(forget(&svc, a, None));
    assert_eq!(refused.code, ErrorCode::InvalidRequest);
    assert_eq!(reason(&refused), Some("root_active"));
    assert_eq!(state_rev(&svc), rev);
    assert_eq!(count(&svc, "SELECT count(*) FROM main.root"), 2);

    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": svc.inner.root_id(a), "purge_catalog": false}),
        ),
    );
    let mut events = svc.subscribe(None);

    // A stale precondition writes nothing.
    let rev = state_rev(&svc);
    let stale = err(forget(&svc, a, Some(rev + 7)));
    assert_eq!(stale.code, ErrorCode::StaleGeneration);
    assert_eq!(reason(&stale), Some("state_rev_changed"));
    assert_eq!(state_rev(&svc), rev);
    assert_eq!(count(&svc, "SELECT count(*) FROM main.root"), 2);

    // The same request_id replays the stored answer; a new one finds the root gone.
    let request_once = request("roots.forget", json!({"root_id": svc.inner.root_id(a)}));
    let first = svc.call(request_once.clone(), &CallContext::http());
    let (done, meta) = ok("roots.forget", first.clone());
    assert_eq!(done["root_id"], json!(svc.inner.root_id(a)));
    assert_eq!(svc.call(request_once, &CallContext::http()), first);
    assert_eq!(state_rev(&svc), meta.state_rev.unwrap().get());
    assert_eq!(err(forget(&svc, a, None)).code, ErrorCode::NotFound);

    // roots.changed follows the commit and no longer lists the root.
    let event = loop {
        match events.recv_timeout(Duration::from_secs(2)) {
            RecvOutcome::Event(e)
                if e.event == EventName::RootsChanged
                    && e.data["roots"].as_array().unwrap().len() == 1 =>
            {
                break e
            }
            RecvOutcome::Event(_) => continue,
            other => panic!("{other:?}"),
        }
    };
    event.validate().unwrap();
    assert_eq!(event.state_rev, meta.state_rev);

    // Every row that named the root is gone; the other root is exactly as it was.
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    assert_eq!(roots["roots"].as_array().unwrap(), &vec![root_b_before]);
    assert_eq!(count(&svc, "SELECT count(*) FROM st.object_ref"), 1);
    assert_eq!(count(&svc, "SELECT count(*) FROM st.human_feedback"), 1);
    assert_eq!(count(&svc, "SELECT count(*) FROM st.collection_member"), 1);
    assert_eq!(count(&svc, "SELECT count(*) FROM st.learning_receipt"), 1);
    assert_eq!(count(&svc, "SELECT count(*) FROM st.learning_node"), 1);
    assert_eq!(
        count(&svc, "SELECT count(*) FROM main.file"),
        count(
            &svc,
            &format!("SELECT count(*) FROM main.file f JOIN main.dir d ON d.id=f.dir_id WHERE d.root_id={b}")
        )
    );
    assert_eq!(
        count(
            &svc,
            "SELECT count(*) FROM main.dir WHERE name LIKE 'Forget%'"
        ),
        0
    );
    let (members, _) = ok(
        "collections.members",
        call(
            &svc,
            "collections.members",
            json!({"collection_id": collection, "limit": 10, "cursor": null}),
        ),
    );
    assert_eq!(members["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        ok("learning.status", call(&svc, "learning.status", json!({}))).0["human_events"],
        1
    );
    // The audit record: dates and an ID, no path, name or volume.
    let (grants, _) = ok("grants.list", call(&svc, "grants.list", json!({})));
    let listed = grants["grants"].as_array().unwrap();
    let record = listed
        .iter()
        .find(|g| g["grant_id"] == json!(format!("gr_{grant_a}")))
        .unwrap();
    assert!(record["forgotten_at"].is_string() && record["revoked_at"].is_string());
    assert!(listed
        .iter()
        .any(|g| g["grant_id"] == json!(format!("gr_{grant_b}")) && g["forgotten_at"].is_null()));
    let text = json!(grants).to_string();
    assert!(!text.contains(PATH_A) && !text.contains(NAME_A));
    assert_eq!(
        count(
            &svc,
            &format!("SELECT display_path='forgotten' FROM st.root_grant WHERE id={grant_a}")
        ),
        1
    );
    // Nothing of it is left in the files or their WALs; the other root's name is.
    drop(svc);
    for needle in [PATH_A, NAME_A] {
        assert!(!on_disk(dir.path(), needle), "{needle} survived on disk");
    }
    assert!(on_disk(dir.path(), PATH_B));
    // The rolling start-up backups predate the forget and would still hold it.
    let backups = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("state.start-")
        })
        .count();
    assert_eq!(backups, 0, "start-up backups must be removed");
}

#[test]
fn a_forgotten_root_stays_forgotten_across_a_restart() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (a, grant_a) = named(&svc, 1, NAME_A, PATH_A);
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": svc.inner.root_id(a), "purge_catalog": false}),
        ),
    );
    ok("roots.forget", forget(&svc, a, None));
    drop(svc);
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    assert!(roots["roots"].as_array().unwrap().is_empty());
    let (grants, _) = ok("grants.list", call(&svc, "grants.list", json!({})));
    assert_eq!(
        grants["grants"][0]["grant_id"],
        json!(format!("gr_{grant_a}"))
    );
    assert!(grants["grants"][0]["forgotten_at"].is_string());
    // Revoking the forgotten grant again is the monotone no-op it always was.
    ok(
        "grants.revoke",
        call(
            &svc,
            "grants.revoke",
            json!({"grant_id": format!("gr_{grant_a}")}),
        ),
    );
    // Forgetting never reactivates it: a new grant of the same folder is a new record.
    assert_eq!(
        count(
            &svc,
            "SELECT count(*) FROM st.root_grant WHERE state='active'"
        ),
        0
    );
}

/// Every file in the state directory that holds `needle`, as UTF-8 or UTF-16LE.
fn holders(dir: &Path, needle: &str) -> Vec<String> {
    let utf16: Vec<u8> = needle.encode_utf16().flat_map(u16::to_le_bytes).collect();
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_file())
        .filter(|e| {
            let bytes = std::fs::read(e.path()).unwrap_or_default();
            [needle.as_bytes(), utf16.as_slice()]
                .iter()
                .any(|n| bytes.windows(n.len()).any(|w| w == *n))
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn forgetting_scrubs_the_migration_copies_so_no_file_keeps_the_name() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (a, _) = named(&svc, 1, NAME_A, PATH_A);
    named(&svc, 2, "KeepServiceFolderBeta", PATH_B);
    svc.inner
        .db()
        .execute_batch("UPDATE main.file SET file_id=cast(printf('%016d',id) AS BLOB),created_ft=1")
        .unwrap();
    let node_a = first_node(&svc, a);
    ok(
        "feedback.record",
        call(
            &svc,
            "feedback.record",
            json!({"node_id": node_a, "client_event_id": "ce_copy_forget", "label": "Documents", "retract": false}),
        ),
    );
    // The copies a v5 and a v6 install's upgrades leave (older schemas: no root_forgotten, and at v5
    // no backup_evidence either), and a copy in a shape the scrub does not know that names the root.
    let copies = [
        (
            "state.before-v5.db",
            "DROP TABLE root_forgotten; DROP TABLE backup_evidence",
        ),
        ("state.before-v6.db", "DROP TABLE root_forgotten"),
    ];
    for (name, downgrade) in copies {
        let copy = dir.path().join(name);
        svc.inner
            .db()
            .execute("VACUUM st INTO ?1", [copy.to_string_lossy().as_ref()])
            .unwrap();
        rusqlite::Connection::open(&copy)
            .unwrap()
            .execute_batch(downgrade)
            .unwrap();
    }
    let unknown = dir.path().join("state.before-v1.db");
    let legacy = rusqlite::Connection::open(&unknown).unwrap();
    legacy
        .execute_batch("CREATE TABLE legacy_root(path TEXT)")
        .unwrap();
    legacy
        .execute("INSERT INTO legacy_root VALUES(?1)", [PATH_A])
        .unwrap();
    drop(legacy);
    for name in [
        "state.before-v5.db",
        "state.before-v6.db",
        "state.before-v1.db",
    ] {
        assert!(
            holders(dir.path(), PATH_A).contains(&name.to_owned()),
            "{name}"
        );
    }

    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": svc.inner.root_id(a), "purge_catalog": false}),
        ),
    );
    ok("roots.forget", forget(&svc, a, None));
    drop(svc);
    let left = holders(dir.path(), PATH_A);
    assert!(left.is_empty(), "the root's path survived in {left:?}");
    // The known copies are scrubbed, not discarded: the other root and the owner's rows stay in them.
    for (name, _) in copies {
        assert!(
            holders(dir.path(), PATH_B).contains(&name.to_owned()),
            "{name}"
        );
        let copy = rusqlite::Connection::open(dir.path().join(name)).unwrap();
        let kept: (i64, i64) = copy
            .query_row(
                "SELECT (SELECT count(*) FROM root_grant),(SELECT count(*) FROM human_feedback)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kept, (2, 0), "{name}");
    }
    assert!(
        !unknown.exists(),
        "a copy the scrub cannot clear is removed"
    );
}

#[test]
fn failed_migration_scrub_preserves_rollback_copy_and_reports_failure() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    svc.inner.revoke(grant).unwrap();
    let copy = dir.path().join("state.before-v6.db");
    svc.inner
        .db()
        .execute("VACUUM st INTO ?1", [copy.to_string_lossy().as_ref()])
        .unwrap();
    let rollback = rusqlite::Connection::open(&copy).unwrap();
    rollback
        .execute_batch("CREATE TRIGGER refuse_scrub BEFORE UPDATE ON root_grant BEGIN SELECT RAISE(ABORT, 'synthetic scrub failure'); END")
        .unwrap();
    drop(rollback);
    let before = std::fs::read(&copy).unwrap();
    // A start-up copy sorting after the failing one must still be swept.
    let start = dir.path().join("state.start-zz.db");
    svc.inner
        .db()
        .execute("VACUUM st INTO ?1", [start.to_string_lossy().as_ref()])
        .unwrap();

    let response = forget(&svc, root, None);
    assert!(copy.exists(), "a failed scrub deleted the rollback copy");
    assert_eq!(std::fs::read(&copy).unwrap(), before);
    assert!(
        !start.exists(),
        "a failed scrub stopped the sweep of the other copies"
    );
    let error = err(response);
    assert_eq!(error.code, ErrorCode::InternalError);
    assert_eq!(
        error.detail.unwrap()["reason"],
        "forget_backup_scrub_failed"
    );
    // The live forget committed and was announced: the root is gone from roots.list.
    let listed = serde_json::Value::Object(ok("roots.list", call(&svc, "roots.list", json!({}))).0)
        .to_string();
    assert!(
        !listed.contains(NAME_A),
        "the forgotten root is still listed"
    );
}

#[test]
fn busy_migration_scrub_preserves_rollback_copy_and_reports_failure() {
    let dir = temp_root();
    let svc = open(dir.path(), DatasetClass::Synthetic);
    let (root, grant) = named(&svc, 1, NAME_A, PATH_A);
    svc.inner.revoke(grant).unwrap();
    let copy = dir.path().join("state.before-v6.db");
    svc.inner
        .db()
        .execute("VACUUM st INTO ?1", [copy.to_string_lossy().as_ref()])
        .unwrap();
    let rollback = rusqlite::Connection::open(&copy).unwrap();
    rollback.execute_batch("BEGIN IMMEDIATE").unwrap();
    let before = std::fs::read(&copy).unwrap();

    let response = forget(&svc, root, None);
    rollback.execute_batch("ROLLBACK").unwrap();
    drop(rollback);
    assert!(copy.exists(), "a busy scrub deleted the rollback copy");
    assert_eq!(std::fs::read(&copy).unwrap(), before);
    assert_eq!(err(response).code, ErrorCode::InternalError);
}
