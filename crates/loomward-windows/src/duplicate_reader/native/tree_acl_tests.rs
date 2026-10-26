use super::*;
use std::os::windows::fs::OpenOptionsExt;
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;

thread_local! {
    pub(super) static FAIL_ROOT_PARENT_READ: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

const WORLD: &[u8] = &[1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];
const OWNER: &[u8] = &[1, 2, 0, 0, 0, 0, 0, 5, 21, 0, 0, 0, 42, 0, 0, 0];
const GROUP: &[u8] = &[1, 2, 0, 0, 0, 0, 0, 5, 21, 0, 0, 0, 43, 0, 0, 0];
const CREATOR_OWNER: &[u8] = &[1, 1, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0];
const CREATOR_GROUP: &[u8] = &[1, 1, 0, 0, 0, 0, 0, 3, 1, 0, 0, 0];

fn ace(flags: u32, mask: u32, sid: &[u8]) -> Vec<u8> {
    let mut ace = vec![0, flags as u8];
    ace.extend_from_slice(&((8 + sid.len()) as u16).to_le_bytes());
    ace.extend_from_slice(&mask.to_le_bytes());
    ace.extend_from_slice(sid);
    ace
}

fn security(aces: Vec<Vec<u8>>, control: u16) -> SecurityDescriptor {
    SecurityDescriptor {
        owner: OWNER.into(),
        group: GROUP.into(),
        dacl: Some(aces),
        control,
    }
}

#[test]
fn legacy_propagation_maps_generic_rights_and_splits_directory_aces() {
    let oi_ci = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;
    let generic = GENERIC_READ | GENERIC_WRITE | GENERIC_EXECUTE | DELETE;
    let mapped = FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE;
    let parent = vec![ace(oi_ci | INHERIT_ONLY_ACE, generic, WORLD)];
    assert_eq!(
        propagated_aces(&parent, Kind::File, OWNER, GROUP),
        vec![ace(0, mapped, WORLD)]
    );
    assert_eq!(
        propagated_aces(&parent, Kind::Directory, OWNER, GROUP),
        vec![
            ace(0, mapped, WORLD),
            ace(oi_ci | INHERIT_ONLY_ACE, generic, WORLD),
        ]
    );
    for (generic, mapped) in [
        (GENERIC_READ, FILE_GENERIC_READ),
        (GENERIC_WRITE, FILE_GENERIC_WRITE),
        (GENERIC_EXECUTE, FILE_GENERIC_EXECUTE),
        (GENERIC_ALL, FILE_ALL_ACCESS),
    ] {
        assert_eq!(
            propagated_aces(&[ace(oi_ci, generic, WORLD)], Kind::File, OWNER, GROUP),
            vec![ace(0, mapped, WORLD)]
        );
    }
}

#[test]
fn legacy_propagation_handles_oi_ci_no_propagate_and_inherit_only() {
    let oi = OBJECT_INHERIT_ACE;
    let ci = CONTAINER_INHERIT_ACE;
    let np = NO_PROPAGATE_INHERIT_ACE;
    let io = INHERIT_ONLY_ACE;
    for flags in [oi, oi | io, oi | INHERITED_ACE] {
        assert_eq!(
            propagated_aces(
                &[ace(flags, FILE_GENERIC_READ, WORLD)],
                Kind::Directory,
                OWNER,
                GROUP
            ),
            vec![ace(oi | io, FILE_GENERIC_READ, WORLD)]
        );
    }
    assert!(propagated_aces(
        &[ace(oi | np, GENERIC_ALL, WORLD)],
        Kind::Directory,
        OWNER,
        GROUP
    )
    .is_empty());
    assert!(propagated_aces(&[ace(ci, GENERIC_ALL, WORLD)], Kind::File, OWNER, GROUP).is_empty());
    for flags in [ci | np, oi | ci | np | io] {
        assert_eq!(
            propagated_aces(
                &[ace(flags, GENERIC_ALL, WORLD)],
                Kind::Directory,
                OWNER,
                GROUP
            ),
            vec![ace(0, FILE_ALL_ACCESS, WORLD)]
        );
    }
    assert_eq!(
        propagated_aces(
            &[ace(oi | ci | io, FILE_GENERIC_READ, WORLD)],
            Kind::Directory,
            OWNER,
            GROUP
        ),
        vec![ace(oi | ci, FILE_GENERIC_READ, WORLD)]
    );
}

#[test]
fn legacy_creator_effective_aces_substitute_owner_and_group_only() {
    let flags = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;
    for (creator, replacement) in [(CREATOR_OWNER, OWNER), (CREATOR_GROUP, GROUP)] {
        for mask in [GENERIC_ALL, FILE_ALL_ACCESS] {
            let parent = vec![ace(flags, mask, creator)];
            assert_eq!(
                propagated_aces(&parent, Kind::File, OWNER, GROUP),
                vec![ace(0, FILE_ALL_ACCESS, replacement)]
            );
            assert_eq!(
                propagated_aces(&parent, Kind::Directory, OWNER, GROUP),
                vec![
                    ace(0, FILE_ALL_ACCESS, replacement),
                    ace(flags | INHERIT_ONLY_ACE, mask, creator),
                ]
            );
        }
    }
}

#[test]
fn legacy_matching_is_exact_and_multiset_and_parent_errors_fail_closed() {
    let inherited = ace(
        OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
        GENERIC_READ,
        WORLD,
    );
    let parent = Ok(security(vec![inherited], 0));
    let effective = ace(0, FILE_GENERIC_READ, WORLD);
    let mut child = security(vec![effective.clone()], 0);
    assert_eq!(explicit_aces(&child, Kind::File, Some(&parent)), Ok(vec![]));
    child.dacl.as_mut().unwrap().push(effective.clone());
    assert_eq!(
        explicit_aces(&child, Kind::File, Some(&parent)),
        Ok(vec![0])
    );
    for aces in [
        vec![
            effective.clone(),
            ace(INHERITED_ACE, FILE_GENERIC_READ, WORLD),
        ],
        vec![
            ace(INHERITED_ACE, FILE_GENERIC_READ, WORLD),
            effective.clone(),
        ],
    ] {
        assert_eq!(
            explicit_aces(&security(aces, 0), Kind::File, Some(&parent)),
            Ok(vec![0])
        );
    }
    assert_eq!(
        explicit_aces(&child, Kind::File, Some(&Err("parent_not_supplied"))),
        Ok(vec![0, 0])
    );
    for mismatch in [
        ace(INHERIT_ONLY_ACE, FILE_GENERIC_READ, WORLD),
        ace(0, FILE_GENERIC_WRITE, WORLD),
        ace(0, FILE_GENERIC_READ, OWNER),
    ] {
        assert_eq!(
            explicit_aces(&security(vec![mismatch], 0), Kind::File, Some(&parent)),
            Ok(vec![0])
        );
    }
    let mut deny = effective.clone();
    deny[0] = 1;
    assert_eq!(
        explicit_aces(&security(vec![deny], 0), Kind::File, Some(&parent)),
        Ok(vec![1])
    );
    for control in [SE_DACL_AUTO_INHERITED, SE_DACL_PROTECTED] {
        assert_eq!(
            explicit_aces(
                &security(vec![effective.clone()], control),
                Kind::File,
                Some(&parent)
            ),
            Ok(vec![0])
        );
    }
    assert!(same_ace(
        &effective,
        &ace(INHERITED_ACE, FILE_GENERIC_READ, WORLD)
    ));
    assert_eq!(
        explicit_aces(
            &security(
                vec![ace(INHERITED_ACE, FILE_GENERIC_READ, WORLD)],
                SE_DACL_AUTO_INHERITED
            ),
            Kind::File,
            None
        ),
        Ok(vec![])
    );
    assert_eq!(
        explicit_aces(&security(vec![effective], 0), Kind::File, None),
        Err("security_descriptor_unknown")
    );
}

#[test]
fn legacy_unreadable_parent_leaves_children_unknown() {
    if crate::enumerate::skip_elevated_test() {
        return;
    }
    let temp = legacy_fixture();
    let root = temp.path().join("legacy");
    // OWNER RIGHTS also suppresses the owner's implicit READ_CONTROL grant.
    let writer = security_file(&root);
    let user = sid_text(&read_security(&writer).unwrap().owner);
    set_dacl(&writer, &format!("D:(D;;RC;;;OW)(A;OICI;FA;;;{user})"));
    assert!(parent_security(&writer).is_err());
    drop(writer);
    let tree = tests::snapshot(&root);
    for name in ["file", "subdir"] {
        assert_eq!(
            tree.nodes[Path::new(name)].as_ref().unwrap().metadata[&Class::SecurityDescriptor],
            Err("security_descriptor_unknown"),
            "{name}"
        );
    }
}

fn security_file(path: &Path) -> File {
    File::options()
        .access_mode(READ_CONTROL | WRITE_DAC)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .unwrap()
}

fn set_dacl(file: &File, sddl: &str) {
    let wide: Vec<_> = sddl.encode_utf16().chain([0]).collect();
    let mut descriptor = ptr::null_mut();
    assert_ne!(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        },
        0
    );
    let result = unsafe {
        SetKernelObjectSecurity(file.as_raw_handle(), DACL_SECURITY_INFORMATION, descriptor)
    };
    unsafe {
        LocalFree(descriptor);
    }
    assert_ne!(result, 0, "{}", io::Error::last_os_error());
}

