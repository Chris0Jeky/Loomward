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

#[test]
fn candidate_space_counts_the_whole_eligible_same_size_population_beyond_the_cap() {
    // 150 pairs of equal-size files: 300 candidates against a cap of 200 entries.
    let mut files: Vec<Observation> = (0..150u64)
        .flat_map(|i| {
            (0..2).map(move |n| Observation::file(format!("p{i:03}-{n}"), 1000 + i, Some(4096)))
        })
        .collect();
    // Ineligible: a unique size, empty files, and a cloud placeholder that would otherwise
    // make a third member of the 1000-byte group.
    files.push(Observation::file("lone", 77, Some(4096)));
    files.push(Observation::file("empty-a", 0, Some(0)));
    files.push(Observation::file("empty-b", 0, Some(0)));
    let mut placeholder = Observation::file("cloud", 1000, Some(0));
    placeholder.flags = 1 << 3;
    files.push(placeholder);
    let (_tmp, catalog, root) = setup_with_files(files);
    let mut reader = catalog.reader().unwrap();

    let all = reader.duplicate_candidates(root, 1, 200).unwrap();
    assert!(all.truncated);
    assert_eq!(all.entries.len(), 200);
    let space = all.space.expect("a small root counts within budget");
    assert_eq!(space.files, 300);
    assert_eq!(
        space.bytes,
        (0..150u64).map(|i| 2 * (1000 + i)).sum::<u64>()
    );

    // The floor applies as in selection: sizes 1100..1149 leave 50 pairs.
    let floored = reader.duplicate_candidates(root, 1100, 200).unwrap();
    assert_eq!(floored.space.unwrap().files, 100);
    assert_eq!(floored.entries.len(), 100);

    // Nothing shares a size above every file: a known zero, distinct from unknown.
    let none = reader.duplicate_candidates(root, 5000, 2).unwrap();
    assert_eq!(none.space.map(|s| (s.files, s.bytes)), Some((0, 0)));
}
