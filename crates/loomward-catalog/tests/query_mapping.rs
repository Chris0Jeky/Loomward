use loomward_catalog::*;
use rusqlite::Connection;

fn setup_with_files(files: Vec<Observation>) -> (tempfile::TempDir, Catalog, i64, i64) {
    let tmp = tempfile::tempdir().unwrap();
    let c = Catalog::open(tmp.path(), "synthetic").unwrap();
    let WriteReply::Root {
        grant_id, dir_id, ..
    } = c
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_key: "fixture".into(),
            display_name: "Root".into(),
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
    c.writer()
        .call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id,
            files,
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
            finished_at_ns: 1,
        })
        .unwrap();
    (tmp, c, grant_id, dir_id)
}

fn sized_flagged(name: &str, logical: u64, flags: u32) -> Observation {
    let mut o = Observation::file(name, logical, Some(logical));
    o.flags = flags;
    o
}

fn file_id_of(db: &Connection, name: &str) -> i64 {
    db.query_row("SELECT id FROM file WHERE name=?1", [name], |r| r.get(0))
        .unwrap()
}

// Behaviour 1: Raw::coverage flags override listing state (query.rs ~82).
#[test]
fn coverage_flag_overrides_complete_listing_via_children_size_desc_logical() {
    let (_tmp, c, _grant, dir) = setup_with_files(vec![
        sized_flagged("synthetic-plain", 60, 0),
        sized_flagged("synthetic-denied", 50, 1 << 6),
        sized_flagged("synthetic-ctx", 40, 1 << 1),
        sized_flagged("synthetic-cloud", 30, 1 << 3),
        sized_flagged("synthetic-reparse", 20, 1 << 4),
        sized_flagged("synthetic-statedir", 10, 1 << 5),
    ]);
    let mut r = c.reader().unwrap();
    let page = r
        .children(&ChildrenRequest {
            dir_id: dir,
            sort: Sort::SizeDesc,
            basis: Basis::Logical,
            limit: 100,
            cursor: None,
        })
        .unwrap();
    assert_eq!(
        page.items
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "synthetic-plain",
            "synthetic-denied",
            "synthetic-ctx",
            "synthetic-cloud",
            "synthetic-reparse",
            "synthetic-statedir",
        ]
    );
    let coverage = |name: &str| {
        page.items
            .iter()
            .find(|e| e.name == name)
            .unwrap()
            .coverage
            .clone()
    };
    assert_eq!(coverage("synthetic-denied"), "denied");
    assert_eq!(coverage("synthetic-ctx"), "excluded");
    assert_eq!(coverage("synthetic-cloud"), "excluded");
    assert_eq!(coverage("synthetic-reparse"), "excluded");
    assert_eq!(coverage("synthetic-statedir"), "excluded");
    assert_eq!(coverage("synthetic-plain"), "complete");
}

#[test]
fn children_rejects_zero_page_limit() {
    let (_tmp, c, _grant, dir) = setup_with_files(vec![sized_flagged("synthetic-plain", 60, 0)]);
    let mut r = c.reader().unwrap();
    assert!(matches!(
        r.children(&ChildrenRequest {
            dir_id: dir,
            sort: Sort::SizeDesc,
            basis: Basis::Logical,
            limit: 0,
            cursor: None,
        }),
        Err(Error::Invalid("page limit"))
    ));
}

// Behaviour 2: inspect threads mapping (query.rs ~188).
#[test]
fn inspect_threads_map_flag_bits_to_permission_state_and_reason() {
    let (tmp, c, _grant, _dir) = setup_with_files(vec![
        sized_flagged("synthetic-sensitive", 50, 1),
        sized_flagged("synthetic-protected", 40, 1 << 1),
        sized_flagged("synthetic-cloud", 30, 1 << 3),
        sized_flagged("synthetic-reparse", 20, 1 << 4),
        sized_flagged("synthetic-denied", 10, 1 << 6),
        sized_flagged("synthetic-plain", 60, 0),
    ]);
    let db = Connection::open(tmp.path().join("catalog.db")).unwrap();
    let mut r = c.reader().unwrap();
    for (name, state, reason) in [
        ("synthetic-sensitive", "excluded", "sensitive_name"),
        ("synthetic-protected", "excluded", "protected_context"),
        ("synthetic-cloud", "excluded", "cloud_placeholder"),
        ("synthetic-reparse", "excluded", "reparse_not_followed"),
        ("synthetic-denied", "denied", "access_denied"),
        ("synthetic-plain", "granted", "metadata_grant_active"),
    ] {
        let detail = r.inspect(NodeKey::File(file_id_of(&db, name))).unwrap();
        assert_eq!(detail.threads.permission.state, state, "{name}");
        assert_eq!(
            detail.threads.permission.reason.as_deref(),
            Some(reason),
            "{name}"
        );
    }
    let detail = r
        .inspect(NodeKey::File(file_id_of(&db, "synthetic-plain")))
        .unwrap();
    assert_eq!(detail.threads.residency.tier, None);
    assert_eq!(detail.threads.residency.tier_basis, "unknown");
}

