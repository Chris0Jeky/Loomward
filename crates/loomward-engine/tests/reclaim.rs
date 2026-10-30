//! LW-131 lane R1c: per-candidate reclaimable bytes and the read-only reclaim plan (docs/52
//! sections 4, 5 and 7). Synthetic catalogue rows and names only; no store path is opened.

use loomward_catalog::*;
use loomward_engine::{
    placement::FileEntry,
    reclaim::{self, *},
    EngineError,
};
use loomward_protocol::NodeId;
use loomward_windows::{
    identity::{IdentityQuality, ObjectKey, ObservedIdentity},
    linkcount::{Stop, MAX_FILES_PER_GROUP},
    recycle_bin::RecycleBinSize,
};
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
                volume_key: "synthetic-reclaim".into(),
                display_name: "Lab".into(),
                display_path: "Synthetic reclaim lab".into(),
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
    fn directory(&self, parent: i64, name: &str, bytes: i64) -> i64 {
        self.db.execute("INSERT INTO dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,flags,listing_state,born_run,seen_run,sub_complete,sub_logical,sub_allocated) VALUES(?1,?2,?3,'path_observation','none',1,16,0,'complete',1,1,1,?4,?4)", params![self.root,parent,name,bytes]).unwrap();
        self.db.last_insert_rowid()
    }
    fn file(&self, parent: i64, name: &str, bytes: i64) -> i64 {
        self.db.execute("INSERT INTO file(dir_id,name,logical,allocated,attrs,flags,born_run,seen_run) VALUES(?1,?2,?3,?3,32,0,1,1)", params![parent,name,bytes]).unwrap();
        self.db.last_insert_rowid()
    }
    fn npm_cache(&self, parent: i64, bytes: i64) -> i64 {
        let cache = self.directory(parent, "_cacache", bytes);
        self.directory(cache, "content-v2", bytes);
        self.directory(cache, "index-v5", 0);
        cache
    }
    fn project(&self, manifest: &str, output: &str, bytes: i64) -> i64 {
        let project = self.directory(self.dir, &format!("project-{output}-{bytes}"), bytes);
        self.file(project, manifest, 0);
        self.directory(project, output, bytes)
    }
    fn input(&self) -> ReclaimInput {
        let mut reader = self.catalog.reader().unwrap().without_repairs();
        ReclaimInput {
            volume: ReclaimVolume {
                volume_key: "synthetic-reclaim".into(),
                capacity_bytes: Some(1_000_000),
                free_bytes: Some(10_000),
                volume_serial: Some("7".into()),
                ntfs: true,
                ..ReclaimVolume::default()
            },
            covered_roots: vec![CoveredRoot {
                root_id: self.root,
                coverage: "complete".into(),
                allocated_bytes: Some(900_000),
            }],
            developer: vec![reader.developer_clutter(self.root, 200).unwrap()],
            machine: vec![reader.machine_stores(self.root, 200).unwrap()],
            limit: 200,
            ..ReclaimInput::default()
        }
    }
}

/// A name with an identity-matched link observation, as the link-count pass reports it.
fn name(node: &str, id: u8, bytes: u64, links: u32, attributes: u32) -> FileEntry {
    let object = ObjectKey {
        volume_serial: "7".into(),
        file_id: [id; 16],
        quality: IdentityQuality::FileId128,
        creation_time: "1".into(),
        reparse_tag: 0,
    };
    FileEntry {
        node_id: NodeId::new(format!("nd_file_{node}")).unwrap(),
        logical_bytes: bytes,
        allocated_bytes: Some(bytes),
        object: Some(object.clone()),
        link_observation: Some(ObservedIdentity {
            object,
            link_count: links,
            attributes,
            size: bytes.to_string(),
            allocation: bytes.to_string(),
            last_access_time: "0".into(),
            last_write_time: "0".into(),
            change_time: "0".into(),
            ancestors: vec![],
        }),
    }
}

fn observed(files: Vec<FileEntry>) -> CandidateFiles {
    CandidateFiles { files, stop: None }
}

fn find(plan: &ReclaimPlan, node: NodeKey) -> (GroupKind, &ReclaimCandidate) {
    let id = node.reference();
    plan.groups
        .iter()
        .flat_map(|g| g.candidates.iter().map(move |c| (g.kind, c)))
        .find(|(_, c)| c.node_id.as_deref() == Some(id.as_str()))
        .unwrap_or_else(|| panic!("{id} is not a candidate"))
}

