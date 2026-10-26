use loomward_catalog::*;
use rusqlite::{params, Connection};

// Synthetic catalogue rows only; no store paths are ever opened.
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
                volume_guid: None,
                volume_key: "synthetic-machine-stores".into(),
                display_name: "Lab".into(),
                display_path: "Synthetic machine store lab".into(),
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
    fn file(&self, parent: i64, name: &str, bytes: i64, allocated: Option<i64>) -> i64 {
        self.db.execute("INSERT INTO file(dir_id,name,logical,allocated,attrs,flags,born_run,seen_run) VALUES(?1,?2,?3,?4,32,0,1,1)", params![parent,name,bytes,allocated]).unwrap();
        self.db.last_insert_rowid()
    }
    fn report(&self, cap: usize) -> MachineStores {
        self.catalog
            .reader()
            .unwrap()
            .machine_stores(self.root, cap)
            .unwrap()
    }
}

#[test]
fn machine_store_signature_produces_catalogue_advice() {
    let lab = Lab::new();
    let cache = lab.directory(lab.dir, "_cacache", 123);
    lab.directory(cache, "content-v2", 100);
    lab.directory(cache, "index-v5", 23);
    let report = lab.report(200);
    assert_eq!(report.rules_version, 2);
    assert!(!report.partial);
    assert!(!report.truncated);
    assert_eq!(report.candidates.len(), 1);
    let candidate = &report.candidates[0];
    assert_eq!(candidate.node, NodeKey::Dir(cache));
    assert_eq!(candidate.name, "_cacache");
    assert_eq!(candidate.class, "package_cache");
    assert_eq!(candidate.provider, "npm");
    assert_eq!(candidate.logical_bytes, Some(123));
    assert_eq!(candidate.allocated_bytes, Some(123));
    assert_eq!(candidate.coverage, "complete");
    assert!(candidate.owner_step.contains("npm cache verify"));
    assert_eq!(candidate.relocation, Some("npm_config_cache"));
}

impl Lab {
    fn store(&self, parent: i64, provider: &str, bytes: i64) -> i64 {
        let name = match provider {
            "npm" => "_cacache",
            "pip" => "pip",
            "nuget" => "packages",
            "gradle" => "caches",
            "cargo-registry" => "registry",
            "cargo-git" => "git",
            "lmstudio" | "ollama" => "models",
            _ => "store",
        };
        let store = self.directory(parent, name, bytes);
        let children: &[&str] = match provider {
            "shared-cargo" => {
                self.file(store, "CACHEDIR.TAG", 1, Some(1));
                self.file(store, ".rustc_info.json", 1, Some(1));
                &["debug"]
            }
            "npm" => &["content-v2", "index-v5"],
            "pnpm" => {
                let version = self.directory(store, "v10", 0);
                self.directory(version, "files", 0);
                &[]
            }
            "pip" => &["http-v2"],
            "uv" => {
                self.file(store, "CACHEDIR.TAG", 1, Some(1));
                &["archive-v0"]
            }
            "gradle" => &["modules-2"],
            "cargo-registry" => &["cache"],
            "cargo-git" => &["db"],
            "huggingface" => {
                let model = self.directory(store, "models--synthetic", 0);
                self.directory(model, "blobs", 0);
                self.directory(model, "snapshots", 0);
                &[]
            }
            "ollama" => &["blobs", "manifests"],
            "git" => {
                self.file(store, ".git", 1, Some(1));
                &[]
            }
            "nuget" | "lmstudio" => &[],
            _ => panic!("unknown fixture provider"),
        };
        for child in children {
            self.directory(store, child, 0);
        }
        store
    }
}

