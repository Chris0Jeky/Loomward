//! LW-131 lane R1c: per-candidate reclaimable bytes and the read-only reclaim plan (docs/52
//! sections 4, 5 and 7). Synthetic catalogue rows and names only; no store path is opened.

use loomward_catalog::*;
use loomward_engine::{
    placement::FileEntry,
    reclaim::{self, *},
};
use loomward_protocol::NodeId;
use loomward_windows::{
    identity::{IdentityQuality, ObjectKey, ObservedIdentity},
    linkcount::Stop,
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
        let mut reader = self.catalog.reader().unwrap();
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
    assert_eq!((c.logical_bytes, c.reclaimable_bytes), (Some(4096), None));
    assert!(c.evidence[0].contains("4096 bytes in 3 recycled items"));
    assert!(c
        .uncertainty
        .contains("frees nothing here until the bin is emptied"));

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