#[test]
fn group_sid_is_required_only_for_legacy_creator_group_substitution() {
    for (sddl, needs_group, unknown) in [
        ("O:SYD:AI(A;ID;FR;;;WD)", true, false),
        ("O:SYD:P(A;;FR;;;WD)", true, false),
        ("O:SYD:(A;;FR;;;WD)", false, false),
        ("O:SYD:(A;;FR;;;WD)", true, true),
        ("O:SYG:BAD:(A;;FR;;;WD)", true, false),
    ] {
        let wide: Vec<_> = sddl.encode_utf16().chain([0]).collect();
        let mut descriptor = ptr::null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    wide.as_ptr(),
                    1,
                    &mut descriptor,
                    ptr::null_mut(),
                )
            },
            0
        );
        let result = decode_security(descriptor, needs_group);
        unsafe {
            LocalFree(descriptor);
        }
        if unknown {
            assert_eq!(result.err(), Some("security_descriptor_unknown"), "{sddl}");
        } else {
            assert!(result.is_ok(), "{sddl}");
        }
    }
}

fn sid_text(sid: &[u8]) -> String {
    let authority = sid[2..8].iter().fold(0u64, |n, b| (n << 8) | u64::from(*b));
    let mut text = format!("S-{}-{authority}", sid[0]);
    for part in sid[8..].chunks_exact(4) {
        text.push_str(&format!(
            "-{}",
            u32::from_le_bytes(part.try_into().unwrap())
        ));
    }
    text
}