#[test]
fn every_directory_rule_has_the_expected_class_provider_and_bounded_advice() {
    let lab = Lab::new();
    let rules = [
        ("shared-cargo", "shared_build_output", "cargo", "home"),
        ("npm", "package_cache", "npm", "home"),
        ("pnpm", "package_cache", "pnpm", "home"),
        ("nuget", "package_cache", "nuget", ".nuget"),
        ("pip", "package_cache", "pip", "home"),
        ("uv", "package_cache", "uv", "home"),
        ("gradle", "package_cache", "gradle", ".gradle"),
        ("cargo-registry", "package_cache", "cargo", ".cargo"),
        ("cargo-git", "package_cache", "cargo", ".cargo"),
        ("huggingface", "model_store", "huggingface", "home"),
        ("lmstudio", "model_store", "lmstudio", ".lmstudio"),
        ("ollama", "model_store", "ollama", "home"),
        ("git", "agent_worktree", "git", "home"),
    ];
    let mut expected = vec![];
    for (i, (fixture, class, provider, parent)) in rules.into_iter().enumerate() {
        let wrapper = lab.directory(lab.dir, &format!("fixture-{i}"), 0);
        let parent = lab.directory(wrapper, parent, 0);
        expected.push((
            NodeKey::Dir(lab.store(parent, fixture, 100)),
            class,
            provider,
        ));
    }
    expected.push((
        NodeKey::Dir(lab.directory(lab.dir, "$Recycle.Bin", 100)),
        "recycle_bin",
        "windows",
    ));
    let windows = lab.directory(lab.dir, "Windows", 0);
    for name in ["WinSxS", "SoftwareDistribution"] {
        expected.push((
            NodeKey::Dir(lab.directory(windows, name, 100)),
            "system_managed",
            "windows",
        ));
    }
    let report = lab.report(200);
    assert_eq!(report.candidates.len(), expected.len());
    assert!(!report.partial);
    for (node, class, provider) in expected {
        let candidate = report.candidates.iter().find(|c| c.node == node).unwrap();
        assert_eq!((candidate.class, candidate.provider), (class, provider));
        for text in [
            candidate.owner_step,
            candidate.reason,
            candidate.relocation.unwrap_or(""),
        ] {
            assert!(text.len() <= 256, "{text}");
        }
        assert!(!candidate.owner_step.is_empty());
        assert!(!candidate.reason.is_empty());
        assert!(candidate.uncertainty.len() <= 512);
        assert!(candidate
            .uncertainty
            .ends_with("This is advice, never a disposability claim."));
        if class == "agent_worktree" {
            for text in [
                "branch",
                "HEAD",
                "uncommitted",
                "git worktree list",
                "git submodule status",
                "owning repository",
                "before removing anything",
            ] {
                assert!(candidate.owner_step.contains(text));
            }
            for text in [
                ".git file",
                "linked worktree",
                "submodule",
                "separate-git-dir",
                "names alone",
            ] {
                assert!(candidate.reason.contains(text), "{}", candidate.reason);
            }
        } else if class == "recycle_bin" {
            assert!(candidate.owner_step.contains("owner"));
            assert!(candidate
                .uncertainty
                .contains("frees nothing here until the bin is emptied"));
        } else if class == "system_managed" {
            assert_eq!(candidate.owner_step, "Informational: managed by Windows (Disk Cleanup or DISM); never counted as reclaimable.");
        }
    }
}

