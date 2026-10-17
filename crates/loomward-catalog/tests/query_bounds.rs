use loomward_catalog::*;
use std::time::{SystemTime, UNIX_EPOCH};

// Fixture helpers mirror crates/loomward-catalog/tests/queries.rs: a synthetic
// root plus one complete DirListing. Synthetic names only.

fn setup_with_files(files: Vec<Observation>) -> (tempfile::TempDir, Catalog, i64, i64) {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(tmp.path(), "synthetic").unwrap();
    let WriteReply::Root {
        grant_id, dir_id, ..
    } = catalog
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
    (tmp, catalog, grant_id, dir_id)
}

fn now_filetime() -> i64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    (nanos / 100) as i64 + 116_444_736_000_000_000
}

fn search_req(grant: i64) -> SearchRequest {
    SearchRequest {
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
        work_budget: 1_000_000,
        time_budget: None,
        scope: None,
    }
}

// Kills removal or widening of the `1..=100` limit disjunct.
#[test]
fn search_limit_zero_and_101_rejected_while_1_and_100_pass() {
    let (_tmp, catalog, grant, _root) = setup_with_files(vec![
        Observation::file("synthetic-000", 10, Some(10)),
        Observation::file("synthetic-001", 20, Some(20)),
        Observation::file("synthetic-002", 30, Some(30)),
    ]);
    let mut reader = catalog.reader().unwrap();
    for limit in [0, 101] {
        let mut req = search_req(grant);
        req.limit = limit;
        assert!(
            matches!(reader.search(&req), Err(Error::Invalid("search bounds"))),
            "limit {limit} must be rejected"
        );
    }
    let mut req = search_req(grant);
    req.limit = 1;
    let page = reader.search(&req).unwrap();
    assert_eq!(page.hits.len(), 1);
    assert!(page.next_cursor.is_some());
    let mut req = search_req(grant);
    req.limit = 100;
    let page = reader.search(&req).unwrap();
    assert_eq!(page.hits.len(), 3);
    assert!(page.next_cursor.is_none());
}

// Kills removal of the text-length disjunct or a `> 256` to `>= 256` change.
#[test]
fn search_text_over_256_chars_rejected() {
    let (_tmp, catalog, grant, _root) =
        setup_with_files(vec![Observation::file("synthetic-000", 10, Some(10))]);
    let mut reader = catalog.reader().unwrap();
    let mut req = search_req(grant);
    req.text = "x".repeat(257);
    assert!(matches!(
        reader.search(&req),
        Err(Error::Invalid("search bounds"))
    ));
    let mut req = search_req(grant);
    req.text = "q".repeat(256);
    let page = reader.search(&req).unwrap();
    assert_eq!(page.hits.len(), 0);
    assert!(page.next_cursor.is_none());
}

// Kills removal of the extension-length disjunct or a `> 32` to `>= 32` change.
#[test]
fn search_extension_over_32_chars_rejected() {
    let (_tmp, catalog, grant, _root) =
        setup_with_files(vec![Observation::file("synthetic-000", 10, Some(10))]);
    let mut reader = catalog.reader().unwrap();
    let mut req = search_req(grant);
    req.extension = Some("y".repeat(33));
    assert!(matches!(
        reader.search(&req),
        Err(Error::Invalid("search bounds"))
    ));
    let mut req = search_req(grant);
    req.extension = Some("z".repeat(32));
    let page = reader.search(&req).unwrap();
    assert_eq!(page.hits.len(), 0);
    assert!(page.next_cursor.is_none());
}

// Kills removal of the kind allowlist or case-folding it.
#[test]
fn search_kind_outside_any_file_dir_rejected() {
    let (_tmp, catalog, grant, _root) =
        setup_with_files(vec![Observation::file("synthetic-000", 10, Some(10))]);
    let mut reader = catalog.reader().unwrap();
    for kind in ["folder", "", "FILE", "Any", "files"] {
        let mut req = search_req(grant);
        req.kind = kind.into();
        assert!(
            matches!(reader.search(&req), Err(Error::Invalid("search bounds"))),
            "kind {kind:?} must be rejected"
        );
    }
    for (kind, expected) in [("file", 1), ("any", 1), ("dir", 0)] {
        let mut req = search_req(grant);
        req.kind = kind.into();
        let page = reader.search(&req).unwrap();
        assert_eq!(page.hits.len(), expected, "kind {kind:?}");
    }
}

