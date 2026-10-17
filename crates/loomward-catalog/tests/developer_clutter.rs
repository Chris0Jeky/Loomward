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
    assert_eq!(lab.report(200).candidates[0].logical_bytes, Some(42));
    assert_eq!(lab.report(200).candidates[0].allocated_bytes, None);
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
