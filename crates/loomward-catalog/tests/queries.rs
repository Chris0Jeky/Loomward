use loomward_catalog::*;
use rusqlite::Connection;

#[test]
fn duplicates_select_exact_sizes_visible_names_and_scan_identity_with_a_cap() {
    let (tmp, catalog, _, dir, _) = setup();
    let db = Connection::open(tmp.path().join("catalog.db")).unwrap();
    let root: i64 = db
        .query_row("SELECT root_id FROM dir WHERE id=?1", [dir], |r| r.get(0))
        .unwrap();
    db.execute_batch("UPDATE file SET logical=0;").unwrap();
    for (name, size, id, flags) in [
        ("a", 4, 1, 0),
        ("alias", 4, 1, 0),
        ("b", 4, 2, 0),
        ("c", 8, 3, 0),
        ("d", 8, 4, 0),
        ("singleton", 9, 5, 0),
        ("excluded", 4, 6, 8),
        ("sensitive", 4, 7, 1),
    ] {
        db.execute("INSERT INTO file(dir_id,name,file_id,logical,attrs,flags,created_ft,modified_ft,changed_ft,born_run,seen_run) VALUES(?1,?2,?3,?4,32,?5,11,12,13,1,1)", rusqlite::params![dir,name,vec![id as u8;16],size,flags]).unwrap();
    }
    // A formerly listed subtree is still stored, but is shielded by an excluded ancestor.
    db.execute("INSERT INTO dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,flags,listing_state,born_run,seen_run) VALUES(?1,?2,'hidden','path_observation','none',1,16,0,'excluded',1,1)", rusqlite::params![root,dir]).unwrap();
    let hidden = db.last_insert_rowid();
    db.execute("INSERT INTO file(dir_id,name,logical,attrs,flags,born_run,seen_run) VALUES(?1,'shielded',4,0,0,1,1)",[hidden]).unwrap();
    let mut reader = catalog.reader().unwrap();
    let all = reader.duplicate_candidates(root, 1, 200).unwrap();
    assert!(!all.truncated);
    assert!(
        !all.partial,
        "policy exclusions alone leave the report complete"
    );
    assert_eq!(
        all.entries
            .iter()
            .map(|e| e.relative.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "alias", "b", "c", "d"]
    );
    assert_eq!(all.entries[0].file_id, all.entries[1].file_id);
    assert_eq!(
        (
            all.entries[0].created,
            all.entries[0].modified,
            all.entries[0].changed,
            all.entries[0].attributes
        ),
        (Some(11), Some(12), Some(13), 32)
    );
    reader.set_deadline(std::time::Instant::now());
    assert!(matches!(
        reader.duplicate_candidates(root, 1, 200),
        Err(Error::Cancelled)
    ));
    let mut reader = catalog.reader().unwrap();
    let capped = reader.duplicate_candidates(root, 0, 4).unwrap();
    assert!(capped.truncated);
    assert_eq!(capped.entries.len(), 3); // cap split the trailing size group: singleton dropped
                                         // A same-sized name the scan could not represent (lossy) or read (denied) is an unknown.
    for (name, flags) in [("lossy", 1 << 9), ("denied", 1 << 6)] {
        db.execute("INSERT INTO file(dir_id,name,file_id,logical,attrs,flags,born_run,seen_run) VALUES(?1,?2,?3,16,32,?4,1,1)", rusqlite::params![dir, name, vec![9u8; 16], flags]).unwrap();
        let mut reader = catalog.reader().unwrap();
        let unknown = reader.duplicate_candidates(root, 1, 200).unwrap();
        assert!(
            unknown.partial,
            "{name} names are filtered but must leave the report partial"
        );
        db.execute("DELETE FROM file WHERE name=?1", [name])
            .unwrap();
    }
    let mut reader = catalog.reader().unwrap();
    let floor = reader.duplicate_candidates(root, 8, 200).unwrap();
    assert_eq!(
        floor.entries.iter().map(|e| e.logical).collect::<Vec<_>>(),
        vec![8, 8]
    );
    db.execute(
        "UPDATE dir SET listing_state='absent_pending' WHERE id=?1",
        [hidden],
    )
    .unwrap();
    assert_eq!(
        reader
            .duplicate_candidates(root, 1, 200)
            .unwrap()
            .entries
            .len(),
        5
    );
    let mut parent = dir;
    for depth in 1..=8 {
        db.execute("INSERT INTO dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,flags,listing_state,born_run,seen_run) VALUES(?1,?2,?3,'path_observation','none',?4,16,0,'complete',1,1)", rusqlite::params![root,parent,"x".repeat(200),depth]).unwrap();
        parent = db.last_insert_rowid();
    }
    for i in 0..50 {
        db.execute("INSERT INTO file(dir_id,name,logical,attrs,flags,born_run,seen_run) VALUES(?1,?2,16,32,0,1,1)",rusqlite::params![parent,format!("synthetic-{i}")]).unwrap();
    }
    let bounded = reader.duplicate_candidates(root, 1, 200).unwrap();
    assert!(
        bounded.truncated,
        "encoded paths have a separate report-size bound"
    );
    assert!(bounded.entries.len() > 5 && bounded.entries.len() < 55);
    assert!(
        bounded
            .entries
            .iter()
            .map(|e| serde_json::to_string(&e.relative).unwrap().len())
            .sum::<usize>()
            <= 64 * 1024
    );
    catalog
        .writer()
        .call(WriteCommand::RevokeGrant {
            grant_id: root,
            revoked_at_ns: 1,
        })
        .unwrap();
    assert!(matches!(
        reader.duplicate_candidates(root, 1, 200),
        Err(Error::NotFound)
    ));
}