fn legacy_fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("legacy");
    let file = security_file(temp.path());
    let user = sid_text(&security_metadata(&file).unwrap().1);
    set_dacl(
        &file,
        &format!(
            concat!(
                "D:(A;OICI;0x1201bf;;;SY)(A;;FA;;;BA)(A;OICIIO;GA;;;BA)",
                "(A;;FA;;;SY)(A;OICIIO;GA;;;SY)(A;;0x1301bf;;;AU)",
                "(A;OICIIO;SDGXGWGR;;;AU)(A;;0x1200a9;;;BU)(A;OICIIO;GXGR;;;BU)",
                "(A;OICI;FA;;;{})"
            ),
            user
        ),
    );
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("file"), b"synthetic").unwrap();
    std::fs::create_dir(root.join("subdir")).unwrap();
    std::fs::write(root.join("subdir/inner"), b"synthetic").unwrap();
    temp
}

fn raw_ace_flags(file: &File) -> Vec<u8> {
    let mut needed = 0;
    unsafe {
        GetKernelObjectSecurity(
            file.as_raw_handle(),
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            0,
            &mut needed,
        );
    }
    assert!(needed > 0);
    let mut buffer = vec![0u32; (needed as usize).div_ceil(4)];
    assert_ne!(
        unsafe {
            GetKernelObjectSecurity(
                file.as_raw_handle(),
                DACL_SECURITY_INFORMATION,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        },
        0
    );
    let (mut present, mut defaulted, mut dacl) = (0, 0, ptr::null_mut());
    assert_ne!(
        unsafe {
            GetSecurityDescriptorDacl(
                buffer.as_mut_ptr().cast(),
                &mut present,
                &mut dacl,
                &mut defaulted,
            )
        },
        0
    );
    assert_ne!(present, 0);
    assert!(!dacl.is_null());
    (0..unsafe { (*dacl).AceCount })
        .map(|index| {
            let mut ace = ptr::null_mut();
            assert_ne!(unsafe { GetAce(dacl, u32::from(index), &mut ace) }, 0);
            unsafe { (*ace.cast::<ACE_HEADER>()).AceFlags }
        })
        .collect()
}

#[test]
fn legacy_measured_root_and_children_are_inherited_with_readable_parent() {
    if crate::enumerate::skip_elevated_test() {
        return;
    }
    let temp = legacy_fixture();
    let root = temp.path().join("legacy");
    for name in ["", "file", "subdir", "subdir/inner"] {
        let flags = raw_ace_flags(&security_file(&root.join(name)));
        assert!(!flags.is_empty());
        assert!(
            flags
                .iter()
                .all(|flag| u32::from(*flag) & INHERITED_ACE == 0),
            "{flags:?}"
        );
    }
    let mut tree = Tree::default();
    let accesses = record_tree_open_accesses(|| tree = tests::snapshot(&root));
    let writes = FILE_WRITE_DATA
        | FILE_APPEND_DATA
        | FILE_WRITE_EA
        | FILE_WRITE_ATTRIBUTES
        | FILE_DELETE_CHILD
        | DELETE
        | WRITE_DAC
        | WRITE_OWNER
        | GENERIC_WRITE
        | GENERIC_ALL;
    assert!(
        accesses.iter().all(|access| access & writes == 0),
        "{accesses:?}"
    );
    assert!(
        accesses.contains(&READ_CONTROL),
        "pinned parent reopen: {accesses:?}"
    );
    assert!(
        accesses
            .iter()
            .filter(|&&access| access == READ_CONTROL | FILE_READ_ATTRIBUTES)
            .count()
            == 2,
        "root parent and binding opens: {accesses:?}"
    );
    for name in ["", "file", "subdir", "subdir/inner"] {
        assert_eq!(
            tree.nodes[Path::new(name)].as_ref().unwrap().metadata[&Class::SecurityDescriptor],
            Ok(vec![]),
            "{name}"
        );
    }
    let report = loomward_operations::tree::compare(&tree, &tree);
    assert!(report.equal(), "{report:?}");
}

#[test]
fn legacy_root_parent_read_failure_is_unknown_for_root_only() {
    if crate::enumerate::skip_elevated_test() {
        return;
    }
    let temp = legacy_fixture();
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            FAIL_ROOT_PARENT_READ.set(false);
        }
    }
    let _reset = Reset;
    FAIL_ROOT_PARENT_READ.set(true);
    let tree = tests::snapshot(&temp.path().join("legacy"));
    for name in ["file", "subdir", "subdir/inner"] {
        assert_eq!(
            tree.nodes[Path::new(name)].as_ref().unwrap().metadata[&Class::SecurityDescriptor],
            Ok(vec![]),
            "{name}"
        );
    }
    let report = loomward_operations::tree::compare(&tree, &tree);
    assert!(!report.equal());
    assert!(report.differences.is_empty(), "{report:?}");
    assert_eq!(report.unknowns.len(), 2, "{report:?}");
    assert!(report.unknowns.iter().all(|u| u.path.as_os_str().is_empty()
        && u.field == loomward_operations::tree::Field::Metadata(Class::SecurityDescriptor)
        && u.reason == "security_descriptor_unknown"));
}