#[test]
fn reclaim_counts_inside_links_once_and_cross_boundary_links_are_unknown() {
    let lab = Lab::new();
    let cache = NodeKey::Dir(lab.npm_cache(lab.dir, 300));
    let mut input = lab.input();
    // Two names of one identity inside the cache, plus one single-link file.
    let inside = vec![
        name("a", 1, 100, 2, 0),
        name("b", 1, 100, 2, 0),
        name("c", 2, 100, 1, 0),
    ];
    input.files.insert(cache, observed(inside.clone()));
    let plan = reclaim::plan(input.clone()).unwrap();
    let (group, c) = find(&plan, cache);
    assert_eq!(group, GroupKind::ToolOwnedCleanup);
    assert_eq!((c.logical_bytes, c.allocated_bytes), (Some(300), Some(300)));
    assert_eq!(
        (c.reclaimable_bytes, c.reclaimable_reason),
        (Some(200), None)
    );

    let unknown = (None, Some(RELIEF_UNOBSERVED));
    // A third link outside the candidate: allocation would survive the removal.
    let mut crossing = inside.clone();
    for f in &mut crossing[..2] {
        f.link_observation.as_mut().unwrap().link_count = 3;
    }
    let cases: Vec<Option<CandidateFiles>> = vec![
        Some(observed(crossing)),
        // The pass has not run, stopped at its cap or deadline, or missed a name.
        None,
        Some(CandidateFiles {
            files: inside.clone(),
            stop: Some(Stop::FileBudget),
        }),
        Some(CandidateFiles {
            files: inside.clone(),
            stop: Some(Stop::Deadline),
        }),
        Some(observed(inside[..2].to_vec())),
        // A repeated name must not stand in for an outside link.
        Some(observed(vec![
            inside[0].clone(),
            inside[0].clone(),
            name("c", 2, 100, 1, 0),
        ])),
        // An identity on another volume is not verified.
        Some(observed({
            let mut other = inside.clone();
            other[2].object.as_mut().unwrap().volume_serial = "8".into();
            other[2]
                .link_observation
                .as_mut()
                .unwrap()
                .object
                .volume_serial = "8".into();
            other
        })),
    ];
    for case in cases {
        let mut input = input.clone();
        match case {
            Some(files) => input.files.insert(cache, files),
            None => input.files.remove(&cache),
        };
        let plan = reclaim::plan(input).unwrap();
        let (_, c) = find(&plan, cache);
        assert_eq!((c.reclaimable_bytes, c.reclaimable_reason), unknown);
        assert_eq!(c.allocated_bytes, Some(300), "sizes stay observed");
    }
}

#[test]
fn reclaim_placeholder_inside_a_cache_leaves_relief_unknown() {
    let lab = Lab::new();
    let cache = NodeKey::Dir(lab.npm_cache(lab.dir, 200));
    for attributes in [0x1000, 0x40000, 0x400000] {
        let mut input = lab.input();
        input.files.insert(
            cache,
            observed(vec![
                name("a", 1, 100, 1, 0),
                name("cloud", 2, 100, 1, attributes),
            ]),
        );
        let plan = reclaim::plan(input).unwrap();
        let (_, c) = find(&plan, cache);
        assert_eq!(c.reclaimable_bytes, None);
        assert_eq!(c.reclaimable_reason, Some(RELIEF_UNOBSERVED));
    }
}

#[test]
fn reclaim_partial_subtree_sizes_are_null_never_zero() {
    let lab = Lab::new();
    let cache = lab.npm_cache(lab.dir, 200);
    lab.db
        .execute(
            "UPDATE dir SET sub_complete=0,listing_state='incomplete' WHERE id=?1",
            [cache],
        )
        .unwrap();
    let mut input = lab.input();
    input.files.insert(
        NodeKey::Dir(cache),
        observed(vec![name("a", 1, 100, 1, 0), name("b", 2, 100, 1, 0)]),
    );
    let plan = reclaim::plan(input).unwrap();
    assert!(plan.partial);
    let (_, c) = find(&plan, NodeKey::Dir(cache));
    assert_eq!(
        (c.logical_bytes, c.allocated_bytes, c.reclaimable_bytes),
        (None, None, None)
    );
    assert!(c.reclaimable_reason.unwrap().starts_with("sizes unknown"));
    let json = serde_json::to_value(c).unwrap();
    for field in ["logical_bytes", "allocated_bytes", "reclaimable_bytes"] {
        assert!(json[field].is_null(), "{field} must be null, not zero");
    }
}

