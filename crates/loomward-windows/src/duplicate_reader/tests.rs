use super::*;

#[test]
fn shortcut_reader_is_capped_feature_gated_and_shell_only() {
    let source = include_str!("../enumerate/native.rs").replace("\r\n", "\n");
    assert!(!source.contains("pub fn open_readonly_child("));
    assert!(
        source.contains("#[cfg(feature = \"shortcut-read\")]\npub fn read_capped_regular_file(")
    );
    let exports = include_str!("../enumerate/mod.rs").replace("\r\n", "\n");
    assert!(exports.contains("#[cfg(all(windows, feature = \"shortcut-read\"))]\npub use native::read_capped_regular_file;"));
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    fn check(path: &Path) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().unwrap() != "target" {
                    check(&path);
                }
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                assert!(
                    !text.contains("read_capped_regular_file")
                        && !text.contains("loomward_windows::*"),
                    "shortcut reader reference outside shell: {}",
                    path.display()
                );
            } else if path.file_name().unwrap() == "Cargo.toml" {
                assert!(
                    !std::fs::read_to_string(&path)
                        .unwrap()
                        .contains("shortcut-read"),
                    "shortcut-read enabled outside shell: {}",
                    path.display()
                );
            }
        }
    }
    for entry in std::fs::read_dir(workspace.join("crates")).unwrap() {
        let path = entry.unwrap().path();
        if !["loomward-shell", "loomward-windows"]
            .iter()
            .any(|name| path.ends_with(name))
        {
            check(&path);
        }
    }
    check(&workspace.join("native"));
    assert!(include_str!("../../Cargo.toml").contains("shortcut-read = []"));
    assert!(
        std::fs::read_to_string(workspace.join("crates/loomward-shell/Cargo.toml"))
            .unwrap()
            .contains("features = [\"shortcut-read\"]")
    );
}

#[test]
fn content_reader_is_feature_gated_and_engine_only() {
    let source = include_str!("../duplicate_reader.rs").replace("\r\n", "\n");
    assert!(source.contains(
        "#[cfg(any(feature = \"content-read\", test))]\n#[doc(hidden)]\npub fn inspect("
    ));
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    fn check(path: &Path) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().unwrap() != "target" {
                    check(&path);
                }
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                assert!(
                    !text.contains("duplicate_reader") && !text.contains("loomward_windows::*"),
                    "content reader reference outside engine: {}",
                    path.display()
                );
            } else if path.file_name().unwrap() == "Cargo.toml" {
                assert!(
                    !std::fs::read_to_string(&path)
                        .unwrap()
                        .contains("content-read"),
                    "content-read enabled outside engine: {}",
                    path.display()
                );
            }
        }
    }
    for entry in std::fs::read_dir(workspace.join("crates")).unwrap() {
        let path = entry.unwrap().path();
        if !["loomward-engine", "loomward-windows"]
            .iter()
            .any(|name| path.ends_with(name))
        {
            check(&path);
        }
    }
    check(&workspace.join("native/src"));
    assert!(
        !std::fs::read_to_string(workspace.join("native/Cargo.toml"))
            .unwrap()
            .contains("content-read")
    );
    assert!(include_str!("../../Cargo.toml").contains("content-read = []"));
    assert!(
        std::fs::read_to_string(workspace.join("crates/loomward-engine/Cargo.toml"))
            .unwrap()
            .contains("features = [\"content-read\"]")
    );
}

#[test]
fn copy_primitives_are_wired_only_into_the_service_copy_module() {
    // docs/47 C5: the one consumer is loomward-service (Cargo.toml and src/copy.rs), which serves
    // synthetic sessions on lab roots only. Nothing else may enable or reference the primitives.
    let allowed = |path: &Path| {
        path.ends_with("crates/loomward-service/Cargo.toml")
            || path.ends_with("crates/loomward-service/src/copy.rs")
    };
    let windows = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = windows.parent().unwrap().parent().unwrap();
    fn check(path: &Path, windows: &Path, allowed: &dyn Fn(&Path) -> bool) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if !["target", ".git", ".loomward", "node_modules", "Resources"]
                    .iter()
                    .any(|name| path.ends_with(name))
                {
                    check(&path, windows, allowed);
                }
            } else if path.file_name().unwrap() == "Cargo.toml" {
                if path != windows.join("Cargo.toml") && !allowed(&path) {
                    assert!(
                        !std::fs::read_to_string(&path)
                            .unwrap()
                            .contains("copy-primitives"),
                        "copy-primitives enabled outside windows: {}",
                        path.display()
                    );
                }
            } else if path.extension().is_some_and(|e| e == "rs")
                && !path.starts_with(windows)
                && !allowed(&path)
            {
                let text = std::fs::read_to_string(&path).unwrap();
                assert!(
                    !text.contains("effects::") && !text.contains("loomward_windows::effects"),
                    "copy primitive reference outside windows: {}",
                    path.display()
                );
                // Only the service enforces the diagnostic digest, dialog and replace confirmation.
                assert!(
                    path.starts_with(windows.parent().unwrap().join("loomward-service"))
                        || !text.contains("loomward_windows::diagnostics"),
                    "diagnostic export reference outside service: {}",
                    path.display()
                );
            }
        }
    }
    // Exercise the scan itself: only the service may consume the diagnostic export writer.
    for (relative, accepted) in [
        ("crates/loomward-engine/src/export.rs", false),
        ("native/src/export.rs", false),
        ("crates/loomward-service-extra/src/export.rs", false),
        ("crates/loomward-service/src/diagnostics.rs", true),
        ("crates/loomward-service/src/copy.rs", true),
        ("crates/loomward-windows/src/diagnostics.rs", true),
    ] {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join(relative);
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "use loomward_windows::diagnostics::ExportFolder;").unwrap();
        let windows = fixture.path().join("crates/loomward-windows");
        assert_eq!(
            std::panic::catch_unwind(|| check(fixture.path(), &windows, &allowed)).is_ok(),
            accepted,
            "diagnostic export consumer: {relative}"
        );
    }
    assert!(include_str!("../../Cargo.toml").contains("copy-primitives = [\"content-read\"]"));
    check(workspace, windows, &allowed);
    assert!(
        std::fs::read_to_string(workspace.join("crates/loomward-service/Cargo.toml"))
            .unwrap()
            .contains("features = [\"copy-primitives\", \"diagnostics-export\"]")
    );
}

