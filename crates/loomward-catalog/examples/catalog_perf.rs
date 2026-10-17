//! Synthetic catalogue path benchmark; same 100k/1M tiers as the lab, no disk enumeration.
//! Run with `cargo run --release -p loomward-catalog --example catalog_perf -- 100000`.
use loomward_catalog::*;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::time::Instant;

fn register(c: &Catalog, key: u8) -> Result<(i64, i64, i64)> {
    match c
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: format!("synthetic-perf-{key}"),
            display_name: format!("Synthetic {key}"),
            display_path: format!("Synthetic root {key}"),
            root_file_id: Some(vec![key; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))? {
        WriteReply::Root {
            grant_id,
            dir_id,
            root_id,
        } => Ok((grant_id, dir_id, root_id)),
        _ => unreachable!(),
    }
}
fn begin(c: &Catalog, grant: i64) -> Result<i64> {
    match c.writer().call(WriteCommand::BeginRun {
        grant_id: grant,
        mode: "full".into(),
        strategy: "synthetic".into(),
        started_at_ns: 0,
    })? {
        WriteReply::Run(run) => Ok(run),
        _ => unreachable!(),
    }
}
fn end(c: &Catalog, run: i64) -> Result<()> {
    c.writer().call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 1,
    })?;
    Ok(())
}
fn listing(
    c: &Catalog,
    run: i64,
    dir: i64,
    files: Vec<Observation>,
    dirs: Vec<Observation>,
) -> Result<()> {
    c.writer().call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: dir,
        files,
        dirs,
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }))?;
    Ok(())
}
fn files(directory: usize, count: usize) -> Vec<Observation> {
    (0..count)
        .map(|i| {
            let mut o = Observation::file(
                if i == 0 {
                    format!("needle-{directory:06}.bin")
                } else {
                    format!("file-{i:06}.bin")
                },
                4096,
                Some(4096),
            );
            o.file_id = Some(
                ((directory * 1000 + i + 10000) as u128)
                    .to_le_bytes()
                    .to_vec(),
            );
            o.created_ft = Some(1);
            o
        })
        .collect()
}
fn measure(mut action: impl FnMut() -> Result<Value>) -> Value {
    let mut ms = Vec::new();
    let mut errors = Vec::new();
    let mut result = Value::Null;
    for _ in 0..7 {
        let start = Instant::now();
        result = match action() {
            Ok(v) => v,
            Err(e) => {
                errors.push(e.to_string());
                json!({"error": e.to_string()})
            }
        };
        ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    ms.sort_by(f64::total_cmp);
    json!({"median_ms": ms[3], "min_ms": ms[0], "max_ms": ms[6], "result": result, "errors": errors})
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let rows: usize = std::env::args()
        .nth(1)
        .ok_or("supply 100000 or 1000000")?
        .parse()?;
    if ![100_000, 1_000_000].contains(&rows) {
        return Err("supply 100000 or 1000000".into());
    }
    let temp = tempfile::tempdir()?;
    let c = Catalog::open(temp.path(), "synthetic")?;
    c.writer().set_bulk_first_scan(true);
    let db = Connection::open(temp.path().join("catalog.db"))?;
    let (grant, root_dir, root) = register(&c, 1)?;
    let run = begin(&c, grant)?;
    let start = Instant::now();
    let dirs = (0..rows / 1000)
        .map(|i| {
            let mut o = Observation::file(format!("dir-{i:06}"), 0, None);
            o.file_id = Some((i as u128 + 1).to_le_bytes().to_vec());
            o.created_ft = Some(1);
            o
        })
        .collect();
    listing(&c, run, root_dir, vec![], dirs)?;
    let ids: Vec<i64> = db
        .prepare("SELECT id FROM bulk_dir WHERE parent_id=?1 ORDER BY id")?
        .query_map([root_dir], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    assert_eq!(ids.len(), rows / 1000);
    for (i, id) in ids.iter().enumerate() {
        listing(&c, run, *id, files(i, 1000), vec![])?;
    }
    let listing_s = start.elapsed().as_secs_f64();
    let start = Instant::now();
    end(&c, run)?;
    let end_s = start.elapsed().as_secs_f64();
    assert_eq!(
        db.query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))?,
        rows as i64
    );
    eprintln!("built {rows} files: listings {listing_s:.3}s, EndRun {end_s:.3}s");
    let run = begin(&c, grant)?;
    let refresh = measure(|| {
        listing(&c, run, ids[0], files(0, 1000), vec![])?;
        Ok(json!({"files": 1000}))
    });
    end(&c, run)?;
    let mut reader = c.reader()?;
    let search = measure(|| {
        let page = reader.search(&SearchRequest {
            root_id: Some(root),
            text: "needle".into(),
            kind: "file".into(),
            limit: 100,
            work_budget: 50_000_000,
            ..Default::default()
        })?;
        assert_eq!(page.hits.len(), 100);
        assert!(!page.budget_hit);
        Ok(
            json!({"hits": page.hits.len(), "bytes": serde_json::to_vec(&page.hits).expect("serializable hits").len()}),
        )
    });
    eprintln!("search complete");
    let mut slices = serde_json::Map::new();
    for cap in [100, 1000, 6000] {
        slices.insert(cap.to_string(), measure(|| {
            let page = reader.slice(&SliceRequest {
                anchor: NodeKey::Dir(root_dir), depth: 2, max_nodes: cap, min_share: 0.0,
                basis: Basis::Logical, include_files: true,
            })?;
            Ok(json!({"nodes": page.nodes.len(), "bytes": serde_json::to_vec(&page).expect("serializable slice").len()}))
        }));
    }
    eprintln!("slices complete");
    // A fixed 200-candidate root isolates path assembly from the catalogue-wide grouping query.
    let (small_grant, small_dir, small_root) = register(&c, 2)?;
    let run = begin(&c, small_grant)?;
    listing(
        &c,
        run,
        small_dir,
        vec![],
        (0..100)
            .map(|i| {
                let mut o = Observation::file(format!("branch-{i:03}"), 0, None);
                o.file_id = Some((i as u128 + 2_000_000).to_le_bytes().to_vec());
                o
            })
            .collect(),
    )?;
    let children: Vec<i64> = db
        .prepare("SELECT id FROM bulk_dir WHERE parent_id=?1 ORDER BY id")?
        .query_map([small_dir], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for (i, dir) in children.iter().enumerate() {
        listing(&c, run, *dir, files(i, 2), vec![])?;
    }
    end(&c, run)?;
    eprintln!("small root built");
    let duplicates = measure(|| {
        let page = reader.duplicate_candidates(small_root, 1, 200)?;
        assert_eq!(page.entries.len(), 200);
        Ok(json!({"candidates": page.entries.len(), "partial": page.partial}))
    });
    let duplicates_pair = measure(|| {
        let page = reader.duplicate_candidates(small_root, 1, 2)?;
        assert_eq!(page.entries.len(), 2);
        Ok(json!({"candidates": page.entries.len()}))
    });
    let duplicates_large = measure(|| {
        let page = reader.duplicate_candidates(root, 1, 200)?;
        Ok(json!({"candidates": page.entries.len(), "partial": page.partial}))
    });
    drop(reader);
    eprintln!("duplicates complete");
    // Bulk publication replaces the file table; probe with a fresh schema/FTS handle.
    drop(db);
    let db = Connection::open(temp.path().join("catalog.db"))?;
    let optimize = measure(|| {
        db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<Value> {
            db.execute(
                "DELETE FROM file WHERE dir_id IN (SELECT id FROM dir WHERE root_id=?1)",
                [small_root],
            )?;
            let start = Instant::now();
            db.execute(
                "INSERT INTO search_name(search_name) VALUES('optimize')",
                [],
            )?;
            Ok(json!({"optimize_only_ms": start.elapsed().as_secs_f64() * 1000.0}))
        })();
        db.execute_batch("ROLLBACK")?;
        result
    });
    eprintln!("optimize complete: {optimize}");
    c.writer().call(WriteCommand::RevokeGrant {
        grant_id: small_grant,
        revoked_at_ns: 2,
    })?;
    let start = Instant::now();
    c.writer().call(WriteCommand::ForgetGrant {
        grant_id: small_grant,
        forgotten_at_ns: 3,
        expected_state_rev: None,
    })?;
    let forget_s = start.elapsed().as_secs_f64();
    assert_eq!(
        db.query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))?,
        rows as i64
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "files": rows, "profile": if cfg!(debug_assertions) { "debug" } else { "release" }, "scope": "synthetic metadata; warm shared host; 7 repeats per reader/refresh",
            "bulk_listing_s": listing_s, "bulk_end_s": end_s, "refresh_1000": refresh,
            "search_100_distinct_parents": search, "slice": slices,
        "duplicates_200_small_root": duplicates, "duplicates_2_small_root": duplicates_pair, "duplicates_large_root": duplicates_large,
            "forget_200_files_s": forget_s, "delete_200_then_optimize_rollback": optimize,
        }))?
    );
    Ok(())
}
