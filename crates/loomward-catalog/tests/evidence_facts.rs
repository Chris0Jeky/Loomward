use loomward_catalog::*;
use rusqlite::{params, Connection};

struct Lab {
    _temp: tempfile::TempDir,
    catalog: Catalog,
    db: Connection,
    root: i64,
    dir: i64,
}

impl Lab {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(temp.path(), "synthetic").unwrap();
        let WriteReply::Root {
            root_id, dir_id, ..
        } = catalog
            .writer()
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "synthetic-lab".into(),
                display_name: "Lab".into(),
                display_path: "Synthetic evidence lab".into(),
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
            root: root_id,
            dir: dir_id,
        }
    }

    fn directory(&self, parent: i64, name: &str) -> i64 {
        self.db.execute("INSERT INTO dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,flags,listing_state,born_run,seen_run,sub_complete) VALUES(?1,?2,?3,'path_observation','none',1,16,0,'complete',1,1,1)", params![self.root,parent,name]).unwrap();
        self.db.last_insert_rowid()
    }

    fn file(
        &self,
        dir: i64,
        name: &str,
        family: &str,
        size: i64,
        allocated: Option<i64>,
        unix: i64,
    ) -> i64 {
        let ext = name
            .rsplit_once('.')
            .filter(|(stem, _)| !stem.is_empty())
            .map(|(_, ext)| ext.to_ascii_lowercase());
        let ext_id = ext.map(|ext| {
            self.db
                .execute(
                    "INSERT OR IGNORE INTO ext(ext,family) VALUES(?1,?2)",
                    params![ext, family],
                )
                .unwrap();
            self.db
                .query_row("SELECT id FROM ext WHERE ext=?1", [ext], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap()
        });
        self.db.execute("INSERT INTO file(dir_id,name,ext_id,logical,allocated,modified_ft,attrs,flags,born_run,seen_run,link_count) VALUES(?1,?2,?3,?4,?5,?6,32,0,1,1,1)", params![dir,name,ext_id,size,allocated,116_444_736_000_000_000i64+unix*10_000_000]).unwrap();
        self.db.last_insert_rowid()
    }

    fn facts(&self, dir: i64) -> EvidenceFacts {
        self.catalog.reader().unwrap().evidence_facts(dir).unwrap()
    }
}

#[test]
fn rust_project_counts_relative_depth_families_markers_and_content_times() {
    let lab = Lab::new();
    let project = lab.directory(lab.dir, "Old Camera-Tool_2023");
    let src = lab.directory(project, "src");
    let target = lab.directory(project, "target");
    let debug = lab.directory(target, "debug");
    lab.directory(project, ".git");
    lab.file(project, "Cargo.toml", "code", 10, Some(16), 100);
    lab.file(src, "MAIN.RS", "code", 20, Some(32), 200);
    lab.file(debug, "demo.exe", "executable", 30, Some(48), 300);
    let facts = lab.facts(project);
    assert_eq!(facts.name, "Old Camera-Tool_2023");
    assert_eq!((facts.files, facts.folders, facts.max_depth), (3, 4, 3));
    assert_eq!(
        (facts.logical_bytes, facts.allocated_bytes),
        (Some(60), Some(96))
    );
    assert!(!facts.partial);
    assert_eq!(facts.coverage, "complete");
    assert!(facts.allocation_known && facts.link_counts_observed && facts.online);
    assert_eq!(facts.time.oldest_content_write, Some(100));
    assert_eq!(facts.time.newest_content_write, Some(300));
    assert_eq!(facts.time.newest_source_write, Some(200));
    assert_eq!(facts.time.newest_generated_write, Some(300));
    for marker in ["cargo_toml", "cargo_target_dir", "git_dir"] {
        assert!(facts.markers.contains(&marker));
    }
    let code = facts.families.iter().find(|f| f.family == "code").unwrap();
    assert_eq!((code.files, code.bytes), (2, Some(30)));
    assert_eq!(facts.extensions[0].ext, "exe");
    assert!(facts.extensions.iter().any(|e| e.ext == "rs"));
}

#[test]
fn photos_pair_only_in_the_same_directory_with_ascii_case_insensitive_stems() {
    let lab = Lab::new();
    let shoot = lab.directory(lab.dir, "shoot");
    let dcim = lab.directory(shoot, "DCIM");
    lab.file(shoot, "Shot.CR3", "image", 10, Some(16), 100);
    lab.file(shoot, "sHOT.xMp", "data", 2, Some(4), 200);
    lab.file(shoot, "alone.NEF", "image", 10, Some(16), 300);
    lab.file(dcim, "alone.XMP", "data", 2, Some(4), 400);
    let facts = lab.facts(shoot);
    let media = facts.media.unwrap();
    assert_eq!(
        (media.raw_files, media.sidecar_files, media.raw_with_sidecar),
        (2, 2, 1)
    );
    assert_eq!(
        (media.capture_start, media.capture_end, media.camera_models),
        (None, None, None)
    );
    assert!(facts.markers.contains(&"dcim_layout"));
    assert!(facts.markers.contains(&"raw_xmp_pairs"));
}

