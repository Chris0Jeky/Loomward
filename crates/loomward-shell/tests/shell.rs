use loomward_shell::{known_folders::*, shortcuts::*};
use std::{cell::Cell, path::PathBuf};

fn put32(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_le_bytes());
}
fn header(flags: u32) -> Vec<u8> {
    let mut b = vec![0; 76];
    put32(&mut b, 0, 76);
    b[4..20].copy_from_slice(&[1, 20, 2, 0, 0, 0, 0, 0, 192, 0, 0, 0, 0, 0, 0, 70]);
    put32(&mut b, 20, flags);
    put32(&mut b, 24, 32);
    b
}
fn finish(mut b: Vec<u8>) -> Vec<u8> {
    b.extend([0; 4]);
    b
}
fn string(b: &mut Vec<u8>, s: &str) {
    let u: Vec<u16> = s.encode_utf16().collect();
    b.extend((u.len() as u16).to_le_bytes());
    for c in u {
        b.extend(c.to_le_bytes());
    }
}
fn local_info() -> Vec<u8> {
    let mut b = vec![0; 28];
    put32(&mut b, 4, 28);
    put32(&mut b, 8, 1);
    put32(&mut b, 12, 28);
    b.extend(18u32.to_le_bytes());
    b.extend(3u32.to_le_bytes());
    b.extend(123u32.to_le_bytes());
    b.extend(16u32.to_le_bytes());
    b.extend(b"V\0");
    let at = b.len();
    put32(&mut b, 16, at as u32);
    b.extend(b"Q:\\fixture\0");
    let at = b.len();
    put32(&mut b, 24, at as u32);
    b.extend(b"\\doc.txt\0");
    let size = b.len();
    put32(&mut b, 0, size as u32);
    b
}
fn local() -> Shortcut {
    let mut b = header(2);
    b.extend(local_info());
    parse(&finish(b)).unwrap()
}
struct Probe {
    state: VolumeState,
    file: MetadataState,
    calls: Cell<usize>,
}
impl TargetProbe for Probe {
    fn volume(&self, drive: char, serial: Option<u32>) -> VolumeState {
        assert_eq!(drive, 'Q');
        assert_eq!(serial, Some(123));
        self.calls.set(self.calls.get() + 1);
        self.state
    }
    fn metadata(&self, path: &str) -> MetadataState {
        assert_eq!(path, "Q:\\fixture\\doc.txt");
        self.calls.set(self.calls.get() + 1);
        self.file
    }
}
fn probe(state: VolumeState, file: MetadataState) -> Probe {
    Probe {
        state,
        file,
        calls: Cell::new(0),
    }
}