#[test]
fn core_hardlink_exclusions_make_coverage_partial() {
    let mut e = synthetic_entry(1, "1", 4);
    e.expected.link_count = 2;
    let report = group(&[e], &mut Collision, &mut Meter::new(Budget::test()));
    assert_eq!(report.links, vec![vec![0]]);
    assert!(report.partial);
    assert_eq!(report.links_not_compared, 1);
    assert_eq!(report.bytes_read, 0);
}

#[test]
fn core_repeated_names_are_refused_not_links() {
    let e = synthetic_entry(1, "1", 4);
    let report = group(
        &[e.clone(), e],
        &mut Collision,
        &mut Meter::new(Budget::test()),
    );
    assert!(report.links.is_empty());
    assert_eq!(report.skipped[0].entry, 1);
    assert_eq!(report.skipped[0].reason.as_str(), "duplicate_name");
    assert!(report.partial);
}

fn synthetic_entry(id: u8, volume: &str, size: u64) -> Entry {
    use crate::identity::{IdentityQuality, ObjectKey};
    Entry {
        relative: format!("fixture-{volume}-{id}").into(),
        expected: ObservedIdentity {
            object: ObjectKey {
                volume_serial: volume.into(),
                file_id: [id; 16],
                quality: IdentityQuality::FileId128,
                creation_time: "1".into(),
                reparse_tag: 0,
            },
            link_count: 1,
            attributes: 0,
            size: size.to_string(),
            allocation: size.to_string(),
            last_access_time: "1".into(),
            last_write_time: "1".into(),
            change_time: "1".into(),
            ancestors: vec![],
        },
    }
}
struct Collision;
impl ContentReader for Collision {
    fn validate(&mut self, _: usize, _: &Entry, _: &Meter) -> Result<(), Reason> {
        Ok(())
    }
    fn hash(
        &mut self,
        _: usize,
        _: &Entry,
        _: bool,
        meter: &mut Meter,
    ) -> Result<[u8; 32], Reason> {
        meter.reserve(1)?;
        meter.consumed(1);
        Ok([0; 32])
    }
    fn equal(
        &mut self,
        left: usize,
        right: usize,
        _: &[Entry],
        _: &mut Meter,
    ) -> Result<bool, Skip> {
        Ok(left != 0 && right != 0)
    }
}
#[test]
fn core_hash_collision_requires_exact_partitioning() {
    let entries = [
        synthetic_entry(1, "1", 4),
        synthetic_entry(2, "1", 4),
        synthetic_entry(3, "1", 4),
    ];
    let report = group(&entries, &mut Collision, &mut Meter::new(Budget::test()));
    assert_eq!(report.groups, vec![vec![1, 2]]);
}