#[test]
fn virtual_disks_and_root_system_files_use_their_own_sizes() {
    let lab = Lab::new();
    let distro = lab.directory(lab.dir, "distro", 9999);
    let rules = [
        (distro, "ext4.vhdx", "virtual_disk", "wsl"),
        (distro, "docker_data.vhdx", "virtual_disk", "docker"),
        (distro, "DOCKER_DATA.VHDX", "virtual_disk", "docker"),
        (distro, "Docker.vhdx", "virtual_disk", "docker"),
        (distro, "dOcKeR.VhDx", "virtual_disk", "docker"),
        (lab.dir, "hiberfil.sys", "system_managed", "windows"),
        (lab.dir, "pagefile.sys", "system_managed", "windows"),
        (lab.dir, "swapfile.sys", "system_managed", "windows"),
    ];
    let mut expected = vec![];
    for (parent, name, class, provider) in rules {
        expected.push((lab.file(parent, name, 51, Some(64)), name, class, provider));
    }
    let report = lab.report(200);
    assert_eq!(report.candidates.len(), expected.len());
    assert!(!report.partial);
    for (id, name, class, provider) in expected {
        let c = report
            .candidates
            .iter()
            .find(|c| c.node == NodeKey::File(id))
            .unwrap();
        assert_eq!(
            (c.name.as_str(), c.class, c.provider),
            (name, class, provider)
        );
        assert_eq!((c.logical_bytes, c.allocated_bytes), (Some(51), Some(64)));
        if class == "virtual_disk" {
            assert!(c.uncertainty.contains("guest free space is unknown"));
            assert!(c.uncertainty.contains("reclaimable bytes are unknown"));
        }
        assert!(c
            .uncertainty
            .ends_with("This is advice, never a disposability claim."));
    }
    let id = lab.file(distro, "ext4.vhdx", 77, None);
    let report = lab.report(200);
    let c = report
        .candidates
        .iter()
        .find(|c| c.node == NodeKey::File(id))
        .unwrap();
    assert!(report.partial);
    assert_eq!((c.logical_bytes, c.allocated_bytes), (Some(77), None));
}

#[test]
fn decoy_names_incomplete_signatures_and_v1_project_outputs_are_not_candidates() {
    let lab = Lab::new();
    let cache = lab.directory(lab.dir, "_cacache", 1);
    lab.directory(cache, "content-v2", 0);
    lab.directory(lab.dir, "models", 1);
    let repo = lab.directory(lab.dir, "ordinary-repository", 1);
    lab.directory(repo, ".git", 0);
    lab.directory(lab.dir, "packages", 1);
    lab.directory(lab.dir, "pip", 1);
    let caches = lab.directory(lab.dir, "caches", 1);
    lab.directory(caches, "modules-2", 0);
    let cargo = lab.directory(lab.dir, ".cargo", 0);
    lab.directory(cargo, "registry", 1);
    let tag_only = lab.directory(lab.dir, "tag-only", 0);
    lab.file(tag_only, "CACHEDIR.TAG", 1, Some(1));
    let rustc_only = lab.directory(lab.dir, "rustc-only", 0);
    lab.file(rustc_only, ".rustc_info.json", 1, Some(1));
    let archive_only = lab.directory(lab.dir, "archive-only", 0);
    lab.directory(archive_only, "archive-v0", 0);
    let uploads = lab.directory(lab.dir, "uploads", 0);
    let version = lab.directory(uploads, "v1", 0);
    lab.directory(version, "files", 0);
    lab.file(lab.dir, "CACHEDIR.TAG", 1, Some(1));
    lab.file(lab.dir, "ext4.vhdx.bak", 1, Some(1));
    lab.file(lab.dir, "EXT4.VHDX", 1, Some(1));
    lab.file(repo, "pagefile.sys", 1, Some(1));
    lab.directory(repo, "$Recycle.Bin", 1);
    let windows = lab.directory(repo, "Windows", 0);
    lab.directory(windows, "WinSxS", 1);
    lab.directory(windows, "SoftwareDistribution", 1);
    let project = lab.directory(lab.dir, "rust-project", 0);
    lab.file(project, "Cargo.toml", 1, Some(1));
    let target = lab.directory(project, "target", 1);
    lab.file(target, "CACHEDIR.TAG", 1, Some(1));
    lab.file(target, ".rustc_info.json", 1, Some(1));
    lab.directory(target, "debug", 0);
    for (i, version) in ["v", "v3x", "v3.0", "V3", "v-3", "v٣"]
        .into_iter()
        .enumerate()
    {
        let wrapper = lab.directory(lab.dir, &format!("pnpm-decoy-{i}"), 0);
        let parent = lab.directory(wrapper, "store", 0);
        let version = lab.directory(parent, version, 0);
        lab.directory(version, "files", 0);
    }
    let hub = lab.directory(lab.dir, "hub", 0);
    let model = lab.directory(hub, "models--decoy", 0);
    lab.directory(model, "blobs", 0);
    let wrong_case = lab.directory(lab.dir, "_CACACHE", 0);
    lab.directory(wrong_case, "content-v2", 0);
    lab.directory(wrong_case, "index-v5", 0);
    assert!(lab.report(200).candidates.is_empty());
}