#[test]
fn partial_invalid_and_unknown_allocation_never_claim_exact_sizes() {
    let lab = Lab::new();
    lab.file(lab.dir, "data.bin", "data", 50, None, 100);
    let facts = lab.facts(lab.dir);
    assert_eq!(facts.logical_bytes, Some(50));
    assert_eq!(facts.allocated_bytes, None);
    assert_eq!(facts.families[0].bytes, None);
    lab.db
        .execute(
            "UPDATE dir SET listing_state='incomplete',sub_complete=0 WHERE id=?1",
            [lab.dir],
        )
        .unwrap();
    let facts = lab.facts(lab.dir);
    assert!(facts.partial);
    assert_eq!(facts.coverage, "partial");
    assert_eq!((facts.logical_bytes, facts.allocated_bytes), (None, None));
    assert!(facts.families.iter().all(|f| f.bytes.is_none()));
    lab.db
        .execute(
            "UPDATE dir SET listing_state='complete',sub_complete=1,dirty_rev=2 WHERE id=?1",
            [lab.dir],
        )
        .unwrap();
    assert_eq!(lab.facts(lab.dir).logical_bytes, None);
}

#[test]
fn shields_stale_rows_and_unreadable_ancestors_are_not_evidence() {
    let lab = Lab::new();
    lab.file(lab.dir, "visible.rs", "code", 5, Some(8), 100);
    for (flags, attrs, state) in [
        (EXCLUDED_FLAGS, 16, "complete"),
        (0, 1040, "complete"),
        (0, 16, "denied"),
        (0, 16, "excluded"),
    ] {
        let hidden = lab.directory(lab.dir, "hidden");
        let child = lab.directory(hidden, "child");
        lab.file(child, "secret.gguf", "model", 999, Some(999), 500);
        lab.db
            .execute(
                "UPDATE dir SET flags=?1,attrs=?2,listing_state=?3 WHERE id=?4",
                params![flags, attrs, state, hidden],
            )
            .unwrap();
        for child_state in ["complete", "incomplete", "unlisted"] {
            lab.db
                .execute(
                    "UPDATE dir SET listing_state=?1 WHERE id=?2",
                    params![child_state, child],
                )
                .unwrap();
            let facts = lab.facts(child);
            assert_eq!(facts.files, 0);
            assert_eq!(facts.coverage, "excluded", "{state}/{child_state}");
        }
    }
    let reparse = lab.file(lab.dir, "skip.gguf", "model", 999, Some(999), 500);
    lab.db
        .execute("UPDATE file SET attrs=1056 WHERE id=?1", [reparse])
        .unwrap();
    let denied = lab.file(lab.dir, "denied.gguf", "model", 999, Some(999), 500);
    lab.db
        .execute("UPDATE file SET flags=64 WHERE id=?1", [denied])
        .unwrap();
    let facts = lab.facts(lab.dir);
    assert_eq!(facts.files, 1);
    assert_eq!(facts.logical_bytes, Some(5));
    assert!(!facts.markers.contains(&"gguf_file"));
    lab.db
        .execute(
            "UPDATE dir SET listing_state='incomplete',seen_run=2 WHERE id=?1",
            [lab.dir],
        )
        .unwrap();
    assert_eq!(lab.facts(lab.dir).files, 0);
}

