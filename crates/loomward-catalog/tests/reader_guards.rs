use loomward_catalog::*;

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

fn valid_children(dir_id: i64, limit: usize) -> ChildrenRequest {
    ChildrenRequest {
        dir_id,
        sort: Sort::SizeDesc,
        basis: Basis::Logical,
        limit,
        cursor: None,
    }
}

fn valid_slice(anchor: NodeKey) -> SliceRequest {
    SliceRequest {
        anchor,
        depth: 1,
        max_nodes: 100,
        min_share: 0.0,
        basis: Basis::Logical,
        include_files: true,
    }
}

#[test]
fn children_refuses_page_limit_above_maximum() {
    let (_tmp, c, _, root, _) = setup();
    let mut r = c.reader().unwrap();
    let err = r.children(&valid_children(root, 201)).unwrap_err();
    assert!(
        matches!(err, Error::Invalid("page limit")),
        "limit 201 must be Invalid(\"page limit\"): {err:?}"
    );
    let err = r.children(&valid_children(root, 0)).unwrap_err();
    assert!(
        matches!(err, Error::Invalid("page limit")),
        "limit 0 must be Invalid(\"page limit\"): {err:?}"
    );
    let first = r.children(&valid_children(root, 1)).unwrap();
    assert_eq!(first.items.len(), 1);
    assert!(first.next_cursor.is_some());
    let full = r.children(&valid_children(root, 200)).unwrap();
    assert_eq!(full.items.len(), 60);
    assert!(full.next_cursor.is_none());
}

#[test]
fn children_refuses_cursor_bound_to_a_different_dir() {
    let (_tmp, c, _, root, _) = setup();
    let mut r = c.reader().unwrap();
    let page = r.children(&valid_children(root, 5)).unwrap();
    let cursor = page.next_cursor.unwrap();
    let mut rebound = cursor.clone();
    rebound.dir_id = root + 9999;
    let req = ChildrenRequest {
        dir_id: root,
        sort: cursor.sort,
        basis: cursor.basis,
        limit: 5,
        cursor: Some(rebound),
    };
    let err = r.children(&req).unwrap_err();
    assert!(
        matches!(err, Error::Invalid("cursor binding")),
        "cursor for another dir must be Invalid(\"cursor binding\"): {err:?}"
    );
}

#[test]
fn children_refuses_cursor_bound_to_a_different_sort_or_basis() {
    let (_tmp, c, _, root, _) = setup();
    let mut r = c.reader().unwrap();
    let page = r.children(&valid_children(root, 5)).unwrap();
    let cursor = page.next_cursor.unwrap();
    let mut wrong_sort = cursor.clone();
    wrong_sort.sort = Sort::NameAsc;
    let err = r
        .children(&ChildrenRequest {
            dir_id: root,
            sort: Sort::SizeDesc,
            basis: Basis::Logical,
            limit: 5,
            cursor: Some(wrong_sort),
        })
        .unwrap_err();
    assert!(
        matches!(err, Error::Invalid("cursor binding")),
        "cursor for another sort must be Invalid(\"cursor binding\"): {err:?}"
    );
    let mut wrong_basis = cursor.clone();
    wrong_basis.basis = Basis::Allocated;
    let err = r
        .children(&ChildrenRequest {
            dir_id: root,
            sort: Sort::SizeDesc,
            basis: Basis::Logical,
            limit: 5,
            cursor: Some(wrong_basis),
        })
        .unwrap_err();
    assert!(
        matches!(err, Error::Invalid("cursor binding")),
        "cursor for another basis must be Invalid(\"cursor binding\"): {err:?}"
    );
}

#[test]
fn children_answers_repair_required_when_aggregate_is_invalid() {
    let (_tmp, c, grant_id, root, _) = setup();
    let mut r = c.reader().unwrap();
    let WriteReply::Run(run) = c
        .writer()
        .call(WriteCommand::BeginRun {
            grant_id,
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
    let err = r.children(&valid_children(root, 5)).unwrap_err();
    assert!(
        matches!(err, Error::RepairRequired),
        "dirty aggregate without a cursor must be RepairRequired: {err:?}"
    );
}

#[test]
fn slice_refuses_depth_outside_1_to_8() {
    let (_tmp, c, _, root, _) = setup();
    let mut r = c.reader().unwrap();
    for depth in [0, 9] {
        let mut req = valid_slice(NodeKey::Dir(root));
        req.depth = depth;
        let err = r.slice(&req).unwrap_err();
        assert!(
            matches!(err, Error::Invalid("slice bounds")),
            "depth {depth} must be Invalid(\"slice bounds\"): {err:?}"
        );
    }
}

#[test]
fn slice_refuses_max_nodes_outside_16_to_6000() {
    let (_tmp, c, _, root, _) = setup();
    let mut r = c.reader().unwrap();
    for max_nodes in [15, 6001] {
        let mut req = valid_slice(NodeKey::Dir(root));
        req.max_nodes = max_nodes;
        let err = r.slice(&req).unwrap_err();
        assert!(
            matches!(err, Error::Invalid("slice bounds")),
            "max_nodes {max_nodes} must be Invalid(\"slice bounds\"): {err:?}"
        );
    }
}

#[test]
fn slice_refuses_min_share_outside_0_to_01_including_nan() {
    let (_tmp, c, _, root, _) = setup();
    let mut r = c.reader().unwrap();
    for min_share in [-0.01, 0.11, f64::NAN] {
        let mut req = valid_slice(NodeKey::Dir(root));
        req.min_share = min_share;
        let err = r.slice(&req).unwrap_err();
        assert!(
            matches!(err, Error::Invalid("slice bounds")),
            "min_share {min_share:?} must be Invalid(\"slice bounds\"): {err:?}"
        );
    }
}

#[test]
fn slice_accepts_boundary_depth_max_nodes_and_min_share() {
    let (_tmp, c, _, root, _) = setup();
    let mut r = c.reader().unwrap();
    let expected_anchor = NodeKey::Dir(root).reference();
    for (depth, max_nodes, min_share) in [
        (1_usize, 16_usize, 0.0_f64),
        (8_usize, 16_usize, 0.0_f64),
        (1_usize, 6000_usize, 0.0_f64),
        (1_usize, 16_usize, 0.1_f64),
    ] {
        let slice = r
            .slice(&SliceRequest {
                anchor: NodeKey::Dir(root),
                depth,
                max_nodes,
                min_share,
                basis: Basis::Logical,
                include_files: true,
            })
            .unwrap();
        assert_eq!(slice.anchor_node_id, expected_anchor);
        assert_eq!(slice.nodes[0].depth, 0);
        assert!(
            slice.nodes.len() <= max_nodes,
            "depth {depth} max_nodes {max_nodes} min_share {min_share}: {} nodes",
            slice.nodes.len()
        );
    }
}