#[test]
fn header_flags_and_opaque_id_list() {
    let mut b = header(1);
    b.extend(6u16.to_le_bytes());
    b.extend([4, 0, 1, 2, 0, 0]);
    let s = parse(&finish(b.clone())).unwrap();
    assert_eq!(s.flags, 1);
    assert_eq!(s.file_attributes, 32);
    assert_eq!(s.id_list, Some(vec![4, 0, 1, 2, 0, 0]));
    b[4] = 9;
    assert!(parse(&finish(b)).is_err());
}
#[test]
fn string_fields_are_separate_unicode_and_inert() {
    let mut b = header(0xfc);
    for s in [
        "桌面",
        ".\\résumé.txt",
        "Q:\\作業",
        "--arg \"& calc.exe; $(payload)\"",
        "Q:\\icon.dll,-4",
    ] {
        string(&mut b, s);
    }
    let s = parse(&finish(b)).unwrap();
    assert_eq!(s.name.unwrap().as_str(), Some("桌面"));
    assert_eq!(s.relative_path.unwrap().as_str(), Some(".\\résumé.txt"));
    assert_eq!(s.working_directory.unwrap().as_str(), Some("Q:\\作業"));
    assert_eq!(
        s.arguments.unwrap().as_str(),
        Some("--arg \"& calc.exe; $(payload)\"")
    );
    assert_eq!(s.icon_location.unwrap().as_str(), Some("Q:\\icon.dll,-4"));
}
#[test]
fn ansi_bytes_are_preserved_without_guessing_code_page() {
    let mut b = header(4);
    b.extend(2u16.to_le_bytes());
    b.extend([0xe9, 0x20]);
    let s = parse(&finish(b)).unwrap();
    assert_eq!(s.name, Some(Text::Ansi(vec![0xe9, 0x20])));
    assert_eq!(s.name.unwrap().as_str(), None);
}
#[test]
fn malformed_and_oversized_inputs_are_refused() {
    let good = finish(header(0));
    for end in 0..good.len() {
        assert!(parse(&good[..end]).is_err(), "prefix {end}");
    }
    assert_eq!(
        parse(&vec![0; MAX_FILE_BYTES + 1]).unwrap_err().0,
        "file size exceeds cap"
    );
    let mut b = header(0x84);
    b.extend(u16::MAX.to_le_bytes());
    assert!(parse(&finish(b)).is_err());
    let mut b = header(2);
    let mut info = local_info();
    put32(&mut info, 16, u32::MAX);
    b.extend(info);
    assert!(parse(&finish(b)).is_err());
    let mut b = header(1);
    b.extend(4u16.to_le_bytes());
    b.extend([1, 0, 0, 0]);
    assert!(parse(&finish(b)).is_err());
    let mut b = header(0);
    b.extend(u32::MAX.to_le_bytes());
    assert!(parse(&finish(b)).is_err());
}
#[test]
fn local_link_info_and_health_keep_volume_absence_distinct() {
    let s = local();
    let i = s.link_info.as_ref().unwrap();
    assert_eq!(
        i.local_base_path.as_ref().unwrap().as_str(),
        Some("Q:\\fixture")
    );
    let v = i.volume.as_ref().unwrap();
    assert_eq!((v.drive_type, v.serial), (3, 123));
    assert_eq!(v.label.as_str(), Some("V"));
    for (vol, file, want, calls) in [
        (
            VolumeState::Unavailable,
            MetadataState::Missing,
            Health::VolumeUnavailable,
            1,
        ),
        (
            VolumeState::Available,
            MetadataState::Missing,
            Health::Missing,
            3,
        ),
        (
            VolumeState::Available,
            MetadataState::Present,
            Health::Present,
            2,
        ),
        (
            VolumeState::Unknown,
            MetadataState::Present,
            Health::Unknown,
            1,
        ),
        (
            VolumeState::Network,
            MetadataState::Present,
            Health::NetworkUnreachableUnknown,
            1,
        ),
    ] {
        let p = probe(vol, file);
        assert_eq!(health(&s, &p), want);
        assert_eq!(p.calls.get(), calls);
    }
}
#[test]
fn network_link_info_is_never_probed() {
    let mut info = vec![0; 28];
    put32(&mut info, 4, 28);
    put32(&mut info, 8, 2);
    put32(&mut info, 20, 28);
    let mut n = vec![0; 20];
    put32(&mut n, 4, 3);
    put32(&mut n, 8, 20);
    put32(&mut n, 16, 0x20000);
    n.extend(b"\\\\server.invalid\\share\0");
    let at = n.len();
    put32(&mut n, 12, at as u32);
    n.extend(b"Z:\0");
    let size = n.len();
    put32(&mut n, 0, size as u32);
    info.extend(n);
    let at = info.len();
    put32(&mut info, 24, at as u32);
    info.extend(b"\\doc.txt\0");
    let size = info.len();
    put32(&mut info, 0, size as u32);
    let mut b = header(2);
    b.extend(info);
    let s = parse(&finish(b)).unwrap();
    let n = s.link_info.as_ref().unwrap().network.as_ref().unwrap();
    assert_eq!(n.share.as_str(), Some("\\\\server.invalid\\share"));
    assert_eq!(n.device.as_ref().unwrap().as_str(), Some("Z:"));
    assert_eq!(n.provider_type, Some(0x20000));
    let p = probe(VolumeState::Available, MetadataState::Present);
    assert_eq!(health(&s, &p), Health::NetworkUnreachableUnknown);
    assert_eq!(p.calls.get(), 0);
}
#[test]
fn environment_known_folder_tracker_and_unknown_blocks_stay_data() {
    let mut b = header(0x200);
    let mut e = vec![0; 788];
    put32(&mut e, 0, 788);
    put32(&mut e, 4, 0xa0000001);
    e[8..18].copy_from_slice(b"%ROOT%\\x\0\0");
    for (i, c) in "%ROOT%\\文書".encode_utf16().enumerate() {
        e[268 + i * 2..270 + i * 2].copy_from_slice(&c.to_le_bytes());
    }
    b.extend(e);
    let mut k = vec![0; 28];
    put32(&mut k, 0, 28);
    put32(&mut k, 4, 0xa000000b);
    k[8..24].fill(7);
    b.extend(k);
    let mut t = vec![0; 96];
    put32(&mut t, 0, 96);
    put32(&mut t, 4, 0xa0000003);
    put32(&mut t, 8, 88);
    t[16..20].copy_from_slice(b"fake");
    t[32..64].fill(1);
    t[64..96].fill(2);
    b.extend(t);
    b.extend(8u32.to_le_bytes());
    b.extend(0xa00000ffu32.to_le_bytes());
    let s = parse(&finish(b)).unwrap();
    assert_eq!(
        s.environment_target.as_ref().unwrap().as_str(),
        Some("%ROOT%\\文書")
    );
    assert_eq!(s.known_folder.as_ref().unwrap().id, [7; 16]);
    let t = s.tracker.as_ref().unwrap();
    assert_eq!(t.machine_id, b"fake");
    assert_eq!(t.droid, [1; 32]);
    assert_eq!(t.birth_droid, [2; 32]);
    assert_eq!(s.unknown_extra_blocks, [0xa00000ff]);
    let p = probe(VolumeState::Available, MetadataState::Present);
    assert_eq!(health(&s, &p), Health::Ambiguous);
    assert_eq!(p.calls.get(), 0);
}
#[test]
fn relative_opaque_and_device_targets_are_not_probed() {
    let p = probe(VolumeState::Available, MetadataState::Present);
    let mut s = Shortcut {
        relative_path: Some(Text::Unicode(".\\x".into())),
        ..Shortcut::default()
    };
    assert_eq!(health(&s, &p), Health::Ambiguous);
    s.relative_path = None;
    assert_eq!(health(&s, &p), Health::Unknown);
    for path in [
        "\\\\?\\Q:\\fixture",
        "Q:\\fixture\\..\\x",
        "Q:\\CON",
        "Q:\\x:stream",
        "Q:\\x.",
        "Q:\\COM¹.txt",
        "Q:\\CON .txt",
        "Q:\\x\u{1}",
    ] {
        let mut s = local();
        s.link_info.as_mut().unwrap().local_base_path = Some(Text::Unicode(path.into()));
        assert_eq!(health(&s, &p), Health::Unknown);
    }
    assert_eq!(p.calls.get(), 0);
}