// Kills removal of the min_bytes overflow disjunct.
#[test]
fn search_min_bytes_above_i64_max_rejected() {
    let (_tmp, catalog, grant, _root) =
        setup_with_files(vec![Observation::file("synthetic-000", 10, Some(10))]);
    let mut reader = catalog.reader().unwrap();
    for min_bytes in [i64::MAX as u64 + 1, u64::MAX] {
        let mut req = search_req(grant);
        req.min_bytes = Some(min_bytes);
        assert!(
            matches!(reader.search(&req), Err(Error::Invalid("search bounds"))),
            "min_bytes {min_bytes} must be rejected"
        );
    }
    let mut req = search_req(grant);
    req.min_bytes = Some(i64::MAX as u64);
    let page = reader.search(&req).unwrap();
    assert_eq!(page.hits.len(), 0);
    assert!(page.next_cursor.is_none());
}

fn age_band_files(now_ft: i64) -> Vec<Observation> {
    const DAY: i64 = 86400 * 10_000_000;
    let mut files = Vec::new();
    for i in 0..4 {
        let mut o = Observation::file(format!("synthetic-recent-{i:02}"), 1000, Some(1000));
        o.modified_ft = Some(now_ft - 5 * DAY);
        files.push(o);
    }
    for i in 0..3 {
        let mut o = Observation::file(format!("synthetic-within-{i:02}"), 100, Some(100));
        o.modified_ft = Some(now_ft - 200 * DAY);
        files.push(o);
    }
    for i in 0..2 {
        let mut o = Observation::file(format!("synthetic-older-{i:02}"), 10, Some(10));
        o.modified_ft = Some(now_ft - 800 * DAY);
        files.push(o);
    }
    files.push(Observation::file("synthetic-unknown-00", 50, Some(50)));
    files
}

// Kills dropping the NULL-mtime arm (unknown collapses to 0, older gains a
// file) or swapping the 30d/365d thresholds (bucket counts move).
#[test]
fn breakdown_age_band_buckets_include_unknown_null_mtime() {
    let (_tmp, catalog, _grant, root) = setup_with_files(age_band_files(now_filetime()));
    let mut reader = catalog.reader().unwrap();
    let b = reader
        .breakdown(root, "age_band", Basis::Logical, 10, 1_000_000)
        .unwrap();
    assert_eq!(b.by, "age_band");
    assert_eq!(b.basis, Basis::Logical);
    assert!(b.complete);
    assert_eq!(b.buckets.len(), 3);
    assert_eq!(b.buckets[0].key, "recent_30d");
    assert_eq!(b.buckets[0].label, "recent_30d");
    assert_eq!(b.buckets[0].bytes, "4000");
    assert_eq!(b.buckets[0].files, 4);
    assert_eq!(b.buckets[1].key, "within_year");
    assert_eq!(b.buckets[1].bytes, "300");
    assert_eq!(b.buckets[1].files, 3);
    assert_eq!(b.buckets[2].key, "older");
    assert_eq!(b.buckets[2].bytes, "20");
    assert_eq!(b.buckets[2].files, 2);
    assert_eq!(b.unknown.bytes, "50");
    assert_eq!(b.unknown.files, 1);
    assert_eq!(b.other.bytes, "0");
    assert_eq!(b.other.files, 0);
}

