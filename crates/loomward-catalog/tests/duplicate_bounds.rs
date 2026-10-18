use loomward_catalog::*;
use std::collections::HashMap;

// Pinning test for `duplicate_candidates` bounds and truncation (see
// crates/loomward-catalog/src/duplicates.rs): cap outside 2..=200 and a floor
// above i64::MAX are rejected, cap+1 probing sets `truncated`, and a cap that
// strands a lone group member prunes it instead of reporting a singleton.

fn setup_with_files(files: Vec<Observation>) -> (tempfile::TempDir, Catalog, i64) {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(tmp.path(), "synthetic").unwrap();
    let WriteReply::Root {
        grant_id,
        dir_id,
        root_id,
    } = catalog
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
    let WriteReply::Run(run) = catalog
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
    catalog
        .writer()
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
    catalog
        .writer()
        .call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 1,
        })
        .unwrap();
    (tmp, catalog, root_id)
}

#[test]
fn duplicate_candidates_rejects_bad_cap_and_prunes_singletons() {
    // Two-file same-size fixture: one duplicate pair, nothing else.
    let (_tmp, catalog, root) = setup_with_files(vec![
        Observation::file("pair-a", 4096, Some(4096)),
        Observation::file("pair-b", 4096, Some(4096)),
    ]);
    let mut reader = catalog.reader().unwrap();

    // Bounds: cap is 2..=200, floor fits in i64.
    assert!(matches!(
        reader.duplicate_candidates(root, 1, 1),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        reader.duplicate_candidates(root, 1, 201),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        reader.duplicate_candidates(root, i64::MAX as u64 + 1, 2),
        Err(Error::Invalid(_))
    ));

    // A fitting cap reports the pair untruncated.
    let report = reader.duplicate_candidates(root, 1, 2).unwrap();
    assert!(!report.truncated);
    assert_eq!(report.entries.len(), 2);
    assert_eq!(report.entries[0].logical, report.entries[1].logical);
    // The top of the valid cap range also accepts the pair.
    assert_eq!(
        reader
            .duplicate_candidates(root, 1, 200)
            .unwrap()
            .entries
            .len(),
        2
    );

    // Two pairs: sizes ordered 4096 then 8192, so cap=3 strands one 8192 member.
    let (_tmp, catalog, root) = setup_with_files(vec![
        Observation::file("pair-a", 4096, Some(4096)),
        Observation::file("pair-b", 4096, Some(4096)),
        Observation::file("quad-a", 8192, Some(8192)),
        Observation::file("quad-b", 8192, Some(8192)),
    ]);
    let mut reader = catalog.reader().unwrap();
    let full = reader.duplicate_candidates(root, 1, 4).unwrap();
    assert!(!full.truncated);
    assert_eq!(full.entries.len(), 4);

    let cut = reader.duplicate_candidates(root, 1, 3).unwrap();
    assert!(cut.truncated, "the cap+1 probe row must set truncated");
    // The stranded 8192 member is pruned: no one-name group survives.
    assert_eq!(cut.entries.len(), 2);
    let mut counts: HashMap<u64, usize> = HashMap::new();
    for e in &cut.entries {
        *counts.entry(e.logical).or_insert(0) += 1;
    }
    assert!(counts.values().all(|&n| n >= 2));
    assert!(counts.values().any(|&n| n == 2));
}
