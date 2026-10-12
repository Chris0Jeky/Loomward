//! Name search (LW-113, q-34, q-36): ranking tiers, containment, revocation and forget, live
//! updates, honest index state. Synthetic names only.
use loomward_catalog::*;
use rusqlite::Connection;
use std::path::Path;

struct Lab {
    temp: tempfile::TempDir,
    c: Catalog,
    next_id: u32,
}
struct Root {
    grant: i64,
    root: i64,
    dir: i64,
}
fn obs(lab: &mut Lab, name: &str, logical: u64) -> Observation {
    lab.next_id += 1;
    let mut o = Observation::file(name, logical, Some(logical));
    o.file_id = Some([lab.next_id.to_le_bytes().to_vec(), vec![7; 12]].concat());
    o.created_ft = Some(i64::from(lab.next_id));
    o.modified_ft = Some(1_000 * i64::from(lab.next_id));
    if let Some((_, ext)) = name.rsplit_once('.') {
        o.extension = Some(ext.to_lowercase());
        o.family = "data".into();
    }
    o
}
fn lab() -> Lab {
    let temp = tempfile::tempdir().unwrap();
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    Lab {
        temp,
        c,
        next_id: 0,
    }
}
fn register(lab: &Lab, volume: &str, name: &str, id: u8) -> Root {
    let WriteReply::Root {
        grant_id,
        dir_id,
        root_id,
    } = lab
        .c
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_key: volume.into(),
            display_name: name.into(),
            display_path: format!(r"C:\Synthetic\{name}"),
            root_file_id: Some([id; 16].to_vec()),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 1,
        }))
        .unwrap()
    else {
        panic!()
    };
    Root {
        grant: grant_id,
        root: root_id,
        dir: dir_id,
    }
}
fn begin(lab: &Lab, r: &Root, mode: &str) -> i64 {
    let WriteReply::Run(run) = lab
        .c
        .writer()
        .call(WriteCommand::BeginRun {
            grant_id: r.grant,
            mode: mode.into(),
            strategy: "fixture".into(),
            started_at_ns: 2,
        })
        .unwrap()
    else {
        panic!()
    };
    run
}
fn list(lab: &Lab, run: i64, dir: i64, files: Vec<Observation>, dirs: Vec<Observation>) {
    lab.c
        .writer()
        .call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id: dir,
            files,
            dirs,
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        }))
        .unwrap();
}
fn end(lab: &Lab, run: i64) {
    lab.c
        .writer()
        .call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 3,
        })
        .unwrap();
}
fn child(lab: &Lab, parent: i64, name: &str) -> i64 {
    Connection::open(lab.temp.path().join("catalog.db"))
        .unwrap()
        .query_row(
            "SELECT id FROM dir WHERE parent_id=?1 AND name=?2",
            rusqlite::params![parent, name],
            |r| r.get(0),
        )
        .unwrap()
}
fn req(text: &str) -> SearchRequest {
    SearchRequest {
        text: text.into(),
        kind: "any".into(),
        limit: 100,
        work_budget: 50_000_000,
        ..SearchRequest::default()
    }
}
fn search(lab: &Lab, r: &SearchRequest) -> SearchPage {
    lab.c.reader().unwrap().search(r).unwrap()
}
fn names(p: &SearchPage) -> Vec<(String, String)> {
    p.hits
        .iter()
        .map(|h| (h.match_kind.clone(), h.path.text.clone()))
        .collect()
}
fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

/// One root: a folder `report` with one file, and files at the top covering every tier.
fn ranked() -> (Lab, Root) {
    let mut lab = lab();
    let r = register(&lab, "vol", "Alpha", 1);
    let run = begin(&lab, &r, "full");
    let files = [
        "xreportx.bin",
        "report.pdf",
        "annual report.docx",
        "Report_2024.xlsx",
        "myreport.txt",
        "reportage.doc",
        "unrelated.txt",
    ]
    .iter()
    .enumerate()
    .map(|(i, n)| obs(&mut lab, n, 10 * (i as u64 + 1)))
    .collect();
    let dirs = vec![
        obs(&mut lab, "report", 0),
        obs(&mut lab, "Draft reports", 0),
    ];
    list(&lab, run, r.dir, files, dirs);
    let inner = child(&lab, r.dir, "report");
    let file = obs(&mut lab, "notes.txt", 5);
    list(&lab, run, inner, vec![file], vec![]);
    let other = child(&lab, r.dir, "Draft reports");
    list(&lab, run, other, vec![], vec![]);
    end(&lab, run);
    (lab, r)
}