#[test]
fn reads_do_not_repair_rows_and_observe_offline_volume_and_deadline() {
    let lab = Lab::new();
    lab.file(lab.dir, "fixture", "none", 0, Some(0), 0);
    lab.db
        .execute("UPDATE dir SET dirty_rev=9 WHERE id=?1", [lab.dir])
        .unwrap();
    lab.db.execute("UPDATE volume SET online=0", []).unwrap();
    let snapshot = || {
        let mut result = Vec::new();
        for table in ["dir", "file"] {
            let mut stmt = lab
                .db
                .prepare(&format!("SELECT * FROM {table} ORDER BY id"))
                .unwrap();
            let columns = stmt.column_count();
            let rows = stmt
                .query_map([], |r| {
                    (0..columns)
                        .map(|i| r.get::<_, rusqlite::types::Value>(i))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap();
            result.push(rows.collect::<rusqlite::Result<Vec<_>>>().unwrap());
        }
        result
    };
    let before = snapshot();
    let facts = lab.facts(lab.dir);
    assert!(!facts.online);
    assert_eq!(facts.logical_bytes, None);
    assert_eq!(before, snapshot());
    let mut reader = lab.catalog.reader().unwrap();
    reader.set_deadline(std::time::Instant::now());
    assert!(matches!(
        reader.evidence_facts(lab.dir),
        Err(Error::Cancelled)
    ));
}

#[test]
fn large_subtree_exhausts_the_fixed_budget_without_returning_a_partial_success() {
    let lab = Lab::new();
    lab.db.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO file(dir_id,name,logical,allocated,attrs,flags,born_run,seen_run) SELECT ?1,'fixture-'||x,1,1,32,0,1,1 FROM n", [lab.dir]).unwrap();
    assert!(matches!(
        lab.catalog.reader().unwrap().evidence_facts(lab.dir),
        Err(Error::ResourceBudget)
    ));
}

#[test]
fn full_closed_marker_vocabulary_and_the_two_level_boundary() {
    let lab = Lab::new();
    for name in [
        "Cargo.toml",
        "package.json",
        "pnpm-lock.yaml",
        "pyproject.toml",
        "requirements.txt",
        "sample.csproj",
        "build.gradle.kts",
        ".git",
        "CACHEDIR.TAG",
        "weights.gguf",
        "weights.safetensors",
        "disk.vhdx",
        "shoot.lrcat",
        "shoot.lrcat.lock",
        "setup.msix",
        "bundle.tar",
        "notes.docx",
        "shot.CR2",
        "shot.XMP",
    ] {
        lab.file(lab.dir, name, "other", 1, Some(1), 100);
    }
    for name in [
        "target",
        "node_modules",
        ".venv",
        "__pycache__",
        "bin",
        "obj",
        ".gradle",
        ".git",
        "DCIM",
        "Shoot Smart Previews.lrdata",
        "~lw-staging-synthetic-138",
    ] {
        lab.directory(lab.dir, name);
    }
    let hf = lab.directory(lab.dir, "models--synthetic");
    lab.directory(hf, "blobs");
    lab.directory(hf, "snapshots");
    let lm = lab.directory(lab.dir, ".lmstudio");
    lab.directory(lm, "models");
    let ollama = lab.directory(lab.dir, "models");
    lab.directory(ollama, "blobs");
    lab.directory(ollama, "manifests");
    let backups = lab.directory(lab.dir, "Backups");
    lab.directory(backups, "2026-10-26 1200");
    let facts = lab.facts(lab.dir);
    assert_eq!(facts.markers.len(), 32, "{:?}", facts.markers);
    assert!(facts.markers.windows(2).all(|pair| pair[0] < pair[1]));
    let boundary = lab.directory(lab.dir, "boundary");
    let child = lab.directory(boundary, "child");
    let grandchild = lab.directory(child, "grandchild");
    lab.file(child, "first.gguf", "model", 1, Some(1), 100);
    lab.file(grandchild, "deep.safetensors", "model", 1, Some(1), 100);
    let facts = lab.facts(boundary);
    assert_eq!(facts.files, 2);
    assert!(facts.markers.contains(&"gguf_file"));
    assert!(!facts.markers.contains(&"safetensors_file"));
}

#[test]
fn generated_time_requires_adjacent_safe_manifests_and_inherits_into_descendants() {
    let lab = Lab::new();
    for (i, (manifest, output)) in [
        ("Cargo.toml", "target"),
        ("package.json", "node_modules"),
        ("requirements.txt", ".venv"),
        ("pyproject.toml", "__pycache__"),
        ("demo.sln", "bin"),
        ("demo.csproj", "obj"),
        ("build.gradle", "build"),
        ("build.gradle.kts", ".gradle"),
    ]
    .into_iter()
    .enumerate()
    {
        let project = lab.directory(lab.dir, &format!("project-{i}"));
        let anchor = lab.file(project, manifest, "code", 1, Some(1), 100);
        let generated = lab.directory(project, output);
        let nested = lab.directory(generated, "nested");
        lab.file(nested, "result", "none", 1, Some(1), 200);
        let facts = lab.facts(project);
        assert_eq!(facts.time.newest_generated_write, Some(200), "{output}");
        assert_eq!(facts.time.newest_source_write, Some(100));
        let inside = lab.facts(nested);
        assert_eq!(inside.time.newest_generated_write, Some(200));
        assert_eq!(inside.time.newest_source_write, None);
        lab.db
            .execute(
                "UPDATE file SET flags=?1 WHERE id=?2",
                params![EXCLUDED_FLAGS, anchor],
            )
            .unwrap();
        let facts = lab.facts(project);
        assert_eq!(facts.time.newest_generated_write, None);
        assert_eq!(facts.time.newest_source_write, Some(200));
    }
}

#[test]
fn top_extensions_are_stable_capped_and_unknown_allocations_are_not_zero() {
    let lab = Lab::new();
    for i in 0..20 {
        lab.file(
            lab.dir,
            &format!("fixture.e{i:02}"),
            "other",
            i + 1,
            Some(i + 1),
            100,
        );
    }
    lab.file(lab.dir, "noextension", "none", 30, Some(30), 100);
    lab.file(lab.dir, "unknown.foo", "other", 100, None, 100);
    let facts = lab.facts(lab.dir);
    assert_eq!(facts.extensions.len(), 16);
    assert_eq!(facts.extensions[0].ext, "");
    assert_eq!(facts.extensions[1].ext, "e19");
    assert_eq!(
        facts
            .families
            .iter()
            .find(|f| f.family == "other")
            .unwrap()
            .bytes,
        None
    );
    assert_eq!(
        facts
            .families
            .iter()
            .find(|f| f.family == "none")
            .unwrap()
            .bytes,
        Some(30)
    );
    assert_eq!(facts.allocated_bytes, None);
}

#[test]
fn partial_extensions_keep_the_sixteen_with_the_most_files() {
    let lab = Lab::new();
    lab.db.execute_batch("BEGIN").unwrap();
    for i in 0..18 {
        for j in 0..=i {
            lab.file(
                lab.dir,
                &format!("fixture{j}.e{i:02}"),
                "other",
                1,
                Some(1),
                100,
            );
        }
    }
    lab.db
        .execute(
            "UPDATE dir SET listing_state='incomplete',seen_run=1 WHERE id=?1",
            [lab.dir],
        )
        .unwrap();
    lab.db.execute_batch("COMMIT").unwrap();
    let facts = lab.facts(lab.dir);
    assert_eq!(facts.coverage, "partial");
    assert_eq!(facts.extensions.len(), 16);
    for (i, ext) in facts.extensions.iter().enumerate() {
        assert_eq!(ext.ext, format!("e{:02}", 17 - i));
        assert_eq!(ext.files, (18 - i) as u64);
        assert_eq!(ext.bytes, None);
    }
}

#[test]
fn extension_ranking_breaks_byte_ties_by_files_then_name_with_unknowns_last() {
    let lab = Lab::new();
    for (name, size, allocated) in [
        ("one.a", 10, Some(10)),
        ("one.b", 5, Some(5)),
        ("two.b", 5, Some(5)),
        ("one.z", 5, Some(5)),
        ("two.z", 5, Some(5)),
        ("one.unknown", 100, None),
    ] {
        lab.file(lab.dir, name, "other", size, allocated, 100);
    }
    let facts = lab.facts(lab.dir);
    assert_eq!(
        facts
            .extensions
            .iter()
            .map(|e| e.ext.as_str())
            .collect::<Vec<_>>(),
        ["b", "z", "a", "unknown"]
    );
}

#[test]
fn unrepresentable_extensions_are_dropped_without_losing_family_counts() {
    let lab = Lab::new();
    for ext in [
        "rs".to_owned(),
        "é".to_owned(),
        "x".repeat(32),
        "y".repeat(33),
    ] {
        lab.file(lab.dir, &format!("fixture.{ext}"), "other", 1, Some(1), 100);
    }
    let facts = lab.facts(lab.dir);
    assert_eq!(facts.files, 4);
    assert_eq!(facts.families[0].files, 4);
    assert_eq!(facts.families[0].bytes, Some(4));
    assert_eq!(facts.extensions.len(), 2);
    assert_eq!(facts.extensions[0].ext, "rs");
    assert_eq!(facts.extensions[1].ext, "x".repeat(32));
}

#[test]
fn backup_dates_with_out_of_range_months_or_days_are_not_markers() {
    let lab = Lab::new();
    for date in ["2026-00-10", "2026-13-10", "2026-10-00", "2026-10-32"] {
        let subject = lab.directory(lab.dir, date);
        let backups = lab.directory(subject, "Backups");
        lab.directory(backups, date);
        assert!(
            !lab.facts(subject)
                .markers
                .contains(&"lightroom_backups_dated"),
            "{date}"
        );
    }
}

#[test]
fn hub_and_ollama_layouts_require_both_children() {
    let lab = Lab::new();
    for (name, child, marker) in [
        ("models--synthetic", "blobs", "hf_hub_layout"),
        ("datasets--synthetic", "snapshots", "hf_hub_layout"),
        ("models", "blobs", "ollama_blobs_layout"),
        ("models", "manifests", "ollama_blobs_layout"),
    ] {
        let subject = lab.directory(lab.dir, name);
        lab.directory(subject, child);
        assert!(
            !lab.facts(subject).markers.contains(&marker),
            "{name}/{child}"
        );
    }
}