#[test]
fn recognized_stores_shield_all_nested_directory_and_file_candidates() {
    let lab = Lab::new();
    let outer = lab.store(lab.dir, "npm", 500);
    lab.store(outer, "ollama", 250);
    lab.store(outer, "git", 150);
    lab.file(outer, "ext4.vhdx", 100, Some(100));
    let report = lab.report(200);
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].node, NodeKey::Dir(outer));
    // The granted root itself can be a store, and also shields its contents.
    lab.db
        .execute("UPDATE dir SET name='_cacache' WHERE id=?1", [lab.dir])
        .unwrap();
    lab.directory(lab.dir, "content-v2", 0);
    lab.directory(lab.dir, "index-v5", 0);
    let report = lab.report(200);
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].node, NodeKey::Dir(lab.dir));
}

#[test]
fn node_modules_interiors_do_not_exhaust_the_default_budget() {
    let lab = Lab::new();
    let project = lab.directory(lab.dir, "project", 0);
    lab.file(project, "package.json", 1, Some(1));
    let modules = lab.directory(project, "node_modules", 0);
    lab.db.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<20000)
        INSERT INTO dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,flags,listing_state,born_run,seen_run,sub_complete)
        SELECT ?1,?2,'synthetic-'||i,'path_observation','none',1,16,0,'complete',1,1,1 FROM n", params![lab.root, modules]).unwrap();
    let npm = lab.store(project, "npm", 42);
    let pip = lab.store(lab.dir, "pip", 10);
    assert_eq!(
        lab.report(200)
            .candidates
            .iter()
            .map(|c| c.node)
            .collect::<Vec<_>>(),
        [NodeKey::Dir(npm), NodeKey::Dir(pip)]
    );
}

#[test]
fn stores_inside_node_modules_or_git_directories_are_not_reported() {
    let lab = Lab::new();
    for name in ["node_modules", ".git"] {
        // Neither pruning rule needs a project manifest.
        let hidden = lab.directory(lab.dir, name, 0);
        lab.store(hidden, "npm", 42);
        lab.file(hidden, "ext4.vhdx", 1, Some(1));
        lab.file(hidden, ".git", 1, Some(1));
    }
    assert!(lab.report(200).candidates.is_empty());
}

#[test]
fn v1_manifest_output_pairs_prune_nested_stores() {
    let lab = Lab::new();
    for (i, (manifest, outputs)) in [
        ("Cargo.toml", &["target"][..]),
        ("pyproject.toml", &[".venv", "venv", "__pycache__"][..]),
        ("requirements.txt", &[".venv", "venv", "__pycache__"][..]),
        ("synthetic.csproj", &["bin", "obj"][..]),
        ("synthetic.sln", &["bin", "obj"][..]),
        ("build.gradle", &["build", ".gradle"][..]),
        ("build.gradle.kts", &["build", ".gradle"][..]),
    ]
    .into_iter()
    .enumerate()
    {
        let project = lab.directory(lab.dir, &format!("project-{i}"), 0);
        lab.file(project, manifest, 1, Some(1));
        for output in outputs {
            let output = lab.directory(project, output, 0);
            // Give the output its own signature too: pruning must happen first.
            lab.file(output, ".git", 1, Some(1));
            lab.store(output, "npm", 42);
            lab.file(output, "ext4.vhdx", 1, Some(1));
        }
    }
    assert!(lab.report(200).candidates.is_empty());
}

