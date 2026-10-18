//! #552 synthetic query-cost probe, not a scan/index-publication benchmark.
//! `search_n1 <fixture-directory> <1000000|10000000>`; reuse the same fixture for A/B runs.
use loomward_catalog::*;
use rusqlite::{params, Connection};
use serde_json::json;
use std::time::Instant;

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: search_n1 <synthetic-fixture-directory> <1000000|10000000>".into());
    }
    let rows: i64 = args[1].parse()?;
    if ![1_000_000, 10_000_000].contains(&rows) {
        return Err("rows must be 1000000 or 10000000".into());
    }
    eprintln!("opening synthetic fixture (existing catalogues run their normal integrity check)");
    let c = Catalog::open(std::path::Path::new(&args[0]), "synthetic")?;
    eprintln!("catalogue open; checking fixture cardinality");
    let db = Connection::open(std::path::Path::new(&args[0]).join("catalog.db"))?;
    let count: i64 = db.query_row("SELECT count(*) FROM file", [], |r| r.get(0))?;
    if count == 0 {
        for i in 0..2 {
            c.writer()
                .call(WriteCommand::RegisterRoot(RootObservation {
                    volume_guid: None,
                    volume_key: format!("synthetic-n1-{i}"),
                    display_name: format!("Synthetic {i}"),
                    display_path: format!("Synthetic {i}"),
                    root_file_id: Some(vec![i; 16]),
                    filesystem: Some("NTFS".into()),
                    origin: "fixture".into(),
                    granted_via: "fixture".into(),
                    observed_at_ns: 0,
                }))?;
        }
        let started = Instant::now();
        for start in (0..rows).step_by(100_000) {
            db.execute("WITH RECURSIVE n(i) AS (VALUES(?1+1) UNION ALL SELECT i+1 FROM n WHERE i<?1+100000) INSERT INTO file(id,dir_id,name,logical,allocated,attrs,born_run,seen_run) SELECT i,1,'common-'||i||'.txt',1,1,0,0,0 FROM n", [start])?;
            eprintln!("synthetic files: {}/{}", start + 100_000, rows);
        }
        // One hundred disjoint depth-20 branches: shared-parent caching cannot hide the walk.
        db.execute_batch("BEGIN")?;
        for branch in 0..100_i64 {
            let mut parent = 2;
            for depth in 1..=20_i64 {
                let id = 3 + branch * 20 + depth - 1;
                db.execute("INSERT INTO dir(id,root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run) VALUES(?1,2,?2,?3,'path_observation','none',?4,0,'complete',0,0)", params![id,parent,format!("branch-{branch}-{depth}"),depth])?;
                parent = id;
            }
            db.execute("INSERT INTO file(dir_id,name,logical,allocated,attrs,born_run,seen_run) VALUES(?1,?2,1,1,0,0,0)",params![parent,format!("deep-{branch}.txt")])?;
        }
        db.execute_batch("COMMIT; PRAGMA wal_checkpoint(TRUNCATE)")?;
        eprintln!("fixture built in {:.3}s", started.elapsed().as_secs_f64());
    } else if count != rows + 100 {
        return Err("fixture row count differs; use a separate synthetic fixture directory".into());
    }
    let mut reader = c.reader()?;
    let mut cases = Vec::new();
    for (label, text, scope) in [
        ("owner_common", "common", None),
        ("narrow_scope_common", "common", Some(vec![2])),
        ("disjoint_ancestors", "deep", Some(vec![2])),
    ] {
        let req = SearchRequest {
            text: text.into(),
            kind: "file".into(),
            scope,
            limit: 100,
            work_budget: 50_000_000,
            ..Default::default()
        };
        for run in 0..3 {
            let start = Instant::now();
            let page = reader.search(&req)?;
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            eprintln!(
                "{label} run {run}: {ms:.3} ms, budget_hit={}",
                page.budget_hit
            );
            cases.push(json!({"case":label,"run":run,"ms":ms,"hits":page.hits.len(),"budget_hit":page.budget_hit,"has_cursor":page.next_cursor.is_some(),"next_key":page.next_cursor.map(|c|c.last_key)}));
        }
    }
    // Exact SQL from freshness::resolve; repeat 1,024 depth-20 changed paths.
    let start = Instant::now();
    for path in 0..1024 {
        let branch = path % 100;
        let mut dir: i64 = db.query_row(
            "SELECT id FROM main.dir WHERE root_id=?1 AND parent_id IS NULL",
            [2],
            |r| r.get(0),
        )?;
        for depth in 1..=20 {
            dir = db.query_row("SELECT id FROM main.dir WHERE parent_id=?1 AND name=?2 AND listing_state NOT IN ('absent_pending','excluded')",params![dir,format!("branch-{branch}-{depth}")],|r|r.get(0))?;
        }
    }
    let resolve_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    for path in 0..1024 {
        let branch = path % 100;
        let mut dir: i64 = db.query_row(
            "SELECT id FROM main.dir WHERE root_id=?1 AND parent_id IS NULL",
            [2],
            |r| r.get(0),
        )?;
        let mut lookup = db.prepare_cached("SELECT id FROM main.dir WHERE parent_id=?1 AND name=?2 AND listing_state NOT IN ('absent_pending','excluded')")?;
        for depth in 1..=20 {
            dir = lookup.query_row(params![dir, format!("branch-{branch}-{depth}")], |r| {
                r.get(0)
            })?;
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"synthetic_files":rows+100,"search":cases,"resolve_1024_depth20_ms":resolve_ms,"resolve_prepared_1024_depth20_ms":start.elapsed().as_secs_f64()*1000.0,"resolve_sql_calls":1024*21,"limits":"generated fixture, first search run included; no filesystem enumeration, service, HTTP or cold-cache claim; resolver compares exact SQL separately"})
        )?
    );
    Ok(())
}