fn family_files() -> Vec<Observation> {
    let mut files = Vec::new();
    for (count, ext, family, alloc) in [
        (2, "docext", "document", 1000u64),
        (2, "imgext", "image", 500u64),
        (2, "vidext", "video", 100u64),
        (2, "archext", "archive", 10u64),
    ] {
        for i in 0..count {
            let mut o = Observation::file(format!("synthetic-{family}-{i:02}"), alloc, Some(alloc));
            o.extension = Some(ext.into());
            o.family = family.into();
            files.push(o);
        }
    }
    for i in 0..3 {
        let mut o = Observation::file(format!("synthetic-unknown-{i:02}"), 700, None);
        o.extension = Some("docext".into());
        o.family = "document".into();
        files.push(o);
    }
    files
}

// Kills folding allocation-unknown rows into buckets/other (unknown.files
// drops to 0) or dropping the over-limit overflow (other.files drops to 0 and
// the file census no longer adds up to 11).
#[test]
fn breakdown_ext_family_keeps_unknown_out_of_other() {
    let (_tmp, catalog, _grant, root) = setup_with_files(family_files());
    let mut reader = catalog.reader().unwrap();
    let b = reader
        .breakdown(root, "ext_family", Basis::Allocated, 2, 1_000_000)
        .unwrap();
    assert_eq!(b.by, "ext_family");
    assert_eq!(b.basis, Basis::Allocated);
    assert!(b.complete);
    assert_eq!(b.buckets.len(), 2);
    assert_eq!(b.buckets[0].key, "document");
    assert_eq!(b.buckets[0].bytes, "2000");
    assert_eq!(b.buckets[0].files, 2);
    assert_eq!(b.buckets[1].key, "image");
    assert_eq!(b.buckets[1].bytes, "1000");
    assert_eq!(b.buckets[1].files, 2);
    assert_eq!(b.other.bytes, "220");
    assert_eq!(b.other.files, 4);
    assert_eq!(b.unknown.bytes, "0");
    assert_eq!(b.unknown.files, 3);
    let census: u64 =
        b.buckets.iter().map(|bucket| bucket.files).sum::<u64>() + b.other.files + b.unknown.files;
    assert_eq!(census, 11);
}

// Kills removal of the `by` allowlist or the `1..=64` limit disjunct; the
// limit-1 case also pins truncation of the smallest bucket into `other`.
#[test]
fn breakdown_by_and_limit_bounds_rejected() {
    let mut a = Observation::file("synthetic-a", 10, Some(10));
    a.extension = Some("docext".into());
    a.family = "document".into();
    let mut b = Observation::file("synthetic-b", 20, Some(20));
    b.extension = Some("imgext".into());
    b.family = "image".into();
    let (_tmp, catalog, _grant, root) = setup_with_files(vec![a, b]);
    let mut reader = catalog.reader().unwrap();
    for by in ["", "ext_families", "AGE_BAND", "age"] {
        assert!(
            matches!(
                reader.breakdown(root, by, Basis::Logical, 10, 1_000_000),
                Err(Error::Invalid("breakdown bounds"))
            ),
            "by {by:?} must be rejected"
        );
    }
    for limit in [0, 65] {
        assert!(
            matches!(
                reader.breakdown(root, "extension", Basis::Logical, limit, 1_000_000),
                Err(Error::Invalid("breakdown bounds"))
            ),
            "limit {limit} must be rejected"
        );
    }
    let b = reader
        .breakdown(root, "extension", Basis::Logical, 1, 1_000_000)
        .unwrap();
    assert_eq!(b.buckets.len(), 1);
    assert_eq!(b.buckets[0].key, "imgext");
    assert_eq!(b.buckets[0].bytes, "20");
    assert_eq!(b.other.files, 1);
    assert_eq!(b.other.bytes, "10");
    let wide = reader
        .breakdown(root, "extension", Basis::Logical, 64, 1_000_000)
        .unwrap();
    assert_eq!(wide.buckets.len(), 2);
}