#[test]
fn duplicate_candidates_skip_one_over_depth_path_without_losing_valid_pairs() {
    let (tmp, catalog, _, dir, _) = setup();
    let db = Connection::open(tmp.path().join("catalog.db")).unwrap();
    let root: i64 = db
        .query_row("SELECT root_id FROM dir WHERE id=?1", [dir], |r| r.get(0))
        .unwrap();
    db.execute_batch("UPDATE file SET logical=0;").unwrap();
    let mut parent = dir;
    for depth in 1..=511 {
        db.execute("INSERT INTO dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,flags,listing_state,born_run,seen_run) VALUES(?1,?2,'nested','path_observation','none',?3,16,0,'complete',1,1)", rusqlite::params![root,parent,depth]).unwrap();
        parent = db.last_insert_rowid();
    }
    // Put the unrepresentable entry first so returning early loses the valid pair.
    for (parent, name) in [(parent, "deep"), (dir, "a"), (dir, "b")] {
        db.execute("INSERT INTO file(dir_id,name,logical,attrs,flags,born_run,seen_run) VALUES(?1,?2,4,32,0,1,1)", rusqlite::params![parent,name]).unwrap();
    }
    let result = catalog
        .reader()
        .unwrap()
        .duplicate_candidates(root, 1, 200)
        .unwrap();
    assert_eq!(
        result
            .entries
            .iter()
            .map(|e| e.relative.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert!(result.partial);
    assert_eq!(result.skipped, ["path_depth"]);
}

fn setup() -> (tempfile::TempDir, Catalog, i64, i64, i64) {
    let tmp = tempfile::tempdir().unwrap();
    let c = Catalog::open(tmp.path(), "synthetic").unwrap();
    let WriteReply::Root {
        grant_id, dir_id, ..
    } = c
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
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
    let mut files = Vec::new();
    for i in 0..60 {
        let mut o = Observation::file(
            format!("synthetic-{i:03}.txt"),
            100 - i,
            if i % 7 == 0 { None } else { Some(100 - i) },
        );
        o.extension = Some("txt".into());
        o.family = "document".into();
        files.push(o);
    }
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
    (tmp, c, grant_id, dir_id, run)
}
fn validate(def: &str, value: serde_json::Value) {
    let source: serde_json::Value = serde_json::from_str(include_str!(
        "../../../contracts/v3/view-service.schema.json"
    ))
    .unwrap();
    let schema = serde_json::json!({"$schema":"https://json-schema.org/draft/2020-12/schema","$defs":source["$defs"],"$ref":format!("#/$defs/{def}")});
    let v = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<_> = v.iter_errors(&value).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "{def}: {errors:?}");
}

#[test]
fn incomplete_listing_reasons_survive_children_slices_and_restart() {
    for staged in [false, true] {
        for reason in [
            "partial",
            "denied",
            "excluded",
            "cancelled",
            "unscanned",
            "stale",
        ] {
            let (tmp, c, grant, root, _) = setup();
            let WriteReply::Run(run) = c
                .writer()
                .call(WriteCommand::BeginRun {
                    grant_id: grant,
                    mode: "targeted".into(),
                    strategy: "fixture".into(),
                    started_at_ns: 2,
                })
                .unwrap()
            else {
                panic!()
            };
            if staged {
                c.writer()
                    .call(WriteCommand::ListingDone {
                        run_id: run,
                        dir_id: root,
                        outcome: ListingOutcome::Incomplete(reason.into()),
                        skipped: 1,
                        errors: 1,
                    })
                    .unwrap();
            } else {
                c.writer()
                    .call(WriteCommand::DirListing(DirListing {
                        run_id: run,
                        dir_id: root,
                        files: vec![],
                        dirs: vec![],
                        state: reason.into(),
                        skipped: 1,
                        errors: 1,
                    }))
                    .unwrap();
            }
            c.writer()
                .call(WriteCommand::EndRun {
                    run_id: run,
                    state: "completed".into(),
                    finished_at_ns: 3,
                })
                .unwrap();
            drop(c);
            let c = Catalog::open(tmp.path(), "synthetic").unwrap();
            let mut reader = c.reader().unwrap();
            let slice = reader
                .slice(&SliceRequest {
                    anchor: NodeKey::Dir(root),
                    depth: 1,
                    max_nodes: 100,
                    min_share: 0.0,
                    basis: Basis::Logical,
                    include_files: true,
                })
                .unwrap();
            assert_eq!(slice.nodes[0].coverage, reason, "{staged}/{reason}");
            validate("TreeSlice", serde_json::to_value(&slice).unwrap());
            let page = reader
                .children(&ChildrenRequest {
                    dir_id: root,
                    sort: Sort::SizeDesc,
                    basis: Basis::Logical,
                    limit: 100,
                    cursor: None,
                })
                .unwrap();
            assert_eq!(page.items.len(), 60);
            assert!(page.items.iter().all(|row| row.coverage == "stale"));
        }
    }
}

#[test]
fn absent_budgeted_search_advances_examined_rows_and_terminates() {
    let (_tmp, c, grant, _, _) = setup();
    let mut reader = c.reader().unwrap();
    let mut req = SearchRequest {
        root_id: Some(grant),
        // Every name matches and the size filter rejects every row: all examined, none returned.
        text: "synthetic".into(),
        extension: None,
        min_bytes: Some(i64::MAX as u64),
        max_bytes: None,
        modified_after: None,
        modified_before: None,
        kind: "file".into(),
        limit: 10,
        cursor: None,
        work_budget: 500,
        time_budget: None,
        scope: None,
    };
    let mut last = (0, i64::MIN);
    let mut hits = 0;
    for _ in 0..100 {
        let page = reader.search(&req).unwrap();
        assert!(page.hits.is_empty());
        if let Some(cursor) = page.next_cursor {
            let at = (cursor.pass, cursor.last_key);
            assert!(at > last, "stalled at {last:?}: {cursor:?}");
            last = at;
            hits += usize::from(page.budget_hit);
            req.cursor = Some(cursor);
        } else {
            assert!(hits > 0, "fixture must exercise interruption");
            return;
        }
    }
    panic!("search never terminated");
}

#[test]
fn v3_upgrade_preserves_observations_and_legacy_coverage() {
    let (tmp, c, grant, root, _) = setup();
    drop(c);
    let db = Connection::open(tmp.path().join("catalog.db")).unwrap();
    let instance: String = db
        .query_row("SELECT value FROM meta WHERE key='instance_id'", [], |r| {
            r.get(0)
        })
        .unwrap();
    db.execute_batch("ALTER TABLE dir DROP COLUMN listing_outcome; PRAGMA user_version=3")
        .unwrap();
    db.close().unwrap();
    let state = Connection::open(tmp.path().join("state.db")).unwrap();
    // Keep this observation-migration fixture's explicit volume binding across its version rewind.
    state
        .execute(
            "UPDATE root_grant SET volume_guid='00000000000000000000000000000001'",
            [],
        )
        .unwrap();
    state.pragma_update(None, "user_version", 3).unwrap();
    state.close().unwrap();
    let c = Catalog::open(tmp.path(), "synthetic").unwrap();
    let mut r = c.reader().unwrap();
    let slice = r
        .slice(&SliceRequest {
            anchor: NodeKey::Dir(root),
            depth: 1,
            max_nodes: 100,
            min_share: 0.0,
            basis: Basis::Logical,
            include_files: true,
        })
        .unwrap();
    assert_eq!(slice.nodes[0].coverage, "complete");
    assert_eq!(slice.nodes[0].files, 60);
    let db = Connection::open(tmp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT value FROM meta WHERE key='instance_id'", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        instance
    );
    assert_eq!(
        db.query_row("SELECT listing_outcome FROM dir WHERE id=?1", [root], |r| r
            .get::<_, Option<String>>(0))
            .unwrap(),
        None
    );
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    let WriteReply::Run(run) = c
        .writer()
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "targeted".into(),
            strategy: "fixture".into(),
            started_at_ns: 2,
        })
        .unwrap()
    else {
        panic!()
    };
    c.writer()
        .call(WriteCommand::ListingDone {
            run_id: run,
            dir_id: root,
            outcome: ListingOutcome::Incomplete("cancelled".into()),
            skipped: 0,
            errors: 0,
        })
        .unwrap();
    c.writer()
        .call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 3,
        })
        .unwrap();
    assert_eq!(
        r.slice(&SliceRequest {
            anchor: NodeKey::Dir(root),
            depth: 1,
            max_nodes: 100,
            min_share: 0.0,
            basis: Basis::Logical,
            include_files: true
        })
        .unwrap()
        .nodes[0]
            .coverage,
        "cancelled"
    );
    assert!(tmp.path().join("state.before-v3.db").exists());
}