#[test]
fn unpaired_outputs_and_stale_v1_manifests_do_not_prune_stores() {
    let lab = Lab::new();
    let project = lab.directory(lab.dir, "project", 0);
    let manifest = lab.file(project, "pyproject.toml", 1, Some(1));
    let venv = lab.directory(project, ".venv", 0);
    let npm = lab.store(venv, "npm", 42);
    let source = lab.directory(project, "bin", 0);
    let pip = lab.store(source, "pip", 10);
    lab.db
        .execute(
            "UPDATE dir SET listing_state='incomplete',seen_run=2 WHERE id=?1",
            [project],
        )
        .unwrap();
    assert_eq!(lab.report(200).candidates.len(), 2);
    lab.db
        .execute("UPDATE file SET seen_run=2 WHERE id=?1", [manifest])
        .unwrap();
    let report = lab.report(200);
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].node, NodeKey::Dir(pip));
    lab.db
        .execute(
            "UPDATE dir SET listing_state='complete' WHERE id=?1",
            [project],
        )
        .unwrap();
    lab.db
        .execute("UPDATE file SET seen_run=1 WHERE id=?1", [manifest])
        .unwrap();
    assert!(!lab
        .report(200)
        .candidates
        .iter()
        .any(|c| c.node == NodeKey::Dir(npm)));
}

#[test]
fn granted_root_with_shared_cargo_signature_has_no_observed_parent() {
    let lab = Lab::new();
    lab.file(lab.dir, "CACHEDIR.TAG", 1, Some(1));
    lab.file(lab.dir, ".rustc_info.json", 1, Some(1));
    lab.directory(lab.dir, "debug", 0);
    assert!(lab.report(200).candidates.is_empty());
}

#[test]
fn shared_cargo_requires_a_complete_parent_listing() {
    let lab = Lab::new();
    let parent = lab.directory(lab.dir, "parent", 0);
    lab.store(parent, "shared-cargo", 42);
    assert_eq!(lab.report(200).candidates.len(), 1);
    for update in [
        "listing_state='incomplete'",
        "listing_state='complete',listing_outcome='partial'",
        "listing_state='complete',listing_outcome='stale'",
    ] {
        lab.db
            .execute(&format!("UPDATE dir SET {update} WHERE id=?1"), [parent])
            .unwrap();
        assert!(lab.report(200).candidates.is_empty(), "{update}");
    }
}

#[test]
fn stale_cargo_manifest_in_an_incomplete_parent_still_vetoes_shared_target() {
    let lab = Lab::new();
    let parent = lab.directory(lab.dir, "parent", 0);
    lab.store(parent, "shared-cargo", 42);
    let manifest = lab.file(parent, "Cargo.toml", 1, Some(1));
    lab.db
        .execute(
            "UPDATE dir SET listing_state='incomplete',seen_run=2 WHERE id=?1",
            [parent],
        )
        .unwrap();
    assert!(lab.report(200).candidates.is_empty());
    // A complete relisting must retain the recorded-manifest veto too.
    lab.db
        .execute(
            "UPDATE dir SET listing_state='complete' WHERE id=?1",
            [parent],
        )
        .unwrap();
    assert!(lab.report(200).candidates.is_empty());
    lab.db
        .execute(
            "UPDATE file SET flags=?2 WHERE id=?1",
            params![manifest, EXCLUDED_FLAGS],
        )
        .unwrap();
    assert!(lab.report(200).candidates.is_empty());
}

