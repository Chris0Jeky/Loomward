//! Find relevance and latency harness (LW-113 F2, docs/46 section 9).
//!
//! `search_relevance --filler 1000000 [--seed 42] [--runs 21] [--out receipt.json] [--dir store]`
//! builds the semantic corpus (loomward-lab `semantic`, synthetic names only) in an in-process
//! catalogue through the first-scan bulk path, then
//! - scores `search` (v1 pass order) and `search_ranked` (each tier alone, then fused) on the
//!   labelled queries: MRR@10, nDCG@10, recall@50 per class and per group;
//! - times first pages of a fixed query set for v1 and fused ranking (P8: 300 ms p95 or
//!   `budget_hit`; token and prefix queries 50 ms p95 at 1M).
//!
//! No filesystem enumeration, no HTTP. Timings are warm and uncontrolled, as for `search_bench`.
use loomward_catalog::*;
use loomward_lab::semantic::{self, metrics, Class, Corpus, FileSpec, Query};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

type Res<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const DAY: i64 = 864_000_000_000;
const TOP: usize = 50;

fn now_ft() -> i64 {
    filetime("2026-10-17T00:00:00Z").expect("a valid time")
}
struct Ids(u128);
impl Ids {
    fn next(&mut self) -> Vec<u8> {
        self.0 += 1;
        self.0.to_le_bytes().to_vec()
    }
}
fn file_obs(ids: &mut Ids, f: &FileSpec) -> Observation {
    let mut o = Observation::file(f.name.clone(), f.size, Some(f.size.div_ceil(4096) * 4096));
    o.extension = f.name.rsplit_once('.').map(|(_, e)| e.to_lowercase());
    o.family = "data".into();
    o.file_id = Some(ids.next());
    o.created_ft = Some(1);
    o.modified_ft = Some(now_ft() - i64::from(f.age_days) * DAY);
    o
}
fn dir_obs(ids: &mut Ids, name: &str) -> Observation {
    let mut o = Observation::file(name.to_string(), 0, None);
    o.file_id = Some(ids.next());
    o.created_ft = Some(1);
    o.modified_ft = Some(now_ft() - 30 * DAY);
    o
}

#[derive(Default)]
struct Node {
    files: Vec<FileSpec>,
    kids: BTreeMap<String, Node>,
}
fn node<'a>(root: &'a mut Node, path: &str) -> &'a mut Node {
    path.split('\\')
        .filter(|p| !p.is_empty())
        .fold(root, |n, part| n.kids.entry(part.to_string()).or_default())
}
fn plant(c: &Corpus) -> Node {
    let mut root = Node::default();
    for d in &c.planted_dirs {
        node(&mut root, d);
    }
    for (d, f) in &c.planted_files {
        node(&mut root, d).files.push(f.clone());
    }
    root
}

