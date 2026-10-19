//! Byte-level replay of the pre-#635 search on a stable synthetic catalogue.
use loomward_catalog::*;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

#[cfg(feature = "test-hooks")]
thread_local! {
    static SQL_COUNTS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

#[cfg(feature = "test-hooks")]
fn count_statement(event: rusqlite::trace::TraceEvent<'_>) {
    if let rusqlite::trace::TraceEvent::Stmt(_, sql) = event {
        SQL_COUNTS.with(|counts| {
            let (fts, ancestry) = counts.get();
            counts.set((
                fts + usize::from(sql.contains("search_name MATCH")),
                ancestry
                    + usize::from(
                        sql.starts_with("WITH RECURSIVE a(")
                            || sql.starts_with("SELECT parent_id,name,listing_state FROM dir"),
                    ),
            ));
        });
    }
}

#[test]
fn search_pages_and_cursors_match_the_pre_streaming_reference_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(temp.path(), "synthetic").unwrap();
    for i in 0..2_u8 {
        catalog
            .writer()
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: format!("synthetic-search-{i}"),
                display_name: format!("Synthetic {i}"),
                display_path: format!("Synthetic {i}"),
                root_file_id: Some(vec![i; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap();
    }
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    db.execute_batch("UPDATE meta SET value='synthetic-search-reference' WHERE key='instance_id';
        INSERT INTO ext(id,ext,family) VALUES(1,'txt','data');
        WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<512)
        INSERT INTO file(id,dir_id,name,logical,allocated,attrs,born_run,seen_run)
        SELECT i,1,'common-'||i||'.txt',i,i,0,0,0 FROM n;
        WITH RECURSIVE n(i) AS (VALUES(3) UNION ALL SELECT i+1 FROM n WHERE i<22)
        INSERT INTO dir(id,root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run)
        SELECT i,2,CASE WHEN i=3 THEN 2 ELSE i-1 END,'level-'||i,'path_observation','none',i-2,0,'complete',0,0 FROM n;
        INSERT INTO dir(id,root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run)
        VALUES(23,2,22,'common-folder','path_observation','none',21,0,'complete',0,0),
              (24,2,2,'hidden','path_observation','none',1,0,'absent_pending',0,0),
              (25,2,24,'nested','path_observation','none',2,0,'complete',0,0);").unwrap();
    for (i, name) in [
        "common.txt",
        "Common",
        "common-prefix.txt",
        "draft common.txt",
        "uncommon.txt",
        "rare-\u{1f600}.txt",
        "common-hidden.txt",
    ]
    .iter()
    .enumerate()
    {
        db.execute(
            "INSERT INTO file(id,dir_id,name,ext_id,logical,allocated,attrs,born_run,seen_run)
            VALUES(?1,?2,?3,1,?4,?4,0,0,0)",
            params![
                513 + i as i64,
                if i == 6 { 25 } else { 22 },
                name,
                100 - i as i64
            ],
        )
        .unwrap();
    }
    let mut reader = catalog.reader().unwrap();
    let mut cases = Vec::new();
    #[cfg(feature = "test-hooks")]
    let mut focused_counts = Vec::new();
    for (text, kind, extension) in [
        ("common", "any", None),
        ("rare", "file", None),
        ("", "file", Some("txt")),
    ] {
        let mut req = SearchRequest {
            text: text.into(),
            kind: kind.into(),
            extension: extension.map(str::to_owned),
            limit: 2,
            scope: Some(vec![2]),
            work_budget: 50_000_000,
            ..Default::default()
        };
        let mut pages = Vec::new();
        for _page_index in 0..20 {
            #[cfg(feature = "test-hooks")]
            {
                SQL_COUNTS.with(|counts| counts.set((0, 0)));
                reader.trace_statements_for_test(Some(count_statement));
            }
            let page = reader.search(&req).unwrap();
            #[cfg(feature = "test-hooks")]
            {
                reader.trace_statements_for_test(None);
                let (fts, ancestry) = SQL_COUNTS.with(|counts| counts.get());
                if text == "rare" || (text == "common" && _page_index == 1) {
                    focused_counts.push((fts, ancestry));
                }
            }
            assert!(!page.budget_hit);
            pages.push(json!({"generation":page.generation,"hits":page.hits,
                "next_cursor":page.next_cursor,"budget_hit":page.budget_hit,
                "index_state":page.index_state,"partial":page.partial,
                "substring_searched":page.substring_searched,"roots":page.roots}));
            req.cursor = page.next_cursor;
            if req.cursor.is_none() {
                break;
            }
        }
        assert!(req.cursor.is_none(), "pagination did not terminate");
        cases.push(json!({"text":text,"kind":kind,"extension":extension,"pages":pages}));
    }
    let actual = serde_json::to_vec_pretty(&Value::Array(cases)).unwrap();
    // The reference bytes were captured from search.rs at 4babf67, before #635.
    assert!(
        actual == include_bytes!("fixtures/search-n1-pages.json"),
        "search page or cursor bytes changed"
    );
    #[cfg(feature = "test-hooks")]
    // Common resolves one visible and one hidden chain; rare resolves only the visible chain.
    // Pre-#635: [(9, 23), (4, 21)] for the same byte-identical pages.
    assert_eq!(focused_counts, [(5, 2), (4, 1)]);
}
