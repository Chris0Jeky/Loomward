use super::*;
use std::path::PathBuf;

fn link(flags: u32) -> Vec<u8> {
    let mut bytes = vec![0; 80];
    bytes[..4].copy_from_slice(&76u32.to_le_bytes());
    bytes[4..20].copy_from_slice(&[1, 20, 2, 0, 0, 0, 0, 0, 192, 0, 0, 0, 0, 0, 0, 70]);
    bytes[20..24].copy_from_slice(&flags.to_le_bytes());
    bytes
}

use crate::junction_fixture::junction;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let canonical = dir.path().canonicalize().unwrap();
    let base = PathBuf::from(canonical.to_str().unwrap().strip_prefix(r"\\?\").unwrap());
    let parent = base.join("parent");
    let outside = base.join("outside");
    std::fs::create_dir_all(parent.join("child")).unwrap();
    std::fs::create_dir_all(outside.join("child")).unwrap();
    std::fs::write(parent.join("child").join("synthetic.lnk"), link(0)).unwrap();
    let mut other = link(4);
    other.splice(76..76, [2, 0, b'A', b'B']);
    std::fs::write(outside.join("child").join("synthetic.lnk"), other).unwrap();
    (dir, parent, outside)
}

#[test]
fn removable_and_optical_targets_do_not_touch_media() {
    for kind in [DRIVE_REMOVABLE, DRIVE_CDROM] {
        let mut touched = false;
        let state = volume_after_type(kind, || {
            touched = true;
            VolumeState::Available
        });
        assert_eq!(state, VolumeState::Unknown);
        assert!(!touched);
    }
    assert_eq!(
        volume_after_type(3, || VolumeState::Available),
        VolumeState::Available
    );
    assert_eq!(
        volume_after_type(3, || VolumeState::Unavailable),
        VolumeState::Unavailable
    );
}

#[test]
fn junction_ancestor_is_refused_with_reason() {
    let (_dir, parent, outside) = fixture();
    let path = parent.join("child").join("synthetic.lnk");
    if loomward_windows::enumerate::skip_elevated_test() {
        assert!(read(&path).is_err());
        assert_eq!(metadata(path.to_str().unwrap()), MetadataState::Unknown);
        return;
    }
    let alias = parent.with_file_name("alias");
    junction(&alias, &outside);
    let path = alias.join("child").join("synthetic.lnk");
    let error = read(&path).unwrap_err();
    assert!(error.to_string().contains("reparse"));
    assert_eq!(metadata(path.to_str().unwrap()), MetadataState::Unknown);
    assert_eq!(
        read(&parent.join("child").join("synthetic.lnk"))
            .unwrap()
            .flags,
        0
    );
    assert_eq!(metadata(parent.to_str().unwrap()), MetadataState::Present);
    std::fs::remove_dir(alias).unwrap();
}

#[test]
fn walk_stays_relative_after_junction_swap() {
    for contents in [false, true] {
        let (_dir, parent, outside) = fixture();
        let path = parent.join("child").join("synthetic.lnk");
        if loomward_windows::enumerate::skip_elevated_test() {
            assert!(read(&path).is_err());
            assert_eq!(metadata(path.to_str().unwrap()), MetadataState::Unknown);
            return;
        }
        let parent_index = parent.to_str().unwrap()[3..].split('\\').count();
        let mut swapped = false;
        let mut after_open = |index| {
            if index == parent_index {
                std::fs::rename(&parent, parent.with_file_name("moved")).unwrap();
                junction(&parent, &outside);
                swapped = true;
            }
        };
        if contents {
            assert_eq!(read_with(&path, &mut after_open).unwrap().flags, 0);
        } else {
            let mut handles = pins(path.to_str().unwrap(), &mut after_open).unwrap();
            let file = handles.pop().unwrap();
            assert_eq!(file.metadata().unwrap().len(), 80);
            drop(handles);
        }
        assert!(swapped);
        std::fs::remove_dir(&parent).unwrap();
    }
}

#[test]
fn reader_uses_final_pinned_handle_after_leaf_swap() {
    // The handle-swap proof lives beside the private capped reader in loomward-windows.
    let (_dir, parent, _outside) = fixture();
    let path = parent.join("child").join("synthetic.lnk");
    if loomward_windows::enumerate::skip_elevated_test() {
        assert!(read(&path).is_err());
        assert_eq!(metadata(path.to_str().unwrap()), MetadataState::Unknown);
    } else {
        assert_eq!(read(&path).unwrap().flags, 0);
    }
}