#[test]
fn reclaim_groups_rank_known_relief_first_and_never_rank_unknown_as_zero() {
    let lab = Lab::new();
    let small = NodeKey::Dir(lab.project("Cargo.toml", "target", 100));
    let large = NodeKey::Dir(lab.project("package.json", "node_modules", 400));
    let unknown_but_huge = NodeKey::Dir(lab.project("demo.csproj", "obj", 90_000));
    let zero = NodeKey::Dir(lab.project("build.gradle", "build", 0));
    let worktree = lab.directory(lab.dir, "agent-worktree", 50);
    lab.file(worktree, ".git", 1);
    let models = lab.directory(lab.dir, "ollama-home", 0);
    let ollama = lab.directory(models, "models", 70);
    lab.directory(ollama, "blobs", 0);
    lab.directory(ollama, "manifests", 0);
    let disk = NodeKey::File(lab.file(lab.dir, "ext4.vhdx", 5_000));
    let windows = lab.directory(lab.dir, "Windows", 0);
    let winsxs = NodeKey::Dir(lab.directory(windows, "WinSxS", 8_000));
    let mut input = lab.input();
    input.relocation_target_volume = Some("synthetic-archive-volume".into());
    input
        .files
        .insert(small, observed(vec![name("s", 1, 100, 1, 0)]));
    input
        .files
        .insert(large, observed(vec![name("l", 2, 400, 1, 0)]));
    input.files.insert(zero, observed(vec![]));
    let plan = reclaim::plan(input).unwrap();
    let kinds: Vec<_> = plan.groups.iter().map(|g| g.kind).collect();
    assert_eq!(
        kinds,
        [
            GroupKind::ToolOwnedCleanup,
            GroupKind::Relocate,
            GroupKind::ArchiveCandidates,
            GroupKind::Informational
        ]
    );
    let order: Vec<_> = plan.groups[0]
        .candidates
        .iter()
        .map(|c| (c.node_id.clone().unwrap(), c.reclaimable_bytes))
        .collect();
    assert_eq!(
        order,
        [
            (large.reference(), Some(400)),
            (small.reference(), Some(100)),
            (zero.reference(), Some(0)),
            (unknown_but_huge.reference(), None),
        ]
    );
    let (_, cargo) = find(&plan, small);
    let relocation = cargo.relocation.as_ref().unwrap();
    assert_eq!(relocation.variable_or_setting, "CARGO_TARGET_DIR");
    assert_eq!(
        relocation.suggested_target_volume.as_deref(),
        Some("synthetic-archive-volume")
    );
    assert!(cargo.owner_step.contains("cargo clean --dry-run"));
    assert_eq!(find(&plan, NodeKey::Dir(ollama)).0, GroupKind::Relocate);
    assert_eq!(
        find(&plan, NodeKey::Dir(worktree)).0,
        GroupKind::ArchiveCandidates
    );
    let (group, vhdx) = find(&plan, disk);
    assert_eq!(group, GroupKind::Informational);
    assert_eq!(vhdx.allocated_bytes, Some(5_000), "the container size");
    assert_eq!(vhdx.reclaimable_bytes, None);
    assert!(vhdx
        .reclaimable_reason
        .unwrap()
        .contains("guest free space"));
    let (group, system) = find(&plan, winsxs);
    assert_eq!(group, GroupKind::Informational);
    assert_eq!(system.reclaimable_bytes, None);
    assert!(system.reclaimable_reason.unwrap().contains("never counted"));
}

#[test]
fn reclaim_decoys_are_not_candidates() {
    let lab = Lab::new();
    // A target folder with no Cargo.toml, and a models folder of documents.
    let loose = lab.directory(lab.dir, "loose", 10);
    lab.directory(loose, "target", 10);
    let documents = lab.directory(lab.dir, "papers", 10);
    let models = lab.directory(documents, "models", 10);
    lab.file(models, "synthetic-notes.txt", 10);
    // A node_modules inside a real Cargo target is covered by the target, not listed twice.
    let target = lab.project("Cargo.toml", "target", 30);
    lab.file(target, "package.json", 1);
    lab.directory(target, "node_modules", 20);
    let plan = reclaim::plan(lab.input()).unwrap();
    let candidates: Vec<_> = plan
        .groups
        .iter()
        .flat_map(|g| &g.candidates)
        .map(|c| c.node_id.clone().unwrap())
        .collect();
    assert_eq!(candidates, [NodeKey::Dir(target).reference()]);
}

#[test]
fn reclaim_plan_is_byte_stable_for_the_same_catalogue() {
    let lab = Lab::new();
    let cache = NodeKey::Dir(lab.npm_cache(lab.dir, 300));
    let target = NodeKey::Dir(lab.project("Cargo.toml", "target", 300));
    lab.project("package.json", "node_modules", 300);
    lab.file(lab.dir, "ext4.vhdx", 300);
    let build = || {
        let mut input = lab.input();
        input.files.insert(
            target,
            observed(vec![name("t", 3, 200, 1, 0), name("u", 4, 100, 1, 0)]),
        );
        input
            .files
            .insert(cache, observed(vec![name("c", 1, 300, 2, 0)]));
        input.recycle_bin = Some(RecycleBinSize { bytes: 9, items: 1 });
        serde_json::to_vec(&reclaim::plan(input).unwrap()).unwrap()
    };
    let first = build();
    assert_eq!(first, build());
    // The serial binding is internal and never serialised.
    assert!(!String::from_utf8(first).unwrap().contains("volume_serial"));
}

