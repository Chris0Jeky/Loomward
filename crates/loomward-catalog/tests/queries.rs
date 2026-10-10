use loomward_catalog::*;
use rusqlite::Connection;

fn setup() -> (tempfile::TempDir, Catalog, i64, i64, i64) {
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
        kind: "any".into(),
        limit: 10,
        cursor: None,
        work_budget: 1,
    };
    let p = r.search(&req).unwrap();
    assert!(p.budget_hit);
    assert!(p.total.is_none());
    req.text = "synthetic".into();
    req.work_budget = 1_000_000;
    let p = r.search(&req).unwrap();
    assert_eq!(p.items.len(), 10);
    assert!(!p.budget_hit);
    assert!(p.next_cursor.is_some());
    req.cursor = p.next_cursor;
    let p = r.search(&req).unwrap();
    assert_eq!(p.items[0].name, "synthetic-010.txt");
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
        kind: "file".into(),
        limit: 10,
        cursor: None,
        work_budget: 1,
    };
    let first = reader.search(&req).unwrap();
    assert!(first.budget_hit);
    assert!(first.next_cursor.is_some());
    req.cursor = first.next_cursor;
    req.work_budget = 100000;
    let second = reader.search(&req).unwrap();
    assert!(!second.budget_hit);
    assert!(second.items.is_empty());
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
    assert_eq!(sums.multi_link_entries, 2);
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