#[test]
fn tiers_rank_exact_prefix_token_substring_with_folders_first_in_each() {
    let (lab, _r) = ranked();
    let page = search(&lab, &req("report"));
    assert_eq!(
        names(&page),
        pairs(&[
            ("exact", "report"),
            ("exact", "report.pdf"),
            ("prefix", "Report_2024.xlsx"),
            ("prefix", "reportage.doc"),
            ("token", "Draft reports"),
            ("token", "annual report.docx"),
            ("substring", "xreportx.bin"),
            ("substring", "myreport.txt"),
        ])
    );
    assert_eq!(page.index_state, "current");
    assert!(page.substring_searched && !page.partial && page.next_cursor.is_none());
    let annual = &page.hits[5];
    assert_eq!(annual.row.name, "annual report.docx");
    assert_eq!(annual.highlights, vec![[7, 13]]);
    assert_eq!(
        annual.row.location_hint.as_ref().unwrap().text,
        "Alpha",
        "the parent's display path"
    );
    // A hit below a folder carries its root-relative path.
    let notes = search(&lab, &req("notes"));
    assert_eq!(names(&notes), pairs(&[("exact", r"report\notes.txt")]));
    assert_eq!(
        notes.hits[0].row.location_hint.as_ref().unwrap().text,
        r"Alpha\report"
    );
}

#[test]
fn several_words_match_at_word_starts_before_anywhere_and_short_texts_say_so() {
    let (lab, _r) = ranked();
    let page = search(&lab, &req("report annual"));
    assert_eq!(names(&page), pairs(&[("token", "annual report.docx")]));
    assert_eq!(page.hits[0].highlights, vec![[0, 6], [7, 13]]);
    let page = search(&lab, &req("port nnual"));
    assert_eq!(names(&page), pairs(&[("substring", "annual report.docx")]));
    // One character: whole names only, and the reply says inner substrings were not searched.
    let page = search(&lab, &req("x"));
    assert!(page.hits.is_empty() && !page.substring_searched);
    // Quotes, operators and sentinels in the text are literal.
    for text in [
        "\"report",
        "report OR unrelated",
        "NOT report",
        "*",
        "\u{1}report",
    ] {
        let page = search(&lab, &req(text));
        assert!(
            page.hits
                .iter()
                .all(|h| h.row.name.to_lowercase().contains("report")),
            "{text:?}: {:?}",
            names(&page)
        );
    }
}