#[test]
fn sparse_budgeted_search_keeps_every_match_without_duplicates() {
    let (_tmp, c, grant, _, _) = setup();
    let mut r = c.reader().unwrap();
    let mut req = SearchRequest {
        root_id: Some(grant),
        text: "5.txt".into(),
        extension: None,
        min_bytes: None,
        max_bytes: None,
        modified_after: None,
        modified_before: None,
        kind: "file".into(),
        limit: 2,
        cursor: None,
        work_budget: 500,
        time_budget: None,
        scope: None,
    };
    let mut names = Vec::new();
    for _ in 0..100 {
        let page = r.search(&req).unwrap();
        names.extend(page.hits.into_iter().map(|h| h.row.name));
        if page.next_cursor.is_none() {
            assert_eq!(
                names,
                (0..6)
                    .map(|i| format!("synthetic-0{i}5.txt"))
                    .collect::<Vec<_>>()
            );
            return;
        }
        req.cursor = page.next_cursor;
    }
    panic!("sparse search never terminated");
}
#[test]
fn children_pages_are_keysets_bound_to_revision_sort_basis_and_anchor() {
    let (_tmp, c, grant, root, _) = setup();
    let mut r = c.reader().unwrap();
    let mut req = ChildrenRequest {
        dir_id: root,
        sort: Sort::SizeDesc,
        basis: Basis::Logical,
        limit: 11,
        cursor: None,
    };
    let mut names = Vec::new();
    loop {
        let page = r.children(&req).unwrap();
        names.extend(page.items.iter().map(|x| x.name.clone()));
        req.cursor = page.next_cursor;
        if req.cursor.is_none() {
            break;
        }
    }
    assert_eq!(names.len(), 60);
    assert!(names.windows(2).all(|w| w[0] < w[1]));
    req.cursor = None;
    let page = r.children(&req).unwrap();
    let cursor = page.next_cursor.unwrap();
    req.cursor = Some(cursor.clone());
    req.basis = Basis::Allocated;
    assert!(r.children(&req).is_err());
    req.basis = Basis::Logical;
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
    c.writer()
        .call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id: root,
            files: vec![Observation::file("replacement", 9, Some(9))],
            dirs: vec![],
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        }))
        .unwrap();
    assert!(matches!(r.children(&req), Err(Error::StaleGeneration)));
}
#[test]
fn slices_fold_exact_sums_and_keep_unknowns_visible_and_contract_shapes() {
    let (_tmp, c, _, root, _) = setup();
    let mut r = c.reader().unwrap();
    for basis in [Basis::Logical, Basis::Allocated] {
        let s = r
            .slice(&SliceRequest {
                anchor: NodeKey::Dir(root),
                depth: 3,
                max_nodes: 16,
                min_share: 0.0,
                basis,
                include_files: true,
            })
            .unwrap();
        validate("TreeSlice", serde_json::to_value(&s).unwrap());
        assert!(s.nodes.len() <= 16);
        assert!(s.truncated);
        let parent = &s.nodes[0];
        let mut sum = 0;
        for n in &s.nodes[1..] {
            assert_eq!(n.parent, Some(0));
            sum += n.size_bytes.parse::<u64>().unwrap();
        }
        assert_eq!(sum, parent.size_bytes.parse::<u64>().unwrap());
        assert!(parent.allocated_bytes.is_none());
        assert_eq!(parent.size_unknown_files, 9);
        assert_eq!(s.ordering, "exact");
        let other = s.nodes.iter().find(|n| n.kind == "other").unwrap();
        assert_eq!(other.folded_count, Some(46));
    }
    let page = r
        .children(&ChildrenRequest {
            dir_id: root,
            sort: Sort::NameAsc,
            basis: Basis::Logical,
            limit: 20,
            cursor: None,
        })
        .unwrap();
    for row in &page.items {
        validate("EntryRow", serde_json::to_value(row).unwrap());
    }
    validate(
        "NodePath",
        serde_json::to_value(r.path(NodeKey::File(1)).unwrap()).unwrap(),
    );
    let detail = r.inspect(NodeKey::File(1)).unwrap();
    validate("NodeDetail", serde_json::to_value(&detail).unwrap());
    assert!(!detail.identity.authorises_effects);
    let b = r
        .breakdown(root, "extension", Basis::Logical, 10, 1_000_000)
        .unwrap();
    validate("Breakdown", serde_json::to_value(&b).unwrap());
    assert_eq!(b.buckets.len(), 1);
    assert_eq!(b.buckets[0].files, 60);
}
#[test]
fn budget_hit_is_visible_and_progress_handler_does_not_poison_later_queries() {
    let (_tmp, c, grant, root, _) = setup();
    let mut r = c.reader().unwrap();
    let mut req = SearchRequest {
        root_id: Some(grant),
        text: "no match".into(),
        extension: None,
        min_bytes: None,
        max_bytes: None,
        modified_after: None,
        modified_before: None,
        kind: "any".into(),
        limit: 10,
        cursor: None,
        work_budget: 1,
        time_budget: None,
        scope: None,
    };
    assert!(matches!(r.search(&req), Err(Error::ResourceBudget)));
    // Skipping the folder range advances a file-only cursor even with this tiny budget.
    req.kind = "file".into();
    let p = r.search(&req).unwrap();
    assert!(p.budget_hit);
    assert!(p.next_cursor.unwrap().last_key > i64::MIN);
    req.kind = "any".into();
    req.text = "synthetic".into();
    req.work_budget = 1_000_000;
    let p = r.search(&req).unwrap();
    assert_eq!(p.hits.len(), 10);
    assert!(!p.budget_hit);
    assert!(p.next_cursor.is_some());
    req.cursor = p.next_cursor;
    let p = r.search(&req).unwrap();
    assert_eq!(p.hits[0].row.name, "synthetic-010.txt");
    let b = r
        .breakdown(root, "extension", Basis::Allocated, 10, 1)
        .unwrap();
    assert!(!b.complete);
    assert!(r
        .children(&ChildrenRequest {
            dir_id: root,
            sort: Sort::SizeDesc,
            basis: Basis::Logical,
            limit: 2,
            cursor: None
        })
        .is_ok());
}