#[test]
fn alternate_signature_children_are_recognized() {
    let lab = Lab::new();
    for (i, child) in ["http", "http-v2", "wheels"].into_iter().enumerate() {
        let wrapper = lab.directory(lab.dir, &format!("pip-{i}"), 0);
        let pip = lab.directory(wrapper, "pip", 1);
        lab.directory(pip, child, 0);
    }
    for (name, version) in [("store", "v0"), (".pnpm-store", "v3"), ("custom", "v10")] {
        let parent = lab.directory(lab.dir, if name == "custom" { "pnpm" } else { "home" }, 0);
        let store = lab.directory(parent, name, 1);
        let version = lab.directory(store, version, 0);
        lab.directory(version, "files", 0);
    }
    let shared = lab.store(lab.dir, "shared-cargo", 1);
    lab.db
        .execute(
            "UPDATE dir SET name='release' WHERE parent_id=?1 AND name='debug'",
            [shared],
        )
        .unwrap();
    let hub = lab.store(lab.dir, "huggingface", 1);
    lab.db
        .execute(
            "UPDATE dir SET name='datasets--synthetic' WHERE parent_id=?1",
            [hub],
        )
        .unwrap();
    let report = lab.report(200);
    assert_eq!(report.candidates.len(), 8);
    assert_eq!(
        report
            .candidates
            .iter()
            .filter(|c| c.provider == "pip")
            .count(),
        3
    );
    assert_eq!(
        report
            .candidates
            .iter()
            .filter(|c| c.provider == "pnpm")
            .count(),
        3
    );
    assert_eq!(
        report
            .candidates
            .iter()
            .filter(|c| c.provider == "cargo")
            .count(),
        1
    );
    assert_eq!(
        report
            .candidates
            .iter()
            .filter(|c| c.provider == "huggingface")
            .count(),
        1
    );
}

#[test]
fn partial_invalid_and_unknown_allocation_sizes_stay_null() {
    let lab = Lab::new();
    let cache = lab.store(lab.dir, "npm", 42);
    lab.db
        .execute("UPDATE dir SET sub_alloc_unknown=1 WHERE id=?1", [cache])
        .unwrap();
    let report = lab.report(200);
    assert!(report.partial);
    assert_eq!(
        (
            report.candidates[0].logical_bytes,
            report.candidates[0].allocated_bytes
        ),
        (Some(42), None)
    );
    for update in [
        "sub_complete=0,listing_state='incomplete'",
        "sub_complete=1,listing_state='complete',listing_outcome='stale'",
        "listing_outcome='complete',dirty_rev=1,agg_valid_rev=0",
    ] {
        lab.db
            .execute(&format!("UPDATE dir SET {update} WHERE id=?1"), [cache])
            .unwrap();
        let report = lab.report(200);
        // A stale listing cannot supply a structural signature; other partial evidence can.
        if update.contains("'stale'") {
            assert!(report.candidates.is_empty());
        } else {
            assert!(report.partial);
            assert_eq!(report.candidates[0].logical_bytes, None);
            assert_eq!(report.candidates[0].allocated_bytes, None);
        }
    }
    lab.db
        .execute(
            "UPDATE dir SET dirty_rev=0,sub_alloc_unknown=0 WHERE id=?1",
            [cache],
        )
        .unwrap();
    for update in ["sub_complete=0", "sub_complete=1,dirty_rev=1"] {
        lab.db
            .execute(&format!("UPDATE dir SET {update} WHERE id=?1"), [lab.dir])
            .unwrap();
        let report = lab.report(200);
        assert!(report.partial);
        assert_eq!(report.candidates[0].logical_bytes, Some(42));
    }
}

#[test]
fn mid_scan_sizes_are_unknown_and_revoked_roots_are_refused() {
    let lab = Lab::new();
    lab.store(lab.dir, "npm", 42);
    lab.file(lab.dir, "ext4.vhdx", 100, Some(128));
    let WriteReply::Run(_) = lab
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
    let report = lab.report(200);
    assert!(report.partial);
    assert_eq!(report.candidates.len(), 2);
    assert!(report
        .candidates
        .iter()
        .all(|c| c.logical_bytes.is_none() && c.allocated_bytes.is_none()));
    lab.catalog
        .writer()
        .call(WriteCommand::RevokeGrant {
            grant_id: lab.grant,
            revoked_at_ns: 2,
        })
        .unwrap();
    assert!(matches!(
        lab.catalog.reader().unwrap().machine_stores(lab.root, 200),
        Err(Error::NotFound)
    ));
}