#[test]
fn reclaim_recycle_bin_size_is_one_informational_candidate() {
    let lab = Lab::new();
    let bin = RecycleBinSize {
        bytes: 4096,
        items: 3,
    };
    let mut input = lab.input();
    input.recycle_bin = Some(bin);
    let plan = reclaim::plan(input).unwrap();
    let informational = &plan.groups[3].candidates;
    assert_eq!(informational.len(), 1);
    let c = &informational[0];
    assert_eq!((c.node_id.as_ref(), c.class), (None, "recycle_bin"));
    assert_eq!(
        (c.name.as_ref(), c.provider, c.kind),
        (None, "windows", None)
    );
    assert_eq!((c.logical_bytes, c.reclaimable_bytes), (Some(4096), None));
    assert_eq!(c.allocated_bytes, None);
    assert_eq!(c.reclaimable_reason, Some(RELIEF_UNOBSERVED));
    assert_eq!(c.coverage, "unknown");
    assert_eq!(
        c.owner_step,
        "Emptying the Recycle Bin in Explorer is the owner's step after reviewing its contents."
    );
    assert!(c.relocation.is_none());
    assert_eq!(
        c.evidence,
        ["Windows reports 4096 bytes in 3 recycled items on this volume."]
    );
    assert_eq!(c.uncertainty, "Recycling on this volume frees nothing here until the bin is emptied, and its contents and recoverability are unverified. This is advice, never a disposability claim.");

    // A recorded bin under the volume-root grant carries the Windows totals as evidence.
    let recorded = NodeKey::Dir(lab.directory(lab.dir, "$Recycle.Bin", 100));
    let mut input = lab.input();
    input.recycle_bin = Some(bin);
    let plan = reclaim::plan(input).unwrap();
    assert_eq!(plan.groups[3].candidates.len(), 1);
    let (_, c) = find(&plan, recorded);
    assert!(c.evidence.iter().any(|e| e.contains("4096 bytes")));
}

#[test]
fn reclaim_budget_hit_is_the_or_of_machine_reports_and_keeps_information() {
    let lab = Lab::new();
    let system = NodeKey::File(lab.file(lab.dir, "pagefile.sys", 100));
    for flags in [
        vec![],
        vec![false, false],
        vec![true, false],
        vec![false, true],
        vec![true, true],
    ] {
        let mut input = lab.input();
        let report = input.machine[0].clone();
        input.machine = flags
            .iter()
            .map(|&budget_hit| MachineStores {
                budget_hit,
                ..report.clone()
            })
            .collect();
        input.recycle_bin = Some(RecycleBinSize {
            bytes: 4096,
            items: 3,
        });
        let plan = reclaim::plan(input).unwrap();
        assert_eq!(plan.budget_hit, flags.iter().any(|&hit| hit), "{flags:?}");
        let informational = &plan.groups[3].candidates;
        assert_eq!(
            informational
                .iter()
                .filter(|c| c.class == "recycle_bin")
                .count(),
            1
        );
        if !flags.is_empty() {
            assert_eq!(find(&plan, system).0, GroupKind::Informational);
        }
    }
}

#[test]
fn reclaim_unaccounted_bytes_are_known_only_from_complete_observations() {
    let lab = Lab::new();
    let unaccounted = |change: &dyn Fn(&mut ReclaimInput)| {
        let mut input = lab.input();
        change(&mut input);
        reclaim::plan(input).unwrap().unaccounted_bytes
    };
    assert_eq!(unaccounted(&|_: &mut ReclaimInput| {}), Some(90_000));
    assert_eq!(unaccounted(&|i| i.covered_roots.clear()), Some(990_000));
    assert_eq!(
        unaccounted(&|i| i.covered_roots[0].allocated_bytes = Some(990_000)),
        Some(0)
    );
    let unknown: [&dyn Fn(&mut ReclaimInput); 5] = [
        &|i: &mut ReclaimInput| i.volume.free_bytes = None,
        &|i: &mut ReclaimInput| i.volume.capacity_bytes = None,
        &|i: &mut ReclaimInput| i.covered_roots[0].allocated_bytes = None,
        &|i: &mut ReclaimInput| i.covered_roots[0].coverage = "partial".into(),
        // More covered than used is inconsistent, not an honest zero.
        &|i: &mut ReclaimInput| i.covered_roots[0].allocated_bytes = Some(990_001),
    ];
    for change in unknown {
        assert_eq!(unaccounted(change), None);
    }
}

#[test]
fn reclaim_limit_is_bounded_and_truncates_each_group() {
    let lab = Lab::new();
    for bytes in [1, 2, 3] {
        lab.project("Cargo.toml", "target", bytes);
    }
    for limit in [0, 201] {
        let mut input = lab.input();
        input.limit = limit;
        assert!(reclaim::plan(input).is_err());
    }
    let mut input = lab.input();
    input.limit = 2;
    let plan = reclaim::plan(input).unwrap();
    assert_eq!(plan.groups[0].candidates.len(), 2);
    assert!(plan.truncated);
}

fn row_checksums(db: &Connection) -> Vec<(usize, u64)> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    ["dir", "file", "root", "volume", "revision"]
        .into_iter()
        .map(|table| {
            let mut checksum = DefaultHasher::new();
            let mut stmt = db
                .prepare(&format!("SELECT * FROM {table} ORDER BY id"))
                .unwrap();
            let columns = stmt.column_count();
            let mut rows = stmt.query([]).unwrap();
            let mut count = 0;
            while let Some(row) = rows.next().unwrap() {
                count += 1;
                for col in 0..columns {
                    format!("{:?}", row.get_ref(col).unwrap()).hash(&mut checksum);
                }
            }
            (count, checksum.finish())
        })
        .collect()
}

