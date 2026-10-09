#[test]
fn volume_unknowns_and_invalid_capacity_do_not_become_zero() {
    use crate::volumes::{Capacity, Fact, Volume};
    let mut volume = Volume {
        volume_name: "synthetic-volume".into(),
        mount_paths: Fact::Known {
            value: vec!["X:\\".into()],
        },
        filesystem: Fact::Unknown {
            reason: "offline".into(),
        },
        capacity: Fact::Unknown {
            reason: "offline".into(),
        },
        drive_type: Fact::Unknown {
            reason: "offline".into(),
        },
        bus_type: Fact::Unknown {
            reason: "permission denied".into(),
        },
        removable_media: Fact::Unknown {
            reason: "unavailable".into(),
        },
        incurs_seek_penalty: Fact::Unknown {
            reason: "unavailable".into(),
        },
        trim_enabled: Fact::Unknown {
            reason: "unavailable".into(),
        },
        availability: Fact::Known { value: false },
    };
    volume.validate().unwrap();
    assert_eq!(
        serde_json::to_value(&volume).unwrap()["capacity"],
        serde_json::json!({ "state": "unknown", "reason": "offline" })
    );
    for (total, free, available) in [("100", "101", "0"), ("100", "90", "91"), ("-1", "0", "0")] {
        volume.capacity = Fact::Known {
            value: Capacity {
                total_bytes: total.into(),
                free_bytes: free.into(),
                available_bytes: available.into(),
            },
        };
        assert!(volume.validate().is_err());
    }
}

