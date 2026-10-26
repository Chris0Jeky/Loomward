use super::*;
use std::{fs, io::Write};

/// Hosted runners are elevated, and every primitive refuses an elevated token (invariant 2): prove the
/// refusal, then skip the rest of the test with the reason.
fn skip_elevated() -> bool {
    if !crate::enumerate::skip_elevated_test() {
        return false;
    }
    let temp = tempfile::tempdir().unwrap();
    assert!(
        crate::identity::observe(temp.path()).is_err(),
        "an elevated token must be refused before any identity is observed"
    );
    eprintln!("skipped: elevated token; copy primitives refuse elevation (invariant 2)");
    true
}

fn lab() -> (tempfile::TempDir, Destination) {
    let temp = tempfile::tempdir().unwrap();
    let expected = crate::identity::observe(temp.path()).unwrap().object;
    let destination = Destination::open(temp.path(), &expected).unwrap();
    (temp, destination)
}

/// Host measurement (#687), not a product invariant: run with `--nocapture`.
#[test]
fn h2_relative_rename_host_probe() {
    if skip_elevated() {
        return;
    }
    use windows_sys::Wdk::Storage::FileSystem::{
        FileRenameInformation, NtSetInformationFile, FILE_RENAME_INFORMATION,
    };
    println!(
        "H2 layout: Win32 size={} root={} length={} name={}; native size={} root={} length={} name={}",
        size_of::<FILE_RENAME_INFO>(),
        offset_of!(FILE_RENAME_INFO, RootDirectory),
        offset_of!(FILE_RENAME_INFO, FileNameLength),
        offset_of!(FILE_RENAME_INFO, FileName),
        size_of::<FILE_RENAME_INFORMATION>(),
        offset_of!(FILE_RENAME_INFORMATION, RootDirectory),
        offset_of!(FILE_RENAME_INFORMATION, FileNameLength),
        offset_of!(FILE_RENAME_INFORMATION, FileName),
    );
    for api in [
        "Win32 ReplaceIfExists=false",
        "Win32 Flags=0",
        "Win32 Ex Flags=0",
        "native",
    ] {
        for hold in ["none", "named", "default"] {
            let (_temp, destination) = lab();
            let stage = destination.stage("staging").unwrap();
            let file = stage.create_file(&[], "payload").unwrap();
            file.write_and_flush(&mut &b"data"[..], 4).unwrap();
            let stream = file.create_stream("note").unwrap();
            stream.write_and_flush(&mut &b"n"[..], 1).unwrap();
            let id = file.identity().unwrap();
            drop((stream, file));
            let default_hold = (hold != "none").then(|| stage.verify(&[], "payload", &id).unwrap());
            let named_hold = (hold == "named").then(|| {
                default_hold
                    .as_ref()
                    .unwrap()
                    .verify_stream("note")
                    .unwrap()
            });
            // Isolate a named-stream hold from a default-stream hold.
            let default_hold = if hold == "named" {
                drop(default_hold);
                None
            } else {
                default_hold
            };
            let file = relative(
                destination.parent(),
                &stage.name,
                DELETE | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                false,
                0,
            )
            .unwrap();
            let name = component("published").unwrap();
            let bytes = size_of::<FILE_RENAME_INFO>()
                .max(offset_of!(FILE_RENAME_INFO, FileName) + (name.len() + 1) * 2);
            let mut storage = vec![0u64; bytes.div_ceil(8)];
            let rename = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
            unsafe {
                if api == "Win32 ReplaceIfExists=false" {
                    (*rename).Anonymous.ReplaceIfExists = false;
                } else {
                    (*rename).Anonymous.Flags = 0;
                }
                (*rename).RootDirectory = destination.parent().as_raw_handle();
                (*rename).FileNameLength = (name.len() * 2) as u32;
                ptr::copy_nonoverlapping(
                    name.as_ptr(),
                    ptr::addr_of_mut!((*rename).FileName).cast::<u16>(),
                    name.len(),
                );
            }
            let result = if api == "native" {
                let mut status = IO_STATUS_BLOCK::default();
                let code = unsafe {
                    NtSetInformationFile(
                        file.as_raw_handle(),
                        &mut status,
                        rename.cast(),
                        bytes as u32,
                        FileRenameInformation,
                    )
                };
                println!("H2 native hold={hold}: NTSTATUS=0x{:08x}", code as u32);
                if code < 0 {
                    Err(nt_error(code))
                } else {
                    Ok(())
                }
            } else if unsafe {
                SetFileInformationByHandle(
                    file.as_raw_handle(),
                    if api == "Win32 Ex Flags=0" {
                        FileRenameInfoEx
                    } else {
                        FileRenameInfo
                    },
                    rename.cast(),
                    bytes as u32,
                )
            } == 0
            {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            };
            println!("H2 {api} hold={hold}: {result:?}");
            drop((file, named_hold, default_hold, stage));
        }
    }
}