#[test]
fn unicode_link_info_paths_volume_and_network_offsets() {
    fn wide(b: &mut Vec<u8>, s: &str) {
        for c in s.encode_utf16().chain(Some(0)) {
            b.extend(c.to_le_bytes());
        }
    }
    let mut info = vec![0; 36];
    put32(&mut info, 4, 36);
    put32(&mut info, 8, 1);
    put32(&mut info, 12, 36);
    let mut v = vec![0; 20];
    put32(&mut v, 4, 3);
    put32(&mut v, 8, 123);
    put32(&mut v, 12, 20);
    put32(&mut v, 16, 20);
    wide(&mut v, "磁碟");
    let size = v.len();
    put32(&mut v, 0, size as u32);
    info.extend(v);
    let at = info.len();
    put32(&mut info, 16, at as u32);
    info.extend(b"Q:\\fallback\0");
    let at = info.len();
    put32(&mut info, 24, at as u32);
    info.extend(b"\0");
    let at = info.len();
    put32(&mut info, 28, at as u32);
    wide(&mut info, "Q:\\桌面");
    let at = info.len();
    put32(&mut info, 32, at as u32);
    wide(&mut info, "\\文書.txt");
    let size = info.len();
    put32(&mut info, 0, size as u32);
    let mut b = header(2);
    b.extend(info);
    let s = parse(&finish(b)).unwrap();
    let i = s.link_info.unwrap();
    assert_eq!(i.local_base_path.unwrap().as_str(), Some("Q:\\桌面"));
    assert_eq!(i.suffix.as_str(), Some("\\文書.txt"));
    assert_eq!(i.volume.unwrap().label.as_str(), Some("磁碟"));
    let mut n = vec![0; 28];
    put32(&mut n, 4, 1);
    put32(&mut n, 8, 28);
    n.extend(b"\\\\server.invalid\\fallback\0");
    let at = n.len();
    put32(&mut n, 12, at as u32);
    n.extend(b"Z:\0");
    let at = n.len();
    put32(&mut n, 20, at as u32);
    wide(&mut n, "\\\\server.invalid\\共享");
    let at = n.len();
    put32(&mut n, 24, at as u32);
    wide(&mut n, "Z:");
    let size = n.len();
    put32(&mut n, 0, size as u32);
    let mut info = vec![0; 28];
    put32(&mut info, 4, 28);
    put32(&mut info, 8, 2);
    put32(&mut info, 20, 28);
    info.extend(n);
    let at = info.len();
    put32(&mut info, 24, at as u32);
    info.extend(b"\0");
    let size = info.len();
    put32(&mut info, 0, size as u32);
    let mut b = header(2);
    b.extend(info);
    let s = parse(&finish(b)).unwrap();
    let n = s.link_info.unwrap().network.unwrap();
    assert_eq!(n.share.as_str(), Some("\\\\server.invalid\\共享"));
    assert_eq!(n.device.unwrap().as_str(), Some("Z:"));
}