fn paging_files() -> Vec<Observation> {
    // Inserted newest-name-first so id order and name order disagree:
    // ModifiedDesc ties must follow row ids, not names.
    let mut files = Vec::new();
    for i in (0..60).rev() {
        let mut o = Observation::file(format!("synthetic-{i:03}.txt"), 100, Some(100));
        o.modified_ft = match i % 3 {
            0 => Some(133_000_000_000_000_000),
            1 => Some(132_000_000_000_000_000),
            _ => None,
        };
        files.push(o);
    }
    files
}

fn collect_pages(
    reader: &mut Reader,
    dir_id: i64,
    sort: Sort,
    limit: usize,
) -> (Vec<String>, Vec<usize>) {
    let mut req = ChildrenRequest {
        dir_id,
        sort,
        basis: Basis::Logical,
        limit,
        cursor: None,
    };
    let mut names = Vec::new();
    let mut sizes = Vec::new();
    loop {
        let page = reader.children(&req).unwrap();
        assert!(page.items.len() <= limit);
        sizes.push(page.items.len());
        names.extend(page.items.iter().map(|row| row.name.clone()));
        match page.next_cursor {
            Some(cursor) => req.cursor = Some(cursor),
            None => break,
        }
    }
    (names, sizes)
}

// Kills inverting the NameAsc comparator (order assertion fails), dropping
// truncation (page sizes differ) or dropping the cursor tiebreak (rows lost).
#[test]
fn children_nameasc_pages_cover_60_without_loss_or_duplicates() {
    let (_tmp, catalog, _grant, root) = setup_with_files(paging_files());
    let mut reader = catalog.reader().unwrap();
    let (names, sizes) = collect_pages(&mut reader, root, Sort::NameAsc, 7);
    assert_eq!(sizes.len(), 9);
    assert_eq!(&sizes[..8], &[7, 7, 7, 7, 7, 7, 7, 7]);
    assert_eq!(sizes[8], 4);
    assert_eq!(names.len(), 60);
    let mut deduped = names.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(deduped.len(), 60);
    let expected: Vec<String> = (0..60).map(|i| format!("synthetic-{i:03}.txt")).collect();
    assert_eq!(names, expected);
}

// Kills removing the ModifiedDesc value comparison (group order fails) or the
// after_cursor row-key tiebreak (equal-key rows are skipped and the census
// drops below 60).
#[test]
fn children_modifieddesc_pages_equal_keys_without_loss_or_duplicates() {
    let (_tmp, catalog, _grant, root) = setup_with_files(paging_files());
    let mut reader = catalog.reader().unwrap();
    let (names, sizes) = collect_pages(&mut reader, root, Sort::ModifiedDesc, 7);
    assert_eq!(sizes.len(), 9);
    assert_eq!(names.len(), 60);
    let mut deduped = names.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(deduped.len(), 60);
    // Insertion order is 59..0, so within each equal-key group the expected
    // order follows descending names (ascending row ids).
    let insertion: Vec<i32> = (0..60).rev().collect();
    let mut expected = Vec::new();
    for group in [0, 1, 2] {
        for i in insertion.iter().filter(|i| **i % 3 == group) {
            expected.push(format!("synthetic-{i:03}.txt"));
        }
    }
    assert_eq!(names, expected);
}

// Kills removal of the `1..=200` page-limit guard.
#[test]
fn children_page_limit_bounds_rejected() {
    let (_tmp, catalog, _grant, root) = setup_with_files(paging_files());
    let mut reader = catalog.reader().unwrap();
    for limit in [0, 201] {
        assert!(
            matches!(
                reader.children(&ChildrenRequest {
                    dir_id: root,
                    sort: Sort::NameAsc,
                    basis: Basis::Logical,
                    limit,
                    cursor: None,
                }),
                Err(Error::Invalid("page limit"))
            ),
            "limit {limit} must be rejected"
        );
    }
    let page = reader
        .children(&ChildrenRequest {
            dir_id: root,
            sort: Sort::NameAsc,
            basis: Basis::Logical,
            limit: 200,
            cursor: None,
        })
        .unwrap();
    assert_eq!(page.items.len(), 60);
    assert!(page.next_cursor.is_none());
}