struct ChangingRepresentative {
    comparisons: Vec<(usize, usize)>,
    survivors_equal: bool,
    fail_survivor: bool,
    fail_at_validation: bool,
}
impl ContentReader for ChangingRepresentative {
    fn validate(&mut self, index: usize, _: &Entry, _: &Meter) -> Result<(), Reason> {
        if self.fail_at_validation && index == 0 && !self.comparisons.is_empty() {
            Err(Reason::ChangedDuringRead)
        } else {
            Ok(())
        }
    }
    fn hash(
        &mut self,
        index: usize,
        entry: &Entry,
        partial: bool,
        meter: &mut Meter,
    ) -> Result<[u8; 32], Reason> {
        Collision.hash(index, entry, partial, meter)
    }
    fn equal(
        &mut self,
        left: usize,
        right: usize,
        _: &[Entry],
        meter: &mut Meter,
    ) -> Result<bool, Skip> {
        self.comparisons.push((left, right));
        meter.reserve(8).map_err(|reason| Skip {
            entry: right,
            reason,
        })?;
        meter.consumed(8); // Both four-byte comparison inputs, including failed attempts.
        match (left, right) {
            (0, 1..=4) if self.fail_survivor => Ok(true),
            (0, 5) => Err(Skip {
                entry: 0,
                reason: Reason::ChangedDuringRead,
            }),
            (0, 1 | 2) => Ok(true), // R witnesses A, then B; neither hash proves equality.
            (0, 3) if self.fail_at_validation => Ok(false),
            (0, 3) => Err(Skip {
                entry: 0,
                reason: Reason::ChangedDuringRead,
            }),
            (1, 2 | 3) if self.fail_survivor => Ok(true),
            (1, 4) if self.fail_survivor => Err(Skip {
                entry: 1,
                reason: Reason::SharingViolation,
            }),
            (2, 3) if self.fail_survivor => Ok(self.survivors_equal),
            (1, 2) => Ok(self.survivors_equal),
            _ => Ok(false),
        }
    }
}
fn changing_representative(
    survivors_equal: bool,
    fail_survivor: bool,
    fail_at_validation: bool,
    bytes: u64,
) -> (Report, Vec<(usize, usize)>) {
    let entries: Vec<_> = (1..=if fail_survivor { 6 } else { 4 })
        .map(|id| synthetic_entry(id, "1", 4))
        .collect();
    let mut reader = ChangingRepresentative {
        comparisons: Vec::new(),
        survivors_equal,
        fail_survivor,
        fail_at_validation,
    };
    let report = group(
        &entries,
        &mut reader,
        &mut Meter::new(Budget {
            bytes,
            ..Budget::test()
        }),
    );
    assert!(report.partial);
    assert_eq!(report.skipped[0].entry, 0);
    assert_eq!(report.skipped[0].reason, Reason::ChangedDuringRead);
    (report, reader.comparisons)
}
#[test]
fn core_failed_representative_reverifies_different_survivors() {
    let (report, comparisons) = changing_representative(false, false, false, 100);
    assert!(
        report.groups.is_empty(),
        "X and Y differ despite matching R"
    );
    assert_eq!(
        comparisons,
        [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)]
    );
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.stop, None);
    assert_eq!(report.bytes_read, 56); // Eight hash bytes plus six two-input comparisons.
}
#[test]
fn core_failed_representative_reverifies_equal_survivors() {
    let (report, comparisons) = changing_representative(true, false, false, 100);
    assert_eq!(report.groups, [vec![1, 2]]);
    assert_eq!(comparisons, [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3)]);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.stop, None);
    assert_eq!(report.bytes_read, 48);
}
#[test]
fn core_failed_representative_reverification_budget_is_partial() {
    let (report, comparisons) = changing_representative(true, false, false, 39);
    assert!(
        report.groups.is_empty(),
        "unexamined survivors cannot be grouped"
    );
    assert!(comparisons.contains(&(1, 2)));
    assert_eq!(report.stop, Some(Reason::ByteBudget));
    assert_eq!(report.bytes_read, 32);
}
#[test]
fn core_failed_representative_reverification_handles_another_failure() {
    let (report, comparisons) = changing_representative(false, true, false, 200);
    assert!(report.groups.is_empty());
    assert!(comparisons.contains(&(1, 2)) && comparisons.contains(&(1, 3)));
    assert!(
        comparisons.contains(&(2, 3)),
        "second failed witness also dissolves its partition"
    );
    assert_eq!(report.skipped.len(), 2);
    assert_eq!(report.skipped[1].entry, 1);
    assert_eq!(report.skipped[1].reason, Reason::SharingViolation);
    assert_eq!(report.bytes_read, 124);
}
#[test]
fn core_final_failed_representative_reverifies_survivors() {
    let (report, comparisons) = changing_representative(false, false, true, 100);
    assert!(report.groups.is_empty());
    assert!(comparisons.contains(&(1, 2)));
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.stop, None);
}
/// A row without a verified 128-bit file identity is never read (q-10, q-12): no hash, no byte.
#[test]
fn core_unverified_identity_is_never_read() {
    let entries = [1, 2].map(|id| {
        let mut e = synthetic_entry(id, "1", 4);
        e.expected.object.quality = crate::identity::IdentityQuality::Legacy64 {
            reason: "fixture".into(),
        };
        e
    });
    let report = group(&entries, &mut Collision, &mut Meter::new(Budget::test()));
    assert!(report.groups.is_empty() && report.partial);
    assert_eq!(report.bytes_read, 0);
    assert_eq!(
        report.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [Reason::Unsupported, Reason::Unsupported]
    );
}
#[test]
fn core_identity_is_volume_scoped_and_empty_files_claim_nothing() {
    let entries = [
        synthetic_entry(1, "1", 4),
        synthetic_entry(1, "2", 4),
        synthetic_entry(2, "1", 4),
        synthetic_entry(3, "1", 0),
        synthetic_entry(4, "1", 0),
    ];
    let report = group(&entries, &mut Collision, &mut Meter::new(Budget::test()));
    assert!(report.links.is_empty());
    assert_eq!(report.groups, vec![vec![1, 2]]);
}
#[test]
fn core_entry_and_time_budgets_are_partial() {
    let e = synthetic_entry(1, "1", 4);
    let report = group(
        &vec![e.clone(); 200_001],
        &mut Collision,
        &mut Meter::new(Budget::test()),
    );
    assert!(report.partial);
    assert_eq!(report.skipped.last().unwrap().reason, Reason::EntryBudget);
    let report = group(
        &[e],
        &mut Collision,
        &mut Meter::new(Budget {
            bytes: 100,
            time: Duration::ZERO,
        }),
    );
    assert!(report.partial);
    assert_eq!(report.skipped[0].reason, Reason::TimeBudget);
    let report = group(
        &[synthetic_entry(1, "1", 4), synthetic_entry(2, "1", 4)],
        &mut Collision,
        &mut Meter::new(Budget {
            bytes: 1,
            time: Duration::from_secs(10),
        }),
    );
    assert!(report.partial);
    assert_eq!(report.stop, Some(Reason::ByteBudget));
    assert_eq!(report.bytes_read, 1);
    assert!(report.groups.is_empty());
}