#[test]
fn live_overlay_requires_one_complete_arena_snapshot() {
    let (_tmp, c, grant, root, _) = setup();
    let w = c.writer();
    let mut r = c.reader().unwrap();
    let WriteReply::Run(run) = w
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
    w.call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: root,
        files: vec![],
        dirs: vec![
            Observation::file("a", 0, None),
            Observation::file("b", 0, None),
        ],
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }))
    .unwrap();
    let req = SliceRequest {
        anchor: NodeKey::Dir(root),
        depth: 1,
        max_nodes: 16,
        min_share: 0.0,
        basis: Basis::Logical,
        include_files: true,
    };
    assert!(matches!(r.slice(&req), Err(Error::RepairRequired)));
    let overlay = std::collections::HashMap::from([
        (
            root,
            Totals {
                files: 1,
                dirs: 2,
                logical: 100,
                allocated: 100,
                ..Totals::default()
            },
        ),
        (
            root + 1,
            Totals {
                files: 1,
                logical: 100,
                allocated: 100,
                complete: true,
                ..Totals::default()
            },
        ),
        (root + 2, Totals::default()),
    ]);
    let slice = r.slice_with_overlay(&req, &overlay).unwrap();
    assert_eq!(slice.ordering, "approximate_live");
    assert_eq!(slice.aggregate_state, "provisional_live");
    assert!(slice.live);
    assert!(!slice.complete);
    assert_eq!(slice.nodes[0].logical_bytes, "100");
    validate("TreeSlice", serde_json::to_value(&slice).unwrap());
}

