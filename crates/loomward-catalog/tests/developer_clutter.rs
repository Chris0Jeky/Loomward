use loomward_catalog::*;
use rusqlite::{params, Connection};

// Deterministic lab catalogue: generated project trees, never a user's inventory.
struct Lab {
    _temp: tempfile::TempDir,
    catalog: Catalog,
    db: Connection,
    grant: i64,
    root: i64,
    dir: i64,
}
impl Lab {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(temp.path(), "synthetic").unwrap();
        let WriteReply::Root {
            grant_id,
            root_id,
            dir_id,
        } = catalog
            .writer()
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_key: "synthetic-lab".into(),
                display_name: "Lab".into(),
                display_path: "Synthetic project lab".into(),
                root_file_id: Some(vec![0; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap()
        else {
            panic!()
        };
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        db.execute(
            "UPDATE dir SET listing_state='complete',sub_complete=1 WHERE id=?1",
            [dir_id],
        )
        .unwrap();
        Self {
            _temp: temp,
            catalog,
            db,
            grant: grant_id,
            root: root_id,
            dir: dir_id,
        }
    }
    fn directory(&self, parent: i64, name: &str, bytes: i64) -> i64 {
        self.db.execute("INSERT INTO dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,flags,listing_state,born_run,seen_run,sub_complete,sub_logical,sub_allocated) VALUES(?1,?2,?3,'path_observation','none',1,16,0,'complete',1,1,1,?4,?4)", params![self.root,parent,name,bytes]).unwrap();
        self.db.last_insert_rowid()
    }
    fn manifest(&self, parent: i64, name: &str) -> i64 {
        self.db.execute("INSERT INTO file(dir_id,name,logical,attrs,flags,born_run,seen_run) VALUES(?1,?2,12,32,0,1,1)", params![parent,name]).unwrap();
        self.db.last_insert_rowid()
    }
    fn report(&self, cap: usize) -> DeveloperClutter {
        self.catalog
            .reader()
            .unwrap()
            .developer_clutter(self.root, cap)
            .unwrap()
    }
}

#[test]
fn only_manifest_and_adjacent_output_pairs_qualify_and_sizes_rank() {
    let lab = Lab::new();
    let providers = [
        ("Cargo.toml", "target", "cargo"),
        ("package.json", "node_modules", "npm"),
        ("pyproject.toml", ".venv", "python"),
        ("pyproject.toml", "venv", "python"),
        ("requirements.txt", ".venv", "python"),
        ("requirements.txt", "venv", "python"),
        ("requirements.txt", "__pycache__", "python"),
        ("pyproject.toml", "__pycache__", "python"),
        ("demo.csproj", "bin", "dotnet"),
        ("demo.csproj", "obj", "dotnet"),
        ("demo.sln", "bin", "dotnet"),
        ("demo.sln", "obj", "dotnet"),
        ("build.gradle", "build", "gradle"),
        ("build.gradle", ".gradle", "gradle"),
        ("build.gradle.kts", "build", "gradle"),
        ("build.gradle.kts", ".gradle", "gradle"),
    ];
    for (i, (manifest, output, provider)) in providers.iter().enumerate() {
        let project = lab.directory(lab.dir, &format!("project-{i}"), 0);
        let anchor = lab.manifest(project, manifest);
        let candidate = lab.directory(project, output, (i + 1) as i64 * 100);
        // Neither valuable source/datasets nor arbitrary compiled-looking names qualify.
        for name in ["src", "datasets", "model-files", "dist", "objects.dll"] {
            lab.directory(project, name, 999_999);
        }
        let report = lab.report(200);
        let row = report
            .candidates
            .iter()
            .find(|c| c.directory == NodeKey::Dir(candidate))
            .unwrap();
        assert_eq!(row.anchor, NodeKey::File(anchor));
        assert_eq!(row.anchor_name, *manifest);
        assert_eq!(row.provider, *provider);
        assert_eq!(
            row.kind,
            match *output {
                "node_modules" => "dependency_install",
                ".venv" | "venv" => "environment",
                "__pycache__" => "bytecode_cache",
                ".gradle" => "build_cache",
                _ => "build_output",
            }
        );
        assert_eq!(row.regeneration_cost, "unknown");
        assert!(!row.regeneration_hint.is_empty());
        assert!(row.reason.contains("conventional"));
        assert!(row.uncertainty.contains("unique"));
    }
    let unanchored = lab.directory(lab.dir, "unanchored", 0);
    for name in ["target", "node_modules", "bin", "build", ".venv"] {
        lab.directory(unanchored, name, 999_999);
    }
    let go = lab.directory(lab.dir, "go-project", 0);
    lab.manifest(go, "go.mod");
    lab.directory(go, "bin", 999_999); // Go has no universal local output directory.
    let report = lab.report(200);
    assert_eq!(report.candidates.len(), providers.len());
    assert!(!report.truncated);
    assert!(report
        .candidates
        .windows(2)
        .all(|p| p[0].logical_bytes >= p[1].logical_bytes));
    assert_eq!(report.candidates[0].logical_bytes, Some(1600));
    assert_eq!(report.candidates[0].allocated_bytes, Some(1600));
    assert_eq!(lab.report(2).candidates.len(), 2);
    assert!(lab.report(2).truncated);
}

#[test]
fn incomplete_and_unknown_allocations_do_not_become_exact_or_zero() {
    let lab = Lab::new();
    lab.manifest(lab.dir, "Cargo.toml");
    let target = lab.directory(lab.dir, "target", 42);
    lab.db
        .execute("UPDATE dir SET sub_alloc_unknown=1 WHERE id=?1", [target])
        .unwrap();
    let report = lab.report(200);
    assert!(report.partial);
    assert_eq!(report.candidates[0].logical_bytes, Some(42));
    assert_eq!(report.candidates[0].allocated_bytes, None);
    lab.db
        .execute(
            "UPDATE dir SET sub_complete=0,listing_state='incomplete' WHERE id=?1",
            [target],
        )
        .unwrap();
    let report = lab.report(200);
    assert!(report.partial);
    assert_eq!(report.candidates[0].logical_bytes, None);
    assert_eq!(report.candidates[0].allocated_bytes, None);
    assert_eq!(report.candidates[0].coverage, "partial");
    lab.db
        .execute(
            "UPDATE dir SET listing_state='unlisted' WHERE id=?1",
            [target],
        )
        .unwrap();
    assert_eq!(lab.report(200).candidates[0].logical_bytes, None);
    let project = lab.directory(lab.dir, "known-empty-project", 0);
    lab.manifest(project, "package.json");
    let empty = lab.directory(project, "node_modules", 0);
    let report = lab.report(200);
    assert_eq!(report.candidates[0].directory, NodeKey::Dir(empty));
    assert_eq!(report.candidates[0].logical_bytes, Some(0));
    assert_eq!(report.candidates[1].logical_bytes, None);
}

#[test]
fn exclusions_stale_anchors_and_output_descendants_are_shielded() {
    let lab = Lab::new();
    let project = lab.directory(lab.dir, "project", 0);
    lab.manifest(project, "package.json");
    lab.manifest(project, "Cargo.toml");
    let output = lab.directory(project, "node_modules", 100);
    lab.manifest(output, "package.json");
    lab.directory(output, "node_modules", 50);
    lab.manifest(project, "pyproject.toml");
    lab.manifest(project, "requirements.txt");
    lab.directory(project, "venv", 20); // Multiple anchors must not duplicate a directory.
    let wrong = lab.directory(project, "source", 0);
    lab.directory(wrong, "target", 10); // Not adjacent to Cargo.toml.
    for (i, (flags, attrs, state)) in [
        (EXCLUDED_FLAGS, 16, "complete"),
        (0, 16 | 1024, "complete"),
        (1, 16, "complete"),
        (0, 16, "excluded"),
        (0, 16, "absent_pending"),
    ]
    .into_iter()
    .enumerate()
    {
        let hidden = lab.directory(lab.dir, &format!("hidden-{i}"), 0);
        lab.manifest(hidden, "Cargo.toml");
        lab.directory(hidden, "target", 1000);
        lab.db
            .execute(
                "UPDATE dir SET flags=?2,attrs=?3,listing_state=?4 WHERE id=?1",
                params![hidden, flags, attrs, state],
            )
            .unwrap();
    }
    let stale = lab.directory(lab.dir, "stale", 0);
    let anchor = lab.manifest(stale, "Cargo.toml");
    lab.directory(stale, "target", 1000);
    lab.db
        .execute(
            "UPDATE dir SET listing_state='incomplete',seen_run=2 WHERE id=?1",
            [stale],
        )
        .unwrap();
    assert_eq!(lab.report(200).candidates.len(), 2);
    lab.db
        .execute(
            "UPDATE file SET seen_run=2,attrs=1024 WHERE id=?1",
            [anchor],
        )
        .unwrap();
    assert_eq!(lab.report(200).candidates.len(), 2);
    lab.catalog
        .writer()
        .call(WriteCommand::RevokeGrant {
            grant_id: lab.grant,
            revoked_at_ns: 1,
        })
        .unwrap();
    assert!(matches!(
        lab.catalog
            .reader()
            .unwrap()
            .developer_clutter(lab.root, 200),
        Err(Error::NotFound)
    ));
}

#[test]
fn bounds_and_deadlines_are_enforced() {
    let lab = Lab::new();
    let mut reader = lab.catalog.reader().unwrap();
    for cap in [0, 201] {
        assert!(matches!(
            reader.developer_clutter(lab.root, cap),
            Err(Error::Invalid(_))
        ));
    }
    reader.set_deadline(std::time::Instant::now());
    assert!(matches!(
        reader.developer_clutter(lab.root, 200),
        Err(Error::Cancelled)
    ));
}

#[test]
fn candidate_shields_preserve_an_adjacent_positive_control() {
    let lab = Lab::new();
    lab.manifest(lab.dir, "Cargo.toml");
    let target = lab.directory(lab.dir, "target", 42);
    for (flags, attrs, state) in [
        (EXCLUDED_FLAGS, 16, "complete"),
        (0, 16 | 1024, "complete"),
        (0, 16, "excluded"),
    ] {
        assert_eq!(
            lab.report(200).candidates[0].directory,
            NodeKey::Dir(target)
        );
        lab.db
            .execute(
                "UPDATE dir SET flags=?2,attrs=?3,listing_state=?4 WHERE id=?1",
                params![target, flags, attrs, state],
            )
            .unwrap();
        assert!(lab.report(200).candidates.is_empty());
        lab.db
            .execute(
                "UPDATE dir SET flags=0,attrs=16,listing_state='complete' WHERE id=?1",
                [target],
            )
            .unwrap();
    }
    assert_eq!(
        lab.report(200).candidates[0].directory,
        NodeKey::Dir(target)
    );
}

#[test]
fn real_catalogue_publication_supplies_aggregates_without_reading_project_paths() {
    let lab = Lab::new();
    let WriteReply::Run(run) = lab
        .catalog
        .writer()
        .call(WriteCommand::BeginRun {
            grant_id: lab.grant,
            mode: "full".into(),
            strategy: "fixture".into(),
            started_at_ns: 1,
        })
        .unwrap()
    else {
        panic!()
    };
    let mut target = Observation::file("target", 0, Some(0));
    target.attrs = 16;
    let mut source = Observation::file("source", 0, Some(0));
    source.attrs = 16;
    lab.catalog
        .writer()
        .call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id: lab.dir,
            files: vec![Observation::file("Cargo.toml", 12, Some(12))],
            dirs: vec![target, source],
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        }))
        .unwrap();
    let mut stmt = lab
        .db
        .prepare("SELECT id,name FROM dir WHERE parent_id=?1")
        .unwrap();
    let dirs = stmt
        .query_map([lab.dir], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for (id, name) in dirs {
        let file = if name == "target" {
            Observation::file("synthetic-build-output", 123, Some(128))
        } else {
            Observation::file("synthetic-local-dataset", 9000, Some(9000))
        };
        lab.catalog
            .writer()
            .call(WriteCommand::DirListing(DirListing {
                run_id: run,
                dir_id: id,
                files: vec![file],
                dirs: vec![],
                state: "complete".into(),
                skipped: 0,
                errors: 0,
            }))
            .unwrap();
    }
    let during = lab.report(200);
    assert!(during.partial);
    assert_eq!(during.candidates[0].logical_bytes, None);
    lab.catalog
        .writer()
        .call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 2,
        })
        .unwrap();
    let revision = || {
        lab.db
            .query_row("SELECT catalog_rev FROM revision", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
    };
    let before = revision();
    let report = lab.report(200);
    assert!(!report.partial);
    assert_eq!(report.rules_version, 1);
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].logical_bytes, Some(123));
    assert_eq!(report.candidates[0].allocated_bytes, Some(128));
    assert_eq!(revision(), before, "advice must not mutate the catalogue");
}

