//! Synthetic adversarial staging/finalisation benchmark; run each case in a fresh process.
//! `memory_bounds listing|rollup 1000000`; measure the process peak working set (RSS) externally.
use loomward_catalog::*;
use rusqlite::{params, Connection};
use std::time::Instant;

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let case = args.get(1).ok_or("supply listing or rollup")?;
    let rows: usize = args.get(2).ok_or("supply at least 1000000 rows")?.parse()?;
    assert!((1_000_000..=2_000_000).contains(&rows));
    assert!(matches!(case.as_str(), "listing" | "rollup"));
    let temp = tempfile::tempdir()?;
    let c = Catalog::open(temp.path(), "synthetic")?;
    c.writer().set_bulk_first_scan(false);
    let WriteReply::Root {
        grant_id,
        dir_id,
        root_id,
    } = c
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_key: "synthetic-memory".into(),
            display_name: "Synthetic".into(),
            display_path: "Synthetic root".into(),
            root_file_id: Some(vec![1; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))?
    else {
        unreachable!()
    };
    let WriteReply::Run(run_id) = c.writer().call(WriteCommand::BeginRun {
        grant_id,
        mode: "full".into(),
        strategy: "fixture".into(),
        started_at_ns: 1,
    })?
    else {
        unreachable!()
    };
    let mut db = Connection::open(temp.path().join("catalog.db"))?;
    let start = Instant::now();
    if case == "listing" {
        for pass in 0..2 {
            let mut receipts = Vec::new();
            for (seq, offset) in (0..rows).step_by(1024).enumerate() {
                let files = (offset..rows.min(offset + 1024))
                    .map(|i| {
                        let mut o =
                            Observation::file(format!("{i:010}{}", "x".repeat(245)), 1, Some(1));
                        o.file_id = Some((i as u128 + 2).to_le_bytes().to_vec());
                        o.created_ft = Some(1);
                        o
                    })
                    .collect();
                receipts.push(c.writer().send(WriteCommand::StageChunk {
                    run_id,
                    dir_id,
                    seq: seq as i64,
                    files,
                    dirs: vec![],
                })?);
            }
            for receipt in receipts {
                receipt.wait()?;
            }
            c.writer().call(WriteCommand::ListingDone {
                run_id,
                dir_id,
                outcome: ListingOutcome::Complete,
                skipped: 0,
                errors: 0,
            })?;
            assert_eq!(
                db.query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))?,
                rows as i64
            );
            eprintln!(
                "published pass {pass}: {:.3}s",
                start.elapsed().as_secs_f64()
            );
        }
    } else {
        // Seed metadata directly so the benchmark measures rollup rather than enumeration/arena caches.
        let tx = db.transaction()?;
        tx.execute(
            "UPDATE dir SET listing_state='complete' WHERE id=?1",
            [dir_id],
        )?;
        tx.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<?1)
            INSERT INTO dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run,own_files,own_logical,own_allocated)
            SELECT ?2,?3,'synthetic-'||i,'path_observation','none',99,0,'complete',?4,?4,1,1,1 FROM n",
            params![rows as i64, root_id, dir_id, run_id])?;
        tx.commit()?;
    }
    c.writer().call(WriteCommand::EndRun {
        run_id,
        state: "completed".into(),
        finished_at_ns: 2,
    })?;
    let totals: (i64, i64, i64) = db.query_row(
        "SELECT sub_files,sub_logical,sub_dirs FROM dir WHERE id=?1",
        [dir_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    assert_eq!(
        totals,
        (
            rows as i64,
            rows as i64,
            if case == "rollup" { rows as i64 } else { 0 }
        )
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM dir WHERE parent_id IS NOT NULL AND depth!=1",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        0
    );
    println!(
        "{}",
        serde_json::json!({"case":case,"rows":rows,"seconds":start.elapsed().as_secs_f64(),"totals":totals})
    );
    Ok(())
}