#[test]
fn reclaim_read_leaves_the_catalogue_unchanged_and_has_no_effect_path() {
    let lab = Lab::new();
    let cache = NodeKey::Dir(lab.npm_cache(lab.dir, 300));
    lab.project("Cargo.toml", "target", 100);
    lab.file(lab.dir, "ext4.vhdx", 300);
    let before = row_checksums(&lab.db);
    let mut input = lab.input();
    input
        .files
        .insert(cache, observed(vec![name("c", 1, 300, 1, 0)]));
    let plan = reclaim::plan(input).unwrap();
    assert_eq!(
        plan.groups
            .iter()
            .map(|g| g.candidates.len())
            .sum::<usize>(),
        3
    );
    assert_eq!(
        row_checksums(&lab.db),
        before,
        "the reclaim read changed the catalogue"
    );

    // The plan and the bin read have no write, delete, rename, process or execute path.
    let sources = [
        include_str!("../src/reclaim.rs"),
        include_str!("../../loomward-windows/src/recycle_bin.rs"),
    ];
    let forbidden = [
        "fs::",
        "File::",
        "OpenOptions",
        "remove_file",
        "remove_dir",
        "CreateFile",
        "DeleteFile",
        "MoveFile",
        "SHEmptyRecycleBin",
        "SHFileOperation",
        "FindFirstFile",
        "Command::new",
        "ShellExecute",
        "WriteCommand",
    ];
    for source in sources {
        for word in forbidden {
            assert!(!source.contains(word), "effect path {word}");
        }
    }
}

/// Caller expectation (1): a nonzero reparse tag on any name's listing object or link
/// observation leaves that candidate's relief unknown (None, never zero). Each mutation is
/// paired with its positive control: the same candidate with the tag restored is Some(100).
#[test]
fn reclaim_reparse_tag_leaves_relief_unknown_never_zero() {
    let lab = Lab::new();
    let target = NodeKey::Dir(lab.project("Cargo.toml", "target", 100));
    let unknown = (None, Some(RELIEF_UNOBSERVED));
    let with_files = |files: Vec<FileEntry>| {
        let mut input = lab.input();
        input.files.insert(target, observed(files));
        reclaim::plan(input).unwrap()
    };
    // Tag on the listing's object only.
    let mut tagged = name("r", 1, 100, 1, 0);
    tagged.object.as_mut().unwrap().reparse_tag = 0xA000_000C;
    let plan = with_files(vec![tagged]);
    let (_, c) = find(&plan, target);
    assert_eq!((c.reclaimable_bytes, c.reclaimable_reason), unknown);
    assert_eq!(c.allocated_bytes, Some(100), "sizes stay observed");
    // Tag on the link observation only: the observation no longer matches the listing.
    let mut mismatched = name("r", 1, 100, 1, 0);
    mismatched
        .link_observation
        .as_mut()
        .unwrap()
        .object
        .reparse_tag = 0xA000_000C;
    let plan = with_files(vec![mismatched]);
    let (_, c) = find(&plan, target);
    assert_eq!((c.reclaimable_bytes, c.reclaimable_reason), unknown);
    // Boundary: the maximum tag on both sides still matches, yet still blocks relief.
    let mut maxed = name("r", 1, 100, 1, 0);
    maxed.object.as_mut().unwrap().reparse_tag = u32::MAX;
    maxed.link_observation.as_mut().unwrap().object.reparse_tag = u32::MAX;
    let plan = with_files(vec![maxed]);
    let (_, c) = find(&plan, target);
    assert_eq!((c.reclaimable_bytes, c.reclaimable_reason), unknown);
    // Positive control: the same candidate with the tag restored verifies exactly.
    let plan = with_files(vec![name("r", 1, 100, 1, 0)]);
    let (_, c) = find(&plan, target);
    assert_eq!(
        (c.reclaimable_bytes, c.reclaimable_reason),
        (Some(100), None)
    );
    let json = serde_json::to_value(c).unwrap();
    assert!(
        json["reclaimable_bytes"].is_number(),
        "known relief serialises as a number"
    );
}