#[test]
fn hostile_extra_blocks_utf16_and_counts_are_bounded() {
    let mut b = header(0);
    for _ in 0..65 {
        b.extend(8u32.to_le_bytes());
        b.extend(0xa00000ffu32.to_le_bytes());
    }
    assert_eq!(
        parse(&finish(b)).unwrap_err().0,
        "extra block count exceeds cap"
    );
    let mut b = header(1);
    b.extend(8194u16.to_le_bytes());
    for _ in 0..4096 {
        b.extend(2u16.to_le_bytes());
    }
    b.extend([0; 2]);
    assert_eq!(parse(&finish(b)).unwrap_err().0, "ItemID count exceeds cap");
    for (size, signature) in [(12, 0xa0000001u32), (12, 0xa000000b), (12, 0xa0000003)] {
        let mut b = header(0);
        b.extend((size as u32).to_le_bytes());
        b.extend(signature.to_le_bytes());
        b.extend([0; 4]);
        assert!(parse(&finish(b)).is_err());
    }
    let mut b = header(2);
    let mut i = local_info();
    i.fill(0xff);
    b.extend(i);
    assert!(parse(&finish(b)).is_err());
    let mut b = finish(header(0));
    b.push(9);
    assert_eq!(
        parse(&b).unwrap_err().0,
        "trailing data after terminal block"
    );
}

#[test]
fn unpaired_surrogates_preserve_raw_units() {
    for units in [vec![0xd800u16], vec![0xdc00, 65], vec![65, 0xd800, 66]] {
        let mut b = header(0x84);
        b.extend((units.len() as u16).to_le_bytes());
        for unit in &units {
            b.extend(unit.to_le_bytes());
        }
        let text = parse(&finish(b)).unwrap().name.unwrap();
        assert_eq!(text, Text::Wide(units));
        assert_eq!(text.as_str(), None);
    }
    let mut s = local();
    s.link_info.as_mut().unwrap().local_base_path = Some(Text::Wide(vec![0xd800]));
    let p = probe(VolumeState::Available, MetadataState::Present);
    assert_eq!(health(&s, &p), Health::Unknown);
    assert_eq!(p.calls.get(), 0);
}

