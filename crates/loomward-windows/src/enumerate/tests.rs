use super::*;

fn record(strategy: Strategy, name: &[u16]) -> Vec<u8> {
    let header = if strategy == Strategy::Extended {
        88
    } else {
        104
    };
    let mut b = vec![0; header + name.len() * 2];
    b[60..64].copy_from_slice(&((name.len() * 2) as u32).to_le_bytes());
    for (i, c) in name.iter().enumerate() {
        b[header + i * 2..header + i * 2 + 2].copy_from_slice(&c.to_le_bytes());
    }
    b
}
#[test]
fn buffer_end_is_not_directory_end() {
    let b = record(Strategy::Extended, &[b'x' as u16]);
    assert_eq!(
        decode_buffer(&b, Strategy::Extended, &mut |_| Flow::Continue),
        Ok(())
    );
    assert_eq!(
        query_stop(0, false),
        ListOutcome::Incomplete(IncompleteReason::Malformed)
    );
    assert_eq!(query_stop(18, false), ListOutcome::Complete);
}
#[test]
fn malformed_next_entry_offset() {
    for next in [1u32, 80, 96, u32::MAX] {
        let mut b = record(Strategy::Extended, &[120]);
        b[..4].copy_from_slice(&next.to_le_bytes());
        assert_eq!(
            decode_buffer(&b, Strategy::Extended, &mut |_| Flow::Continue),
            Err(IncompleteReason::Malformed)
        );
    }
}
#[test]
fn name_must_fit_its_record() {
    let mut b = record(Strategy::Extended, &[120]);
    b[60..64].copy_from_slice(&4u32.to_le_bytes());
    assert_eq!(
        decode_buffer(&b, Strategy::Extended, &mut |_| Flow::Continue),
        Err(IncompleteReason::Malformed)
    );
    b[60..64].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(
        decode_buffer(&b, Strategy::Extended, &mut |_| Flow::Continue),
        Err(IncompleteReason::Malformed)
    );
}
#[test]
fn zero_length_buffer_and_name_are_malformed() {
    for b in [vec![], record(Strategy::Extended, &[])] {
        assert_eq!(
            decode_buffer(&b, Strategy::Extended, &mut |_| Flow::Continue),
            Err(IncompleteReason::Malformed)
        );
    }
}
#[test]
fn missing_post_open_id_does_not_trust_listing() {
    assert_eq!(
        validate_identity(Some(FileIdObs::Id64(7)), None, false),
        Err(SourceError::IdentityUnverified)
    );
    assert_eq!(
        validate_identity(None, None, false),
        Ok((None, IdBasis::None))
    );
    assert_eq!(
        validate_identity(None, Some(FileIdObs::Id128([7; 16])), false),
        Ok((Some(FileIdObs::Id128([7; 16])), IdBasis::PostOpen))
    );
}
#[test]
fn id_width_is_preserved_and_refs64_is_not_unique() {
    let id = FileIdObs::Id64(7);
    assert_eq!(
        validate_identity(Some(id), Some(id), false),
        Ok((Some(id), IdBasis::Listed))
    );
    assert_eq!(
        validate_identity(Some(id), Some(id), true),
        Ok((Some(id), IdBasis::NonUnique))
    );
    assert_eq!(
        validate_identity(Some(id), Some(FileIdObs::Id128([7; 16])), false),
        Err(SourceError::IdentityChanged)
    );
    let mut b = record(Strategy::Both, &[120]);
    b[96..104].copy_from_slice(&7u64.to_le_bytes());
    decode_buffer(&b, Strategy::Both, &mut |e| {
        assert!(matches!(e.file_id, Some(FileIdObs::Id64(_))));
        Flow::Continue
    })
    .unwrap();
}
#[test]
fn placeholders_and_reparse_attributes_are_refused() {
    for flag in [0x400, 0x1000, 0x40000, 0x400000] {
        assert!(refused_attributes(0x10 | flag));
    }
    assert!(!refused_attributes(0x10));
}
#[cfg(windows)]
#[test]
fn native_handle_relative_open_and_all_strategy_widths() {
    use std::fs;
    let temp = canonical_tempdir();
    fs::create_dir(temp.path().join("child")).unwrap();
    fs::write(temp.path().join("file"), b"synthetic").unwrap();
    for strategy in [Strategy::Extended, Strategy::Both, Strategy::Find] {
        let source = NativeSource::with_strategy(strategy);
        let (dir, _) = source.open_root(temp.path()).unwrap();
        let mut files = 0;
        let mut children = 0;
        let outcome = source.list(&dir, &mut |e| {
            if e.name == [99, 104, 105, 108, 100] {
                let (_, opened) = source.open_child(&dir, &e).unwrap();
                children += 1;
                match strategy {
                    Strategy::Extended => assert!(matches!(opened.id, Some(FileIdObs::Id128(_)))),
                    Strategy::Both => assert!(matches!(opened.id, Some(FileIdObs::Id64(_)))),
                    Strategy::Find => assert_eq!(opened.basis, IdBasis::PostOpen),
                    _ => unreachable!(),
                }
            } else {
                files += 1;
                assert_eq!(e.end_of_file, 9);
            }
            Flow::Continue
        });
        assert_eq!(outcome, ListOutcome::Complete);
        assert_eq!((children, files), (1, 1));
        assert_eq!(dir.strategy(), strategy);
    }
}
#[cfg(windows)]
#[test]
fn child_open_remains_bound_to_renamed_parent_handle() {
    let temp = canonical_tempdir();
    let a = temp.path().join("a");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(a.join("child")).unwrap();
    let source = NativeSource::default();
    let (parent, _) = source.open_root(&a).unwrap();
    let mut observed = None;
    source.list(&parent, &mut |e| {
        observed = e.file_id;
        Flow::Continue
    });
    std::fs::rename(&a, temp.path().join("moved")).unwrap();
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(a.join("child")).unwrap();
    let e = RawEntry {
        name: &[99, 104, 105, 108, 100],
        file_id: observed,
        attributes: 0x10,
        reparse_tag: None,
        end_of_file: 0,
        allocation_size: None,
        creation: None,
        last_write: None,
        change: None,
        last_access: None,
    };
    let (_, identity) = source.open_child(&parent, &e).unwrap();
    assert_eq!(identity.id, observed);
}
#[cfg(windows)]
#[test]
fn junction_never_opened_or_followed() {
    let temp = canonical_tempdir();
    let target = temp.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("sentinel"), b"synthetic").unwrap();
    let link = temp.path().join("junction");
    crate::fixtures::junction(&link, &target).unwrap();
    let source = NativeSource::default();
    assert!(matches!(source.open_root(&link), Err(SourceError::Refused)));
    let (dir, _) = source.open_root(temp.path()).unwrap();
    let mut seen = false;
    assert_eq!(
        source.list(&dir, &mut |e| {
            if e.name == "junction".encode_utf16().collect::<Vec<_>>() {
                seen = true;
                assert!(matches!(
                    source.open_child(&dir, &e),
                    Err(SourceError::Refused)
                ));
            }
            Flow::Continue
        }),
        ListOutcome::Complete
    );
    assert!(seen);
    std::fs::remove_dir(&link).unwrap();
}
#[cfg(windows)]
#[test]
fn root_with_intermediate_junction_is_refused() {
    let temp = canonical_tempdir();
    let target = temp.path().join("outside");
    std::fs::create_dir_all(target.join("root")).unwrap();
    let link = temp.path().join("parent");
    crate::fixtures::junction(&link, &target).unwrap();
    let source = NativeSource::default();
    // The leaf is ordinary; only the intermediate junction redirects resolution.
    let result = source.open_root(&link.join("root"));
    std::fs::remove_dir(&link).unwrap();
    assert!(matches!(result, Err(SourceError::Refused)));
    let (_, ordinary) = source.open_root(&target.join("root")).unwrap();
    assert!(ordinary.id.is_some());
    let (_, verbatim) = source
        .open_root(&std::fs::canonicalize(target.join("root")).unwrap())
        .unwrap();
    assert_eq!(ordinary.id, verbatim.id);
    let (_, mixed_case) = source
        .open_root(&temp.path().join("OUTSIDE").join("root"))
        .unwrap();
    assert_eq!(ordinary.id, mixed_case.id);
}
#[cfg(windows)]
#[test]
fn post_open_attributes_and_id_replacement_are_rejected() {
    let temp = canonical_tempdir();
    let child = temp.path().join("child");
    std::fs::create_dir(&child).unwrap();
    let source = NativeSource::default();
    let (dir, _) = source.open_root(temp.path()).unwrap();
    let mut id = None;
    source.list(&dir, &mut |e| {
        id = e.file_id;
        Flow::Continue
    });
    std::fs::rename(&child, temp.path().join("old")).unwrap();
    std::fs::create_dir(&child).unwrap();
    let e = RawEntry {
        name: &[99, 104, 105, 108, 100],
        file_id: id,
        attributes: 0x10,
        reparse_tag: None,
        end_of_file: 0,
        allocation_size: None,
        creation: None,
        last_write: None,
        change: None,
        last_access: None,
    };
    assert!(matches!(
        source.open_child(&dir, &e),
        Err(SourceError::IdentityChanged)
    ));
}
#[cfg(windows)]
#[test]
fn post_open_offline_race_is_refused_before_listing() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        SetFileAttributesW, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_OFFLINE,
    };
    let temp = canonical_tempdir();
    let child = temp.path().join("child");
    std::fs::create_dir(&child).unwrap();
    let source = NativeSource::default();
    let (parent, _) = source.open_root(temp.path()).unwrap();
    let mut id = None;
    source.list(&parent, &mut |e| {
        id = e.file_id;
        Flow::Continue
    });
    let path: Vec<u16> = child.as_os_str().encode_wide().chain(Some(0)).collect();
    assert_ne!(
        unsafe { SetFileAttributesW(path.as_ptr(), FILE_ATTRIBUTE_OFFLINE) },
        0
    );
    let entry = RawEntry {
        name: &[99, 104, 105, 108, 100],
        file_id: id,
        attributes: 0x10,
        reparse_tag: None,
        end_of_file: 0,
        allocation_size: None,
        creation: None,
        last_write: None,
        change: None,
        last_access: None,
    };
    let result = source.open_child(&parent, &entry);
    assert_ne!(
        unsafe { SetFileAttributesW(path.as_ptr(), FILE_ATTRIBUTE_NORMAL) },
        0
    );
    assert!(matches!(result, Err(SourceError::Refused)));
}
#[test]
fn absent_zero_ids_do_not_become_unique_observations() {
    for strategy in [Strategy::Extended, Strategy::Both] {
        let mut buffer = record(strategy, &[120]);
        if strategy == Strategy::Both {
            buffer[96..104].fill(0);
        }
        decode_buffer(&buffer, strategy, &mut |e| {
            assert_eq!(e.file_id, None);
            Flow::Continue
        })
        .unwrap();
    }
}

/// Native roots are opened component by component and refuse reparse ancestors, so tests grant a
/// canonical temp base (hosted runners may junction the temp folder), as a real grant would.
#[cfg(windows)]
fn canonical_tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
}