/// Caller expectation (2): a legacy 64-bit identity is unusable on a non-NTFS volume or
/// with a missing volume serial, so relief is unknown (None, never zero). Positive control:
/// the same legacy identity on NTFS with the serial present verifies exactly.
#[test]
fn reclaim_legacy64_identity_needs_ntfs_and_serial() {
    let legacy = || IdentityQuality::Legacy64 {
        reason: "synthetic-legacy".into(),
    };
    let legacy_name = || {
        let mut f = name("g", 1, 100, 1, 0);
        f.object.as_mut().unwrap().quality = legacy();
        f.link_observation.as_mut().unwrap().object.quality = legacy();
        f
    };
    let lab = Lab::new();
    let target = NodeKey::Dir(lab.project("Cargo.toml", "target", 100));
    let unknown = (None, Some(RELIEF_UNOBSERVED));
    // Legacy identity on a non-NTFS volume.
    let mut input = lab.input();
    input.volume.ntfs = false;
    input.files.insert(target, observed(vec![legacy_name()]));
    let plan = reclaim::plan(input).unwrap();
    let (_, c) = find(&plan, target);
    assert_eq!((c.reclaimable_bytes, c.reclaimable_reason), unknown);
    assert_eq!(c.allocated_bytes, Some(100), "sizes stay observed");
    // Legacy identity with a missing volume serial.
    let mut input = lab.input();
    input.volume.volume_serial = None;
    input.files.insert(target, observed(vec![legacy_name()]));
    let plan = reclaim::plan(input).unwrap();
    let (_, c) = find(&plan, target);
    assert_eq!((c.reclaimable_bytes, c.reclaimable_reason), unknown);
    // Boundary: only the observation carries the legacy quality, so the pair mismatches.
    let mut input = lab.input();
    let mut half = name("g", 1, 100, 1, 0);
    half.link_observation.as_mut().unwrap().object.quality = legacy();
    input.files.insert(target, observed(vec![half]));
    let plan = reclaim::plan(input).unwrap();
    let (_, c) = find(&plan, target);
    assert_eq!((c.reclaimable_bytes, c.reclaimable_reason), unknown);
    // Positive control: the same legacy identity verifies on NTFS with the serial bound.
    let mut input = lab.input();
    input.files.insert(target, observed(vec![legacy_name()]));
    let plan = reclaim::plan(input).unwrap();
    let (_, c) = find(&plan, target);
    assert_eq!(
        (c.reclaimable_bytes, c.reclaimable_reason),
        (Some(100), None)
    );
    // The NTFS/serial rule gates legacy identities only: a 128-bit identity on a
    // non-NTFS volume still verifies.
    let mut input = lab.input();
    input.volume.ntfs = false;
    input
        .files
        .insert(target, observed(vec![name("g", 1, 100, 1, 0)]));
    let plan = reclaim::plan(input).unwrap();
    let (_, c) = find(&plan, target);
    assert_eq!(
        (c.reclaimable_bytes, c.reclaimable_reason),
        (Some(100), None)
    );
}

/// Caller expectation (3): a link observation whose size differs from the listing's
/// logical bytes leaves relief unknown (None, never zero). Positive control: the same
/// candidate with the size restored verifies exactly.
#[test]
fn reclaim_size_mismatch_leaves_relief_unknown_never_zero() {
    let lab = Lab::new();
    let target = NodeKey::Dir(lab.project("Cargo.toml", "target", 100));
    let unknown = (None, Some(RELIEF_UNOBSERVED));
    let with_files = |files: Vec<FileEntry>| {
        let mut input = lab.input();
        input.files.insert(target, observed(files));
        reclaim::plan(input).unwrap()
    };
    // Observed 99 bytes against a 100-byte listing.
    let mut short = name("s", 1, 100, 1, 0);
    short.link_observation.as_mut().unwrap().size = "99".into();
    let plan = with_files(vec![short]);
    let (_, c) = find(&plan, target);
    assert_eq!((c.reclaimable_bytes, c.reclaimable_reason), unknown);
    assert_eq!(c.allocated_bytes, Some(100), "sizes stay observed");
    // Boundary: an empty size string is not a match either.
    let mut empty = name("s", 1, 100, 1, 0);
    empty.link_observation.as_mut().unwrap().size = String::new();
    let plan = with_files(vec![empty]);
    let (_, c) = find(&plan, target);
    assert_eq!((c.reclaimable_bytes, c.reclaimable_reason), unknown);
    // Positive control: the same candidate with the size restored verifies exactly.
    let plan = with_files(vec![name("s", 1, 100, 1, 0)]);
    let (_, c) = find(&plan, target);
    assert_eq!(
        (c.reclaimable_bytes, c.reclaimable_reason),
        (Some(100), None)
    );
}

/// Caller expectation (4): unknown allocation on the listing leaves relief unknown
/// (None, never zero). Positive control: the same candidate with allocation restored
/// verifies exactly.
#[test]
fn reclaim_unknown_allocation_leaves_relief_unknown_never_zero() {
    let lab = Lab::new();
    let target = NodeKey::Dir(lab.project("Cargo.toml", "target", 100));
    let unknown = (None, Some(RELIEF_UNOBSERVED));
    // The listed name carries no allocation (unsupported listing).
    let mut unallocated = name("u", 1, 100, 1, 0);
    unallocated.allocated_bytes = None;
    let mut input = lab.input();
    input.files.insert(target, observed(vec![unallocated]));
    let plan = reclaim::plan(input).unwrap();
    let (_, c) = find(&plan, target);
    assert_eq!((c.reclaimable_bytes, c.reclaimable_reason), unknown);
    assert_eq!(
        c.allocated_bytes,
        Some(100),
        "candidate sizes stay observed"
    );
    // Boundary: an observation allocation that disagrees with the listing is also unknown.
    let mut disagreed = name("u", 1, 100, 1, 0);
    disagreed.link_observation.as_mut().unwrap().allocation = "99".into();
    let mut input = lab.input();
    input.files.insert(target, observed(vec![disagreed]));
    let plan = reclaim::plan(input).unwrap();
    let (_, c) = find(&plan, target);
    assert_eq!((c.reclaimable_bytes, c.reclaimable_reason), unknown);
    // Positive control: the same candidate with allocation restored verifies exactly.
    let mut input = lab.input();
    input
        .files
        .insert(target, observed(vec![name("u", 1, 100, 1, 0)]));
    let plan = reclaim::plan(input).unwrap();
    let (_, c) = find(&plan, target);
    assert_eq!(
        (c.reclaimable_bytes, c.reclaimable_reason),
        (Some(100), None)
    );
}

