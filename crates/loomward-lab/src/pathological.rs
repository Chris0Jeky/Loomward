//! Small, explicit native test profiles, separate from the G:/E: scale-lab CLI.
//! Callers supply a new empty directory and retain the guard until before removing it.
use std::{
    ffi::OsString,
    fs, io,
    os::windows::{ffi::OsStringExt, fs::MetadataExt},
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    DepthLimit,
    LongPath,
    UnpairedSurrogate,
    JunctionCycle,
    SymlinkCycle,
    HardLinks,
    DeniedDirectory,
    Churn { files: usize },
}

/// Restores the fixture's deny ACE and removes its cycle before its temporary owner cleans up.
pub struct GeneratedTree {
    pub root: PathBuf,
    pub leaf: PathBuf,
    denied: Option<(PathBuf, String)>,
    cycle: Option<PathBuf>,
}

fn checked(command: &mut Command) -> io::Result<()> {
    let output = command.output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "fixture command failed: {}",
            output.status
        )))
    }
}

impl Drop for GeneratedTree {
    fn drop(&mut self) {
        if let Some((path, sid)) = &self.denied {
            let result =
                checked(
                    Command::new("icacls.exe")
                        .arg(path)
                        .args(["/remove:d", sid, "/q"]),
                );
            if let Err(error) = result {
                // Do not silently leave an inaccessible fixture behind, including on assertion failure.
                eprintln!("UNVERIFIED: fixture deny ACE cleanup failed: {error}");
                if !std::thread::panicking() {
                    panic!("fixture deny ACE cleanup failed");
                }
            }
        }
        if let Some(path) = &self.cycle {
            if let Err(error) = fs::remove_dir(path) {
                eprintln!("UNVERIFIED: fixture cycle cleanup failed: {error}");
                if !std::thread::panicking() {
                    panic!("fixture cycle cleanup failed");
                }
            }
        }
    }
}

pub fn generate(root: &Path, profile: Profile) -> io::Result<GeneratedTree> {
    if loomward_windows::enumerate::running_elevated() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "non-elevated fixture required",
        ));
    }
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.is_dir()
        || metadata.file_attributes() & 0x400 != 0
        || root.parent().is_none()
        || fs::read_dir(root)?.next().is_some()
    {
        return Err(crate::invalid(
            "fixture requires an empty plain non-root directory",
        ));
    }
    if let Profile::Churn { files } = profile {
        if !(1..=100_000).contains(&files) {
            return Err(crate::invalid("churn fixture requires 1..=100000 files"));
        }
    }
    let root = root.canonicalize()?;
    let mut tree = GeneratedTree {
        leaf: root.join("synthetic.bin"),
        root,
        denied: None,
        cycle: None,
    };
    match profile {
        Profile::DepthLimit | Profile::LongPath => {
            let (levels, component) = if profile == Profile::DepthLimit {
                (511, "d".to_owned()) // 512 directory identities, including the root.
            } else {
                (6, "long-synthetic-component-".repeat(2))
            };
            let mut directory = tree.root.clone();
            for _ in 0..levels {
                directory.push(&component);
                fs::create_dir(&directory)?;
            }
            tree.leaf = directory.join("synthetic.bin");
            fs::write(&tree.leaf, b"synthetic")?;
        }
        Profile::UnpairedSurrogate => {
            let name: Vec<u16> = "synthetic-"
                .encode_utf16()
                .chain([0xd800])
                .chain(".bin".encode_utf16())
                .collect();
            tree.leaf = tree.root.join(OsString::from_wide(&name));
            fs::write(&tree.leaf, b"synthetic")?;
        }
        Profile::JunctionCycle | Profile::SymlinkCycle => {
            fs::write(&tree.leaf, b"synthetic")?;
            let link = tree.root.join("cycle");
            if profile == Profile::SymlinkCycle {
                std::os::windows::fs::symlink_dir(&tree.root, &link)?;
            } else {
                // Pass paths as data, never interpolate them into shell source.
                let plain = |p: &Path| p.to_str().unwrap().trim_start_matches(r"\\?\").to_owned();
                checked(Command::new("powershell.exe")
                    .args(["-NoProfile", "-NonInteractive", "-Command",
                        "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:LOOMWARD_FIXTURE_LINK -Target $env:LOOMWARD_FIXTURE_ROOT | Out-Null"])
                    .env("LOOMWARD_FIXTURE_LINK", plain(&link))
                    .env("LOOMWARD_FIXTURE_ROOT", plain(&tree.root)))?;
            }
            tree.cycle = Some(link);
        }
        Profile::HardLinks => {
            // NTFS allows at most 1024 names per file; eight groups exercise 4096 names.
            for group in 0..8 {
                let original = tree.root.join(format!("object-{group:02}-0000.bin"));
                fs::write(&original, b"synthetic")?;
                for link in 1..512 {
                    fs::hard_link(
                        &original,
                        tree.root.join(format!("object-{group:02}-{link:04}.bin")),
                    )?;
                }
                tree.leaf = original;
            }
        }
        Profile::DeniedDirectory => {
            fs::write(&tree.leaf, b"visible")?;
            let denied = tree.root.join("denied");
            fs::create_dir(&denied)?;
            tree.leaf = denied.join("hidden.bin");
            fs::write(&tree.leaf, b"hidden")?;
            let identity = Command::new("whoami.exe")
                .args(["/user", "/fo", "csv", "/nh"])
                .output()?;
            if !identity.status.success() {
                return Err(io::Error::other("cannot resolve fixture user SID"));
            }
            let output = String::from_utf8(identity.stdout)
                .map_err(|_| crate::invalid("invalid SID output"))?;
            let sid = output
                .trim()
                .split(',')
                .next_back()
                .unwrap_or("")
                .trim_matches('"');
            if !sid.starts_with("S-1-")
                || !sid
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b == b'-' || b == b'S')
            {
                return Err(crate::invalid("invalid fixture SID"));
            }
            let sid = format!("*{sid}");
            tree.denied = Some((denied.clone(), sid.clone()));
            checked(Command::new("icacls.exe").arg(&denied).args([
                "/deny",
                &format!("{sid}:(RD)"),
                "/q",
            ]))?;
        }
        Profile::Churn { files } => {
            for bucket in 0..files.div_ceil(100) {
                let directory = tree.root.join(format!("bucket-{bucket:04}"));
                fs::create_dir(&directory)?;
                for index in 0..100.min(files - bucket * 100) {
                    tree.leaf = directory.join(format!("file-{index:04}.bin"));
                    fs::File::create(&tree.leaf)?.set_len(17)?;
                }
            }
        }
    }
    Ok(tree)
}