#[test]
fn atlas_volume_and_depth_bounds_preserve_preorder_and_all_root_generations() {
    let (_tmp, c, _, root, _) = setup();
    let mut r = c.reader().unwrap();
    for anchor in [NodeKey::Atlas, NodeKey::Volume(1), NodeKey::Dir(root)] {
        let slice = r
            .slice(&SliceRequest {
                anchor,
                depth: 2,
                max_nodes: 100,
                min_share: 0.1,
                basis: Basis::Logical,
                include_files: false,
            })
            .unwrap();
        validate("TreeSlice", serde_json::to_value(&slice).unwrap());
        assert_eq!(slice.root_generations.len(), 1);
        assert!(slice
            .nodes
            .iter()
            .enumerate()
            .all(|(i, n)| n.parent.is_none_or(|p| p < i)));
        assert!(slice.nodes.iter().all(|n| n.depth <= 2));
    }
}

#[test]
fn complete_breakdown_cache_is_generation_bound() {
    let (_tmp, c, grant, root, _) = setup();
    let mut r = c.reader().unwrap();
    let first = r
        .breakdown(root, "extension", Basis::Logical, 10, 1_000_000)
        .unwrap();
    assert!(first.complete);
    let cached = r
        .breakdown(root, "extension", Basis::Logical, 10, 1)
        .unwrap();
    assert!(cached.complete);
    assert_eq!(cached.buckets[0].bytes, first.buckets[0].bytes);
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
    c.writer()
        .call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id: root,
            files: vec![Observation::file("new", 1, Some(1))],
            dirs: vec![],
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        }))
        .unwrap();
    assert!(matches!(
        r.breakdown(root, "extension", Basis::Logical, 10, 1),
        Err(Error::RepairRequired)
    ));
    c.writer()
        .call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 3,
        })
        .unwrap();
    let updated = r
        .breakdown(root, "extension", Basis::Logical, 10, 1_000_000)
        .unwrap();
    assert_eq!(updated.buckets[0].bytes, "1");
}
#[test]
fn sql_rollup_matches_brute_force_for_random_trees() {
    for seed in 1..=8_u64 {
        let (tmp, c, grant, root, _) = setup();
        let w = c.writer();
        let db = Connection::open(tmp.path().join("catalog.db")).unwrap();
        let WriteReply::Run(run) = w
            .call(WriteCommand::BeginRun {
                grant_id: grant,
                mode: "refresh".into(),
                strategy: "fixture".into(),
                started_at_ns: 3,
            })
            .unwrap()
        else {
            panic!()
        };
        let mut rng = seed;
        let mut ids = vec![root];
        let mut expected = vec![(0_u64, 0_u64, 0_u64, 0_u64)];
        let mut parents = vec![None];
        for i in 0..20 {
            let parent = if i == 0 {
                None
            } else {
                Some((rng as usize) % i)
            };
            if let Some(p) = parent {
                parents.push(Some(p));
            } else {
                parents[0] = None;
            }
            if i > 0 {
                let p = parent.unwrap();
                let mut o = Observation::file(format!("dir-{i}"), 0, None);
                o.file_id = Some((i as u128).to_le_bytes().to_vec());
                let existing: Vec<String> = {
                    let mut s = db
                        .prepare("SELECT name FROM dir WHERE parent_id=?1")
                        .unwrap();
                    let rows = s
                        .query_map([ids[p]], |r| r.get(0))
                        .unwrap()
                        .map(|v| v.unwrap())
                        .collect();
                    rows
                };
                let dirs = existing
                    .into_iter()
                    .map(|name| {
                        let j: usize = name[4..].parse().unwrap();
                        let mut d = Observation::file(name, 0, None);
                        d.file_id = Some((j as u128).to_le_bytes().to_vec());
                        d
                    })
                    .chain(std::iter::once(o))
                    .collect();
                w.call(WriteCommand::DirListing(DirListing {
                    run_id: run,
                    dir_id: ids[p],
                    files: vec![],
                    dirs,
                    state: "complete".into(),
                    skipped: 0,
                    errors: 0,
                }))
                .unwrap();
                ids.push(
                    db.query_row(
                        "SELECT id FROM dir WHERE file_id=?1",
                        [&(i as u128).to_le_bytes()[..]],
                        |r| r.get(0),
                    )
                    .unwrap(),
                );
                expected.push((0, 0, 0, 0));
            }
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        }
        for i in 0..20 {
            let mut files = Vec::new();
            for j in 0..(rng % 9 + 1) {
                rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                let size = rng % 10000;
                let allocated = if rng % 3 == 0 { None } else { Some(size / 2) };
                expected[i].0 += 1;
                expected[i].1 += size;
                expected[i].2 += allocated.unwrap_or(0);
                expected[i].3 += u64::from(allocated.is_none());
                files.push(Observation::file(format!("f-{j}"), size, allocated));
            }
            let dirs = {
                let mut s = db
                    .prepare("SELECT name,file_id FROM dir WHERE parent_id=?1")
                    .unwrap();
                let rows = s
                    .query_map([ids[i]], |r| {
                        let mut o = Observation::file(r.get::<_, String>(0)?, 0, None);
                        o.file_id = r.get::<_, Option<Vec<u8>>>(1)?;
                        Ok(o)
                    })
                    .unwrap()
                    .map(|v| v.unwrap())
                    .collect();
                rows
            };
            w.call(WriteCommand::DirListing(DirListing {
                run_id: run,
                dir_id: ids[i],
                files,
                dirs,
                state: "complete".into(),
                skipped: 0,
                errors: 0,
            }))
            .unwrap();
        }
        w.call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 4,
        })
        .unwrap();
        for i in (1..20).rev() {
            let p = parents[i].unwrap();
            let child = expected[i];
            expected[p].0 += child.0;
            expected[p].1 += child.1;
            expected[p].2 += child.2;
            expected[p].3 += child.3;
        }
        for (id, expected) in ids.iter().zip(expected) {
            let got=db.query_row("SELECT sub_files,sub_logical,sub_allocated,sub_alloc_unknown FROM dir WHERE id=?1",[id],|r|Ok((r.get::<_,i64>(0)? as u64,r.get::<_,i64>(1)? as u64,r.get::<_,i64>(2)? as u64,r.get::<_,i64>(3)? as u64))).unwrap();
            assert_eq!(got, expected, "seed {seed}");
        }
    }
}
#[test]
fn allocated_keysets_put_unknowns_after_known_zero_and_use_both_indexes() {
    let (_tmp, c, grant, root, _) = setup();
    let w = c.writer();
    let WriteReply::Run(run) = w
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
    w.call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: root,
        files: vec![
            Observation::file("unknown", 1000, None),
            Observation::file("sparse", 10000, Some(0)),
            Observation::file("allocated", 100, Some(4096)),
            Observation::file("zero", 0, Some(0)),
        ],
        dirs: vec![],
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }))
    .unwrap();
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 3,
    })
    .unwrap();
    let mut reader = c.reader().unwrap();
    let mut req = ChildrenRequest {
        dir_id: root,
        sort: Sort::SizeDesc,
        basis: Basis::Allocated,
        limit: 1,
        cursor: None,
    };
    let mut names = Vec::new();
    loop {
        let page = reader.children(&req).unwrap();
        names.extend(page.items.iter().map(|r| r.name.clone()));
        req.cursor = page.next_cursor;
        if req.cursor.is_none() {
            break;
        }
    }
    assert_eq!(names, ["allocated", "sparse", "zero", "unknown"]);
    for (table, index, key, parent) in [
        ("file", "file_by_allocated", "allocated", "dir_id"),
        ("dir", "dir_by_allocated", "sub_allocated", "parent_id"),
    ] {
        let db = Connection::open(_tmp.path().join("catalog.db")).unwrap();
        let mut s=db.prepare(&format!("EXPLAIN QUERY PLAN SELECT id FROM {table} WHERE {parent}=?1 ORDER BY {key} DESC,id LIMIT 200")).unwrap();
        let plan = s
            .query_map([root], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join(" ");
        assert!(plan.contains(index), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    }
}

#[test]
fn name_and_modified_sorts_refuse_over_10000_direct_children() {
    let (_tmp, c, grant, root, _) = setup();
    let w = c.writer();
    let WriteReply::Run(run) = w
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
    w.call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: root,
        files: (0..10001)
            .map(|i| Observation::file(format!("f-{i}"), 1, Some(1)))
            .collect(),
        dirs: vec![],
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }))
    .unwrap();
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 3,
    })
    .unwrap();
    let mut reader = c.reader().unwrap();
    for sort in [Sort::NameAsc, Sort::ModifiedDesc] {
        assert!(matches!(
            reader.children(&ChildrenRequest {
                dir_id: root,
                sort,
                basis: Basis::Logical,
                limit: 200,
                cursor: None
            }),
            Err(Error::Invalid("sort exceeds 10000 children"))
        ));
    }
    assert_eq!(
        reader
            .children(&ChildrenRequest {
                dir_id: root,
                sort: Sort::SizeDesc,
                basis: Basis::Logical,
                limit: 200,
                cursor: None
            })
            .unwrap()
            .items
            .len(),
        200
    );
}