/// Rules v3: creative-AI model folders and Steam games are relocation advice, never cleanup.
#[test]
fn reclaim_rules_v3_model_folders_and_game_libraries_are_relocate_candidates() {
    let lab = Lab::new();
    let comfy = lab.directory(lab.dir, "ComfyUI", 0);
    lab.directory(comfy, "comfy", 0);
    lab.directory(comfy, "custom_nodes", 0);
    let models = lab.directory(comfy, "models", 0);
    let checkpoints = NodeKey::Dir(lab.directory(models, "checkpoints", 700));
    let steamapps = lab.directory(lab.dir, "steamapps", 0);
    lab.file(steamapps, "appmanifest_480.acf", 1);
    let common = lab.directory(steamapps, "common", 0);
    let game = NodeKey::Dir(lab.directory(common, "Synthetic Game", 900));
    let plan = reclaim::plan(lab.input()).unwrap();
    assert_eq!(plan.rules_version, 4);
    for (node, class, provider) in [
        (checkpoints, "model_store", "comfyui"),
        (game, "game_library", "steam"),
    ] {
        let (group, c) = find(&plan, node);
        assert_eq!(group, GroupKind::Relocate);
        assert_eq!((c.class, c.provider), (class, provider));
        assert!(c.relocation.is_some());
        // No link pass ran: relief is unobserved, never claimed from names.
        assert_eq!(c.reclaimable_bytes, None);
    }
}

/// Caller expectation (5): a recogniser report from the wrong rules version is refused
/// with a "reclaim rules version" error. Positive control: the shipped versions plan Ok.
#[test]
fn reclaim_wrong_rules_version_is_refused() {
    let lab = Lab::new();
    let refused = |input: &ReclaimInput| {
        let err = reclaim::plan(input.clone()).unwrap_err();
        assert!(
            matches!(&err, EngineError::InvalidRequest { .. }),
            "wrong rules version must be InvalidRequest, got {err:?}"
        );
        assert!(
            err.to_string().contains("reclaim rules version"),
            "error must name the reclaim rules version, got {err}"
        );
    };
    // Developer clutter must be rules v1.
    for version in [0, 2, u32::MAX] {
        let mut input = lab.input();
        input.developer[0].rules_version = version;
        refused(&input);
    }
    // Machine stores must be rules v4 (the plan's RULES_VERSION).
    for version in [0, 1, 2, 3, 5, u32::MAX] {
        let mut input = lab.input();
        input.machine[0].rules_version = version;
        refused(&input);
    }
    // Positive control: the shipped versions plan Ok and stamp the plan version.
    let plan = reclaim::plan(lab.input()).unwrap();
    assert_eq!(plan.rules_version, RULES_VERSION);
    assert_eq!(plan.rules_version, 4);
}