#[test]
fn inspect_threads_report_declared_tier_basis_after_tier_declaration() {
    let (tmp, c, _grant, _dir) = setup_with_files(vec![sized_flagged("synthetic-plain", 60, 0)]);
    let state = Connection::open(tmp.path().join("state.db")).unwrap();
    state
        .busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    state
        .execute(
            "INSERT INTO tier_declaration(volume_key, tier, declared_at_ns) VALUES('fixture', 3, 1)",
            [],
        )
        .unwrap();
    drop(state);
    let db = Connection::open(tmp.path().join("catalog.db")).unwrap();
    let mut r = c.reader().unwrap();
    let detail = r
        .inspect(NodeKey::File(file_id_of(&db, "synthetic-plain")))
        .unwrap();
    assert_eq!(detail.threads.residency.tier, Some(3));
    assert_eq!(detail.threads.residency.tier_basis, "declared");
}

#[test]
fn inspect_unknown_file_is_not_found() {
    let (_tmp, c, _grant, _dir) = setup_with_files(vec![sized_flagged("synthetic-plain", 60, 0)]);
    let mut r = c.reader().unwrap();
    assert!(matches!(
        r.inspect(NodeKey::File(999_999)),
        Err(Error::NotFound)
    ));
}

// Behaviour 3: inspect identity and path limits (query.rs ~424).
#[test]
fn inspect_identity_pins_available_and_unavailable_durable_quality() {
    let (tmp, c, _grant, _dir) = setup_with_files(vec![
        {
            let mut o = sized_flagged("synthetic-durable", 100, 0);
            o.file_id = Some(vec![7u8; 16]);
            o.created_ft = Some(133_0000_0000_0000_000);
            o
        },
        sized_flagged("synthetic-noid", 90, 0),
    ]);
    let db = Connection::open(tmp.path().join("catalog.db")).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    let mut r = c.reader().unwrap();
    let durable = r
        .inspect(NodeKey::File(file_id_of(&db, "synthetic-durable")))
        .unwrap();
    assert_eq!(durable.identity.durable_reference, "available");
    assert_eq!(durable.identity.quality, "native_file_id_128");
    assert_eq!(
        durable.identity.file_id_hex.as_deref(),
        Some("07070707070707070707070707070707")
    );
    assert!(!durable.identity.authorises_effects);
    let noid = r
        .inspect(NodeKey::File(file_id_of(&db, "synthetic-noid")))
        .unwrap();
    assert_eq!(
        noid.identity.durable_reference,
        "unavailable_identity_quality"
    );
    assert_eq!(noid.identity.quality, "path_observation");
    assert_eq!(noid.identity.file_id_hex, None);
    db.execute("UPDATE volume SET filesystem='synthetic-fs'", [])
        .unwrap();
    let foreign = r
        .inspect(NodeKey::File(file_id_of(&db, "synthetic-durable")))
        .unwrap();
    assert_eq!(foreign.identity.durable_reference, "unavailable_filesystem");
}

#[test]
fn inspect_refuses_display_path_beyond_limit_on_overlong_chain() {
    let (tmp, c, _grant, dir) = setup_with_files(vec![]);
    let db = Connection::open(tmp.path().join("catalog.db")).unwrap();
    let root: i64 = db
        .query_row("SELECT root_id FROM dir WHERE id=?1", [dir], |r| r.get(0))
        .unwrap();
    let mut parent = dir;
    for depth in 1..=170 {
        db.execute(
            "INSERT INTO dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,flags,listing_state,born_run,seen_run) VALUES(?1,?2,?3,'path_observation','none',?4,16,0,'complete',1,1)",
            rusqlite::params![root, parent, "x".repeat(200), depth],
        )
        .unwrap();
        parent = db.last_insert_rowid();
    }
    let mut r = c.reader().unwrap();
    assert!(matches!(
        r.inspect(NodeKey::Dir(parent)),
        Err(Error::Invalid("display path limit"))
    ));
}

// Behaviour 4: incarnation binding (query.rs ~791).
#[test]
fn check_incarnation_accepts_fresh_binding_and_refuses_after_instance_change() {
    let (tmp, c, _grant, _dir) = setup_with_files(vec![sized_flagged("synthetic-plain", 60, 0)]);
    let db = Connection::open(tmp.path().join("catalog.db")).unwrap();
    let key = NodeKey::File(file_id_of(&db, "synthetic-plain"));
    let mut r = c.reader().unwrap();
    let (instance, born) = r.incarnation(key).unwrap();
    assert_eq!(born, 1);
    assert!(r.check_incarnation(key, &instance, born).is_ok());
    assert!(matches!(
        r.check_incarnation(key, &instance, born + 1),
        Err(Error::NotFound)
    ));
    db.execute(
        "UPDATE meta SET value='synthetic-new-instance' WHERE key='instance_id'",
        [],
    )
    .unwrap();
    assert!(matches!(
        r.check_incarnation(key, &instance, born),
        Err(Error::NotFound)
    ));
    let (current, reborn) = r.incarnation(key).unwrap();
    assert_eq!(current, "synthetic-new-instance");
    assert_eq!(reborn, born);
    assert!(r.check_incarnation(key, &current, reborn).is_ok());
}
