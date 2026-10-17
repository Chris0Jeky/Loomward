use super::*;
use std::{fs, io::Write};

fn lab() -> (tempfile::TempDir, Destination) {
    let temp = tempfile::tempdir().unwrap();
    let expected = crate::identity::observe(temp.path()).unwrap().object;
    let destination = Destination::open(temp.path(), &expected).unwrap();
    (temp, destination)
}

#[test]
fn create_new_and_publish_never_replace() {
    let (temp, destination) = lab();
    fs::write(temp.path().join("taken"), b"keep").unwrap();
    assert!(destination.stage("taken").is_err());
    let mut stage = destination.stage("staging").unwrap();
    let file = stage.create_file(&[], "payload").unwrap();
    assert!(stage.create_file(&[], "payload").is_err());
    let record = file.identity().unwrap();
    file.write_and_flush(&mut &b"copied bytes"[..], 12).unwrap();
    drop(file);
    let hold = stage.verify(&[], "payload", &record).unwrap();
    assert_eq!(
        hold.digest,
        sha2::Sha256::digest(b"copied bytes").as_slice()
    );
    assert!(stage.publish("taken").is_err());
    assert_eq!(fs::read(temp.path().join("taken")).unwrap(), b"keep");
    stage.publish("published").unwrap();
    println!("H2 publish with own noncached deny-write hold: succeeded");
    assert!(stage.cleanup_file(&[], "payload", &record).is_err());
    assert_eq!(
        fs::read(temp.path().join("published/payload")).unwrap(),
        b"copied bytes"
    );
}

#[test]
fn noncached_partial_sector_sizes() {
    let (_temp, destination) = lab();
    println!(
        "H3 IOCTL physical-sector alignment: {} bytes",
        destination.sector
    );
    let stage = destination.stage("staging").unwrap();
    for size in [0, 1, 511, 512, 4095, 4096, 4097, 1_048_577] {
        let name = format!("size-{size}");
        let bytes = vec![0x5a; size];
        let file = stage.create_file(&[], &name).unwrap();
        let record = file.identity().unwrap();
        assert_eq!(
            file.write_and_flush(&mut bytes.as_slice(), size as u64)
                .unwrap(),
            <[u8; 32]>::from(sha2::Sha256::digest(&bytes))
        );
        drop(file);
        let held = stage.verify(&[], &name, &record).unwrap();
        assert_eq!(held.digest, <[u8; 32]>::from(sha2::Sha256::digest(&bytes)));
        assert_eq!(held.size, size as u64);
    }
}