#[test]
fn random_and_mutated_links_never_panic() {
    let mut state = 0x248_044_045u64;
    let mut random = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut local = header(2);
    local.extend(local_info());
    let mut text = header(0x84);
    string(&mut text, "synthetic name");
    let seeds = [finish(header(0)), finish(local), finish(text)];
    for index in 0..12_000 {
        let mut bytes = if index % 2 == 0 {
            seeds[index % seeds.len()].clone()
        } else {
            (0..random() % 2048).map(|_| random() as u8).collect()
        };
        for _ in 0..1 + random() % 8 {
            if !bytes.is_empty() {
                let at = random() as usize % bytes.len();
                bytes[at] = random() as u8;
            }
        }
        if index % 3 == 0 {
            let len = random() as usize % (bytes.len() + 1);
            bytes.truncate(len);
        }
        let _ = parse(&bytes);
    }
}

#[test]
fn health_rechecks_volume_and_refuses_resolver_flags() {
    struct Disconnect(Cell<u32>);
    impl TargetProbe for Disconnect {
        fn volume(&self, _: char, _: Option<u32>) -> VolumeState {
            let n = self.0.get();
            self.0.set(n + 1);
            if n == 0 {
                VolumeState::Available
            } else {
                VolumeState::Unavailable
            }
        }
        fn metadata(&self, _: &str) -> MetadataState {
            MetadataState::Missing
        }
    }
    let mut s = local();
    assert_eq!(
        health(&s, &Disconnect(Cell::new(0))),
        Health::VolumeUnavailable
    );
    let p = probe(VolumeState::Available, MetadataState::Present);
    for flags in [0x100, 0x1000] {
        s.flags = flags;
        assert_eq!(health(&s, &p), Health::Unknown);
    }
    s.flags = 0;
    s.link_info.as_mut().unwrap().local_base_path = Some(Text::Unicode("%ROOT%\\x".into()));
    assert_eq!(health(&s, &p), Health::Ambiguous);
    s.link_info.as_mut().unwrap().local_base_path = Some(Text::Unicode("relative".into()));
    assert_eq!(health(&s, &p), Health::Ambiguous);
    s.link_info.as_mut().unwrap().local_base_path = Some(Text::Unicode("Q:relative".into()));
    assert_eq!(health(&s, &p), Health::Ambiguous);
    s.link_info.as_mut().unwrap().local_base_path =
        Some(Text::Unicode("\\\\server.invalid\\share".into()));
    assert_eq!(health(&s, &p), Health::NetworkUnreachableUnknown);
    assert_eq!(p.calls.get(), 0);
}
struct FakeFolders;
impl FolderProvider for FakeFolders {
    fn location(&self, f: Folder) -> Location {
        match f {
            Folder::Desktop => Location::FileSystem {
                current: PathBuf::from("Q:\\OneDrive\\桌面"),
                default: Some(PathBuf::from("Q:\\default\\Desktop")),
            },
            Folder::PublicDesktop => Location::FileSystem {
                current: PathBuf::from("Q:\\public\\Desktop"),
                default: None,
            },
            Folder::Documents => Location::Virtual,
            _ => Location::Unavailable {
                reason: "provider offline".into(),
            },
        }
    }
}
#[test]
fn known_folders_preserve_redirection_ownership_unicode_and_failures() {
    let c = contexts(&FakeFolders);
    assert_eq!(c.len(), 5);
    assert_eq!(c[0].ownership, Ownership::PerUser);
    assert_eq!(c[0].location, FakeFolders.location(Folder::Desktop));
    assert_eq!(c[1].ownership, Ownership::Public);
    assert_eq!(c[2].location, Location::Virtual);
    assert_eq!(
        c[4].location,
        Location::Unavailable {
            reason: "provider offline".into()
        }
    );
}
#[test]
fn source_has_no_execution_com_initialisation_or_library_loading() {
    assert!(guard_tree(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
    ));
}