#[test]
fn root_parent_read_refuses_non_directories_and_reparse_parents() {
    if crate::enumerate::skip_elevated_test() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let expected = observe(&security_file(temp.path())).unwrap();
    let file = temp.path().join("file");
    std::fs::write(&file, b"synthetic").unwrap();
    assert!(root_parent_security(&file.join("root"), &expected).is_err());
    let link = temp.path().join("link");
    crate::fixtures::junction(&link, temp.path()).unwrap();
    assert!(root_parent_security(&link.join("root"), &expected).is_err());
}

#[test]
fn legacy_root_with_a_different_lexical_parent_binding_is_unknown() {
    if crate::enumerate::skip_elevated_test() {
        return;
    }
    let temp = legacy_fixture();
    let root = temp.path().join("legacy");
    let file = security_file(&root);
    let expected = observe(&file).unwrap();
    let link = temp.path().join("link");
    crate::fixtures::junction(&link, &root).unwrap();
    // The lexical entry is a junction; the observed root is its target.
    let parent = root_parent_security(&link, &expected).ok().map(Ok);
    assert!(parent.is_none());
    assert_eq!(
        explicit_aces(
            &read_security(&file).unwrap(),
            Kind::Directory,
            parent.as_ref()
        ),
        Err("security_descriptor_unknown")
    );
}