#[test]
fn budgeted_absent_search_resumes_and_catalogue_instance_invalidates_cursors() {
    let (tmp, c, _, root, _) = setup();
    let mut reader = c.reader().unwrap();
    let mut req = SearchRequest {
        root_id: None,
        text: "absent".into(),
        extension: None,
        min_bytes: None,
        max_bytes: None,
        modified_after: None,
        modified_before: None,
        kind: "file".into(),
        limit: 10,
        cursor: None,
        work_budget: 1,
        time_budget: None,
        scope: None,
    };
    let first = reader.search(&req).unwrap();
    assert!(first.budget_hit);
    assert!(first.next_cursor.is_some());
    req.cursor = first.next_cursor;
    req.work_budget = 100000;
    let second = reader.search(&req).unwrap();
    assert!(!second.budget_hit);
    assert!(second.hits.is_empty());
    assert!(second.next_cursor.is_none());
    let mut page_req = ChildrenRequest {
        dir_id: root,
        sort: Sort::NameAsc,
        basis: Basis::Logical,
        limit: 1,
        cursor: None,
    };
    page_req.cursor = reader.children(&page_req).unwrap().next_cursor;
    let db = Connection::open(tmp.path().join("catalog.db")).unwrap();
    db.execute(
        "UPDATE meta SET value='synthetic-new-instance' WHERE key='instance_id'",
        [],
    )
    .unwrap();
    assert!(matches!(
        reader.children(&page_req),
        Err(Error::StaleGeneration)
    ));
}

