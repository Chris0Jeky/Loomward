//! Client-scoped name search benchmark (#887, #916): generated synthetic names only.
//!
//! `scoped_search_bench --dir <empty or existing folder> [--build 1000000]` builds (with `--build`)
//! one large "lab" root holding 85 percent of the entries, then twelve small roots, through the
//! first-scan bulk path; the lab root is built first, so its keys come first in index order. Every
//! run then times the open (the migration, on a store built by an older engine) and first pages
//! for three principals: the owner, a client granted every root, and a client granted only the
//! small roots, with the service's budgets (50M VM steps, 250 ms soft cut, limit 20).
use loomward_catalog::*;
use serde_json::json;
use std::time::Instant;

const WORDS: [&str; 30] = [
    "annual", "report", "budget", "invoice", "photo", "holiday", "scan", "draft", "final",
    "backup", "project", "notes", "music", "video", "readme", "setup", "config", "data", "export",
    "image", "thumb", "cache", "log", "archive", "client", "meeting", "summary", "plan", "design",
    "spec",
];
const EXTS: [&str; 8] = ["pdf", "docx", "jpg", "png", "txt", "mp3", "zip", "json"];
const SMALL_ROOTS: usize = 12;

struct Rng(u64);
impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}
/// A generated name; `lab` names carry `labonly` one time in twenty, small-root names `zephyrine`
/// one time in twenty thousand.
fn name(rng: &mut Rng, serial: usize, lab: bool) -> String {
    let words = 1 + rng.below(3) as usize;
    let sep = [" ", "_", "-", ""][rng.below(4) as usize];
    let mut parts: Vec<String> = (0..words)
        .map(|_| WORDS[rng.below(WORDS.len() as u64) as usize].to_string())
        .collect();
    if lab && serial.is_multiple_of(20) {
        parts.push("labonly".into());
    }
    if !lab && serial % 20_000 == 7 {
        parts.push("zephyrine".into());
    }
    let ext = EXTS[rng.below(EXTS.len() as u64) as usize];
    format!("{}~{serial}.{ext}", parts.join(sep))
}

fn build(
    c: &Catalog,
    at: &std::path::Path,
    rows: usize,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    c.writer().set_bulk_first_scan(true);
    let lab_rows = rows * 85 / 100;
    let small = (rows - lab_rows) / SMALL_ROOTS;
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut serial = 0usize;
    for r in 0..=SMALL_ROOTS {
        let (label, count) = if r == 0 {
            ("lab".to_string(), lab_rows)
        } else {
            (format!("small-{r:02}"), small)
        };
        let WriteReply::Root {
            grant_id, dir_id, ..
        } = c
            .writer()
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: format!("synthetic-scoped-bench-{label}"),
                display_name: format!("Synthetic {label}"),
                display_path: format!("Synthetic scoped bench {label}"),
                root_file_id: Some((r as u128 + 1).to_le_bytes().to_vec()),
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
        let folders = count.div_ceil(1000);
        let dirs = (0..folders)
            .map(|i| {
                let mut o = Observation::file(format!("{}-{i:05}", WORDS[i % 30]), 0, None);
                o.file_id = Some(
                    (serial as u128 + i as u128 + 1_000_000_000)
                        .to_le_bytes()
                        .to_vec(),
                );
                o.created_ft = Some(1);
                o
            })
            .collect();
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
            let db = rusqlite::Connection::open(at.join("catalog.db"))?;
            let mut s = db.prepare("SELECT id FROM dir WHERE parent_id=?1 UNION ALL SELECT id FROM bulk_dir WHERE parent_id=?1 ORDER BY id")?;
            let v = s
                .query_map([dir_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            v
        };
        for (i, id) in ids.iter().enumerate() {
            let n = 1000.min(count - i * 1000);
            let files = (0..n)
                .map(|_| {
                    serial += 1;
                    let mut f = Observation::file(name(&mut rng, serial, r == 0), 1, Some(4096));
                    f.file_id = Some((serial as u128 + 10_000_000).to_le_bytes().to_vec());
                    f.created_ft = Some(serial as i64);
                    f.modified_ft = Some(133_000_000_000_000_000 + serial as i64);
                    f
                })
                .collect();
            c.writer().call(WriteCommand::StageChunk {
                run_id: run,
                dir_id: *id,
                seq: 0,
                files,
                dirs: vec![],
            })?;
            c.writer().call(WriteCommand::ListingDone {
                run_id: run,
                dir_id: *id,
                outcome: ListingOutcome::Complete,
                skipped: 0,
                errors: 0,
            })?;
        }
        serial += folders;
        let started = Instant::now();
        c.writer().call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 1,
        })?;
        eprintln!(
            "root {label}: {count} files, EndRun {:.2} s",
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let arg = |f: &str| {
        args.iter()
            .position(|a| a == f)
            .and_then(|i| args.get(i + 1))
    };
    let dir = std::path::PathBuf::from(arg("--dir").ok_or("usage: --dir <folder> [--build rows]")?);
    std::fs::create_dir_all(&dir)?;
    let started = Instant::now();
    let c = Catalog::open(&dir, "synthetic")?;
    let open_s = started.elapsed().as_secs_f64();
    eprintln!("open: {open_s:.2} s");
    if let Some(rows) = arg("--build") {
        let rows: usize = rows.parse()?;
        if !(1000..=10_000_000).contains(&rows) {
            return Err("rows must be 1000..=10000000".into());
        }
        build(&c, &dir, rows)?;
    }
    let roots: Vec<i64> = {
        let db = rusqlite::Connection::open(dir.join("catalog.db"))?;
        let mut s = db.prepare("SELECT id FROM root ORDER BY id")?;
        let v = s
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        v
    };
    let scopes: [(&str, Option<Vec<i64>>); 3] = [
        ("owner", None),
        ("client_all", Some(roots.clone())),
        ("client_small", Some(roots[1..].to_vec())),
    ];
    let mut reader = c.reader()?;
    let mut out = serde_json::Map::new();
    for text in [
        "report",
        "jpg",
        "zephyrine",
        "labonly",
        "zzqqxx",
        "annual report",
    ] {
        for (who, scope) in &scopes {
            let req = SearchRequest {
                text: text.into(),
                kind: "any".into(),
                limit: 20,
                work_budget: 50_000_000,
                time_budget: Some(std::time::Duration::from_millis(250)),
                scope: scope.clone(),
                ..Default::default()
            };
            let mut ms = Vec::new();
            let mut last = None;
            for _ in 0..7 {
                let started = Instant::now();
                let page = reader.search(&req)?;
                ms.push(started.elapsed().as_secs_f64() * 1000.0);
                last = Some((page.hits.len(), page.budget_hit));
            }
            let first = ms[0];
            ms.sort_by(f64::total_cmp);
            let (rows, hit) = last.unwrap_or_default();
            out.insert(
                format!("{text} / {who}"),
                json!({"rows": rows, "budget_hit": hit, "first_ms": first, "median_ms": ms[3], "max_ms": ms[6]}),
            );
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"open_seconds": open_s, "queries": out}))?
    );
    Ok(())
}