#[test]
fn caps_truncation_and_ordering_distinguish_known_zero_from_unknown() {
    let lab = Lab::new();
    let mut reader = lab.catalog.reader().unwrap();
    for cap in [0, 201] {
        assert!(matches!(
            reader.machine_stores(lab.root, cap),
            Err(Error::Invalid(_))
        ));
    }
    let small = lab.store(lab.dir, "git", 10);
    let large = lab.store(lab.dir, "npm", 100);
    let empty = lab.store(lab.dir, "ollama", 0);
    let unknown = lab.store(lab.dir, "pnpm", 9999);
    lab.db
        .execute("UPDATE dir SET sub_complete=0 WHERE id=?1", [unknown])
        .unwrap();
    let expected = vec![
        NodeKey::Dir(large),
        NodeKey::Dir(small),
        NodeKey::Dir(empty),
        NodeKey::Dir(unknown),
    ];
    let report = lab.report(200);
    assert_eq!(
        report.candidates.iter().map(|c| c.node).collect::<Vec<_>>(),
        expected
    );
    assert_eq!(report.candidates[2].logical_bytes, Some(0));
    assert_eq!(report.candidates[3].logical_bytes, None);
    assert!(!report.truncated);
    for cap in [1, 2, 3] {
        let report = lab.report(cap);
        assert!(report.truncated);
        assert!(
            report.partial,
            "even candidates beyond the cap have unknown sizes"
        );
        assert_eq!(
            report.candidates.iter().map(|c| c.node).collect::<Vec<_>>(),
            expected[..cap]
        );
    }
    reader.set_deadline(std::time::Instant::now());
    assert!(matches!(
        reader.machine_stores(lab.root, 200),
        Err(Error::Cancelled)
    ));
}

#[test]
fn equal_sizes_sort_by_node_id_with_directory_before_file_on_id_ties() {
    let lab = Lab::new();
    let store = lab.store(lab.dir, "git", 10);
    // Give the disk file the same numeric id as the directory, in a separate key space.
    lab.db.execute("INSERT INTO file(id,dir_id,name,logical,allocated,attrs,flags,born_run,seen_run) VALUES(?1,?2,'ext4.vhdx',10,10,32,0,1,1)", params![store, lab.dir]).unwrap();
    let other = lab.file(lab.dir, "Docker.vhdx", 10, Some(10));
    assert_eq!(
        lab.report(200)
            .candidates
            .iter()
            .map(|c| c.node)
            .collect::<Vec<_>>(),
        [
            NodeKey::Dir(store),
            NodeKey::File(store),
            NodeKey::File(other)
        ]
    );
}