fn guard_tree(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir).unwrap().all(|entry| {
        let path = entry.unwrap().path();
        if path.is_dir() {
            guard_tree(&path)
        } else {
            guard_source(&std::fs::read_to_string(path).unwrap())
        }
    })
}

fn guard_source(src: &str) -> bool {
    if src
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|word| matches!(word, "process" | "Command"))
    {
        return false;
    }
    for token in [
        "std::process",
        "CoInitialize",
        "LoadLibrary",
        "IShellLink",
        "IPersistFile",
        "ShellExecuteW",
        "CreateProcessW",
        "CoCreateInstance",
        "SHGetFileInfo",
        "ExtractIcon",
        "SHParseDisplayName",
        "SHCreateItemFromParsingName",
        "GetProcAddress",
    ] {
        if src.contains(token) {
            return false;
        }
    }
    true
}

#[test]
fn source_guard_covers_nested_files_and_import_forms() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("nested/deeper")).unwrap();
    let path = dir.path().join("nested/deeper/adapter.rs");
    std::fs::write(&path, "use std::{io, path::Path};").unwrap();
    assert!(guard_tree(dir.path()));
    for source in [
        "use std::{io, process::Command};",
        "use std::{process::{self, Command as Run}};",
        "use std::{process as p};",
        "use std::process;",
        "use std::process::*;",
        "Command :: new(\"inert\");",
        "process :: id();",
        "ShellExecuteW",
        "CreateProcessW",
        "CoCreateInstance",
        "SHGetFileInfoW",
        "ExtractIconExW",
        "SHParseDisplayName",
        "SHCreateItemFromParsingName",
        "GetProcAddress",
        "CoInitialize",
        "LoadLibrary",
        "IShellLink",
        "IPersistFile",
    ] {
        std::fs::write(&path, source).unwrap();
        assert!(!guard_tree(dir.path()), "guard missed {source}");
    }
}
#[cfg(windows)]
#[test]
fn windows_known_folder_shape_and_disposable_shortcut_read() {
    let c = contexts(&WindowsFolders);
    assert_eq!(c.len(), 5);
    for c in c {
        match c.location {
            Location::FileSystem { current, .. } => assert!(current.is_absolute()),
            Location::Unavailable { reason } => assert!(!reason.is_empty()),
            Location::Virtual => panic!("requested filesystem folder"),
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.lnk");
    std::fs::write(&path, finish(header(0))).unwrap();
    if loomward_windows::enumerate::running_elevated() {
        assert!(read(&path).is_err());
    } else {
        assert_eq!(read(&path).unwrap().flags, 0);
    }
}

#[cfg(windows)]
#[test]
fn windows_reader_cap_local_metadata_and_sharing_refusal() {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.lnk");
    std::fs::write(&path, finish(header(0))).unwrap();
    if loomward_windows::enumerate::running_elevated() {
        assert!(read(&path).is_err());
        return;
    }
    assert_eq!(
        WindowsProbe.metadata(path.to_str().unwrap()),
        MetadataState::Present
    );
    assert_eq!(
        WindowsProbe.metadata(dir.path().join("absent.lnk").to_str().unwrap()),
        MetadataState::Missing
    );
    let writer = OpenOptions::new()
        .write(true)
        .share_mode(1 | 2 | 4)
        .open(&path)
        .unwrap();
    assert!(read(&path).is_err());
    drop(writer);
    let file = OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len((MAX_FILE_BYTES + 1) as u64).unwrap();
    drop(file);
    assert_eq!(
        read(&path).unwrap_err().to_string(),
        "file size exceeds cap"
    );
    std::fs::write(&path, [0; 4]).unwrap();
    assert!(read(&path).is_err());
    assert!(read(dir.path()).is_err());
    assert!(read(std::path::Path::new("\\\\server.invalid\\share\\x.lnk")).is_err());
    assert_eq!(
        WindowsProbe.metadata("\\\\server.invalid\\share\\x"),
        MetadataState::Unknown
    );
}