#[test]
fn copy_staged_writes_and_metadata_keep_file_and_ancestry_pinned() {
    if skip_elevated() {
        return;
    }
    let (temp, destination) = lab();
    let mut stage = destination.stage("staging").unwrap();
    let child = stage.create_directory(&[], "child").unwrap();
    let file = stage.create_file(&[("child", child)], "payload").unwrap();
    let mut reader = std::io::Cursor::new(b"payload").chain(std::io::empty());
    struct Attempt<'a, R>(&'a Path, R);
    impl<R: Read> Read for Attempt<'_, R> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            assert_rename_blocked(&self.0.join("staging/child/payload"));
            assert_rename_blocked(&self.0.join("staging/child"));
            assert_rename_blocked(&self.0.join("staging"));
            self.1.read(bytes)
        }
    }
    file.write_and_flush(&mut Attempt(temp.path(), &mut reader), 7)
        .unwrap();
    assert_rename_blocked(&temp.path().join("staging/child/payload"));
    assert_rename_blocked(&temp.path().join("staging/child"));
    let metadata = BasicMetadata::read(&file.file).unwrap();
    file.set_metadata(metadata).unwrap();
    drop(file);
    stage.publish("published").unwrap();
    assert_eq!(
        fs::read(temp.path().join("published/child/payload")).unwrap(),
        b"payload"
    );
}

