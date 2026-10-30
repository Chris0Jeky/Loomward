//! Synthetic publication/FTS segment experiment for #937. No filesystem enumeration.
use loomward_catalog::*;
use rusqlite::Connection;
use std::path::Path;
use std::time::Instant;

fn segments(path: &Path, index: &str) -> u64 {
    let db = Connection::open(path.join("catalog.db")).unwrap();
    let block: Vec<u8> = db
        .query_row(
            &format!("SELECT block FROM {index}_data WHERE id=10"),
            [],
            |r| r.get(0),
        )
        .unwrap();
    // FTS5 structure record: 4-byte cookie, then varints for levels and segment count.
    // Contentless-delete (search_token) adds a four-byte V2 marker after the cookie.
    let start = if block[4..].starts_with(&[0xff, 0, 0, 1]) {
        8
    } else {
        4
    };
    let mut bytes = block[start..].iter();
    let mut varint = || {
        let mut value = 0_u64;
        for byte in bytes.by_ref() {
            value = (value << 7) | u64::from(byte & 127);
            if byte & 128 == 0 {
                return value;
            }
        }
        panic!("truncated FTS5 structure record")
    };
    varint();
    let count = varint();
    let indexed: i64 = db
        .query_row(
            &format!("SELECT count(DISTINCT segid) FROM {index}_idx"),
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count as i64, indexed,
        "structure/segment-directory counts disagree"
    );
    count
}

fn begin(c: &Catalog, grant_id: i64, mode: &str) -> i64 {
    let WriteReply::Run(run) = c
        .writer()
        .call(WriteCommand::BeginRun {
            grant_id,
            mode: mode.into(),
            strategy: "synthetic-fts-merge".into(),
            started_at_ns: 1,
        })
        .unwrap()
    else {
        panic!("run receipt")
    };
    run
}

fn end(c: &Catalog, run_id: i64) -> f64 {
    let start = Instant::now();
    c.writer()
        .call(WriteCommand::EndRun {
            run_id,
            state: "completed".into(),
            finished_at_ns: 2,
        })
        .unwrap();
    start.elapsed().as_secs_f64() * 1000.0
}

fn listing(c: &Catalog, run_id: i64, dir_id: i64, files: Vec<Observation>, dirs: Vec<Observation>) {
    c.writer()
        .call(WriteCommand::DirListing(DirListing {
            run_id,
            dir_id,
            files,
            dirs,
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        }))
        .unwrap();
}

fn observation(name: String, id: u128) -> Observation {
    let mut o = Observation::file(name, 100, Some(4096));
    o.file_id = Some(id.to_le_bytes().to_vec());
    o.created_ft = Some(1);
    o
}

fn build(path: &Path, bulk: bool, directories: usize, per_dir: usize, ranked: bool) {
    let c = Catalog::open_with(
        path,
        "synthetic",
        &CatalogOptions {
            ranked_index: ranked,
        },
    )
    .unwrap();
    c.writer().set_bulk_first_scan(bulk);
    let WriteReply::Root {
        grant_id, dir_id, ..
    } = c
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "synthetic-fts-merge".into(),
            display_name: "Synthetic FTS merge".into(),
            display_path: "Synthetic FTS merge root".into(),
            root_file_id: Some(vec![0; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))
        .unwrap()
    else {
        panic!("root receipt")
    };
    let run = begin(&c, grant_id, "full");
    listing(
        &c,
        run,
        dir_id,
        vec![],
        (0..directories)
            .map(|i| observation(format!("directory-{i:04}"), i as u128 + 1))
            .collect(),
    );
    let db = Connection::open(path.join("catalog.db")).unwrap();
    let ids: Vec<i64> = db.prepare("SELECT id FROM dir WHERE parent_id=?1 UNION ALL SELECT id FROM bulk_dir WHERE parent_id=?1 ORDER BY id").unwrap().query_map([dir_id], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    drop(db);
    if !bulk {
        end(&c, run);
    }
    let mut ends = Vec::new();
    for (i, dir) in ids.into_iter().enumerate() {
        let run = if bulk {
            run
        } else {
            begin(&c, grant_id, "targeted")
        };
        let files = (0..per_dir)
            .map(|j| {
                let serial = i * per_dir + j;
                let word = [
                    "annual", "budget", "invoice", "photo", "archive", "meeting", "design",
                    "report",
                ][serial % 8];
                observation(
                    format!(
                        "{word}-{:016x}-draft.txt",
                        (serial as u64).wrapping_mul(0x9e3779b97f4a7c15)
                    ),
                    serial as u128 + 1_000_000,
                )
            })
            .collect();
        listing(&c, run, dir, files, vec![]);
        if !bulk {
            ends.push(end(&c, run));
        }
    }
    if bulk {
        ends.push(end(&c, run));
    }
    ends.sort_by(f64::total_cmp);
    eprintln!(
        "EndRun bulk={bulk}: p50_ms={:.3} max_ms={:.3}",
        ends[ends.len() / 2],
        ends.last().unwrap()
    );
}

fn measure(path: &Path, label: &str, ranked: bool) {
    let c = Catalog::open_with(
        path,
        "synthetic",
        &CatalogOptions {
            ranked_index: ranked,
        },
    )
    .unwrap();
    let mut reader = c.reader().unwrap();
    let request = SearchRequest {
        text: "report".into(),
        kind: "any".into(),
        limit: 20,
        work_budget: 50_000_000,
        scope: Some(vec![1]),
        ..Default::default()
    };
    let mut times = Vec::new();
    for _ in 0..2 {
        let start = Instant::now();
        let page = reader.search(&request).unwrap();
        assert_eq!(page.hits.len(), 20);
        assert!(!page.budget_hit);
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    eprintln!(
        "{label} client_root_1: name_segments={} token_segments={} first_ms={:.3} second_ms={:.3}",
        segments(path, "search_name"),
        if ranked {
            segments(path, "search_token")
        } else {
            0
        },
        times[0],
        times[1]
    );
}

#[test]
#[ignore = "#937 synthetic 1M-name publication/first-query benchmark"]
fn synthetic_fts_segments_first_query_bench() {
    for bulk in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let start = Instant::now();
        build(temp.path(), bulk, 1000, 1000, true);
        eprintln!(
            "build bulk={bulk}: seconds={:.3}",
            start.elapsed().as_secs_f64()
        );
        measure(temp.path(), if bulk { "bulk" } else { "incremental" }, true);
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        db.execute_batch("INSERT INTO search_name(search_name) VALUES('optimize'); INSERT INTO search_token(search_token) VALUES('optimize');").unwrap();
        drop(db);
        measure(temp.path(), "same store optimized", true);
    }
}