#[test]
fn legacy_child_extra_explicit_ace_and_protected_dacl_stay_explicit() {
    if crate::enumerate::skip_elevated_test() {
        return;
    }
    let temp = legacy_fixture();
    let root = temp.path().join("legacy");
    let child = security_file(&root.join("file"));
    let user = sid_text(&read_security(&child).unwrap().owner);
    set_dacl(
        &child,
        &format!(
            concat!(
                "D:(A;;0x1201bf;;;SY)(A;;FA;;;BA)(A;;FA;;;SY)(A;;0x1301bf;;;AU)",
                "(A;;0x1200a9;;;BU)(A;;FA;;;{})(A;;FR;;;WD)"
            ),
            user
        ),
    );
    let parent = parent_security(&security_file(&root));
    let descriptor = read_security(&child).unwrap();
    assert_eq!(
        descriptor.control & (SE_DACL_AUTO_INHERITED | SE_DACL_PROTECTED),
        0
    );
    assert_eq!(
        explicit_aces(&descriptor, Kind::File, Some(&parent)),
        Ok(vec![0])
    );
    // Close the ACL writer before directory-index metadata is used by the C1 walk.
    drop(child);
    assert_eq!(
        tests::snapshot(&root).nodes[Path::new("file")]
            .as_ref()
            .unwrap()
            .metadata[&Class::SecurityDescriptor],
        Ok(vec![0])
    );
    let child = security_file(&root.join("file"));
    set_dacl(&child, &format!("D:P(A;;FA;;;{user})(A;;FR;;;WD)"));
    let descriptor = read_security(&child).unwrap();
    assert_ne!(descriptor.control & SE_DACL_PROTECTED, 0);
    assert_eq!(
        explicit_aces(&descriptor, Kind::File, Some(&parent)),
        Ok(vec![0, 0])
    );
    drop(child);
    assert_eq!(
        tests::snapshot(&root).nodes[Path::new("file")]
            .as_ref()
            .unwrap()
            .metadata[&Class::SecurityDescriptor],
        Ok(vec![0, 0])
    );
}

#[test]
fn auto_inherited_temp_tree_remains_fully_equal() {
    if crate::enumerate::skip_elevated_test() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("subdir")).unwrap();
    std::fs::write(temp.path().join("subdir/file"), b"synthetic").unwrap();
    let root = read_security(&security_file(temp.path())).unwrap();
    assert_ne!(
        root.control & SE_DACL_AUTO_INHERITED,
        0,
        "TMP must be auto-inherited for this control"
    );
    for name in ["subdir", "subdir/file"] {
        let file = security_file(&temp.path().join(name));
        assert!(raw_ace_flags(&file)
            .iter()
            .all(|flag| u32::from(*flag) & INHERITED_ACE != 0));
        assert!(security_metadata(&file).unwrap().0.is_empty());
    }
    let tree = tests::snapshot(temp.path());
    let report = loomward_operations::tree::compare(&tree, &tree);
    assert!(report.equal(), "{report:?}");
}

#[test]
fn legacy_creator_and_propagation_rules_match_windows_children() {
    if crate::enumerate::skip_elevated_test() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let user = sid_text(&read_security(&security_file(temp.path())).unwrap().owner);
    for (index, inherited) in [
        "(A;OICIIO;GA;;;CO)",
        "(A;OICIIO;GA;;;CG)",
        "(A;OICI;FR;;;CO)",
        "(A;OI;GR;;;WD)",
        "(A;CI;GA;;;WD)",
        "(A;CI;FR;;;CO)",
        "(A;CINP;GA;;;WD)",
        "(A;OICINP;GA;;;CO)",
    ]
    .into_iter()
    .enumerate()
    {
        let root = temp.path().join(format!("case{index}"));
        std::fs::create_dir(&root).unwrap();
        set_dacl(
            &security_file(&root),
            &format!("D:{inherited}(A;OICI;FA;;;{user})"),
        );
        std::fs::create_dir(root.join("dir")).unwrap();
        std::fs::write(root.join("file"), b"synthetic").unwrap();
        let tree = tests::snapshot(&root);
        for name in ["dir", "file"] {
            assert_eq!(
                tree.nodes[Path::new(name)].as_ref().unwrap().metadata[&Class::SecurityDescriptor],
                Ok(vec![]),
                "{index}: {name}"
            );
        }
    }
}