#[test]
fn staging_entry_limit_is_inclusive() {
    assert_eq!(loomward_operations::MAX_ITEMS, 200_000);
    if skip_elevated() {
        return;
    }
    let (_temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    for n in 0..9 {
        drop(stage.create_file(&[], &format!("file-{n}")).unwrap());
    }
    assert_eq!(stage.entries_with_limit(&[], 9).unwrap().len(), 9);
    assert_eq!(
        stage.entries_with_limit(&[], 8).unwrap_err().to_string(),
        "incomplete staging listing"
    );
}

#[test]
fn journaled_staging_lists_entries_before_and_after_publish() {
    if skip_elevated() {
        return;
    }
    let (temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let recorded = stage.identity().clone();
    let creation = recorded.creation_time.parse().unwrap();
    let child = stage.create_directory(&[], "child").unwrap();
    let empty = stage.create_directory(&[], "empty").unwrap();
    // More than one native listing buffer, including maximum-length components.
    for i in 0..160 {
        let name = format!("{i:03}{}", "x".repeat(252));
        drop(
            stage
                .create_file(&[("child", child.clone())], &name)
                .unwrap(),
        );
    }
    drop(stage);
    let mut stage =
        Staging::attach(&destination, "staging", recorded.file_id, &[creation]).unwrap();
    let mut names = stage.entries(&[]).unwrap();
    names.sort();
    assert_eq!(names, ["child", "empty"]);
    assert_eq!(
        stage.entries(&[("child", child.clone())]).unwrap().len(),
        160
    );
    assert!(stage.entries(&[("empty", empty)]).unwrap().is_empty());
    stage.publish("published").unwrap();
    drop(stage);
    let published =
        Staging::attach_published(&destination, "published", recorded.file_id, &[creation])
            .unwrap();
    assert_eq!(
        published
            .entries(&[("child", child.clone())])
            .unwrap()
            .len(),
        160
    );
    fs::write(temp.path().join("published/extra"), b"extra").unwrap();
    assert!(published
        .entries(&[])
        .unwrap()
        .contains(&"extra".to_string()));
    // A replacement child or root must never be listed under the journaled identity.
    fs::rename(
        temp.path().join("published/child"),
        temp.path().join("published/moved"),
    )
    .unwrap();
    fs::create_dir(temp.path().join("published/child")).unwrap();
    assert!(published.entries(&[("child", child)]).is_err());
    fs::rename(temp.path().join("published"), temp.path().join("moved")).unwrap();
    fs::create_dir(temp.path().join("published")).unwrap();
    assert!(published.entries(&[]).is_err());
    assert!(
        Staging::attach_published(&destination, "published", recorded.file_id, &[creation])
            .is_err()
    );
}

#[test]
fn journaled_published_attachment_rehashes_and_refuses_effects() {
    if skip_elevated() {
        return;
    }
    let (temp, destination) = lab();
    let mut stage = destination.stage("staging").unwrap();
    let root = stage.identity().clone();
    let creation = root.creation_time.parse().unwrap();
    let file = stage.create_file(&[], "payload").unwrap();
    let recorded = file.identity().unwrap();
    file.write_and_flush(&mut &b"before"[..], 6).unwrap();
    let stream = file.create_stream("named").unwrap();
    stream.write_and_flush(&mut &b"stream"[..], 6).unwrap();
    drop(stream);
    drop(file);
    let child = stage.create_directory(&[], "child").unwrap();
    stage.publish("published").unwrap();
    drop(stage);
    // No holds survive a crash: confirmation must read the current bytes again.
    fs::write(temp.path().join("published/payload"), b"after!").unwrap();
    let mut published =
        Staging::attach_published(&destination, "published", root.file_id, &[creation]).unwrap();
    let held = published.verify(&[], "payload", &recorded).unwrap();
    assert_eq!(held.digest, <[u8; 32]>::from(Sha256::digest(b"after!")));
    assert_eq!(held.size, 6);
    assert_eq!(
        held.streams().unwrap(),
        vec![(":named:$DATA".to_string(), 6)]
    );
    assert_eq!(held.ea_size().unwrap(), 0);
    assert_eq!(
        held.verify_stream("named").unwrap().digest,
        <[u8; 32]>::from(Sha256::digest(b"stream"))
    );
    assert!(published.create_file(&[], "new").is_err());
    assert!(published.create_directory(&[], "new").is_err());
    assert!(published
        .set_metadata(published.own_metadata().unwrap())
        .is_err());
    assert!(published
        .set_directory_metadata(
            &[],
            "child",
            &child,
            published.metadata(&[], "child", &child).unwrap()
        )
        .is_err());
    assert!(published.cleanup_file(&[], "payload", &recorded).is_err());
    assert!(published.cleanup_directory(&[], "child", &child).is_err());
    assert!(published.publish("again").is_err());
    assert!(published.cleanup().is_err());
    assert_eq!(
        fs::read(temp.path().join("published/payload")).unwrap(),
        b"after!"
    );
}

#[test]
fn staging_guard_rename_is_blocked() {
    if skip_elevated() {
        return;
    }
    let (temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let guard = stage.guard().unwrap();
    assert_rename_blocked(&temp.path().join("staging"));
    drop(guard);
    fs::rename(temp.path().join("staging"), temp.path().join("renamed")).unwrap();
}

#[test]
fn destination_pin_rename_is_blocked() {
    if skip_elevated() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("parent");
    fs::create_dir(&parent).unwrap();
    let expected = identity::observe(&parent).unwrap().object;
    let destination = Destination::open(&parent, &expected).unwrap();
    assert_rename_blocked(&parent);
    drop(destination);
    fs::rename(&parent, temp.path().join("renamed")).unwrap();
}

#[test]
fn nested_pin_rename_is_blocked() {
    if skip_elevated() {
        return;
    }
    let (temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let child = stage.create_directory(&[], "child").unwrap();
    let pins = stage.parents(&[("child", child)]).unwrap();
    assert_rename_blocked(&temp.path().join("staging/child"));
    drop(pins);
    fs::rename(
        temp.path().join("staging/child"),
        temp.path().join("renamed"),
    )
    .unwrap();
}

fn assert_rename_blocked(from: &Path) {
    let from = from.to_owned();
    let to = from.with_file_name("renamed");
    let result = std::thread::spawn(move || fs::rename(from, to))
        .join()
        .unwrap();
    assert_eq!(result.unwrap_err().raw_os_error(), Some(32));
}

#[test]
fn unsupported_staged_attributes_have_a_typed_refusal() {
    if skip_elevated() {
        return;
    }
    for attribute in [
        FILE_ATTRIBUTE_COMPRESSED,
        FILE_ATTRIBUTE_ENCRYPTED,
        FILE_ATTRIBUTE_INTEGRITY_STREAM,
        FILE_ATTRIBUTE_SPARSE_FILE,
        FILE_ATTRIBUTE_SYSTEM,
        FILE_ATTRIBUTE_TEMPORARY,
    ] {
        let error = supported_attributes(attribute | FILE_ATTRIBUTE_ARCHIVE).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(error.to_string(), "unsupported_attribute");
        assert_eq!(
            error
                .get_ref()
                .unwrap()
                .downcast_ref::<UnsupportedAttribute>()
                .unwrap()
                .attributes,
            attribute
        );
    }
    supported_attributes(PRESERVED | FILE_ATTRIBUTE_DIRECTORY).unwrap();
    supported_attributes(FILE_ATTRIBUTE_NORMAL).unwrap();
}

#[test]
fn compressed_destination_is_refused_before_staging() {
    if skip_elevated() {
        return;
    }
    let (temp, destination) = lab();
    compress_path(temp.path());
    let error = destination
        .stage("staging")
        .err()
        .expect("compressed parent accepted");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(error.to_string(), "unsupported_attribute");
    assert!(!temp.path().join("staging").exists());
}

#[test]
fn compressed_staged_directory_is_refused() {
    if skip_elevated() {
        return;
    }
    let (temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    compress_path(&temp.path().join("staging"));
    let error = stage
        .create_file(&[], "payload")
        .err()
        .expect("compressed staging accepted");
    assert_eq!(error.to_string(), "unsupported_attribute");
    assert!(!temp.path().join("staging/payload").exists());
}

#[test]
fn compressed_staged_file_is_refused_by_verification() {
    if skip_elevated() {
        return;
    }
    let (temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let file = stage.create_file(&[], "payload").unwrap();
    let record = file.identity().unwrap();
    file.write_and_flush(&mut &b"data"[..], 4).unwrap();
    drop(file);
    compress_path(&temp.path().join("staging/payload"));
    let error = stage
        .verify(&[], "payload", &record)
        .err()
        .expect("compressed file verified");
    assert_eq!(error.to_string(), "unsupported_attribute");
    assert_eq!(
        fs::read(temp.path().join("staging/payload")).unwrap(),
        b"data"
    );
}

#[test]
fn compressed_metadata_is_refused_before_masking() {
    if skip_elevated() {
        return;
    }
    let (temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let file = stage.create_file(&[], "payload").unwrap();
    compress_path(&temp.path().join("staging/payload"));
    let error = BasicMetadata::read(&file.file).unwrap_err();
    assert_eq!(error.to_string(), "unsupported_attribute");
}

fn compress_path(path: &Path) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::System::{Ioctl::FSCTL_SET_COMPRESSION, IO::DeviceIoControl};
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(SHARE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .unwrap();
    let format: u16 = 1; // COMPRESSION_FORMAT_DEFAULT
    let mut returned = 0;
    assert_ne!(
        unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                FSCTL_SET_COMPRESSION,
                ptr::from_ref(&format).cast(),
                size_of::<u16>() as u32,
                ptr::null_mut(),
                0,
                &mut returned,
                ptr::null_mut(),
            )
        },
        0,
        "FSCTL_SET_COMPRESSION: {}",
        io::Error::last_os_error()
    );
    assert_ne!(
        identity::from_file(&file).unwrap().attributes & FILE_ATTRIBUTE_COMPRESSED,
        0
    );
}

#[test]
fn create_new_and_publish_never_replace() {
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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

/// H7 (docs/47 section 13, review of #522): another process maps a staged file writable, closes its
/// file and section handles, and keeps only the view. Does the executor's deny-write verification hold
/// still fail while that view exists? A mapping is the one writer a closed handle leaves behind.
#[test]
fn h7_writable_mapped_view_blocks_the_deny_write_hold() {
    if skip_elevated() {
        return;
    }
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
    };
    let (temp, destination) = lab();
    let stage = destination.stage("staging").unwrap();
    let file = stage.create_file(&[], "payload").unwrap();
    file.write_and_flush(&mut &[7u8; 8192][..], 8192).unwrap();
    let id = file.identity().unwrap();
    drop(file);
    let mut mapper = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "effects::tests::mapped_child_process",
            "--nocapture",
        ])
        .env(
            "LOOMWARD_C2_MAPPED_FIXTURE",
            temp.path().join("staging/payload"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(mapper.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        assert!(output.read_line(&mut line).unwrap() > 0, "the child died");
        if line.contains("fixture-mapped") {
            break;
        }
        line.clear();
    }
    let held = stage.verify(&[], "payload", &id);
    println!("H7 hold under a writable view: {:?}", held.as_ref().err());
    assert_eq!(
        held.err().and_then(|e| e.raw_os_error()),
        Some(32),
        "H7: a deny-write hold must not coexist with a writable mapped view"
    );
    drop(mapper.stdin.take());
    assert!(mapper.wait().unwrap().success());
    // The view is gone with the child: the hold is granted again.
    stage.verify(&[], "payload", &id).unwrap();
}

#[test]
fn mapped_child_process() {
    if skip_elevated() {
        return;
    }
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::System::Memory::*;
    let Some(path) = std::env::var_os("LOOMWARD_C2_MAPPED_FIXTURE") else {
        return;
    };
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(path)
        .unwrap();
    let section = unsafe {
        CreateFileMappingW(
            file.as_raw_handle(),
            ptr::null(),
            PAGE_READWRITE,
            0,
            0,
            ptr::null(),
        )
    };
    assert!(!section.is_null());
    let view = unsafe { MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, 0) };
    assert!(!view.Value.is_null());
    // Only the view is left: no file handle and no section handle.
    unsafe { CloseHandle(section) };
    drop(file);
    println!("fixture-mapped");
    std::io::stdout().flush().unwrap();
    let _ = std::io::stdin().read(&mut [0]).unwrap();
    unsafe { UnmapViewOfFile(view) };
}

#[test]
fn h5_null_descriptor_inherits_all_aces() {
    if skip_elevated() {
        return;
    }
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
fn protected_empty_destination_dacl_is_not_inherited() {
    if skip_elevated() {
        return;
    }
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("payload.bin");
    fs::write(&path, b"payload").unwrap();
    let file = fs::OpenOptions::new()
        .access_mode(READ_CONTROL | WRITE_DAC)
        .open(&path)
        .unwrap();
    inherited_acl(&file).unwrap();
    assert!(std::process::Command::new("icacls")
        .arg(&path)
        .arg("/inheritance:r")
        .output()
        .unwrap()
        .status
        .success());
    let (explicit, _, protected) = duplicate_reader::security_metadata(&file).unwrap();
    assert!(
        explicit.is_empty() && protected,
        "fixture must have an empty protected DACL"
    );
    // Restore inherited access before asserting, so even the red run leaves a removable fixture.
    let result = inherited_acl(&file);
    assert!(std::process::Command::new("icacls")
        .arg(&path)
        .arg("/inheritance:e")
        .output()
        .unwrap()
        .status
        .success());
    assert!(
        result.is_err(),
        "a protected empty DACL grants no inherited permissions"
    );
    inherited_acl(&file).unwrap();
}

#[test]
fn nested_parents_are_identity_bound() {
    if skip_elevated() {
        return;
    }
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
    let (_, expected) = <enumerate::NativeSource as enumerate::DirSource>::open_root(
        &enumerate::NativeSource::default(),
        temp.path(),
    )
    .unwrap();
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let expected = identity::observe(temp.path()).unwrap().object;
    {
        let _reset = enumerate::force_elevation_for_test();
        assert!(Destination::open(temp.path(), &expected).is_err());
    }
    let destination = Destination::open(temp.path(), &expected).unwrap();
    let stage = destination.stage("staging").unwrap();
    {
        let _reset = enumerate::force_elevation_for_test();
        let key = stage.identity();
        let creation = key.creation_time.parse().unwrap();
        assert!(Staging::attach(&destination, "staging", key.file_id, &[creation]).is_err());
        assert!(
            Staging::attach_published(&destination, "staging", key.file_id, &[creation]).is_err()
        );
        assert!(stage.entries(&[]).is_err());
    }
    fs::create_dir(temp.path().join("outside")).unwrap();
    crate::fixtures::junction(
        &temp.path().join("staging/link"),
        &temp.path().join("outside"),
    )
    .unwrap();
    let link = identity::observe(&temp.path().join("staging/link"))
        .unwrap()
        .object;
    assert_eq!(stage.entries(&[]).unwrap(), ["link"]);
    assert!(stage.entries(&[("link", link.clone())]).is_err());
    assert!(stage.create_file(&[("link", link)], "escape").is_err());
    assert!(!temp.path().join("outside/escape").exists());
}

#[test]
fn unsupported_source_attribute_refuses_before_copy() {
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
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
    if skip_elevated() {
        return;
    }
    assert_eq!(crate::noncached::sector_alignment(512, 4096).unwrap(), 4096);
    assert_eq!(crate::noncached::sector_alignment(512, 512).unwrap(), 512);
    for (logical, physical) in [
        (0, 512),
        (512, 0),
        (4096, 512),
        (512, 768),
        (512, 2_097_152),
    ] {
        assert!(crate::noncached::sector_alignment(logical, physical).is_err());
    }
}

/// A `subst` alias for `target`, removed on drop. A subst letter names a folder, not a volume.
struct Subst(char);

impl Subst {
    fn new(target: &Path) -> Option<Self> {
        let letter = ('D'..='Z')
            .rev()
            .find(|c| !Path::new(&format!("{c}:\\")).exists())?;
        let status = std::process::Command::new("subst")
            .arg(format!("{letter}:"))
            .arg(target)
            .status()
            .ok()?;
        status.success().then_some(Self(letter))
    }
}

impl Drop for Subst {
    fn drop(&mut self) {
        let _ = std::process::Command::new("subst")
            .arg(format!("{}:", self.0))
            .arg("/D")
            .status();
    }
}

#[test]
fn destination_volume_hint_follows_the_observed_volume_not_the_drive_letter() {
    if skip_elevated() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let expected = identity::observe(temp.path()).unwrap().object;
    let subst = Subst::new(temp.path()).expect("a free drive letter for a subst alias");
    // The alias names the same folder on the same volume, through a drive letter that is not a volume.
    let alias = std::path::PathBuf::from(format!("{}:\\", subst.0));
    let destination = Destination::open(&alias, &expected)
        .expect("the alias opens its pinned parent through the observed volume");
    let _stage = destination.stage("staging").unwrap();
    assert!(temp.path().join("staging").is_dir());
}

/// Muse bug hunt (copy-exec): a verification read may legally return fewer bytes than asked. A short
/// read on a sector boundary is continued, not reported as the file changing; an early end, a short
/// read off a boundary (the end moved) or extra bytes (the file grew) still are.
#[test]
fn sector_hash_continues_short_reads_and_still_catches_a_changed_size() {
    struct Trickle<'a>(&'a [u8], usize);
    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = buf.len().min(self.1).min(self.0.len());
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }
    let data: Vec<u8> = (0..10_000u32).map(|i| (i * 7) as u8).collect();
    let mut buffer = vec![0u8; 4096];
    let want = <[u8; 32]>::from(Sha256::digest(&data));
    let size = data.len() as u64;
    for step in [512, 1024, 4096, usize::MAX] {
        assert_eq!(
            sector_hash(Trickle(&data, step), size, 512, &mut buffer).unwrap(),
            want,
            "step {step}"
        );
    }
    let changed = |reader: Trickle<'_>, size: u64, buffer: &mut [u8]| {
        let e = sector_hash(reader, size, 512, buffer).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof, "{e}");
    };
    changed(Trickle(&data[..9_000], usize::MAX), size, &mut buffer);
    changed(Trickle(&data, 100), size, &mut buffer);
    changed(Trickle(&data, usize::MAX), size - 1, &mut buffer);
}