#[test]
fn paging_one_hit_at_a_time_crosses_tiers_without_repeats_or_gaps() {
    let (lab, _r) = ranked();
    let all = names(&search(&lab, &req("report")));
    let mut r = req("report");
    r.limit = 1;
    let mut paged = Vec::new();
    for _ in 0..20 {
        let page = search(&lab, &r);
        paged.extend(names(&page));
        match page.next_cursor {
            Some(c) => r.cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(paged, all);
    // A cursor is bound to its filters.
    let mut r = req("report");
    r.limit = 1;
    let cursor = search(&lab, &r).next_cursor;
    let mut other = req("reports");
    other.cursor = cursor;
    assert!(matches!(
        lab.c.reader().unwrap().search(&other),
        Err(Error::Invalid("search cursor binding"))
    ));
}

#[test]
fn filters_narrow_by_kind_extension_size_and_modified_time() {
    let (lab, _r) = ranked();
    let only = |f: &dyn Fn(&mut SearchRequest)| {
        let mut r = req("report");
        f(&mut r);
        search(&lab, &r)
            .hits
            .into_iter()
            .map(|h| h.row.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        only(&|r| r.kind = "dir".into()),
        ["report", "Draft reports"]
    );
    assert_eq!(only(&|r| r.extension = Some("pdf".into())), ["report.pdf"]);
    // Sizes are 10 per position in the listing: report.pdf 20, annual 30, Report_2024 40.
    assert_eq!(
        only(&|r| {
            r.kind = "file".into();
            (r.min_bytes, r.max_bytes) = (Some(25), Some(40));
        }),
        ["Report_2024.xlsx", "annual report.docx"]
    );
    // modified_ft is 1000 x the creation order: xreportx 1, report.pdf 2, annual 3.
    assert_eq!(
        only(&|r| (r.modified_after, r.modified_before) = (Some(2_000), Some(3_000))),
        ["report.pdf", "annual report.docx"]
    );
    // No text: a filter-only listing, one extension largest first.
    let mut r = req("");
    r.extension = Some("txt".into());
    let page = search(&lab, &r);
    assert_eq!(
        names(&page),
        pairs(&[
            ("filter", "unrelated.txt"),
            ("filter", "myreport.txt"),
            ("filter", r"report\notes.txt"),
        ])
    );
}

#[test]
fn search_stays_inside_granted_roots_and_drops_a_revoked_one() {
    let mut lab = lab();
    let a = register(&lab, "vol", "Alpha", 1);
    let b = register(&lab, "vol", "Beta", 2);
    for r in [&a, &b] {
        let run = begin(&lab, r, "full");
        let f = obs(&mut lab, "shared-budget.xlsx", 1);
        list(&lab, run, r.dir, vec![f], vec![]);
        end(&lab, run);
    }
    let roots = |p: &SearchPage| p.hits.iter().map(|h| h.root_id).collect::<Vec<_>>();
    assert_eq!(roots(&search(&lab, &req("budget"))), [a.root, b.root]);
    let mut only_b = req("budget");
    only_b.root_id = Some(b.root);
    let page = search(&lab, &only_b);
    assert_eq!(roots(&page), [b.root]);
    assert_eq!(page.roots.len(), 1);
    // The root directories themselves (the grants) are never hits.
    assert!(search(&lab, &req("Alpha")).hits.is_empty());
    lab.c
        .writer()
        .call(WriteCommand::RevokeGrant {
            grant_id: a.grant,
            revoked_at_ns: 9,
        })
        .unwrap();
    let page = search(&lab, &req("budget"));
    assert_eq!(roots(&page), [b.root]);
    assert_eq!(
        page.roots.iter().map(|r| r.root_id).collect::<Vec<_>>(),
        [b.root]
    );
    let mut only_a = req("budget");
    only_a.root_id = Some(a.root);
    assert!(matches!(
        lab.c.reader().unwrap().search(&only_a),
        Err(Error::NotFound)
    ));
}

fn on_disk(dir: &Path, needle: &str) -> bool {
    ["catalog.db", "catalog.db-wal"]
        .iter()
        .filter_map(|n| std::fs::read(dir.join(n)).ok())
        .any(|bytes| bytes.windows(needle.len()).any(|w| w == needle.as_bytes()))
}

#[test]
fn a_forgotten_root_leaves_no_index_entry_and_no_name_trigram_on_disk() {
    let mut lab = lab();
    let a = register(&lab, "vol", "Alpha", 1);
    let b = register(&lab, "vol", "Beta", 2);
    for (r, name) in [(&a, "qzxforgetme.dat"), (&b, "keepme.dat")] {
        let run = begin(&lab, r, "full");
        let f = obs(&mut lab, name, 1);
        list(&lab, run, r.dir, vec![f], vec![]);
        end(&lab, run);
    }
    let db = Connection::open(lab.temp.path().join("catalog.db")).unwrap();
    let fold = || {
        db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap()
    };
    fold();
    // The trigram index stores the folded name in three-character terms: "qzx" is unique to root A.
    assert!(
        on_disk(lab.temp.path(), "qzx"),
        "the index must hold the term before the forget"
    );
    lab.c
        .writer()
        .call(WriteCommand::RevokeGrant {
            grant_id: a.grant,
            revoked_at_ns: 9,
        })
        .unwrap();
    lab.c
        .writer()
        .call(WriteCommand::ForgetGrant {
            grant_id: a.grant,
            forgotten_at_ns: 10,
            expected_state_rev: None,
        })
        .unwrap();
    fold();
    let entries = |text: &str| -> i64 {
        db.query_row(
            "SELECT count(*) FROM search_name WHERE search_name MATCH ?1",
            [format!("\"{text}\"")],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(entries("qzxforget"), 0);
    assert_eq!(entries("keepme"), 1);
    assert!(
        !on_disk(lab.temp.path(), "qzx"),
        "a forgotten name's terms stay in the index file"
    );
    assert_eq!(
        search(&lab, &req("keepme")).hits[0].root_id,
        b.root,
        "the other root is still searchable"
    );
}

#[test]
fn live_relists_rename_and_remove_entries_and_a_vanished_folder_hides_at_once() {
    let mut lab = lab();
    let r = register(&lab, "vol", "Alpha", 1);
    let run = begin(&lab, &r, "full");
    let keep = obs(&mut lab, "draft-plan.txt", 1);
    let gone = obs(&mut lab, "obsolete-plan.txt", 1);
    let folder = obs(&mut lab, "planning", 0);
    list(
        &lab,
        run,
        r.dir,
        vec![keep.clone(), gone],
        vec![folder.clone()],
    );
    let inner = child(&lab, r.dir, "planning");
    let deep = obs(&mut lab, "inner-plan.txt", 1);
    list(&lab, run, inner, vec![deep], vec![]);
    end(&lab, run);
    assert_eq!(search(&lab, &req("plan")).hits.len(), 4);
    // A watcher relist (targeted run): one file renamed in place, one gone, the folder gone.
    let run = begin(&lab, &r, "targeted");
    let mut renamed = keep;
    renamed.name = "final-plan.txt".into();
    list(&lab, run, r.dir, vec![renamed], vec![]);
    // Before EndRun deletes the folder its rows are absent_pending: hidden already.
    let page = search(&lab, &req("plan"));
    assert_eq!(names(&page), pairs(&[("token", "final-plan.txt")]));
    end(&lab, run);
    assert_eq!(
        names(&search(&lab, &req("plan"))),
        pairs(&[("token", "final-plan.txt")])
    );
    assert!(search(&lab, &req("draft")).hits.is_empty());
    let page = search(&lab, &req("final"));
    assert_eq!(
        page.roots[0].coverage, "partial",
        "a targeted run leaves the root partial"
    );
    assert!(page.partial);
}

#[test]
fn a_bulk_first_scan_is_building_until_end_run_and_then_fully_indexed() {
    let mut lab = lab();
    lab.c.writer().set_bulk_first_scan(true);
    let r = register(&lab, "vol", "Alpha", 1);
    let run = begin(&lab, &r, "full");
    let files = (0..300)
        .map(|i| obs(&mut lab, &format!("bulk-item-{i:03}.dat"), 1))
        .collect();
    list(&lab, run, r.dir, files, vec![]);
    let page = search(&lab, &req("bulk-item"));
    assert!(
        page.hits.is_empty(),
        "a build is not visible before it publishes"
    );
    assert_eq!(page.index_state, "building");
    assert!(page.partial);
    end(&lab, run);
    let mut all = req("bulk-item");
    all.limit = 100;
    let mut count = 0;
    loop {
        let page = search(&lab, &all);
        assert_eq!(page.index_state, "current");
        count += page.hits.len();
        match page.next_cursor {
            Some(c) => all.cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(count, 300);
}

#[test]
fn a_store_without_the_index_gains_it_filled_on_open_and_a_stale_root_says_so() {
    let mut lab = lab();
    let r = register(&lab, "vol", "Alpha", 1);
    let run = begin(&lab, &r, "full");
    let f = obs(&mut lab, "legacy-ledger.csv", 1);
    let d = obs(&mut lab, "ledgers", 0);
    list(&lab, run, r.dir, vec![f], vec![d]);
    end(&lab, run);
    let dir = lab.temp.path().to_path_buf();
    drop(lab.c);
    let db = Connection::open(dir.join("catalog.db")).unwrap();
    db.execute_batch("DROP TABLE search_name; DROP TRIGGER IF EXISTS search_file_insert; DROP TRIGGER IF EXISTS search_file_delete; DROP TRIGGER IF EXISTS search_file_rename; DROP TRIGGER IF EXISTS search_dir_insert; DROP TRIGGER IF EXISTS search_dir_delete; DROP TRIGGER IF EXISTS search_dir_rename;").unwrap();
    drop(db);
    let lab = Lab {
        c: Catalog::open(&dir, "synthetic").unwrap(),
        temp: lab.temp,
        next_id: 0,
    };
    assert_eq!(
        names(&search(&lab, &req("ledger"))),
        pairs(&[("prefix", "ledgers"), ("token", "legacy-ledger.csv")])
    );
    // No bulk build table carries a copy of the search triggers (they would index twice).
    let triggers: i64 = Connection::open(dir.join("catalog.db"))
        .unwrap()
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name LIKE 'bulk_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(triggers, 0);
    Connection::open(dir.join("catalog.db"))
        .unwrap()
        .execute("UPDATE root SET state='stale' WHERE id=?1", [r.root])
        .unwrap();
    let page = search(&lab, &req("ledger"));
    assert_eq!(page.index_state, "stale");
    assert!(page.partial);
}

#[test]
fn a_spent_time_budget_returns_a_partial_page_that_resumes() {
    let (lab, _r) = ranked();
    let all = names(&search(&lab, &req("report")));
    let mut r = req("report");
    r.time_budget = Some(std::time::Duration::ZERO);
    let mut paged = Vec::new();
    let mut cut = 0;
    for _ in 0..1000 {
        let page = search(&lab, &r);
        paged.extend(names(&page));
        cut += usize::from(page.budget_hit);
        match page.next_cursor {
            Some(c) => r.cursor = Some(c),
            None => break,
        }
        // Once some progress exists, finish without the cut so the loop always ends.
        if cut > 3 {
            r.time_budget = None;
        }
    }
    assert!(cut > 0, "a zero budget must cut the page");
    assert_eq!(paged, all);
}

/// The index holds exactly one entry per non-root folder and per file, keyed as the triggers key them.
fn assert_index_parity(lab: &Lab) {
    let db = Connection::open(lab.temp.path().join("catalog.db")).unwrap();
    let keys = |sql: &str| -> Vec<i64> {
        let mut s = db.prepare(sql).unwrap();
        let mut v: Vec<i64> = s
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        v.sort_unstable();
        v
    };
    assert_eq!(
        keys("SELECT rowid FROM search_name"),
        keys("SELECT id FROM dir WHERE parent_id IS NOT NULL UNION ALL SELECT id+281474976710656 FROM file"),
        "search index and catalogue rows differ"
    );
}

/// Root → `outer` → `middle` → `inner`, a file at each level, all named `nest-*`.
fn nested(lab: &mut Lab, r: &Root) -> i64 {
    let run = begin(lab, r, "full");
    let top = obs(lab, "nest-top.txt", 1);
    let outer = obs(lab, "nest-outer", 0);
    let keep = obs(lab, "nest-keep", 0);
    list(lab, run, r.dir, vec![top], vec![outer, keep]);
    let keep = child(lab, r.dir, "nest-keep");
    list(lab, run, keep, vec![], vec![]);
    let mut parent = child(lab, r.dir, "nest-outer");
    for level in ["middle", "inner"] {
        let f = obs(lab, &format!("nest-{level}-file.txt"), 1);
        let d = obs(lab, &format!("nest-{level}"), 0);
        list(lab, run, parent, vec![f], vec![d]);
        parent = child(lab, parent, &format!("nest-{level}"));
    }
    let f = obs(lab, "nest-deepest.txt", 1);
    list(lab, run, parent, vec![f], vec![]);
    end(lab, run);
    run
}

#[test]
fn a_grandchild_of_a_vanished_folder_is_hidden_and_a_full_run_deletes_the_subtree_from_the_index() {
    let mut lab = lab();
    let r = register(&lab, "vol", "Alpha", 1);
    nested(&mut lab, &r);
    assert_index_parity(&lab);
    assert_eq!(search(&lab, &req("nest")).hits.len(), 8);
    // A full rescan lists the root without `nest-outer`: it goes absent_pending at once, and only
    // it does, so everything below it is hidden through its ancestor alone.
    let run = begin(&lab, &r, "full");
    let top = Connection::open(lab.temp.path().join("catalog.db"))
        .unwrap()
        .query_row(
            "SELECT created_ft FROM file WHERE name='nest-top.txt'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let mut keep_file = Observation::file("nest-top.txt", 1, Some(1));
    keep_file.file_id = Some([(top as u32).to_le_bytes().to_vec(), vec![7; 12]].concat());
    keep_file.created_ft = Some(top);
    keep_file.extension = Some("txt".into());
    keep_file.family = "data".into();
    let mut keep_dir = Observation::file("nest-keep", 0, Some(0));
    let kid: i64 = Connection::open(lab.temp.path().join("catalog.db"))
        .unwrap()
        .query_row(
            "SELECT created_ft FROM dir WHERE name='nest-keep'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    keep_dir.file_id = Some([(kid as u32).to_le_bytes().to_vec(), vec![7; 12]].concat());
    keep_dir.created_ft = Some(kid);
    list(&lab, run, r.dir, vec![keep_file], vec![keep_dir]);
    let keep = child(&lab, r.dir, "nest-keep");
    list(&lab, run, keep, vec![], vec![]);
    let state = |name: &str| -> String {
        Connection::open(lab.temp.path().join("catalog.db"))
            .unwrap()
            .query_row(
                "SELECT listing_state FROM dir WHERE name=?1",
                [name],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(state("nest-outer"), "absent_pending");
    assert_ne!(
        state("nest-inner"),
        "absent_pending",
        "the fixture must hide by ancestry"
    );
    // Files only: mid-run, a folder hit's aggregates are unsettled and the page asks for a repair.
    let mut files = req("nest");
    files.kind = "file".into();
    assert_eq!(
        names(&search(&lab, &files)),
        pairs(&[("prefix", "nest-top.txt")]),
        "nothing under the vanished folder, however deep"
    );
    end(&lab, run);
    // EndRun's delete cascades through dir and file foreign keys: the triggers must follow it.
    let gone: i64 = Connection::open(lab.temp.path().join("catalog.db"))
        .unwrap()
        .query_row(
            "SELECT count(*) FROM dir WHERE name LIKE 'nest-%' AND name NOT IN ('nest-keep')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(gone, 0, "the full run deleted the vanished subtree");
    assert_index_parity(&lab);
}

#[test]
fn forgetting_a_multi_level_root_leaves_no_index_entry_for_it() {
    let mut lab = lab();
    let a = register(&lab, "vol", "Alpha", 1);
    let b = register(&lab, "vol", "Beta", 2);
    nested(&mut lab, &a);
    let run = begin(&lab, &b, "full");
    let f = obs(&mut lab, "beta-only.txt", 1);
    list(&lab, run, b.dir, vec![f], vec![]);
    end(&lab, run);
    assert_index_parity(&lab);
    lab.c
        .writer()
        .call(WriteCommand::RevokeGrant {
            grant_id: a.grant,
            revoked_at_ns: 9,
        })
        .unwrap();
    lab.c
        .writer()
        .call(WriteCommand::ForgetGrant {
            grant_id: a.grant,
            forgotten_at_ns: 10,
            expected_state_rev: None,
        })
        .unwrap();
    assert_index_parity(&lab);
    let left: i64 = Connection::open(lab.temp.path().join("catalog.db"))
        .unwrap()
        .query_row("SELECT count(*) FROM search_name", [], |row| row.get(0))
        .unwrap();
    assert_eq!(left, 1, "only Beta's file is indexed");
}