/// Rules v4: Windows leftovers plan under tool-owned cleanup with the usual relief rule, and the
/// installer caches stay informational, never reclaimable.
#[test]
fn reclaim_rules_v4_windows_leftovers_are_cleanup_and_installer_caches_are_informational() {
    let lab = Lab::new();
    let old = NodeKey::Dir(lab.directory(lab.dir, "Windows.old", 400));
    let windows = lab.directory(lab.dir, "Windows", 0);
    let temp = NodeKey::Dir(lab.directory(windows, "Temp", 200));
    let installer = NodeKey::Dir(lab.directory(windows, "Installer", 300));
    let dump = NodeKey::File(lab.file(windows, "MEMORY.DMP", 500));
    let programdata = lab.directory(lab.dir, "ProgramData", 0);
    let package_cache = NodeKey::Dir(lab.directory(programdata, "Package Cache", 600));
    let local = lab.directory(lab.dir, "AppData", 0);
    let local = lab.directory(local, "Local", 0);
    let crash = NodeKey::Dir(lab.directory(local, "CrashDumps", 150));
    let extract = lab.directory(lab.dir, "NVIDIA", 700);
    lab.directory(extract, "DisplayDriver", 0);
    let extract = NodeKey::Dir(extract);
    let mut input = lab.input();
    // One leftover has an observed deletion set; the others have none.
    input.files.insert(
        old,
        observed(vec![name("a", 1, 200, 1, 0), name("b", 2, 200, 1, 0)]),
    );
    let plan = reclaim::plan(input).unwrap();
    assert_eq!(plan.rules_version, 4);
    for node in [old, temp, dump] {
        let (group, c) = find(&plan, node);
        assert_eq!(group, GroupKind::ToolOwnedCleanup, "{node:?}");
        assert_eq!((c.class, c.provider), ("windows_leftover", "windows"));
        assert!(c.relocation.is_none());
        assert!(c.owner_step.contains("Disk Cleanup") || c.owner_step.contains("Storage"));
    }
    let (_, c) = find(&plan, old);
    assert_eq!(c.reclaimable_bytes, Some(400));
    // No link pass ran for the others: relief is unobserved, never claimed from names.
    for node in [temp, dump] {
        let (_, c) = find(&plan, node);
        assert_eq!(
            (c.reclaimable_bytes, c.reclaimable_reason),
            (None, Some(RELIEF_UNOBSERVED))
        );
    }
    // No supported cleanup command: reviewed and removed by hand, so Archive candidates.
    for (node, step) in [(crash, "Review"), (extract, "Delete by hand")] {
        let (group, c) = find(&plan, node);
        assert_eq!(group, GroupKind::ArchiveCandidates, "{node:?}");
        assert_eq!(
            (c.class, c.provider),
            ("windows_manual_leftover", "windows")
        );
        assert!(c.owner_step.starts_with(step), "{}", c.owner_step);
        assert_eq!(c.reclaimable_bytes, None);
    }
    for node in [installer, package_cache] {
        let (group, c) = find(&plan, node);
        assert_eq!(group, GroupKind::Informational, "{node:?}");
        assert_eq!((c.class, c.provider), ("system_managed", "windows"));
        assert_eq!(c.reclaimable_bytes, None);
        assert!(c
            .reclaimable_reason
            .is_some_and(|r| r.contains("never counted")));
        assert!(c.owner_step.contains("never remove by hand"));
    }
}

/// Caller expectation (6): a deletion set larger than linkcount::MAX_FILES_PER_GROUP gets
/// reclaimable None with the RELIEF_UNOBSERVED reason even when every name would verify.
/// Names are built in memory (no catalogue rows); the positive control with one verifiable
/// name on the same candidate is Some(100).
#[test]
fn reclaim_oversize_deletion_set_is_unobserved() {
    let lab = Lab::new();
    // Totals match the candidate and every name has its own single-link identity, so the cap is
    // the only reason relief is unknown (removing it would verify 200,001 bytes).
    let total = MAX_FILES_PER_GROUP as i64 + 1;
    let target = NodeKey::Dir(lab.project("Cargo.toml", "target", total));
    assert_eq!(MAX_FILES_PER_GROUP, 200_000);
    let mut files = Vec::with_capacity(MAX_FILES_PER_GROUP + 1);
    for i in 0..=MAX_FILES_PER_GROUP {
        let mut f = name(&format!("bulk{i:06}"), 1, 1, 1, 0);
        let mut id = [0xAB; 16];
        id[..8].copy_from_slice(&(i as u64).to_le_bytes());
        f.object.as_mut().unwrap().file_id = id;
        f.link_observation.as_mut().unwrap().object.file_id = id;
        files.push(f);
    }
    assert_eq!(files.len(), MAX_FILES_PER_GROUP + 1);
    let mut input = lab.input();
    input.files.insert(target, observed(files));
    let plan = reclaim::plan(input).unwrap();
    let (_, c) = find(&plan, target);
    assert_eq!(
        (c.reclaimable_bytes, c.reclaimable_reason),
        (None, Some(RELIEF_UNOBSERVED))
    );
    assert_eq!(c.allocated_bytes, Some(total as u64), "sizes stay observed");
    let json = serde_json::to_value(c).unwrap();
    assert!(
        json["reclaimable_bytes"].is_null(),
        "unknown relief is null, never zero"
    );
    // Positive control: one verifiable name carrying the whole candidate verifies exactly.
    let mut input = lab.input();
    input
        .files
        .insert(target, observed(vec![name("solo", 1, total as u64, 1, 0)]));
    let plan = reclaim::plan(input).unwrap();
    let (_, c) = find(&plan, target);
    assert_eq!(
        (c.reclaimable_bytes, c.reclaimable_reason),
        (Some(total as u64), None)
    );
}

/// #954: names whose sizes overflow u64 leave that candidate's relief unknown; a saturated sum
/// used to match a u64::MAX total and then fail the whole plan on the relief sum.
#[test]
fn reclaim_overflowing_name_sizes_leave_relief_unknown() {
    let volume = ReclaimVolume {
        volume_serial: Some("7".into()),
        ntfs: true,
        ..ReclaimVolume::default()
    };
    let files = observed(vec![
        name("a", 1, u64::MAX, 1, 0),
        name("b", 2, u64::MAX, 1, 0),
    ]);
    assert_eq!(
        reclaim::reclaimable(
            "cache",
            Some(u64::MAX),
            Some(u64::MAX),
            Some(&files),
            None,
            &volume
        )
        .unwrap(),
        (None, Some(RELIEF_UNOBSERVED))
    );
}
