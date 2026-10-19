//! #715: the unique-totals work guard must never fail a whole view; over budget, the unique
//! totals are unknown (invariant 4) and every other aggregate stays exact.
use loomward_catalog::*;

fn big_root(n: usize) -> (tempfile::TempDir, Catalog, i64) {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(tmp.path(), "synthetic").unwrap();
    let w = catalog.writer();
    let WriteReply::Root {
        grant_id, dir_id, ..
    } = w
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
    let WriteReply::Run(run) = w
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
    const PER_DIR: usize = 1000;
    let dirs = n.div_ceil(PER_DIR);
    let mut observed = Vec::new();
    for d in 0..dirs {
        let mut o = Observation::file(format!("d{d}"), 0, Some(0));
        o.attrs = 0x10;
        o.file_id = Some(
            vec![0xD0 + 1; 15]
                .into_iter()
                .chain([d as u8 + 1])
                .collect(),
        );
        o.created_ft = Some(1);
        observed.push(o);
    }
    w.call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id,
        files: vec![],
        dirs: observed,
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }))
    .unwrap();
    let db = rusqlite::Connection::open(tmp.path().join("catalog.db")).unwrap();
    let ids: Vec<i64> = db
        .prepare("SELECT id FROM dir WHERE parent_id=?1 ORDER BY id")
        .unwrap()
        .query_map([dir_id], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, rusqlite::Error>>()
        .unwrap();
    for (d, child) in ids.into_iter().enumerate() {
        let files = (0..PER_DIR.min(n - d * PER_DIR))
            .map(|i| {
                let mut o = Observation::file(format!("f{i}.bin"), 10, Some(4096));
                let mut id = [0u8; 16];
                id[..8].copy_from_slice(&((d * PER_DIR + i) as u64 + 1).to_le_bytes());
                o.file_id = Some(id.to_vec());
                o
            })
            .collect();
        w.call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id: child,
            files,
            dirs: vec![],
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        }))
        .unwrap();
    }
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 1,
    })
    .unwrap();
    (tmp, catalog, dir_id)
}

/// Real-machine shape (#715): 130k files, five times the old 2M-step guard's reach, stay exact.
#[test]
fn ordinary_large_root_has_exact_unique_totals() {
    let (_t, c, dir) = big_root(130_000);
    let totals = c
        .reader()
        .unwrap()
        .inspect(NodeKey::Dir(dir))
        .unwrap()
        .subtree
        .unwrap();
    assert_eq!(totals.files, 130_000);
    assert_eq!(totals.unique_objects, Some(130_000));
    assert_eq!(totals.unique_allocated_bytes.as_deref(), Some("532480000"));
    assert_eq!(totals.multi_link_entries, Some(0));
}

/// Over budget, the unique totals are unknown (never zero, never an error) and every stored
/// aggregate stays exact.
#[test]
fn over_budget_unique_totals_are_unknown_and_the_rest_stays_exact() {
    let (_t, c, dir) = big_root(3_000);
    let mut reader = c.reader().unwrap();
    reader.unique_totals_steps = 10_000;
    let totals = reader.inspect(NodeKey::Dir(dir)).unwrap().subtree.unwrap();
    assert_eq!(totals.files, 3_000);
    assert_eq!(totals.logical_bytes, "30000");
    assert!(totals.complete);
    assert_eq!(
        (
            totals.unique_objects,
            totals.unique_allocated_bytes,
            totals.multi_link_entries
        ),
        (None, None, None)
    );
    // The same reader recovers once the budget allows the work; the interrupt left no residue.
    reader.unique_totals_steps = 32_000_000;
    let totals = reader.inspect(NodeKey::Dir(dir)).unwrap().subtree.unwrap();
    assert_eq!(totals.unique_objects, Some(3_000));
}

#[test]
fn duplicate_candidates_on_a_large_root_answer() {
    let (_t, c, dir) = big_root(130_000);
    let root = c
        .reader()
        .unwrap()
        .inspect(NodeKey::Dir(dir))
        .unwrap()
        .root_id;
    let id: i64 = root.trim_start_matches("rt_").parse().unwrap();
    let found = c
        .reader()
        .unwrap()
        .duplicate_candidates(id, 1, 200)
        .unwrap();
    assert_eq!(
        (found.entries.len(), found.truncated, found.partial),
        (200, true, false)
    );
}