struct FinalStop {
    validations: usize,
    reason: Reason,
}
impl ContentReader for FinalStop {
    fn validate(&mut self, _: usize, _: &Entry, meter: &Meter) -> Result<(), Reason> {
        meter.check()?;
        self.validations += 1;
        if self.validations == 4 {
            match self.reason {
                Reason::Cancelled => meter
                    .cancel
                    .as_ref()
                    .unwrap()
                    .store(true, Ordering::Release),
                Reason::TimeBudget => std::thread::sleep(
                    meter.deadline.saturating_duration_since(Instant::now())
                        + Duration::from_millis(1),
                ),
                _ => unreachable!(),
            }
        }
        Ok(())
    }
    fn hash(
        &mut self,
        index: usize,
        entry: &Entry,
        partial: bool,
        meter: &mut Meter,
    ) -> Result<[u8; 32], Reason> {
        Collision.hash(index, entry, partial, meter)
    }
    fn equal(&mut self, _: usize, _: usize, _: &[Entry], _: &mut Meter) -> Result<bool, Skip> {
        Ok(true)
    }
}
fn final_stop_keeps_verified_groups(reason: Reason) {
    let entries = [synthetic_entry(1, "1", 4), synthetic_entry(2, "1", 4)];
    let mut meter = Meter::new(Budget {
        bytes: 100,
        time: Duration::from_millis(50),
    });
    meter.cancel = Some(Default::default());
    let report = group(
        &entries,
        &mut FinalStop {
            validations: 0,
            reason,
        },
        &mut meter,
    );
    assert_eq!(report.groups, vec![vec![0, 1]]);
    assert!(report.partial);
    assert_eq!(report.stop, Some(reason));
    assert!(report.skipped.is_empty());
    assert_eq!(report.bytes_read, 4);
}
#[test]
fn core_final_deadline_keeps_verified_groups() {
    final_stop_keeps_verified_groups(Reason::TimeBudget);
}
#[test]
fn core_final_cancel_keeps_verified_groups() {
    final_stop_keeps_verified_groups(Reason::Cancelled);
}

#[cfg(windows)]
mod native {
    use super::*;
    use crate::enumerate::{DirSource, NativeSource};
    use std::{fs, path::Path};