struct Store<'a> {
    c: &'a Catalog,
    db: &'a Connection,
    run: i64,
    ids: Ids,
}
impl Store<'_> {
    fn children(&self, dir: i64) -> Res<BTreeMap<String, i64>> {
        let mut s = self.db.prepare("SELECT name,id FROM dir WHERE parent_id=?1 UNION ALL SELECT name,id FROM bulk_dir WHERE parent_id=?1")?;
        let rows = s
            .query_map([dir], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
    /// Stage one planted folder and everything below it.
    fn tree(&mut self, dir: i64, node: &Node) -> Res<()> {
        let files = node
            .files
            .iter()
            .map(|f| file_obs(&mut self.ids, f))
            .collect();
        let dirs = node
            .kids
            .keys()
            .map(|k| dir_obs(&mut self.ids, k))
            .collect();
        self.c.writer().call(WriteCommand::StageChunk {
            run_id: self.run,
            dir_id: dir,
            seq: 0,
            files,
            dirs,
        })?;
        self.c.writer().call(WriteCommand::ListingDone {
            run_id: self.run,
            dir_id: dir,
            outcome: ListingOutcome::Complete,
            skipped: 0,
            errors: 0,
        })?;
        let ids = self.children(dir)?;
        for (name, kid) in &node.kids {
            self.tree(ids[name], kid)?;
        }
        Ok(())
    }
}

fn stats(mut ms: Vec<f64>) -> Value {
    ms.sort_by(f64::total_cmp);
    json!({"runs": ms.len(), "p50_ms": ms[ms.len() / 2], "p95_ms": ms[(ms.len() * 95).div_ceil(100) - 1], "max_ms": ms[ms.len() - 1]})
}

#[derive(Clone, Copy)]
enum System {
    V1,
    Ranked(u8),
}
impl System {
    fn label(self) -> String {
        match self {
            System::V1 => "v1_passes".into(),
            System::Ranked(1) => "ranked_trigram_only".into(),
            System::Ranked(2) => "ranked_token_only".into(),
            System::Ranked(4) => "ranked_context_only".into(),
            System::Ranked(3) => "ranked_trigram_plus_token".into(),
            System::Ranked(7) => "ranked_fused".into(),
            System::Ranked(t) => format!("ranked_tiers_{t}"),
        }
    }
}
fn request(root: i64, text: &str, extension: Option<&str>, max_age: Option<u32>) -> SearchRequest {
    SearchRequest {
        root_id: Some(root),
        text: text.into(),
        extension: extension.map(Into::into),
        modified_after: max_age.map(|d| now_ft() - i64::from(d) * DAY),
        kind: "any".into(),
        limit: TOP,
        work_budget: 50_000_000,
        time_budget: Some(Duration::from_millis(250)),
        ..SearchRequest::default()
    }
}
fn run(reader: &mut Reader, sys: System, r: &SearchRequest) -> Res<SearchPage> {
    Ok(match sys {
        System::V1 => reader.search(r)?,
        System::Ranked(tiers) => reader.search_ranked(
            r,
            &RankOptions {
                now_ft: Some(now_ft()),
                tiers,
            },
        )?,
    })
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}
#[derive(Default)]
struct Acc {
    rr: Vec<f64>,
    ndcg: Vec<f64>,
    recall: Vec<f64>,
}
impl Acc {
    fn add(&mut self, q: &Query, ranked: &[String]) -> (f64, f64, f64) {
        let m = (
            metrics::reciprocal_rank(q, ranked, 10),
            metrics::ndcg(q, ranked, 10),
            metrics::recall(q, ranked, TOP),
        );
        self.rr.push(m.0);
        self.ndcg.push(m.1);
        self.recall.push(m.2);
        m
    }
    fn json(&self) -> Value {
        json!({"queries": self.rr.len(), "mrr_at_10": round(mean(&self.rr)), "ndcg_at_10": round(mean(&self.ndcg)), "recall_at_50": round(mean(&self.recall))})
    }
}
fn round(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str| -> Option<&String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
    };
    let filler: usize = arg("--filler").map_or(Ok(100_000), |s| s.parse())?;
    let seed: u64 = arg("--seed").map_or(Ok(42), |s| s.parse())?;
    let runs: usize = arg("--runs").map_or(Ok(21), |s| s.parse())?;
    if !(1000..=20_000_000).contains(&filler) {
        return Err("--filler must be 1000..=20000000".into());
    }
    let corpus = semantic::generate(seed, filler);
    let temp;
    let dir = match arg("--dir") {
        Some(d) => std::path::PathBuf::from(d),
        None => {
            temp = tempfile::tempdir()?;
            temp.path().to_path_buf()
        }
    };
    // Ranked search needs the token index, which a catalogue builds only when asked.
    let catalog = Catalog::open_with(&dir, "synthetic", &CatalogOptions { ranked_index: true })?;
    let db = Connection::open(dir.join("catalog.db"))?;
    catalog.writer().set_bulk_first_scan(true);
    let WriteReply::Root {
        grant_id,
        dir_id,
        root_id,
    } = catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_key: "synthetic-semantic-corpus".into(),
            display_name: "Synthetic semantic corpus".into(),
            display_path: "Synthetic semantic corpus root".into(),
            root_file_id: Some(0_u128.to_le_bytes().to_vec()),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))?
    else {
        unreachable!()
    };
    let WriteReply::Run(run_id) = catalog.writer().call(WriteCommand::BeginRun {
        grant_id,
        mode: "full".into(),
        strategy: "in_process_synthetic".into(),
        started_at_ns: 0,
    })?
    else {
        unreachable!()
    };
    let started = Instant::now();
    let mut store = Store {
        c: &catalog,
        db: &db,
        run: run_id,
        ids: Ids(10_000_000),
    };
    // The root lists the planted top-level folders and every filler folder.
    let planted = plant(&corpus);
    let filler_dirs: Vec<String> = (0..corpus.filler_dirs())
        .map(|d| corpus.filler_dir_name(d))
        .collect();
    let mut top: Vec<Observation> = planted
        .kids
        .keys()
        .map(|k| dir_obs(&mut store.ids, k))
        .collect();
    top.extend(filler_dirs.iter().map(|k| dir_obs(&mut store.ids, k)));
    catalog.writer().call(WriteCommand::StageChunk {
        run_id,
        dir_id,
        seq: 0,
        files: vec![],
        dirs: top,
    })?;
    catalog.writer().call(WriteCommand::ListingDone {
        run_id,
        dir_id,
        outcome: ListingOutcome::Complete,
        skipped: 0,
        errors: 0,
    })?;
    let top_ids = store.children(dir_id)?;
    for (name, node) in &planted.kids {
        store.tree(top_ids[name], node)?;
    }
    let cancel = Cancellation::default();
    let mut receipts = Vec::new();
    for (d, name) in filler_dirs.iter().enumerate() {
        let first = d * semantic::FILLER_PER_DIR;
        let last = ((d + 1) * semantic::FILLER_PER_DIR).min(filler);
        let files: Vec<_> = (first..last)
            .map(|i| file_obs(&mut store.ids, &corpus.filler_file(i)))
            .collect();
        let permit = catalog.writer().reserve_bytes(4 * 1024 * 1024, &cancel)?;
        receipts.push(catalog.writer().send_reserved(
            WriteCommand::StageChunk {
                run_id,
                dir_id: top_ids[name],
                seq: 0,
                files,
                dirs: vec![],
            },
            permit,
            &cancel,
        )?);
        receipts.push(catalog.writer().send(WriteCommand::ListingDone {
            run_id,
            dir_id: top_ids[name],
            outcome: ListingOutcome::Complete,
            skipped: 0,
            errors: 0,
        })?);
    }
    for r in receipts {
        r.wait()?;
    }
    let publish_s = started.elapsed().as_secs_f64();
    let t = Instant::now();
    catalog.writer().call(WriteCommand::EndRun {
        run_id,
        state: "completed".into(),
        finished_at_ns: 1,
    })?;
    let end_run_s = t.elapsed().as_secs_f64();
    db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    let entries: i64 = db.query_row(
        "SELECT (SELECT count(*) FROM file)+(SELECT count(*) FROM dir WHERE parent_id IS NOT NULL)",
        [],
        |r| r.get(0),
    )?;
    let bytes = |prefix: &str| -> Res<i64> {
        Ok(db.query_row(
            "SELECT coalesce(sum(pgsize),0) FROM dbstat WHERE name LIKE ?1",
            [format!("{prefix}%")],
            |r| r.get(0),
        )?)
    };
    let (trigram_bytes, token_bytes) = (bytes("search_name")?, bytes("search_token")?);
    eprintln!("built {entries} entries in {:.1}s", publish_s + end_run_s);

    let mut reader = catalog.reader()?;
    let systems = [
        System::V1,
        System::Ranked(1),
        System::Ranked(2),
        System::Ranked(4),
        System::Ranked(3),
        System::Ranked(7),
    ];
    let mut relevance = serde_json::Map::new();
    let mut per_query: Vec<Value> = Vec::new();
    for sys in systems {
        let mut all = Acc::default();
        let mut by_class: BTreeMap<&str, Acc> = BTreeMap::new();
        let mut by_group: BTreeMap<&str, Acc> = BTreeMap::new();
        let mut budget_hits = 0;
        for q in &corpus.queries {
            let page = run(
                &mut reader,
                sys,
                &request(root_id, &q.text, q.extension.as_deref(), q.max_age_days),
            )?;
            budget_hits += usize::from(page.budget_hit);
            let ranked: Vec<String> = page.hits.iter().map(|h| h.path.text.clone()).collect();
            let m = all.add(q, &ranked);
            by_class.entry(q.class.label()).or_default().add(q, &ranked);
            by_group.entry(q.group).or_default().add(q, &ranked);
            if matches!(sys, System::V1 | System::Ranked(7)) {
                per_query.push(json!({"system": sys.label(), "class": q.class.label(), "group": q.group, "text": q.text, "extension": q.extension, "max_age_days": q.max_age_days, "relevant": q.relevant.len(), "rr": round(m.0), "ndcg": round(m.1), "recall50": round(m.2), "returned": ranked.len(), "budget_hit": page.budget_hit}));
            }
        }
        relevance.insert(
            sys.label(),
            json!({
                "overall": all.json(),
                "by_class": by_class.iter().map(|(k, v)| (k.to_string(), v.json())).collect::<BTreeMap<_, _>>(),
                "by_group": by_group.iter().map(|(k, v)| (k.to_string(), v.json())).collect::<BTreeMap<_, _>>(),
                "budget_hit_queries": budget_hits,
            }),
        );
    }

    // Latency: first pages at limit 50 as the service asks, warm, after one warm-up run.
    let fk = corpus
        .queries
        .iter()
        .find(|q| q.group == "filler")
        .map(|q| q.text.clone())
        .unwrap_or_default();
    let word = corpus.vocab_word(7).to_string();
    let prefix: String = corpus.vocab_word(11).chars().take(4).collect();
    let inner: String = corpus.vocab_word(13).chars().skip(1).take(4).collect();
    let cases: Vec<(&str, String, Option<&str>)> = vec![
        ("word", word.clone(), None),
        ("prefix_4", prefix, None),
        ("inner_substring", inner, None),
        ("digits", format!("{}", filler / 3), None),
        ("two_words_known_item", fk, None),
        ("word_and_extension", word, Some("pdf")),
        ("two_chars", "ka".into(), None),
        ("one_char", "k".into(), None),
        ("no_match", "zzqqxx".into(), None),
        ("concept_folder_context", "invoices 2023".into(), None),
        ("camel_hump", "texture pack".into(), None),
    ];
    let mut latency = serde_json::Map::new();
    for (label, text, ext) in cases {
        let mut row = serde_json::Map::new();
        for sys in [System::V1, System::Ranked(7)] {
            let r = request(root_id, &text, ext, None);
            let page = run(&mut reader, sys, &r)?;
            let mut ms = Vec::new();
            for _ in 0..runs {
                let t = Instant::now();
                let page = run(&mut reader, sys, &r)?;
                serde_json::to_vec(&page.hits)?;
                ms.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            row.insert(
                sys.label(),
                json!({"hits": page.hits.len(), "budget_hit": page.budget_hit, "more": page.next_cursor.is_some(), "timing": stats(ms)}),
            );
        }
        row.insert("text".into(), json!(text));
        latency.insert(label.into(), Value::Object(row));
    }

    drop(reader);
    drop(db);
    drop(catalog);
    let counts = |c: Class| corpus.queries.iter().filter(|q| q.class == c).count();
    let receipt = json!({
        "lane": "LW-113 F2 relevance and latency",
        "scope": "in-process generated synthetic names (loomward-lab semantic corpus); no enumeration, no HTTP; warm, uncontrolled timings on a shared host",
        "corpus": {
            "seed": seed, "filler_files": filler, "entries": entries,
            "planted_files": corpus.planted_files.len(), "planted_dirs": corpus.planted_dirs.len(),
            "queries": corpus.queries.len(),
            "known_item": counts(Class::KnownItem), "attribute": counts(Class::Attribute), "concept": counts(Class::Concept),
            "relevance_at_ms_budget": 250,
        },
        "build": {
            "publish_seconds": publish_s, "end_run_seconds": end_run_s,
            "trigram_index_bytes": trigram_bytes, "trigram_bytes_per_entry": trigram_bytes as f64 / entries as f64,
            "token_index_bytes": token_bytes, "token_bytes_per_entry": token_bytes as f64 / entries as f64,
        },
        "relevance": relevance,
        "latency": latency,
        "queries": per_query,
    });
    let text = serde_json::to_string_pretty(&receipt)?;
    match arg("--out") {
        Some(path) => std::fs::write(path, text + "\n")?,
        None => println!("{text}"),
    }
    Ok(())
}
