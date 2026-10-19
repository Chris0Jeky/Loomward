//! Test-only measurement for issue #700: how long `Catalog::validate_stores` takes as a synthetic
//! catalogue grows. Full catalogue integrity is checked on open, not preflight. Not a product path.
//! Build a catalogue: cargo run --release -p loomward-catalog --example quick_check_measure -- build <files> <dir>
//! Time validate:    cargo run --release -p loomward-catalog --example quick_check_measure -- time <dir> [runs] [label]
use loomward_catalog::*;
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use std::{path::Path, time::Instant};

type BoxResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

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

fn size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn build(rows: usize, dir: &Path) -> BoxResult<()> {
    if rows == 0 || !rows.is_multiple_of(1000) {
        return Err("rows must be a positive multiple of 1000".into());
    }
    std::fs::create_dir_all(dir)?;
    let c = Catalog::open(dir, "synthetic")?;
    c.writer().set_bulk_first_scan(true);
    let db = Connection::open(dir.join("catalog.db"))?;
    let (grant, root_dir, _root) = register(&c, 1)?;
    let run = begin(&c, grant)?;
    let start = Instant::now();
    let dir_count = rows / 1000;
    let dirs = (0..dir_count)
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
    assert_eq!(ids.len(), dir_count);
    for (i, id) in ids.iter().enumerate() {
        listing(&c, run, *id, files(i, 1000), vec![])?;
    }
    let listing_s = start.elapsed().as_secs_f64();
    let start = Instant::now();
    end(&c, run)?;
    let end_s = start.elapsed().as_secs_f64();
    let counted: i64 = db.query_row("SELECT count(*) FROM file", [], |r| r.get(0))?;
    assert_eq!(counted, rows as i64);
    drop(db);
    drop(c);
    println!(
        "{}",
        json!({
            "mode": "build", "files": rows, "dirs": dir_count,
            "listing_s": listing_s, "end_s": end_s,
            "catalog_db_bytes": size(&dir.join("catalog.db")),
            "state_db_bytes": size(&dir.join("state.db")),
            "catalog_wal_bytes": size(&dir.join("catalog.db-wal")),
            "state_wal_bytes": size(&dir.join("state.db-wal")),
        })
    );
    Ok(())
}

/// One `PRAGMA quick_check` on a read-only connection, the same statement `db::inspect` runs.
fn quick_check_ms(path: &Path) -> BoxResult<f64> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let start = Instant::now();
    let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    if check != "ok" {
        return Err(format!("{}: quick_check returned {check}", path.display()).into());
    }
    Ok(ms)
}

fn time(dir: &Path, runs: usize, label: &str) -> BoxResult<()> {
    for run in 1..=runs {
        let start = Instant::now();
        Catalog::validate_stores(dir, "synthetic")?;
        let validate_ms = start.elapsed().as_secs_f64() * 1000.0;
        // Per-DB split is measured after validate_stores, so these reads are warm.
        let state_ms = quick_check_ms(&dir.join("state.db"))?;
        let catalog_ms = quick_check_ms(&dir.join("catalog.db"))?;
        println!(
            "{}",
            json!({
                "mode": "time", "label": label, "run": run,
                "validate_stores_ms": validate_ms,
                "state_quick_check_ms_after": state_ms,
                "catalog_quick_check_ms_after": catalog_ms,
                "catalog_db_bytes": size(&dir.join("catalog.db")),
                "state_db_bytes": size(&dir.join("state.db")),
            })
        );
    }
    Ok(())
}

fn main() -> BoxResult<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("build") if args.len() == 4 => build(args[2].parse()?, Path::new(&args[3])),
        Some("time") if args.len() >= 3 => {
            let runs = match args.get(3) {
                Some(n) => n.parse()?,
                None => 3,
            };
            let label = args.get(4).map(String::as_str).unwrap_or("run");
            time(Path::new(&args[2]), runs, label)
        }
        _ => {
            Err("usage: quick_check_measure build <files> <dir> | time <dir> [runs] [label]".into())
        }
    }
}