#[test]
fn budgeted_search_resumes_after_the_last_returned_match() {
    let (_tmp, c, _, _, _) = setup();
    let mut reader = c.reader().unwrap();
    let mut req = SearchRequest {
        root_id: None,
        text: "synthetic".into(),
        extension: None,
        min_bytes: None,
        max_bytes: None,
        modified_after: None,
        modified_before: None,
        kind: "file".into(),
        limit: 100,
        cursor: None,
        work_budget: 1,
        time_budget: None,
        scope: None,
    };
    // Find a SQLite opcode budget that interrupts after matches inside the first batch.
    let first = (1..=200)
        .find_map(|budget| {
            req.work_budget = budget * 100;
            let page = reader.search(&req).unwrap();
            (page.budget_hit && !page.hits.is_empty()).then_some(page)
        })
        .expect("a budget must interrupt after returning matches");
    let mut names: Vec<_> = first.hits.into_iter().map(|h| h.row.name).collect();
    req.cursor = first.next_cursor;
    req.work_budget = 100_000;
    let rest = reader.search(&req).unwrap();
    assert!(!rest.budget_hit);
    assert!(rest.next_cursor.is_none());
    names.extend(rest.hits.into_iter().map(|h| h.row.name));
    assert_eq!(
        names,
        (0..60)
            .map(|i| format!("synthetic-{i:03}.txt"))
            .collect::<Vec<_>>()
    );
}

#[test]
fn unique_objects_are_separate_from_entry_bytes_and_not_relief() {
    let (_tmp, c, grant, root, _) = setup();
    let w = c.writer();
    let WriteReply::Run(run) = w
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
    let mut a = Observation::file("one", 100, Some(100));
    a.file_id = Some(vec![2; 16]);
    let mut b = a.clone();
    b.name = "link".into();
    w.call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: root,
        files: vec![a, b],
        dirs: vec![],
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }))
    .unwrap();
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 3,
    })
    .unwrap();
    let detail = c.reader().unwrap().inspect(NodeKey::Dir(root)).unwrap();
    let sums = detail.subtree.as_ref().unwrap();
    assert_eq!(sums.logical_bytes, "200");
    assert_eq!(sums.unique_objects, Some(1));
    assert_eq!(sums.unique_allocated_bytes.as_deref(), Some("100"));
    assert_eq!(sums.multi_link_entries, Some(2));
    assert_eq!(sums.stream_coverage, "default_stream_only");
    validate("NodeDetail", serde_json::to_value(detail).unwrap());
}
#[test]
fn live_other_clamps_are_counted_and_committed_other_never_clamps() {
    let (tmp, c, grant, root, _) = setup();
    let mut reader = c.reader().unwrap();
    let req = SliceRequest {
        anchor: NodeKey::Dir(root),
        depth: 1,
        max_nodes: 16,
        min_share: 0.0,
        basis: Basis::Logical,
        include_files: true,
    };
    let db = Connection::open(tmp.path().join("catalog.db")).unwrap();
    db.execute("UPDATE dir SET sub_logical=0 WHERE id=?1", [root])
        .unwrap();
    assert!(matches!(
        reader.slice(&req),
        Err(Error::Invalid("inconsistent committed aggregate"))
    ));
    let WriteReply::Run(_run) = c
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
    let overlay = std::collections::HashMap::from([(root, Totals::default())]);
    let slice = reader.slice_with_overlay(&req, &overlay).unwrap();
    assert_eq!(slice.aggregate_state, "provisional_live");
    assert!(reader.live_clamp_count > 0);
}