    fn fixture() -> Option<tempfile::TempDir> {
        let temp = tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        if crate::enumerate::running_elevated() {
            assert_eq!(
                inspect(
                    temp.path(),
                    &dummy_root(),
                    &[],
                    Budget::test(),
                    Default::default()
                )
                .unwrap_err(),
                Reason::Elevated
            );
            return None;
        }
        Some(temp)
    }
    fn dummy_root() -> crate::enumerate::OpenedIdentity {
        crate::enumerate::OpenedIdentity {
            id: None,
            basis: crate::enumerate::IdBasis::None,
            volume_guid: None,
            volume_serial: None,
            attributes: 0x10,
            reparse_tag: None,
        }
    }
    fn entry(root: &Path, relative: &str) -> Entry {
        Entry {
            relative: relative.into(),
            expected: crate::identity::observe(&root.join(relative)).unwrap(),
        }
    }
    fn root_id(root: &Path) -> crate::enumerate::OpenedIdentity {
        NativeSource::default().open_root(root).unwrap().1
    }
    /// #219: an identity-unverified root is refused before any pin, metadata or content read.
    #[test]
    fn identity_unverified_root_is_never_a_content_read_input() {
        let Some(temp) = fixture() else { return };
        fs::write(temp.path().join("a"), b"same").unwrap();
        fs::write(temp.path().join("b"), b"same").unwrap();
        let mut root = root_id(temp.path());
        assert!(inspect(
            temp.path(),
            &root,
            &[entry(temp.path(), "a"), entry(temp.path(), "b")],
            Budget::test(),
            Default::default()
        )
        .is_ok());
        root.basis = crate::enumerate::IdBasis::Unverified;
        assert_eq!(
            inspect(
                temp.path(),
                &root,
                &[entry(temp.path(), "a"), entry(temp.path(), "b")],
                Budget::test(),
                Default::default()
            )
            .unwrap_err(),
            Reason::IdentityUnverified
        );
        assert_eq!(Reason::IdentityUnverified.as_str(), "identity_unverified");
    }
    #[cfg(feature = "content-read")]
    #[test]
    fn native_candidate_hydration_is_root_relative_and_never_follows_a_junction() {
        let Some(temp) = fixture() else { return };
        fs::create_dir(temp.path().join("nested")).unwrap();
        fs::write(temp.path().join("nested/a"), b"same").unwrap();
        let expected = root_id(temp.path());
        let observed = observe_candidate(
            temp.path(),
            &expected,
            Path::new("nested/a"),
            Budget::test(),
            Default::default(),
        )
        .unwrap();
        assert_eq!(observed, entry(temp.path(), "nested/a").expected);
        assert_eq!(
            observe_candidate(
                temp.path(),
                &expected,
                Path::new("../outside"),
                Budget::test(),
                Default::default()
            )
            .unwrap_err(),
            Reason::InvalidPath
        );
        crate::fixtures::junction(&temp.path().join("redirect"), &temp.path().join("nested"))
            .unwrap();
        assert_eq!(
            observe_candidate(
                temp.path(),
                &expected,
                Path::new("redirect/a"),
                Budget::test(),
                Default::default()
            )
            .unwrap_err(),
            Reason::ChangedDuringRead
        );
    }
    fn run(root: &Path, entries: &[Entry]) -> Report {
        inspect(
            root,
            &root_id(root),
            entries,
            Budget::test(),
            Default::default(),
        )
        .unwrap()
    }
    #[test]
    fn native_equal_and_same_size_different_bytes() {
        let Some(temp) = fixture() else { return };
        for (name, bytes) in [("a", b"same"), ("b", b"same"), ("c", b"else")] {
            fs::write(temp.path().join(name), bytes).unwrap();
        }
        let entries: Vec<_> = ["a", "b", "c"].map(|n| entry(temp.path(), n)).into();
        let report = run(temp.path(), &entries);
        assert_eq!(report.groups, vec![vec![0, 1]]);
        assert!(!report.partial);
        assert_eq!(report.bytes_read, 28); // partial 12, full 8, comparison 8
    }
    #[test]
    fn native_hardlinks_are_links_not_duplicates() {
        let Some(temp) = fixture() else { return };
        fs::write(temp.path().join("a"), b"same").unwrap();
        fs::hard_link(temp.path().join("a"), temp.path().join("b")).unwrap();
        let report = run(
            temp.path(),
            &[entry(temp.path(), "a"), entry(temp.path(), "b")],
        );
        assert!(report.groups.is_empty());
        assert_eq!(report.links, vec![vec![0, 1]]);
        assert_eq!(report.bytes_read, 0);
    }
    #[test]
    fn native_budget_exhaustion_is_partial() {
        let Some(temp) = fixture() else { return };
        for n in ["a", "b"] {
            fs::write(temp.path().join(n), b"same").unwrap();
        }
        let entries = [entry(temp.path(), "a"), entry(temp.path(), "b")];
        for budget in [3, 8, 16, 23]
            .map(|bytes| Budget {
                bytes,
                time: Duration::from_secs(10),
            })
            .into_iter()
            .chain([Budget {
                bytes: 100,
                time: Duration::ZERO,
            }])
        {
            let r = inspect(
                temp.path(),
                &root_id(temp.path()),
                &entries,
                budget,
                Default::default(),
            )
            .unwrap();
            assert!(r.partial);
            assert!(r.groups.is_empty());
            assert!(r.bytes_read <= budget.bytes);
        }
    }
    #[test]
    fn native_equal_ends_different_middle_need_full_hash() {
        let Some(temp) = fixture() else { return };
        let mut bytes = vec![7; 3 * 64 * 1024];
        fs::write(temp.path().join("a"), &bytes).unwrap();
        bytes[64 * 1024] = 8;
        fs::write(temp.path().join("b"), &bytes).unwrap();
        let r = run(
            temp.path(),
            &[entry(temp.path(), "a"), entry(temp.path(), "b")],
        );
        assert!(r.groups.is_empty());
        assert!(!r.partial);
        assert_eq!(r.bytes_read, 10 * 64 * 1024);
    }
    #[test]
    fn native_changed_representative_keeps_remaining_equal_pair() {
        let Some(temp) = fixture() else { return };
        for n in ["a", "b", "c"] {
            fs::write(temp.path().join(n), b"same").unwrap();
        }
        let entries = [
            entry(temp.path(), "a"),
            entry(temp.path(), "b"),
            entry(temp.path(), "c"),
        ];
        let r = inspect_with_hook(
            temp.path(),
            &root_id(temp.path()),
            &entries,
            Budget::test(),
            |event, index| {
                if event == Event::Compare && index == 2 {
                    fs::write(temp.path().join("a"), b"changed").unwrap();
                }
            },
        )
        .unwrap();
        assert_eq!(r.groups, vec![vec![1, 2]]);
        assert!(r.partial);
        assert_eq!(r.skipped.len(), 1);
        assert_eq!(r.skipped[0].entry, 0);
        assert_eq!(r.skipped[0].reason, Reason::ChangedDuringRead);
    }
    #[test]
    fn native_failed_representative_reverification_uses_shared_budget() {
        for bytes in [100, 55] {
            let Some(temp) = fixture() else { return };
            for name in ["a", "b", "c", "d"] {
                fs::write(temp.path().join(name), b"same").unwrap();
            }
            let entries = ["a", "b", "c", "d"].map(|name| entry(temp.path(), name));
            let mut changed = false;
            let mut comparisons = Vec::new();
            let report = inspect_with_hook(
                temp.path(),
                &root_id(temp.path()),
                &entries,
                Budget {
                    bytes,
                    ..Budget::test()
                },
                |event, index| {
                    if event == Event::Compare {
                        comparisons.push(index);
                        if index == 3 && !changed {
                            fs::write(temp.path().join("a"), b"changed").unwrap();
                            changed = true;
                        }
                    }
                },
            )
            .unwrap();
            assert!(report.partial);
            assert_eq!(report.skipped[0].entry, 0);
            assert_eq!(report.skipped[0].reason, Reason::ChangedDuringRead);
            assert_eq!(comparisons, [1, 2, 3, 2, 3]);
            if bytes == 100 {
                assert_eq!(report.groups, [vec![1, 2, 3]]);
                assert_eq!(report.bytes_read, 64);
                assert_eq!(report.stop, None);
            } else {
                assert!(report.groups.is_empty());
                assert_eq!(report.bytes_read, 48);
                assert_eq!(report.stop, Some(Reason::ByteBudget));
            }
        }
    }
    #[test]
    fn native_final_validation_drops_changed_member() {
        let Some(temp) = fixture() else { return };
        for name in ["a", "b", "c"] {
            fs::write(temp.path().join(name), b"same").unwrap();
        }
        let entries = ["a", "b", "c"].map(|name| entry(temp.path(), name));
        let report = inspect_with_hook(
            temp.path(),
            &root_id(temp.path()),
            &entries,
            Budget::test(),
            |event, index| {
                if event == Event::Compare && index == 2 {
                    fs::write(temp.path().join("b"), b"changed").unwrap();
                }
            },
        )
        .unwrap();
        assert_eq!(report.groups, vec![vec![0, 2]]);
        assert!(report.partial);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].entry, 1);
        assert_eq!(report.skipped[0].reason, Reason::ChangedDuringRead);
    }
    #[test]
    fn native_sharing_failed_representative_keeps_remaining_equal_pair() {
        use std::os::windows::fs::OpenOptionsExt;
        let Some(temp) = fixture() else { return };
        for name in ["a", "b", "c"] {
            fs::write(temp.path().join(name), b"same").unwrap();
        }
        let entries = ["a", "b", "c"].map(|name| entry(temp.path(), name));
        let mut exclusive = None;
        let report = inspect_with_hook(
            temp.path(),
            &root_id(temp.path()),
            &entries,
            Budget::test(),
            |event, index| {
                if event == Event::Compare && index == 1 {
                    exclusive = Some(
                        fs::OpenOptions::new()
                            .read(true)
                            .share_mode(0)
                            .open(temp.path().join("a"))
                            .unwrap(),
                    );
                }
            },
        )
        .unwrap();
        assert_eq!(report.groups, vec![vec![1, 2]]);
        assert!(report.partial);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].entry, 0);
        assert_eq!(report.skipped[0].reason, Reason::SharingViolation);
    }
    #[test]
    fn native_changed_candidate_keeps_representative() {
        let Some(temp) = fixture() else { return };
        for name in ["a", "b", "c"] {
            fs::write(temp.path().join(name), b"same").unwrap();
        }
        let entries = ["a", "b", "c"].map(|name| entry(temp.path(), name));
        let report = inspect_with_hook(
            temp.path(),
            &root_id(temp.path()),
            &entries,
            Budget::test(),
            |event, index| {
                if event == Event::Compare && index == 1 {
                    fs::write(temp.path().join("b"), b"changed").unwrap();
                }
            },
        )
        .unwrap();
        assert_eq!(report.groups, vec![vec![0, 2]]);
        assert!(report.partial);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].entry, 1);
        assert_eq!(report.skipped[0].reason, Reason::ChangedDuringRead);
    }
    #[test]
    fn native_compare_finish_attributes_failure_to_its_operand() {
        use std::os::windows::fs::OpenOptionsExt;
        let Some(temp) = fixture() else { return };
        for failed in [0, 1] {
            for name in ["a", "b", "c"] {
                fs::write(temp.path().join(name), b"same").unwrap();
            }
            let entries = ["a", "b", "c"].map(|name| entry(temp.path(), name));
            let mut comparing = false;
            let mut changed = false;
            let report = inspect_with_hook(
                temp.path(),
                &root_id(temp.path()),
                &entries,
                Budget::test(),
                |event, index| {
                    if event == Event::Compare {
                        comparing = true;
                    }
                    if comparing && !changed && event == Event::Read && index == failed {
                        let file = fs::OpenOptions::new()
                            .access_mode(
                                windows_sys::Win32::Storage::FileSystem::FILE_WRITE_ATTRIBUTES,
                            )
                            .open(temp.path().join(&entries[failed].relative))
                            .unwrap();
                        file.set_times(fs::FileTimes::new().set_modified(
                            std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                        ))
                        .unwrap();
                        changed = true;
                    }
                },
            )
            .unwrap();
            assert!(changed);
            assert_eq!(
                report.groups,
                vec![if failed == 0 { vec![1, 2] } else { vec![0, 2] }]
            );
            assert!(report.partial);
            assert_eq!(report.skipped.len(), 1);
            assert_eq!(report.skipped[0].entry, failed);
            assert_eq!(report.skipped[0].reason, Reason::ChangedDuringRead);
        }
    }
    #[test]
    fn native_stop_keeps_earlier_verified_group_and_later_input_unknown() {
        let Some(temp) = fixture() else { return };
        for (name, bytes) in [
            ("a", b"same".as_slice()),
            ("b", b"same"),
            ("c", b"longer"),
            ("d", b"longer"),
        ] {
            fs::write(temp.path().join(name), bytes).unwrap();
        }
        let entries = ["a", "b", "c", "d"].map(|name| entry(temp.path(), name));
        for reason in [Reason::Cancelled, Reason::TimeBudget] {
            let cancel = Arc::new(AtomicBool::new(false));
            let mut meter = Meter::new(Budget {
                bytes: 100,
                time: Duration::from_millis(100),
            });
            // Stop at the chosen comparison, independent of host scheduling and disk latency.
            let clock = std::rc::Rc::new(std::cell::Cell::new(Instant::now()));
            meter.deadline = clock.get() + Duration::from_millis(100);
            meter.clock = Some(clock.clone());
            meter.cancel = Some(cancel.clone());
            let mut reader = crate::duplicate_reader::native::Reader::new(
                temp.path(),
                &root_id(temp.path()),
                |event, index| {
                    if event == Event::Compare && index == 3 {
                        if reason == Reason::Cancelled {
                            cancel.store(true, Ordering::Release);
                        } else {
                            clock.set(clock.get() + Duration::from_millis(100));
                        }
                    }
                },
            )
            .unwrap();
            let report = group(&entries, &mut reader, &mut meter);
            assert_eq!(report.groups, vec![vec![0, 1]]);
            assert!(report.partial);
            assert_eq!(report.stop, Some(reason));
            assert!(report.skipped.iter().all(|s| s.entry >= 2));
            assert_eq!(report.bytes_read, 48);
        }
    }
    #[test]
    fn native_public_inspect_stop_keeps_verified_group_and_marks_partial() {
        let Some(temp) = fixture() else { return };
        for (name, bytes) in [
            ("a", b"same".as_slice()),
            ("b", b"same"),
            ("c", b"longer"),
            ("d", b"longer"),
        ] {
            fs::write(temp.path().join(name), bytes).unwrap();
        }
        let entries = ["a", "b", "c", "d"].map(|name| entry(temp.path(), name));
        let report = inspect(
            temp.path(),
            &root_id(temp.path()),
            &entries,
            Budget {
                bytes: 48,
                time: Duration::from_secs(30),
            },
            Default::default(),
        )
        .unwrap();
        assert_eq!(report.groups, [vec![0, 1]]);
        assert_eq!(report.stop, Some(Reason::ByteBudget));
        assert!(report.partial);
        assert_eq!(report.bytes_read, 48);
        assert!(report.skipped.iter().all(|s| s.entry >= 2));
    }
    #[test]
    fn native_sharing_violation_visible() {
        use std::os::windows::fs::OpenOptionsExt;
        let Some(temp) = fixture() else { return };
        for n in ["a", "b"] {
            fs::write(temp.path().join(n), b"same").unwrap();
        }
        let entries = [entry(temp.path(), "a"), entry(temp.path(), "b")];
        let _exclusive = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(temp.path().join("b"))
            .unwrap();
        let r = run(temp.path(), &entries);
        assert!(r.groups.is_empty());
        assert!(r
            .skipped
            .iter()
            .any(|s| s.entry == 1 && s.reason == Reason::SharingViolation));
    }
    #[test]
    fn native_existing_writer_is_a_visible_sharing_violation() {
        let Some(temp) = fixture() else { return };
        for n in ["a", "b"] {
            fs::write(temp.path().join(n), b"same").unwrap();
        }
        let entries = [entry(temp.path(), "a"), entry(temp.path(), "b")];
        let _writer = fs::OpenOptions::new()
            .write(true)
            .open(temp.path().join("b"))
            .unwrap();
        let report = run(temp.path(), &entries);
        assert!(report.groups.is_empty());
        assert!(report.partial);
        assert!(report
            .skipped
            .iter()
            .any(|s| s.entry == 1 && s.reason == Reason::SharingViolation));
    }
    #[test]
    fn native_stream_syntax_is_invalid_path() {
        let Some(temp) = fixture() else { return };
        fs::write(temp.path().join("file"), b"same").unwrap();
        let mut e = entry(temp.path(), "file");
        for name in ["file:stream", "directory:stream/file"] {
            e.relative = name.into();
            let report = run(temp.path(), &[e.clone()]);
            assert_eq!(report.skipped[0].reason, Reason::InvalidPath);
            assert_eq!(report.bytes_read, 0);
        }
    }
    #[test]
    fn native_refuses_special_and_placeholder_classes() {
        let Some(temp) = fixture() else { return };
        fs::write(temp.path().join("a"), b"same").unwrap();
        fs::create_dir(temp.path().join("dir")).unwrap();
        let r = run(temp.path(), &[entry(temp.path(), "dir")]);
        assert_eq!(r.skipped[0].reason, Reason::SpecialFile);
        for (attributes, reason) in [
            (0x1000, Reason::Offline),
            (0x40000, Reason::Placeholder),
            (0x400000, Reason::Placeholder),
            (0x400, Reason::ReparsePoint),
            (0x40, Reason::SpecialFile),
        ] {
            let mut e = entry(temp.path(), "a");
            e.expected.attributes |= attributes;
            let r = run(temp.path(), &[e]);
            assert_eq!(r.skipped[0].reason, reason);
            assert_eq!(r.bytes_read, 0);
        }
    }
    #[test]
    fn native_modified_during_read_is_rejected() {
        let Some(temp) = fixture() else { return };
        for n in ["a", "b"] {
            fs::write(temp.path().join(n), b"same").unwrap();
        }
        let entries = [entry(temp.path(), "a"), entry(temp.path(), "b")];
        let r = inspect_with_hook(
            temp.path(),
            &root_id(temp.path()),
            &entries,
            Budget::test(),
            |event, index| {
                if event == Event::Read && index == 1 {
                    use std::os::windows::fs::OpenOptionsExt;
                    // Content writers are now excluded; metadata can still change under a read.
                    let file = fs::OpenOptions::new()
                        .access_mode(windows_sys::Win32::Storage::FileSystem::FILE_WRITE_ATTRIBUTES)
                        .open(temp.path().join("b"))
                        .unwrap();
                    file.set_times(
                        fs::FileTimes::new().set_modified(
                            std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                        ),
                    )
                    .unwrap();
                }
            },
        )
        .unwrap();
        assert!(r.groups.is_empty());
        assert!(r
            .skipped
            .iter()
            .any(|s| s.reason == Reason::ChangedDuringRead));
    }
    #[test]
    fn native_deadline_during_read_is_partial() {
        let Some(temp) = fixture() else { return };
        for n in ["a", "b"] {
            fs::write(temp.path().join(n), b"same").unwrap();
        }
        let entries = [entry(temp.path(), "a"), entry(temp.path(), "b")];
        let mut meter = Meter::new(Budget {
            bytes: 100,
            time: Duration::from_millis(100),
        });
        // The deadline runs on the injected clock: time moves only at the read hook below,
        // so it expires exactly there, independent of host scheduling and disk latency.
        let clock = std::rc::Rc::new(std::cell::Cell::new(Instant::now()));
        meter.deadline = clock.get() + Duration::from_millis(100);
        meter.clock = Some(clock.clone());
        let mut reader = crate::duplicate_reader::native::Reader::new(
            temp.path(),
            &root_id(temp.path()),
            |event, _| {
                if event == Event::Read {
                    clock.set(clock.get() + Duration::from_millis(110));
                }
            },
        )
        .unwrap();
        let r = group(&entries, &mut reader, &mut meter);
        assert!(r.partial);
        assert!(r.groups.is_empty());
        assert_eq!(r.stop, Some(Reason::TimeBudget));
        assert_eq!(r.bytes_read, 4);
    }
    #[test]
    fn native_junction_swap_between_stages_is_rejected() {
        let Some(temp) = fixture() else { return };
        fs::create_dir(temp.path().join("nested")).unwrap();
        fs::create_dir(temp.path().join("target")).unwrap();
        for n in ["a", "b"] {
            fs::write(temp.path().join("nested").join(n), b"same").unwrap();
            fs::write(temp.path().join("target").join(n), b"same").unwrap();
        }
        let entries = [
            entry(temp.path(), "nested/a"),
            entry(temp.path(), "nested/b"),
        ];
        let mut swapped = false;
        let r = inspect_with_hook(
            temp.path(),
            &root_id(temp.path()),
            &entries,
            Budget::test(),
            |event, _| {
                if event == Event::Full && !swapped {
                    fs::rename(temp.path().join("nested"), temp.path().join("old")).unwrap();
                    crate::fixtures::junction(
                        &temp.path().join("nested"),
                        &temp.path().join("target"),
                    )
                    .unwrap();
                    swapped = true;
                }
            },
        )
        .unwrap();
        assert!(swapped);
        assert!(r.groups.is_empty());
        assert!(r
            .skipped
            .iter()
            .any(|s| s.reason == Reason::ChangedDuringRead));
        fs::remove_dir(temp.path().join("nested")).unwrap();
    }
    #[test]
    fn native_replacement_and_traversal_are_rejected() {
        let Some(temp) = fixture() else { return };
        for n in ["a", "b"] {
            fs::write(temp.path().join(n), b"same").unwrap();
        }
        let mut entries = [entry(temp.path(), "a"), entry(temp.path(), "b")];
        fs::rename(temp.path().join("b"), temp.path().join("old")).unwrap();
        fs::write(temp.path().join("b"), b"same").unwrap();
        let r = run(temp.path(), &entries);
        assert!(r.groups.is_empty());
        assert_eq!(r.skipped[0].reason, Reason::ChangedDuringRead);
        entries[1].relative = "../outside".into();
        assert_eq!(
            run(temp.path(), &entries).skipped[0].reason,
            Reason::InvalidPath
        );
    }
}
