use super::*;

fn synthetic_entry(id: u8, volume: &str, size: u64) -> Entry {
    use crate::identity::{IdentityQuality, ObjectKey};
    Entry {
        relative: format!("fixture-{id}").into(),
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
    ) -> Result<bool, Reason> {
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

#[cfg(windows)]
mod native {
    use super::*;
    use crate::enumerate::{DirSource, NativeSource};
    use std::{fs, path::Path};

    fn fixture() -> Option<tempfile::TempDir> {
        let temp = tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        if crate::enumerate::running_elevated() {
            assert_eq!(
                inspect(temp.path(), &dummy_root(), &[], Budget::test()).unwrap_err(),
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
    fn run(root: &Path, entries: &[Entry]) -> Report {
        inspect(root, &root_id(root), entries, Budget::test()).unwrap()
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
            let r = inspect(temp.path(), &root_id(temp.path()), &entries, budget).unwrap();
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
    fn native_final_validation_drops_an_earlier_equal_pair() {
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
        assert!(r.groups.is_empty());
        assert!(r
            .skipped
            .iter()
            .any(|s| s.entry == 0 && s.reason == Reason::ChangedDuringRead));
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
                    fs::write(temp.path().join("b"), b"else").unwrap();
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
        let r = inspect_with_hook(
            temp.path(),
            &root_id(temp.path()),
            &entries,
            Budget {
                bytes: 100,
                time: Duration::from_millis(100),
            },
            |event, _| {
                if event == Event::Read {
                    std::thread::sleep(Duration::from_millis(110));
                }
            },
        )
        .unwrap();
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