#[test]
fn cleanup_refuses_replacement_hardlink_and_renamed_staging() {
    let (temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let file = stage.create_file(&[], "payload").unwrap();
    let record = file.identity().unwrap();
    drop(file);
    fs::hard_link(
        temp.path().join("staging/payload"),
        temp.path().join("outside"),
    )
    .unwrap();
    assert!(stage.cleanup_file(&[], "payload", &record).is_err());
    fs::remove_file(temp.path().join("outside")).unwrap();
    fs::rename(
        temp.path().join("staging/payload"),
        temp.path().join("staging/old"),
    )
    .unwrap();
    fs::write(temp.path().join("staging/payload"), b"replacement").unwrap();
    assert!(stage.cleanup_file(&[], "payload", &record).is_err());
    fs::rename(temp.path().join("staging"), temp.path().join("renamed")).unwrap();
    assert!(stage.cleanup_file(&[], "old", &record).is_err());
    assert!(temp.path().join("renamed/old").exists());
    assert_eq!(
        fs::read(temp.path().join("renamed/payload")).unwrap(),
        b"replacement"
    );
}

#[test]
fn invalid_components_never_escape_staging() {
    let (_temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    for name in [
        "",
        ".",
        "..",
        "../escape",
        "a\\b",
        "a:b",
        "NUL",
        "tail.",
        "tail ",
        "COM¹",
        "LPT³.txt",
    ] {
        assert!(stage.create_file(&[], name).is_err(), "{name}");
    }
}

#[test]
fn metadata_survives_reopen_and_readonly_cleanup() {
    let (_temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let file = stage.create_file(&[], "payload").unwrap();
    file.write_and_flush(&mut &b"data"[..], 4).unwrap();
    let metadata = BasicMetadata {
        creation_time: 133_000_000_000_000_000,
        last_write_time: 133_000_123_456_789_000,
        attributes: FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_ARCHIVE,
    };
    file.set_metadata(metadata).unwrap();
    let record = file.identity().unwrap();
    drop(file);
    let held = stage.verify(&[], "payload", &record).unwrap();
    assert_eq!(held.metadata().unwrap(), metadata);
    drop(held);
    stage.cleanup_file(&[], "payload", &record).unwrap();
}

#[test]
fn verification_holds_deny_writers_and_detects_corruption() {
    let (temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let file = stage.create_file(&[], "payload").unwrap();
    let record = file.identity().unwrap();
    let digest = file.write_and_flush(&mut &b"data"[..], 4).unwrap();
    assert!(stage.verify(&[], "payload", &record).is_err());
    drop(file);
    let held = stage.verify(&[], "payload", &record).unwrap();
    assert_eq!(held.digest, digest);
    assert!(fs::OpenOptions::new()
        .write(true)
        .open(temp.path().join("staging/payload"))
        .is_err());
    drop(held);
    fs::OpenOptions::new()
        .write(true)
        .open(temp.path().join("staging/payload"))
        .unwrap()
        .write_all(b"evil")
        .unwrap();
    assert_ne!(
        stage.verify(&[], "payload", &record).unwrap().digest,
        digest
    );
}

#[test]
fn h1_relative_named_stream_open_measurement() {
    let (temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let file = stage.create_file(&[], "payload").unwrap();
    fs::write(
        temp.path().join("staging/payload:synthetic"),
        b"named bytes",
    )
    .unwrap();
    assert_eq!(
        streams(&file.file).unwrap(),
        vec![(":synthetic:$DATA".to_string(), 11)]
    );
    let name: Vec<_> = ":synthetic:$DATA".encode_utf16().collect();
    match relative(
        &file.file,
        &name,
        GENERIC_READ,
        SHARE,
        false,
        FILE_NON_DIRECTORY_FILE,
    ) {
        Ok(mut stream) => {
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"named bytes");
            println!("H1 relative named stream open: supported");
        }
        Err(error) => {
            println!("H1 relative named stream open: {error:?}; named-stream sources must refuse");
            assert!(matches!(error.raw_os_error(), Some(3 | 87 | 123 | 267)));
        }
    }
}

#[test]
fn h2_child_handle_without_delete_share_blocks_publish() {
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
    };
    let (temp, destination) = lab();
    let mut stage = destination.stage("staging").unwrap();
    drop(stage.create_file(&[], "payload").unwrap());
    let mut blocker = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "effects::tests::held_child_process",
            "--nocapture",
        ])
        .env(
            "LOOMWARD_C2_HELD_FIXTURE",
            temp.path().join("staging/payload"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(blocker.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        assert!(output.read_line(&mut line).unwrap() > 0);
        if line.contains("fixture-held") {
            break;
        }
        line.clear();
    }
    let error = stage.publish("published").unwrap_err();
    println!("H2 child without delete share: {error:?}");
    assert!(matches!(error.raw_os_error(), Some(5 | 32)));
    drop(blocker.stdin.take());
    assert!(blocker.wait().unwrap().success());
    stage.publish("published").unwrap();
}

#[test]
fn held_child_process() {
    use std::os::windows::fs::OpenOptionsExt;
    let Some(path) = std::env::var_os("LOOMWARD_C2_HELD_FIXTURE") else {
        return;
    };
    let _file = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .unwrap();
    println!("fixture-held");
    std::io::stdout().flush().unwrap();
    let _ = std::io::stdin().read(&mut [0]).unwrap();
}

#[test]
fn h5_null_descriptor_inherits_all_aces() {
    use windows_sys::Win32::Security::*;
    let (_temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let file = stage.create_file(&[], "payload").unwrap();
    let mut needed = 0;
    unsafe {
        GetKernelObjectSecurity(
            file.file.as_raw_handle(),
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            0,
            &mut needed,
        );
    }
    assert!(needed > 0);
    let mut descriptor = vec![0u64; (needed as usize).div_ceil(8)];
    assert_ne!(
        unsafe {
            GetKernelObjectSecurity(
                file.file.as_raw_handle(),
                DACL_SECURITY_INFORMATION,
                descriptor.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        },
        0
    );
    let mut present = 0;
    let mut defaulted = 0;
    let mut acl = ptr::null_mut();
    assert_ne!(
        unsafe {
            GetSecurityDescriptorDacl(
                descriptor.as_mut_ptr().cast(),
                &mut present,
                &mut acl,
                &mut defaulted,
            )
        },
        0
    );
    assert_ne!(present, 0);
    assert!(!acl.is_null());
    let count = unsafe { (*acl).AceCount };
    assert!(count > 0);
    for i in 0..count {
        let mut ace = ptr::null_mut();
        assert_ne!(unsafe { GetAce(acl, i as u32, &mut ace) }, 0);
        assert_ne!(
            unsafe { (*(ace.cast::<ACE_HEADER>())).AceFlags } as u32 & INHERITED_ACE,
            0
        );
    }
    println!("H5 child DACL: {count} inherited ACEs, zero explicit ACEs");
}

#[test]
fn nested_parents_are_identity_bound() {
    let (temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let dir = stage.create_directory(&[], "nested").unwrap();
    drop(
        stage
            .create_file(&[("nested", dir.clone())], "payload")
            .unwrap(),
    );
    fs::rename(
        temp.path().join("staging/nested"),
        temp.path().join("staging/old"),
    )
    .unwrap();
    fs::create_dir(temp.path().join("staging/nested")).unwrap();
    assert!(stage.create_file(&[("nested", dir)], "escape").is_err());
    assert!(!temp.path().join("staging/nested/escape").exists());
}

fn source_fixture(
    temp: &tempfile::TempDir,
) -> (enumerate::OpenedIdentity, duplicate_reader::Entry) {
    let root = identity::observe(temp.path()).unwrap();
    let expected = enumerate::OpenedIdentity {
        id: Some(enumerate::FileIdObs::Id128(root.object.file_id)),
        basis: enumerate::IdBasis::PostOpen,
        volume_serial: Some(root.object.volume_serial.parse().unwrap()),
        attributes: root.attributes,
        reparse_tag: Some(0),
    };
    let entry = duplicate_reader::Entry {
        relative: "source".into(),
        expected: identity::observe(&temp.path().join("source")).unwrap(),
    };
    (expected, entry)
}
fn budget() -> duplicate_reader::Budget {
    duplicate_reader::Budget {
        bytes: 1024 * 1024,
        time: std::time::Duration::from_secs(10),
    }
}

#[test]
fn source_pins_signatures_denies_writers_and_never_changes_source() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("source"), b"untouched").unwrap();
    let (root, entry) = source_fixture(&temp);
    let before = entry.expected.clone();
    let writer = fs::OpenOptions::new()
        .write(true)
        .open(temp.path().join("source"))
        .unwrap();
    assert!(matches!(
        Source::open(temp.path(), &root, entry.clone(), budget()),
        Err(duplicate_reader::Reason::SharingViolation)
    ));
    drop(writer);
    let mut source = Source::open(temp.path(), &root, entry, budget()).unwrap();
    assert!(fs::OpenOptions::new()
        .write(true)
        .open(temp.path().join("source"))
        .is_err());
    let mut bytes = Vec::new();
    source.read_to_end(&mut bytes).unwrap();
    source.finish().unwrap();
    assert_eq!(bytes, b"untouched");
    assert_eq!(
        identity::observe(&temp.path().join("source")).unwrap(),
        before
    );
    fs::rename(temp.path().join("source"), temp.path().join("renamed")).unwrap();
    assert!(source.finish().is_err());
}

#[test]
fn named_stream_source_keeps_bytes_and_deny_write_holds() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("source"), b"data").unwrap();
    fs::write(temp.path().join("source:synthetic"), b"must survive").unwrap();
    let (root, entry) = source_fixture(&temp);
    let mut source = Source::open(temp.path(), &root, entry, budget()).unwrap();
    assert_eq!(source.streams(), vec![(":synthetic:$DATA".to_string(), 12)]);
    let mut bytes = Vec::new();
    source
        .stream_reader(":synthetic:$DATA")
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"must survive");
    assert!(fs::OpenOptions::new()
        .write(true)
        .open(temp.path().join("source:synthetic"))
        .is_err());
    source.finish().unwrap();
}

#[test]
fn by_id_flag_measurement() {
    let (_temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let file = stage.create_file(&[], "payload").unwrap();
    let recorded = file.identity().unwrap();
    drop(file);
    let descriptor = FILE_ID_DESCRIPTOR {
        dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32,
        Type: FileIdType,
        Anonymous: FILE_ID_DESCRIPTOR_0 {
            FileId: i64::from_le_bytes(recorded.file_id[..8].try_into().unwrap()),
        },
    };
    for flags in [
        0,
        FILE_FLAG_OPEN_NO_RECALL,
        FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_FLAG_NO_BUFFERING,
        FILE_FLAG_OPEN_NO_RECALL | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_NO_BUFFERING,
    ] {
        let handle = unsafe {
            OpenFileById(
                destination.parent().as_raw_handle(),
                &descriptor,
                GENERIC_READ,
                HOLD,
                ptr::null(),
                flags,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            println!(
                "OpenFileById flags {flags:x}: {:?}",
                io::Error::last_os_error()
            );
        } else {
            println!("OpenFileById flags {flags:x}: supported");
            drop(unsafe { File::from_raw_handle(handle) });
        }
    }
    let path: Vec<_> = r"\\.\C:".encode_utf16().chain(Some(0)).collect();
    let volume = unsafe {
        CreateFileW(
            path.as_ptr(),
            0,
            SHARE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    assert_ne!(volume, INVALID_HANDLE_VALUE);
    let volume = unsafe { File::from_raw_handle(volume) };
    let handle = unsafe {
        OpenFileById(
            volume.as_raw_handle(),
            &descriptor,
            GENERIC_READ,
            HOLD,
            ptr::null(),
            FILE_FLAG_OPEN_NO_RECALL | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_NO_BUFFERING,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        println!("OpenFileById volume hint: {:?}", io::Error::last_os_error());
    } else {
        println!("OpenFileById volume hint: supported");
        drop(unsafe { File::from_raw_handle(handle) });
    }
}

#[test]
fn named_stream_create_new_write_and_noncached_verify() {
    let (_temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let file = stage.create_file(&[], "payload").unwrap();
    let recorded = file.identity().unwrap();
    let stream = file.create_stream("synthetic").unwrap();
    assert!(file.create_stream("synthetic").is_err());
    let digest = stream
        .write_and_flush(&mut &b"stream bytes"[..], 12)
        .unwrap();
    drop(stream);
    drop(file);
    let held = stage.verify(&[], "payload", &recorded).unwrap();
    let stream_hold = held.verify_stream("synthetic").unwrap();
    assert_eq!(stream_hold.digest, digest);
    assert_eq!(stream_hold.size, 12);
}

#[test]
fn bottom_up_directory_metadata_and_identity_bound_cleanup() {
    let (temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let original = stage.create_directory(&[], "nested").unwrap();
    let file = stage
        .create_file(&[("nested", original.clone())], "payload")
        .unwrap();
    let record = file.identity().unwrap();
    drop(file);
    assert!(stage.cleanup_directory(&[], "nested", &original).is_err());
    stage
        .cleanup_file(&[("nested", original.clone())], "payload", &record)
        .unwrap();
    let metadata = BasicMetadata {
        creation_time: 133_000_000_000_000_000,
        last_write_time: 133_000_123_456_789_000,
        attributes: FILE_ATTRIBUTE_HIDDEN,
    };
    let updated = stage
        .set_directory_metadata(&[], "nested", &original, metadata)
        .unwrap();
    let observed = identity::observe(&temp.path().join("staging/nested")).unwrap();
    assert_eq!(observed.object, updated);
    assert_eq!(
        observed.last_write_time,
        metadata.last_write_time.to_string()
    );
    stage.cleanup_directory(&[], "nested", &updated).unwrap();
    stage.cleanup().unwrap();
    assert!(!temp.path().join("staging").exists());
}

#[test]
fn cleanup_rejects_forged_creation_time_and_preserves_unrecorded_entries() {
    let (temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let file = stage.create_file(&[], "payload").unwrap();
    let mut recorded = file.identity().unwrap();
    drop(file);
    recorded.creation_time = "1".to_string();
    assert!(stage.cleanup_file(&[], "payload", &recorded).is_err());
    fs::write(temp.path().join("staging/unrecorded"), b"keep").unwrap();
    assert!(stage.cleanup().is_err());
    assert_eq!(
        fs::read(temp.path().join("staging/unrecorded")).unwrap(),
        b"keep"
    );
    assert!(temp.path().join("staging/payload").exists());
}

#[test]
fn partial_write_is_kept_and_cannot_be_appended_as_a_new_copy() {
    let (temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let file = stage.create_file(&[], "payload").unwrap();
    let bytes = vec![0x5a; BLOCK];
    assert!(file
        .write_and_flush(&mut bytes.as_slice(), (BLOCK + 1) as u64)
        .is_err());
    assert!(file.write_and_flush(&mut &b"other"[..], 5).is_err());
    assert_eq!(
        fs::read(temp.path().join("staging/payload")).unwrap(),
        bytes
    );
}

#[test]
fn elevation_and_reparse_parents_refuse() {
    let temp = tempfile::tempdir().unwrap();
    let expected = identity::observe(temp.path()).unwrap().object;
    {
        let _reset = enumerate::force_elevation_for_test();
        assert!(Destination::open(temp.path(), &expected).is_err());
    }
    let destination = Destination::open(temp.path(), &expected).unwrap();
    let stage = destination.stage("staging").unwrap();
    fs::create_dir(temp.path().join("outside")).unwrap();
    crate::fixtures::junction(
        &temp.path().join("staging/link"),
        &temp.path().join("outside"),
    )
    .unwrap();
    let link = identity::observe(&temp.path().join("staging/link"))
        .unwrap()
        .object;
    assert!(stage.create_file(&[("link", link)], "escape").is_err());
    assert!(!temp.path().join("outside/escape").exists());
}

#[test]
fn unsupported_source_attribute_refuses_before_copy() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("source"), b"data").unwrap();
    let file = fs::OpenOptions::new()
        .write(true)
        .open(temp.path().join("source"))
        .unwrap();
    set(
        &file,
        FileBasicInfo,
        &FILE_BASIC_INFO {
            FileAttributes: FILE_ATTRIBUTE_SYSTEM,
            ..Default::default()
        },
    )
    .unwrap();
    drop(file);
    let (root, entry) = source_fixture(&temp);
    assert!(matches!(
        Source::open(temp.path(), &root, entry, budget()),
        Err(duplicate_reader::Reason::Unsupported)
    ));
}

#[test]
fn staging_root_metadata_remains_bound_through_publish() {
    let (temp, destination) = lab();
    let mut stage = destination.stage("staging").unwrap();
    let metadata = BasicMetadata {
        creation_time: 133_000_000_000_000_000,
        last_write_time: 133_000_123_456_789_000,
        attributes: FILE_ATTRIBUTE_HIDDEN,
    };
    stage.set_metadata(metadata).unwrap();
    stage.publish("published").unwrap();
    let current = identity::observe(&temp.path().join("published")).unwrap();
    assert_eq!(current.object, *stage.identity());
    assert_eq!(
        current.last_write_time,
        metadata.last_write_time.to_string()
    );
    assert!(stage.cleanup().is_err());
}

#[test]
fn too_many_streams_refuse() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("source"), b"data").unwrap();
    for i in 0..65 {
        fs::write(temp.path().join(format!("source:s{i}")), b"stream").unwrap();
    }
    let (root, entry) = source_fixture(&temp);
    assert!(matches!(
        Source::open(temp.path(), &root, entry, budget()),
        Err(duplicate_reader::Reason::Unsupported)
    ));
}

#[test]
fn existing_named_stream_writer_is_source_in_use() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("source"), b"data").unwrap();
    fs::write(temp.path().join("source:synthetic"), b"stream").unwrap();
    let writer = fs::OpenOptions::new()
        .write(true)
        .open(temp.path().join("source:synthetic"))
        .unwrap();
    let (root, entry) = source_fixture(&temp);
    assert!(matches!(
        Source::open(temp.path(), &root, entry, budget()),
        Err(duplicate_reader::Reason::SharingViolation)
    ));
    drop(writer);
}

#[test]
fn physical_sector_geometry_requires_known_compatible_powers_of_two() {
    assert_eq!(sector_alignment(512, 4096).unwrap(), 4096);
    assert_eq!(sector_alignment(512, 512).unwrap(), 512);
    for (logical, physical) in [
        (0, 512),
        (512, 0),
        (4096, 512),
        (512, 768),
        (512, 2_097_152),
    ] {
        assert!(sector_alignment(logical, physical).is_err());
    }
}