// ---- #515: join shape, VM-step cost and old-versus-new equivalence ----------------------------

const SQL_V1: &str = include_str!("fixtures/developer-clutter-v1.sql");
const SQL_NOW: &str = include_str!("../src/developer-clutter.sql");
const FLAGS: i64 = EXCLUDED_FLAGS as i64 | 1 | (1 << 1) | (1 << 6) | (1 << 7) | (1 << 9);
type Row = (i64, i64, String, String);

/// Runs one detector SQL on the lab's own connection, counting VM steps (progress ticks of 100)
/// and aborting past `abort_steps`. Returns the rows (None when aborted), steps and wall time.
fn run_sql(
    db: &Connection,
    sql: &str,
    root_dir: i64,
    cap: i64,
    abort_steps: u64,
) -> (Option<Vec<Row>>, u64, std::time::Duration) {
    use std::sync::atomic::{AtomicU64, Ordering};
    let ticks = std::sync::Arc::new(AtomicU64::new(0));
    let t = ticks.clone();
    let _ = db.progress_handler(
        100,
        Some(move || t.fetch_add(100, Ordering::Relaxed) + 100 >= abort_steps),
    );
    let started = std::time::Instant::now();
    let rows = (|| -> rusqlite::Result<Vec<Row>> {
        let mut stmt = db.prepare(sql)?;
        let rows = stmt
            .query_map(params![root_dir, FLAGS, cap + 1, false], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect();
        rows
    })()
    .ok();
    let spent = started.elapsed();
    let _ = db.progress_handler(0, None::<fn() -> bool>);
    (rows, ticks.load(Ordering::Relaxed), spent)
}

impl Lab {
    fn bulk(&self, work: impl FnOnce(&Lab)) {
        self.db.execute_batch("BEGIN").unwrap();
        work(self);
        self.db.execute_batch("COMMIT").unwrap();
    }
    fn files(&self, parent: i64, count: usize) {
        let mut stmt = self.db.prepare_cached("INSERT INTO file(dir_id,name,logical,attrs,flags,born_run,seen_run) VALUES(?1,?2,1,32,0,1,1)").unwrap();
        for i in 0..count {
            stmt.execute(params![parent, format!("f{i}.dat")]).unwrap();
        }
    }
    /// A package-manager-shaped subtree: `fanout` packages, each with `depth` nested dirs, one
    /// file per dir, and a nested node_modules in every package.
    fn dependency_tree(&self, output: i64, fanout: usize, depth: usize) -> usize {
        let mut dirs = 0;
        for p in 0..fanout {
            let mut at = self.directory(output, &format!("pkg-{p}"), 1);
            self.manifest(at, "package.json");
            self.directory(at, "node_modules", 1);
            dirs += 2;
            for d in 0..depth {
                at = self.directory(at, &format!("lib-{d}"), 1);
                self.files(at, 1);
                dirs += 1;
            }
        }
        dirs
    }
}

/// One 50k-file directory with 3k child dirs and four output-named children, beside a
/// dependency tree of 51k dirs: exceeding both shapes #515 feared.
fn wide_lab() -> (Lab, Vec<i64>) {
    let lab = Lab::new();
    let mut expected = vec![];
    lab.bulk(|lab| {
        let wide = lab.directory(lab.dir, "wide", 0);
        lab.manifest(wide, "Cargo.toml");
        lab.manifest(wide, "build.gradle");
        lab.manifest(wide, "demo.csproj");
        lab.files(wide, 50_000);
        for i in 0..3_000 {
            let child = lab.directory(wide, &format!("child-{i}"), 0);
            lab.files(child, 2);
        }
        for name in ["target", "build", "bin", "obj"] {
            expected.push(lab.directory(wide, name, 10));
        }
        let app = lab.directory(lab.dir, "app", 0);
        lab.manifest(app, "package.json");
        let modules = lab.directory(app, "node_modules", 20_000);
        expected.push(modules);
        assert_eq!(lab.dependency_tree(modules, 500, 100), 500 * 102);
    });
    (lab, expected)
}

#[test]
fn wide_directory_and_large_node_modules_stay_inside_the_work_budget() {
    let (lab, expected) = wide_lab();
    let root = lab.dir;
    let (old, old_steps, old_time) = run_sql(&lab.db, SQL_V1, root, 200, 30_000_000);
    let (new, new_steps, new_time) = run_sql(&lab.db, SQL_NOW, root, 200, 100_000_000);
    println!(
        "wide lab: v1 {} after {old_steps} steps in {old_time:?}; now {} rows after {new_steps} steps in {new_time:?}",
        old.as_ref().map_or("aborted".to_string(), |r| format!("{} rows", r.len())),
        new.as_ref().map_or(0, |r| r.len()),
    );
    let new = new.expect("the current detector finishes");
    let mut ids: Vec<i64> = new.iter().map(|r| r.0).collect();
    ids.sort();
    let mut want = expected;
    want.sort();
    assert_eq!(ids, want);
    assert!(new_steps < 1_000_000, "{new_steps} VM steps");
    if let Some(old) = old {
        assert_eq!(old, new);
    }
    // The real reader, with its 2,000,000-step WorkGuard, must answer instead of refusing.
    let report = lab.report(200);
    assert_eq!(report.candidates.len(), 5);
}

#[test]
#[ignore = "1M-row synthetic scale benchmark; run explicitly with --ignored --nocapture"]
fn million_row_output_interiors_do_not_increase_detector_work() {
    let (lab, expected) = wide_lab();
    let (before, before_steps, _) = run_sql(&lab.db, SQL_NOW, lab.dir, 200, 2_000_000);
    let rows = || {
        lab.db
            .query_row(
                "SELECT (SELECT count(*) FROM file)+(SELECT count(*) FROM dir)",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
    };
    let initial_rows = rows();
    let started = std::time::Instant::now();
    lab.bulk(|lab| {
        lab.files(
            *expected.last().unwrap(),
            (1_000_000 - initial_rows) as usize,
        )
    });
    println!(
        "1M-row lab: added {} output-interior files in {:?}",
        1_000_000 - initial_rows,
        started.elapsed()
    );
    assert_eq!(rows(), 1_000_000);
    println!(
        "v1 plan: {:#?}\ncurrent plan: {:#?}",
        plan(&lab.db, SQL_V1),
        plan(&lab.db, SQL_NOW)
    );
    for _ in 0..3 {
        let (after, steps, elapsed) = run_sql(&lab.db, SQL_NOW, lab.dir, 200, 2_000_000);
        println!("1M-row detector: {steps} VM steps in {elapsed:?}");
        assert_eq!(after, before);
        assert_eq!(steps, before_steps, "output interiors must not be visited");
    }
    let started = std::time::Instant::now();
    let report = lab.report(200);
    println!(
        "1M-row Reader: {} candidates in {:?}",
        report.candidates.len(),
        started.elapsed()
    );
    assert_eq!(report.candidates.len(), expected.len());
    for id in expected {
        assert!(report
            .candidates
            .iter()
            .any(|c| c.directory == NodeKey::Dir(id)));
    }
    assert!(!report.partial);
    assert!(!report.truncated);
}

#[test]
fn rewritten_detector_matches_the_original_on_random_trees() {
    const DIRS: [&str; 12] = [
        "target",
        "node_modules",
        ".venv",
        "venv",
        "__pycache__",
        "bin",
        "obj",
        "build",
        ".gradle",
        "src",
        "x",
        "Target",
    ];
    const FILES: [&str; 12] = [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "requirements.txt",
        "a.csproj",
        "b.sln",
        ".csproj",
        "build.gradle",
        "build.gradle.kts",
        "notes.txt",
        "cargo.toml",
        "x.sln.bak",
    ];
    const STATES: [&str; 6] = [
        "complete",
        "complete",
        "complete",
        "incomplete",
        "excluded",
        "absent_pending",
    ];
    const OUTCOMES: [&str; 5] = ["complete", "partial", "stale", "unscanned", "denied"];
    let mut compared = 0;
    let mut found = 0;
    for seed in 1..=16u64 {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move |n: usize| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % n as u64) as usize
        };
        let lab = Lab::new();
        lab.bulk(|lab| {
            let mut dirs = vec![lab.dir];
            for i in 0..300 {
                let parent = dirs[next(dirs.len())];
                let name = if next(3) == 0 { format!("d{i}") } else { DIRS[next(DIRS.len())].to_string() };
                let exists: i64 = lab.db.query_row("SELECT count(*) FROM dir WHERE parent_id=?1 AND name=?2", params![parent, name], |r| r.get(0)).unwrap();
                if exists == 0 {
                    dirs.push(lab.directory(parent, &name, next(1000) as i64));
                }
            }
            for &d in &dirs {
                for _ in 0..next(4) {
                    lab.manifest(d, FILES[next(FILES.len())]);
                }
            }
            for &d in dirs.iter().skip(1) {
                if next(6) == 0 {
                    lab.db.execute("UPDATE dir SET listing_state=?2,listing_outcome=?3,seen_run=?4,flags=?5,attrs=?6 WHERE id=?1",
                        params![d, STATES[next(STATES.len())], if next(2) == 0 { None } else { Some(OUTCOMES[next(OUTCOMES.len())]) }, 1 + next(2) as i64, if next(5) == 0 { 1i64 } else { 0 }, if next(5) == 0 { 16i64 | 1024 } else { 16 }]).unwrap();
                }
            }
            lab.db.execute("UPDATE file SET seen_run=2 WHERE id%5=0", []).unwrap();
            lab.db.execute("UPDATE file SET flags=1 WHERE id%11=0", []).unwrap();
            lab.db.execute("UPDATE file SET attrs=1024 WHERE id%13=0", []).unwrap();
        });
        for cap in [3, 200] {
            let (old, _, _) = run_sql(&lab.db, SQL_V1, lab.dir, cap, u64::MAX);
            let (new, _, _) = run_sql(&lab.db, SQL_NOW, lab.dir, cap, u64::MAX);
            let (old, new) = (old.unwrap(), new.unwrap());
            assert_eq!(old, new, "seed {seed} cap {cap}");
            found += new.len();
            compared += 1;
        }
    }
    assert_eq!(compared, 32);
    assert!(
        found > 100,
        "the random trees must exercise matches, found {found}"
    );
}

fn plan(db: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    stmt.query_map(params![1, FLAGS, 201, false], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn query_plan_reads_the_tree_only_through_indexed_lookups() {
    let lab = Lab::new();
    let plan = plan(&lab.db, SQL_NOW);
    // v1 scanned all of `dir` and probed `file` through an automatic index per candidate pair.
    let scans: Vec<_> = plan.iter().filter(|l| l.starts_with("SCAN ")).collect();
    assert_eq!(scans, ["SCAN t", "SCAN t"], "{plan:#?}");
    assert!(!plan.iter().any(|l| l.contains("AUTOMATIC")), "{plan:#?}");
    for table in [
        "file_by_allocated (dir_id=?)",
        "dir_by_allocated (parent_id=?)",
    ] {
        assert!(
            plan.iter().any(|l| l.contains(table)),
            "{table} in {plan:#?}"
        );
    }
}
