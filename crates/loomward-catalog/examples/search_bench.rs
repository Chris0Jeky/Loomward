//! Search index benchmark (LW-113): generated synthetic names only, no filesystem enumeration.
//!
//! `search_bench --rows 1000000 [--no-index]` builds one root through the first-scan bulk path,
//! relists 100 directories as a watcher would (renames and removals), then times first pages of a
//! fixed query set. `--no-index` drops the index and its triggers right after open, for an A/B of
//! the write path; queries are skipped in that mode.
use loomward_catalog::*;
use rusqlite::Connection;
use serde_json::json;
use std::time::Instant;

const WORDS: [&str; 32] = [
    "annual", "report", "budget", "invoice", "photo", "holiday", "scan", "draft", "final",
    "backup", "project", "notes", "music", "video", "readme", "setup", "config", "data", "export",
    "image", "thumb", "cache", "log", "archive", "client", "meeting", "summary", "plan", "design",
    "spec", "café", "λ資料",
];
const EXTS: [(&str, &str); 12] = [
    ("pdf", "document"),
    ("docx", "document"),
    ("xlsx", "spreadsheet"),
    ("jpg", "image"),
    ("png", "image"),
    ("txt", "document"),
    ("mp3", "audio"),
    ("mp4", "video"),
    ("zip", "archive"),
    ("log", "data"),
    ("json", "data"),
    ("rs", "code"),
];

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}
fn name(rng: &mut Rng, serial: usize) -> (String, usize) {
    let words = 1 + rng.below(3) as usize;
    let sep = [" ", "_", "-", ""][rng.below(4) as usize];
    let mut parts: Vec<String> = (0..words)
        .map(|_| WORDS[rng.below(WORDS.len() as u64) as usize].to_string())
        .collect();
    if rng.below(10) < 6 {
        parts.push(format!("{}", rng.below(100_000)));
    }
    if rng.below(2) == 0 {
        if let Some(first) = parts.first_mut() {
            let mut c = first.chars();
            *first = c
                .next()
                .map_or(String::new(), |f| f.to_uppercase().chain(c).collect());
        }
    }
    let ext = rng.below(EXTS.len() as u64) as usize;
    // The serial keeps every name in a directory distinct.
    (format!("{}~{serial}.{}", parts.join(sep), EXTS[ext].0), ext)
}
fn file(rng: &mut Rng, serial: usize) -> Observation {
    let (n, ext) = name(rng, serial);
    let size = rng.below(2_000_000) + 1;
    let mut f = Observation::file(n, size, Some(size.div_ceil(4096) * 4096));
    f.extension = Some(EXTS[ext].0.into());
    f.family = EXTS[ext].1.into();
    f.file_id = Some((serial as u128 + 10_000_000).to_le_bytes().to_vec());
    f.created_ft = Some(serial as i64 + 1);
    f.modified_ft = Some(133_000_000_000_000_000 + serial as i64 * 10_000_000);
    f
}
fn summary(mut ms: Vec<f64>) -> serde_json::Value {
    ms.sort_by(f64::total_cmp);
    json!({"runs":ms.len(),"p50_ms":ms[ms.len()/2],"p95_ms":ms[(ms.len()*95).div_ceil(100)-1],"max_ms":ms[ms.len()-1]})
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 2 || args[0] != "--rows" {
        return Err("usage: search_bench --rows 1000000 [--no-index]".into());
    }
    let rows: usize = args[1].parse()?;
    let indexed = !args.iter().any(|a| a == "--no-index");
    if !(1000..=2_000_000).contains(&rows) {
        return Err("rows must be 1000..=2000000".into());
    }
    let temp = tempfile::tempdir()?;
    let c = Catalog::open(temp.path(), "synthetic")?;
    let db = Connection::open(temp.path().join("catalog.db"))?;
    if !indexed {
        db.execute_batch("DROP TRIGGER search_file_insert; DROP TRIGGER search_file_delete; DROP TRIGGER search_file_rename; DROP TRIGGER search_dir_insert; DROP TRIGGER search_dir_delete; DROP TRIGGER search_dir_rename; DROP TABLE search_name; DROP TRIGGER search_tok_file_insert; DROP TRIGGER search_tok_file_delete; DROP TRIGGER search_tok_file_rename; DROP TRIGGER search_tok_dir_insert; DROP TRIGGER search_tok_dir_delete; DROP TRIGGER search_tok_dir_rename; DROP TABLE search_token; DROP TABLE search_dirty;")?;
    }
    c.writer().set_bulk_first_scan(true);
    let WriteReply::Root {
        grant_id,
        dir_id,
        root_id,
    } = c
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_key: "synthetic-search-bench".into(),
            display_name: "Synthetic search bench".into(),
            display_path: "Synthetic search bench root".into(),
            root_file_id: Some(0_u128.to_le_bytes().to_vec()),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))?
    else {
        unreachable!()
    };
    let begin = |mode: &str| -> Result<i64> {
        match c.writer().call(WriteCommand::BeginRun {
            grant_id,
            mode: mode.into(),
            strategy: "in_process_synthetic".into(),
            started_at_ns: 0,
        })? {
            WriteReply::Run(run) => Ok(run),
            _ => unreachable!(),
        }
    };
    let run = begin("full")?;
    let directories = rows.div_ceil(1000);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let dirs = (0..directories)
        .map(|i| {
            let mut o = Observation::file(format!("{}-{i:05}", WORDS[i % 30]), 0, None);
            o.file_id = Some((i as u128 + 1).to_le_bytes().to_vec());
            o.created_ft = Some(1);
            o
        })
        .collect();
    let started = Instant::now();
    c.writer().call(WriteCommand::StageChunk {
        run_id: run,
        dir_id,
        seq: 0,
        files: vec![],
        dirs,
    })?;
    c.writer().call(WriteCommand::ListingDone {
        run_id: run,
        dir_id,
        outcome: ListingOutcome::Complete,
        skipped: 0,
        errors: 0,
    })?;
    let ids: Vec<i64> = {
        let mut s = db.prepare("SELECT id FROM dir WHERE parent_id=?1 UNION ALL SELECT id FROM bulk_dir WHERE parent_id=?1 ORDER BY id")?;
        let v = s
            .query_map([dir_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        v
    };
    let cancel = Cancellation::default();
    let mut receipts = Vec::new();
    let mut listings: Vec<Vec<Observation>> = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let count = 1000.min(rows - i * 1000);
        let files: Vec<_> = (0..count).map(|j| file(&mut rng, i * 1000 + j)).collect();
        if i < 100 {
            listings.push(files.clone());
        }
        let permit = c.writer().reserve_bytes(4 * 1024 * 1024, &cancel)?;
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
    }
    for r in receipts {
        r.wait()?;
    }
    let publish_s = started.elapsed().as_secs_f64();
    let started = Instant::now();
    c.writer().call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 1,
    })?;
    let end_run_s = started.elapsed().as_secs_f64();
    // A watcher's targeted relist of 100 directories: 10 renames and 10 removals in each.
    let run = begin("targeted")?;
    let started = Instant::now();
    for (i, mut files) in listings.into_iter().enumerate() {
        for f in files.iter_mut().take(10) {
            f.name = format!("renamed {}", f.name);
        }
        files.truncate(files.len().saturating_sub(10));
        c.writer().call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id: ids[i],
            files,
            dirs: vec![],
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        }))?;
    }
    let relist_s = started.elapsed().as_secs_f64();
    c.writer().call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 2,
    })?;
    db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    let catalog_bytes = std::fs::metadata(temp.path().join("catalog.db"))?.len();
    let index_bytes: i64 = if indexed {
        db.query_row(
            "SELECT coalesce(sum(pgsize),0) FROM dbstat WHERE name LIKE 'search_name%'",
            [],
            |r| r.get(0),
        )?
    } else {
        0
    };
    let entries = rows + directories;
    let mut receipt = json!({
        "scope": "in-process generated synthetic names; no enumeration, no HTTP",
        "indexed": indexed,
        "files": rows, "directories": directories + 1,
        "publish_seconds": publish_s,
        "end_run_seconds": end_run_s,
        "through_end_run_rows_per_second": entries as f64 / (publish_s + end_run_s),
        "targeted_relist_100_dirs_seconds": relist_s,
        "catalog_bytes": catalog_bytes,
        "catalog_bytes_per_entry": catalog_bytes as f64 / entries as f64,
        "index_bytes": index_bytes,
        "index_bytes_per_entry": index_bytes as f64 / entries as f64,
    });
    if indexed {
        let mut reader = c.reader()?;
        let mut queries = serde_json::Map::new();
        let cases: Vec<(&str, SearchRequest)> = vec![
            (
                "word",
                SearchRequest {
                    text: "budget".into(),
                    ..Default::default()
                },
            ),
            (
                "inner_substring",
                SearchRequest {
                    text: "udge".into(),
                    ..Default::default()
                },
            ),
            (
                "digits",
                SearchRequest {
                    text: "4242".into(),
                    ..Default::default()
                },
            ),
            (
                "two_words",
                SearchRequest {
                    text: "annual report".into(),
                    ..Default::default()
                },
            ),
            (
                "word_and_extension",
                SearchRequest {
                    text: "invoice".into(),
                    extension: Some("pdf".into()),
                    ..Default::default()
                },
            ),
            (
                "word_size_and_time",
                SearchRequest {
                    text: "photo".into(),
                    min_bytes: Some(1_900_000),
                    modified_after: Some(133_000_000_000_000_000 + 500_000 * 10_000_000),
                    ..Default::default()
                },
            ),
            (
                "extension_only_largest",
                SearchRequest {
                    text: String::new(),
                    extension: Some("mp4".into()),
                    ..Default::default()
                },
            ),
            (
                "two_chars",
                SearchRequest {
                    text: "pl".into(),
                    ..Default::default()
                },
            ),
            (
                "no_match",
                SearchRequest {
                    text: "zzqqxx".into(),
                    ..Default::default()
                },
            ),
            (
                "rare_filter",
                SearchRequest {
                    text: "report".into(),
                    min_bytes: Some(1_999_990),
                    ..Default::default()
                },
            ),
        ];
        for (label, mut r) in cases {
            r.kind = "any".into();
            r.limit = 50;
            r.root_id = Some(root_id);
            r.work_budget = 50_000_000;
            r.time_budget = Some(std::time::Duration::from_millis(250));
            let page = reader.search(&r)?;
            let mut ms = Vec::new();
            for _ in 0..21 {
                let t = Instant::now();
                let page = reader.search(&r)?;
                serde_json::to_vec(&page.hits)?;
                ms.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            queries.insert(label.into(), json!({"text": r.text, "hits": page.hits.len(), "first_kind": page.hits.first().map(|h| h.match_kind.clone()), "budget_hit": page.budget_hit, "more": page.next_cursor.is_some(), "timing": summary(ms)}));
        }
        receipt["queries"] = json!(queries);
        receipt["query_note"] = json!("first page, limit 50, warm, 21 runs after one warm-up, including hit serialisation; P8 target p95 300 ms");
    }
    drop(db);
    drop(c);
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}