#[test]
fn shields_apply_to_candidates_ancestors_and_signature_rows() {
    let lab = Lab::new();
    let outer = lab.directory(lab.dir, "outer", 0);
    let cache = lab.store(outer, "npm", 42);
    let content: i64 = lab
        .db
        .query_row(
            "SELECT id FROM dir WHERE parent_id=?1 AND name='content-v2'",
            [cache],
            |r| r.get(0),
        )
        .unwrap();
    let disk = lab.file(outer, "ext4.vhdx", 42, Some(42));
    let shields = [
        (EXCLUDED_FLAGS, 16, "complete"),
        (1, 16, "complete"),
        (1 << 1, 16, "complete"),
        (1 << 6, 16, "complete"),
        (1 << 7, 16, "complete"),
        (1 << 9, 16, "complete"),
        (0, 16 | 1024, "complete"),
        (0, 16, "excluded"),
        (0, 16, "denied"),
        (0, 16, "absent_pending"),
    ];
    for id in [cache, content, outer] {
        for (flags, attrs, state) in shields {
            lab.db
                .execute(
                    "UPDATE dir SET flags=?2,attrs=?3,listing_state=?4 WHERE id=?1",
                    params![id, flags, attrs, state],
                )
                .unwrap();
            let report = lab.report(200);
            assert!(
                !report
                    .candidates
                    .iter()
                    .any(|c| c.node == NodeKey::Dir(cache)),
                "shield {id}: {flags}/{attrs}/{state}"
            );
            assert_eq!(report.candidates.len(), usize::from(id != outer));
            lab.db
                .execute(
                    "UPDATE dir SET flags=0,attrs=16,listing_state='complete' WHERE id=?1",
                    [id],
                )
                .unwrap();
        }
    }
    let worktree = lab.store(lab.dir, "git", 10);
    let marker: i64 = lab
        .db
        .query_row("SELECT id FROM file WHERE dir_id=?1", [worktree], |r| {
            r.get(0)
        })
        .unwrap();
    for id in [disk, marker] {
        for (flags, attrs) in [
            (EXCLUDED_FLAGS, 32),
            (1, 32),
            (1 << 1, 32),
            (1 << 6, 32),
            (1 << 7, 32),
            (1 << 9, 32),
            (0, 32 | 1024),
        ] {
            lab.db
                .execute(
                    "UPDATE file SET flags=?2,attrs=?3 WHERE id=?1",
                    params![id, flags, attrs],
                )
                .unwrap();
            let report = lab.report(200);
            let blocked = if id == disk {
                NodeKey::File(disk)
            } else {
                NodeKey::Dir(worktree)
            };
            assert!(!report.candidates.iter().any(|c| c.node == blocked));
            lab.db
                .execute("UPDATE file SET flags=0,attrs=32 WHERE id=?1", [id])
                .unwrap();
        }
    }
    lab.db
        .execute(
            "UPDATE dir SET listing_state='incomplete',seen_run=2 WHERE id IN (?1,?2)",
            params![outer, worktree],
        )
        .unwrap();
    let report = lab.report(200);
    assert!(!report
        .candidates
        .iter()
        .any(|c| c.node == NodeKey::File(disk) || c.node == NodeKey::Dir(worktree)));
    lab.db
        .execute(
            "UPDATE file SET seen_run=2 WHERE id IN (?1,?2)",
            params![disk, marker],
        )
        .unwrap();
    assert_eq!(lab.report(200).candidates.len(), 3);
}

fn row_checksums(db: &Connection) -> Vec<(usize, u64)> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    ["dir", "file"]
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
fn reading_never_changes_dir_or_file_rows_even_when_the_root_needs_repair() {
    let lab = Lab::new();
    lab.store(lab.dir, "npm", 42);
    lab.file(lab.dir, "ext4.vhdx", 51, Some(64));
    for dirty in [false, true] {
        if dirty {
            lab.db
                .execute(
                    "UPDATE dir SET dirty_rev=agg_valid_rev+1 WHERE id=?1",
                    [lab.dir],
                )
                .unwrap();
        }
        let before = row_checksums(&lab.db);
        let report = lab.report(200);
        assert_eq!(report.candidates.len(), 2);
        assert_eq!(report.partial, dirty);
        assert_eq!(
            row_checksums(&lab.db),
            before,
            "the read must not repair or change any observation"
        );
    }
}

#[test]
fn exhausted_work_budget_is_an_error_and_does_not_poison_the_reader() {
    let lab = Lab::new();
    lab.db.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<20000)
        INSERT INTO dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,flags,listing_state,born_run,seen_run,sub_complete)
        SELECT ?1,?2,'synthetic-'||i,'path_observation','none',1,16,0,'complete',1,1,1 FROM n", params![lab.root, lab.dir]).unwrap();
    let mut reader = lab.catalog.reader().unwrap();
    assert!(matches!(
        reader.machine_stores(lab.root, 200),
        Err(Error::ResourceBudget)
    ));
    lab.db
        .execute("DELETE FROM dir WHERE parent_id=?1", [lab.dir])
        .unwrap();
    lab.store(lab.dir, "npm", 42);
    assert_eq!(
        reader
            .machine_stores(lab.root, 200)
            .unwrap()
            .candidates
            .len(),
        1
    );
}
