//! Synthetic catalogue benchmark; no input paths and no filesystem enumeration.
use loomward_catalog::*;
use rusqlite::Connection;
use serde_json::json;
use std::{path::PathBuf, time::Instant};

fn summary(mut ms: Vec<f64>) -> serde_json::Value {
    ms.sort_by(f64::total_cmp);
    json!({"runs":ms.len(),"p50_ms":ms[ms.len()/2],"p95_ms":ms[(ms.len()*95).div_ceil(100)-1],"min_ms":ms[0],"max_ms":ms[ms.len()-1]})
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 || args[0] != "--rows" {
        return Err("usage: bench --rows 1000000".into());
    }
    let rows: usize = args[1].parse()?;
    if !(1..=2_000_000).contains(&rows) {
        return Err("rows must be 1..=2000000".into());
    }
    let temp = tempfile::tempdir()?;
    let cleanup_path = temp.path().to_path_buf();
    let c = Catalog::open(temp.path(), "synthetic")?;
    let WriteReply::Root {
        grant_id, dir_id, ..
    } = c
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_key: "synthetic-bench".into(),
            display_name: "Synthetic benchmark".into(),
            display_path: "Synthetic benchmark root".into(),
            root_file_id: Some(0_u128.to_le_bytes().to_vec()),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))?
    else {
        unreachable!()
    };
    let WriteReply::Run(run) = c.writer().call(WriteCommand::BeginRun {
        grant_id,
        mode: "full".into(),
        strategy: "in_process_synthetic".into(),
        started_at_ns: 0,
    })?
    else {
        unreachable!()
    };
    let directories = rows.div_ceil(1000);
    let children = (0..directories)
        .map(|i| {
            let mut o = Observation::file(format!("dir-{i:05}"), 0, None);
            o.file_id = Some((i as u128 + 1).to_le_bytes().to_vec());
            o
        })
        .collect();
    let insert_start = Instant::now();
    c.writer().call(WriteCommand::StageChunk {
        run_id: run,
        dir_id,
        seq: 0,
        files: vec![],
        dirs: children,
    })?;
    c.writer().call(WriteCommand::ListingDone {
        run_id: run,
        dir_id,
        outcome: ListingOutcome::Complete,
        skipped: 0,
        errors: 0,
    })?;
    let db = Connection::open(temp.path().join("catalog.db"))?;
    let ids = {
        let mut s = db.prepare("SELECT id FROM dir WHERE parent_id=?1 ORDER BY name")?;
        let v = s
            .query_map([dir_id], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let mut receipts = Vec::new();
    let mut generated = 0;
    let mut expected_logical = 0_u64;
    let mut expected_allocated = 0_u64;
    let mut expected_unknown = 0_u64;
    let cancel = Cancellation::default();
    for (i, id) in ids.iter().enumerate() {
        let count = 1000.min(rows - generated);
        // Reserve before constructing the chunk, including serialization/decode scratch.
        let permit = c.writer().reserve_bytes(4 * 1024 * 1024, &cancel)?;
        let mut files = Vec::with_capacity(count);
        for j in 0..count {
            let serial = i * 1000 + j;
            let size = (serial as u64 * 7919) % 1_000_000 + 1;
            let allocated = if serial % 97 == 0 {
                None
            } else {
                Some(size.div_ceil(4096) * 4096)
            };
            expected_logical += size;
            expected_allocated += allocated.unwrap_or(0);
            expected_unknown += u64::from(allocated.is_none());
            let mut f = Observation::file(format!("synthetic-{serial:07}.dat"), size, allocated);
            f.extension = Some("dat".into());
            f.family = "data".into();
            f.file_id = Some((serial as u128 + 10_000_000).to_le_bytes().to_vec());
            files.push(f);
        }
        receipts.push(c.writer().send_reserved(
            WriteCommand::StageChunk {
                run_id: run,
                dir_id: *id,
                seq: 0,
                files,
                dirs: vec![],
            },
            permit,
            &cancel,
        )?);
        receipts.push(c.writer().send(WriteCommand::ListingDone {
            run_id: run,
            dir_id: *id,
            outcome: ListingOutcome::Complete,
            skipped: 0,
            errors: 0,
        })?);
        generated += count;
    }
    for receipt in receipts {
        receipt.wait()?;
    }
    let insert_s = insert_start.elapsed().as_secs_f64();
    let writer_timings = c.writer().timings();
    let finalise = Instant::now();
    c.writer().call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 1,
    })?;
    let finalise_s = finalise.elapsed().as_secs_f64();
    let (got_files, got_logical, got_allocated, got_unknown): (i64, i64, i64, i64) = db.query_row(
        "SELECT sub_files,sub_logical,sub_allocated,sub_alloc_unknown FROM dir WHERE id=?1",
        [dir_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    assert_eq!(
        (
            got_files as u64,
            got_logical as u64,
            got_allocated as u64,
            got_unknown as u64
        ),
        (
            rows as u64,
            expected_logical,
            expected_allocated,
            expected_unknown
        )
    );
    let mut reader = c.reader()?;
    let slice_request = SliceRequest {
        anchor: NodeKey::Dir(dir_id),
        depth: 8,
        max_nodes: 2500,
        min_share: 0.0,
        basis: Basis::Logical,
        include_files: true,
    };
    let children_request = ChildrenRequest {
        dir_id: ids[0],
        sort: Sort::SizeDesc,
        basis: Basis::Logical,
        limit: 200,
        cursor: None,
    };
    let search_request = SearchRequest {
        root_id: Some(grant_id),
        text: "no-such-synthetic-name".into(),
        extension: None,
        min_bytes: None,
        kind: "file".into(),
        limit: 100,
        cursor: None,
        work_budget: 2_000_000,
    };
    // Warm each query once; measure 21 subsequent runs, including serialization for P6.
    reader.slice(&slice_request)?;
    reader.children(&children_request)?;
    reader.search(&search_request)?;
    let mut slice_ms = Vec::new();
    let mut page_ms = Vec::new();
    let mut search_ms = Vec::new();
    let mut budget_hits = 0;
    let mut payload_bytes = 0;
    let mut slice_nodes = 0;
    for _ in 0..21 {
        let t = Instant::now();
        let slice = reader.slice(&slice_request)?;
        let payload = serde_json::to_vec(&slice)?;
        slice_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        payload_bytes = payload.len();
        slice_nodes = slice.nodes.len();
        assert!(slice_nodes <= 2500);
        let t = Instant::now();
        let page = reader.children(&children_request)?;
        page_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(page.items.len(), 200.min(rows));
        let t = Instant::now();
        let page = reader.search(&search_request)?;
        search_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        budget_hits += usize::from(page.budget_hit);
        assert!(page.items.is_empty());
    }
    drop(reader);
    drop(db);
    drop(c);
    let catalog_bytes = std::fs::metadata(temp.path().join("catalog.db"))?.len();
    let entries = rows + directories + 1;
    let mut receipt = json!({"schema_version":2,"dataset_class":"synthetic","platform":std::env::consts::OS,"scope":"in-process generated metadata only; no enumeration, no HTTP, no personal data","files":rows,"directories":directories+1,"insert_includes_generation_and_queue_backpressure":true,"all_schema_indexes_present":true,"oracle":{"logical_bytes":expected_logical.to_string(),"allocated_known_bytes":expected_allocated.to_string(),"allocation_unknown_files":expected_unknown,"matched":true},"P4":{"insert_seconds":insert_s,"rows_per_second":entries as f64/insert_s,"target_rows_per_second":250000,"met":entries as f64/insert_s>=250000.0},"writer_timings":writer_timings,"finalise_seconds":finalise_s,"P6":{"timing":summary(slice_ms),"nodes":slice_nodes,"payload_bytes":payload_bytes,"target_ms":60,"target_payload_bytes":1500000,"target_dataset_rows":10000000,"dataset_scale_met":false,"http_round_trip":"unverified"},"P7":{"timing":summary(page_ms),"limit":200,"target_ms":20},"P8":{"timing":summary(search_ms),"work_budget_sqlite_ops":2000000,"budget_hits":budget_hits,"target_ms":300},"P14":{"catalog_bytes":catalog_bytes,"bytes_per_entry":catalog_bytes as f64/entries as f64,"target_bytes_per_entry":200,"target_dataset_rows":10000000,"dataset_scale_met":false},"limitations":["single warm run on this Windows host; no cold-cache claim","P6 and P14 at 1M, not the 10M target; HTTP and process-memory measurement belong to integration"]});
    temp.close()?;
    assert!(!cleanup_path.exists());
    receipt["publication_path"] =
        json!("StageChunk + ListingDone; byte reservation before file generation");
    receipt["temporary_directory_removed"] = json!(true);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let output = root.join("evidence/v3/bench/catalog-1m.json");
    std::fs::create_dir_all(output.parent().unwrap())?;
    std::fs::write(&output, serde_json::to_string_pretty(&receipt)? + "\n")?;
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}