#[test]
fn atlas_arithmetic_is_checked_and_unscanned_generations_are_null() {
    let temp = tempfile::tempdir().unwrap();
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    let mut reader = c.reader().unwrap();
    let req = SliceRequest {
        anchor: NodeKey::Atlas,
        depth: 1,
        max_nodes: 16,
        min_share: 0.0,
        basis: Basis::Logical,
        include_files: false,
    };
    for i in 0..3 {
        let WriteReply::Root {
            grant_id, dir_id, ..
        } = c
            .writer()
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: format!("fixture-{i}"),
                display_name: format!("Fixture-{i}"),
                display_path: "Fixture root".into(),
                root_file_id: Some(vec![i; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap()
        else {
            panic!()
        };
        if i == 0 {
            let slice = reader.slice(&req).unwrap();
            assert!(slice.root_generations[0].generation.is_none());
        }
        let WriteReply::Run(run) = c
            .writer()
            .call(WriteCommand::BeginRun {
                grant_id,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 1,
            })
            .unwrap()
        else {
            panic!()
        };
        c.writer()
            .call(WriteCommand::DirListing(DirListing {
                run_id: run,
                dir_id,
                files: vec![Observation::file(
                    "large",
                    i64::MAX as u64,
                    Some(i64::MAX as u64),
                )],
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
                finished_at_ns: 2,
            })
            .unwrap();
        if i == 1 {
            assert_eq!(
                reader.slice(&req).unwrap().nodes[0].logical_bytes,
                (2 * (i64::MAX as u64)).to_string()
            );
        }
    }
    assert!(matches!(
        reader.slice(&req),
        Err(Error::Invalid("aggregate byte overflow"))
    ));
}

#[test]
fn search_sql_error_survives_exhausted_budget() {
    let (tmp, c, grant, _, _) = setup();
    let mut reader = c.reader().unwrap();
    let req = SearchRequest {
        root_id: Some(grant),
        text: "synthetic".into(),
        extension: None,
        min_bytes: None,
        max_bytes: None,
        modified_after: None,
        modified_before: None,
        kind: "file".into(),
        limit: 10,
        cursor: None,
        work_budget: 1,
        time_budget: None,
        scope: None,
    };
    let healthy = reader.search(&req).unwrap();
    assert!(healthy.budget_hit, "tiny budget must exhaust");
    let db = Connection::open(tmp.path().join("catalog.db")).unwrap();
    db.execute_batch("ALTER TABLE file RENAME TO file_missing;")
        .unwrap();
    drop(db);
    let res = reader.search(&req);
    assert!(
        matches!(res, Err(Error::Sql(_))),
        "SQL error must not hide as budget cut: {res:?}"
    );
}

#[test]
fn breakdown_sql_error_survives_exhausted_budget() {
    let (tmp, c, _, root, _) = setup();
    let mut reader = c.reader().unwrap();
    let healthy = reader
        .breakdown(root, "extension", Basis::Allocated, 10, 1)
        .unwrap();
    assert!(!healthy.complete, "tiny budget must hit");
    let db = Connection::open(tmp.path().join("catalog.db")).unwrap();
    db.execute_batch("ALTER TABLE file RENAME TO file_missing;")
        .unwrap();
    drop(db);
    let res = reader.breakdown(root, "extension", Basis::Allocated, 10, 1);
    assert!(
        matches!(res, Err(Error::Sql(_))),
        "SQL error must not hide as budget cut: {res:?}"
    );
}

#[test]
fn pure_budget_interrupt_still_reports_budget_cut() {
    let (_tmp, c, grant, root, _) = setup();
    let mut reader = c.reader().unwrap();
    let page = reader.search(&SearchRequest {
        root_id: Some(grant),
        text: "no match".into(),
        extension: None,
        min_bytes: None,
        max_bytes: None,
        modified_after: None,
        modified_before: None,
        kind: "any".into(),
        limit: 10,
        cursor: None,
        work_budget: 1,
        time_budget: None,
        scope: None,
    });
    assert!(matches!(page, Err(Error::ResourceBudget)));
    let b = reader
        .breakdown(root, "extension", Basis::Allocated, 10, 1)
        .unwrap();
    assert!(!b.complete);
}