#[cfg(not(windows))]
#[test]
fn non_windows_calls_are_explicitly_unsupported() {
    use crate::{fixtures, identity, volumes};
    use std::{io::ErrorKind, path::Path};
    assert_eq!(
        identity::observe(Path::new("synthetic"))
            .unwrap_err()
            .kind(),
        ErrorKind::Unsupported
    );
    assert_eq!(
        volumes::enumerate().unwrap_err().kind(),
        ErrorKind::Unsupported
    );
    assert_eq!(
        fixtures::create(Path::new("synthetic")).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
    assert_eq!(
        fixtures::destroy(Path::new("synthetic"))
            .unwrap_err()
            .kind(),
        ErrorKind::Unsupported
    );
}

#[cfg(windows)]
mod windows {
    use crate::{
        fixtures,
        identity::{observe, verify_current, Verification},
        volumes,
    };
    use std::fs;

    fn temp() -> tempfile::TempDir {
        let base = std::env::current_dir()
            .unwrap()
            .join("target/test-fixtures");
        fs::create_dir_all(&base).unwrap();
        tempfile::tempdir_in(base).unwrap()
    }

    #[test]
    fn fixture_cleanup_refuses_foreign_content_and_outside_roots() {
        let base = temp();
        let root = base.path().join("lab");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("precious"), b"retain").unwrap();
        assert!(fixtures::create_in(&root, base.path()).is_err());
        assert!(fixtures::destroy_in(&root, base.path()).is_err());
        assert!(fixtures::create_in(&base.path().join("../escape"), base.path()).is_err());
        fs::remove_file(root.join("precious")).unwrap();
        let manifest = fixtures::create_in(&root, base.path()).unwrap();
        assert!(!manifest.entries.is_empty());
        let repeated = fixtures::create_in(&root, base.path()).unwrap();
        assert_eq!(repeated.entries.len(), manifest.entries.len());
        fs::write(root.join("unlisted"), b"retain").unwrap();
        assert!(fixtures::destroy_in(&root, base.path()).is_err());
        assert!(root.join("ordinary.txt").exists());
        fs::remove_file(root.join("unlisted")).unwrap();
        fixtures::destroy_in(&root, base.path()).unwrap();
        assert!(!root.exists());
    }

    #[test]
    fn fixture_root_aliases_are_rejected_before_creation() {
        let base = temp();
        assert!(fixtures::create_in(&base.path().join("alias "), base.path()).is_err());
        assert!(fixtures::create_in(&base.path().join("alias."), base.path()).is_err());
        assert_eq!(fs::read_dir(base.path()).unwrap().count(), 0);
    }

    #[test]
    fn unlisted_alternate_streams_block_cleanup_without_deletion() {
        let base = temp();
        let root = base.path().join("lab");
        fixtures::create_in(&root, base.path()).unwrap();
        let stream = root.join("ordinary.txt:foreign");
        fs::write(&stream, b"retain unlisted stream").unwrap();
        assert!(fixtures::destroy_in(&root, base.path()).is_err());
        assert_eq!(fs::read(&stream).unwrap(), b"retain unlisted stream");
        fs::remove_file(stream).unwrap();
        fixtures::destroy_in(&root, base.path()).unwrap();
    }

    #[test]
    fn fixture_guards_pin_ancestors_against_junction_replacement() {
        let base = temp();
        let parent = base.path().join("parent");
        let root = parent.join("lab");
        fs::create_dir(&parent).unwrap();
        let guards = fixtures::pin(&root).unwrap();
        assert!(fs::rename(&parent, base.path().join("retargeted")).is_err());
        drop(guards);
        fs::rename(&parent, base.path().join("retargeted")).unwrap();
        fs::remove_dir(base.path().join("retargeted")).unwrap();
    }

    #[test]
    fn handle_identity_distinguishes_links_changes_recreation_and_gone() {
        let base = temp();
        let root = base.path().join("lab");
        fixtures::create_in(&root, base.path()).unwrap();
        let path = root.join("ordinary.txt");
        let before = observe(&path).unwrap();
        let linked = observe(&root.join("hard-link.txt")).unwrap();
        let equal = observe(&root.join("equal.txt")).unwrap();
        assert!(before.same_object(&linked));
        assert!(!before.same_object(&equal));
        assert!(before.link_count >= 2);
        assert_eq!(verify_current(&path, &before), Verification::Same);
        fs::write(&path, b"changed length").unwrap();
        assert_eq!(verify_current(&path, &before), Verification::Changed);
        let stale_path = root.join("recreated.txt");
        let stale = observe(&stale_path).unwrap();
        fs::remove_file(&stale_path).unwrap();
        assert_eq!(verify_current(&stale_path, &stale), Verification::Gone);
        fs::write(&stale_path, b"new incarnation").unwrap();
        assert_eq!(verify_current(&stale_path, &stale), Verification::Recreated);
        // Replacement is intentionally not an owned fixture anymore.
        assert!(fixtures::destroy_in(&root, base.path()).is_err());
        fs::remove_file(&stale_path).unwrap();
        fixtures::destroy_in(&root, base.path()).unwrap();
    }

    #[test]
    fn volume_bounds_are_real_or_named_unknowns() {
        let inventory = volumes::enumerate().unwrap();
        assert!(!inventory.is_empty());
        for volume in inventory {
            volume.validate().unwrap();
        }
    }

    #[test]
    fn junction_swaps_are_rejected_even_when_the_leaf_is_the_same_hardlink() {
        let base = temp();
        let a = base.path().join("a");
        let b = base.path().join("b");
        let link = base.path().join("junction");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join("leaf.txt"), b"shared synthetic object").unwrap();
        fs::hard_link(a.join("leaf.txt"), b.join("leaf.txt")).unwrap();
        if let Err(error) = fixtures::junction(&link, &a) {
            eprintln!("SKIP junction-swap: {error}");
            return;
        }
        let root_before = observe(&link).unwrap();
        let leaf_before = observe(&link.join("leaf.txt")).unwrap();
        assert_eq!(verify_current(&link, &root_before), Verification::Same);
        assert!(fixtures::create_in(&link.join("escape"), base.path()).is_err());
        assert!(fixtures::destroy_in(&link, base.path()).is_err());
        fs::remove_dir(&link).unwrap();
        fixtures::junction(&link, &b).unwrap();
        assert_eq!(verify_current(&link, &root_before), Verification::Recreated);
        assert!(leaf_before.same_object(&observe(&link.join("leaf.txt")).unwrap()));
        assert_eq!(
            verify_current(&link.join("leaf.txt"), &leaf_before),
            Verification::Recreated
        );
        fs::remove_dir(link).unwrap();
        fs::remove_file(a.join("leaf.txt")).unwrap();
        fs::remove_file(b.join("leaf.txt")).unwrap();
        fs::remove_dir(a).unwrap();
        fs::remove_dir(b).unwrap();
    }

    #[test]
    fn manifest_traversal_is_refused_without_partial_deletion() {
        let base = temp();
        let root = base.path().join("lab");
        fixtures::create_in(&root, base.path()).unwrap();
        let marker = root.join(".loomward-fixtures.json");
        let original = fs::read(&marker).unwrap();
        let mut manifest: serde_json::Value = serde_json::from_slice(&original).unwrap();
        manifest["entries"][0]["relative"] = "../outside.txt".into();
        fs::write(base.path().join("outside.txt"), b"must remain").unwrap();
        fs::write(&marker, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(fixtures::destroy_in(&root, base.path()).is_err());
        assert_eq!(
            fs::read(base.path().join("outside.txt")).unwrap(),
            b"must remain"
        );
        assert!(root.join("ordinary.txt").exists());
        fs::write(marker, original).unwrap();
        fixtures::destroy_in(&root, base.path()).unwrap();
        fs::remove_file(base.path().join("outside.txt")).unwrap();
    }

    #[test]
    fn fixture_classes_are_native_or_have_named_skips() {
        use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt};
        use windows_sys::Win32::Storage::FileSystem::*;
        let base = temp();
        let root = base.path().join("lab");
        let manifest = fixtures::create_in(&root, base.path()).unwrap();
        let skipped = |case: &str| {
            manifest
                .skips
                .iter()
                .any(|s| s.case == case && !s.reason.is_empty())
        };
        let attrs = observe(&root.join("attributes.txt")).unwrap().attributes;
        for flag in [
            FILE_ATTRIBUTE_READONLY,
            FILE_ATTRIBUTE_HIDDEN,
            FILE_ATTRIBUTE_SYSTEM,
        ] {
            assert_ne!(attrs & flag, 0);
        }
        if !skipped("sparse-file") {
            let sparse = observe(&root.join("sparse.bin")).unwrap();
            assert_ne!(sparse.attributes & FILE_ATTRIBUTE_SPARSE_FILE, 0);
            assert_eq!(sparse.size, (8 * 1024 * 1024).to_string());
            assert!(
                sparse.allocation.parse::<u64>().unwrap() < sparse.size.parse::<u64>().unwrap()
            );
        }
        if !skipped("ntfs-compression") {
            assert_ne!(
                observe(&root.join("compressed.txt")).unwrap().attributes
                    & FILE_ATTRIBUTE_COMPRESSED,
                0
            );
        }
        if !skipped("alternate-data-stream") {
            assert_eq!(
                fs::read(root.join("streams.txt:loomward")).unwrap(),
                b"synthetic alternate stream"
            );
        }
        for (name, case, tag) in [
            ("junction", "directory-junction", 0xA0000003),
            ("file-symlink", "file-symlink", 0xA000000C),
            ("directory-symlink", "directory-symlink", 0xA000000C),
        ] {
            if !skipped(case) {
                assert_eq!(observe(&root.join(name)).unwrap().object.reparse_tag, tag);
            }
        }
        let long = manifest
            .entries
            .iter()
            .find(|e| e.relative.ends_with("long-file.txt"))
            .unwrap();
        let long_path = fs::canonicalize(root.join(&long.relative)).unwrap();
        assert!(long_path.as_os_str().encode_wide().count() > 260);
        assert!(long_path.to_string_lossy().starts_with(r"\\?\"));
        assert_eq!(fs::read(long_path).unwrap(), b"extended length fixture");
        assert!(root.join("日本語-🧵.txt").is_file());
        assert_ne!(
            manifest.stale_identity.as_ref().unwrap(),
            &observe(&root.join("recreated.txt")).unwrap().object
        );
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(root.join("locked.txt"))
            .unwrap();
        assert!(fs::File::open(root.join("locked.txt")).is_err());
        drop(lock);
        assert!(fs::File::open(root.join("locked.txt")).is_ok());
        assert!(skipped("cloud-placeholder"));
        assert!(skipped("removable-media"));
        fixtures::destroy_in(&root, base.path()).unwrap();
    }
}
