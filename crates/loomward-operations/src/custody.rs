//! Archive custody manifest root (LW-136, docs/51 section 2.2): a Merkle-style root over the entries of one
//! directory, under a versioned collation. Pure: no I/O, no file or process effect, no authority.
//!
//! Each entry is `(name, kind, size, digest)`. A file's digest is its content digest; a directory's digest is
//! the root the caller computed for that directory's own entries (the Merkle recursion lives in the caller:
//! compute the child root first, then pass it as the parent's entry). A directory entry must have `size` 0;
//! any other size is refused.
//!
//! The root of a directory is the SHA-256 of the following byte sequence, in this order, with no separators
//! beyond the length prefixes given:
//!
//! 1. the domain tag `loomward/manifest-root/1` followed by one `0x00` byte;
//! 2. `collation_version` as a `u32`, little-endian (4 bytes); only [`COLLATION_V1`] is accepted;
//! 3. the entry count as a `u64`, little-endian (8 bytes);
//! 4. for each entry, in collation order (below):
//!    - the name's length in UTF-16 code units as a `u64`, little-endian, then each code unit as a `u16`,
//!      little-endian. The name is the exact Windows name; it is never case-folded or normalised, and it may
//!      contain unpaired surrogates (so `Readme` and `readme` produce different roots);
//!    - the kind as one byte: `0x00` for a file, `0x01` for a directory;
//!    - the size as a `u64`, little-endian;
//!    - the 32-byte digest.
//!
//! Collation: entries are ordered by their name's UTF-16 code units compared ordinally (each unit as an
//! unsigned 16-bit integer, a proper prefix sorting first). There is no case folding and no Unicode table:
//! the order and the hash depend on the code units alone. Two entries with the same exact name are refused,
//! so no tiebreak exists.
//!
//! Domain tag: the digest of file content is part of the root's meaning. Changing the file digest algorithm
//! requires a new domain tag, not a new collation version. The collation version covers only the ordering and
//! encoding of the entries above.
//!
//! An empty directory has the root defined by steps 1 to 3 with an entry count of zero.
//!
//! Names are single path components. This function does not validate them; the caller supplies exact names.

use sha2::{Digest as _, Sha256};
use std::{
    fmt,
    path::{Path, PathBuf},
};

const DOMAIN: &[u8] = b"loomward/manifest-root/1\0";

/// The only collation this module defines. Any other `collation_version` is refused.
pub const COLLATION_V1: u32 = 1;

/// Kind of a directory entry, encoded as one byte in the root (`0x00` file, `0x01` directory).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
}

impl EntryKind {
    fn byte(self) -> u8 {
        match self {
            EntryKind::File => 0,
            EntryKind::Directory => 1,
        }
    }
}

/// One directory entry as the manifest root sees it. `name` is the exact name as UTF-16 code units. `digest`
/// is the content digest for a file and the child root for a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyEntry {
    pub name: Vec<u16>,
    pub kind: EntryKind,
    pub size: u64,
    pub digest: [u8; 32],
}

impl CustodyEntry {
    /// Convenience for names that are known Unicode text (tests and synthetic fixtures): the name is encoded
    /// as UTF-16 with no normalisation. Names that cannot be represented as text (unpaired surrogates) are
    /// built by setting `name` directly.
    pub fn from_utf8_name(name: &str, kind: EntryKind, size: u64, digest: [u8; 32]) -> Self {
        Self {
            name: name.encode_utf16().collect(),
            kind,
            size,
            digest,
        }
    }
}

/// Why a directory could not be rooted. Nothing is hashed on error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CustodyError {
    /// `collation_version` is not [`COLLATION_V1`].
    UnknownCollation { version: u32 },
    /// A directory entry carries a non-zero size.
    DirectorySize { name: Vec<u16>, size: u64 },
    /// Two entries share the same exact name.
    DuplicateName { name: Vec<u16> },
}

impl fmt::Display for CustodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CustodyError::UnknownCollation { version } => write!(
                f,
                "unknown manifest collation version {version}; only collation {COLLATION_V1} is defined"
            ),
            CustodyError::DirectorySize { name, size } => write!(
                f,
                "directory entry {:?} has size {size}; directories must have size 0",
                String::from_utf16_lossy(name)
            ),
            CustodyError::DuplicateName { name } => write!(
                f,
                "duplicate exact name {:?} in one directory",
                String::from_utf16_lossy(name)
            ),
        }
    }
}

impl std::error::Error for CustodyError {}

/// Validates the inputs and returns the entries in collation order.
fn checked_order(
    entries: &[CustodyEntry],
    collation_version: u32,
) -> Result<Vec<&CustodyEntry>, CustodyError> {
    if collation_version != COLLATION_V1 {
        return Err(CustodyError::UnknownCollation {
            version: collation_version,
        });
    }
    for e in entries {
        if e.kind == EntryKind::Directory && e.size != 0 {
            return Err(CustodyError::DirectorySize {
                name: e.name.clone(),
                size: e.size,
            });
        }
    }
    let mut sorted: Vec<&CustodyEntry> = entries.iter().collect();
    sorted.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    if let Some(pair) = sorted.windows(2).find(|w| w[0].name == w[1].name) {
        return Err(CustodyError::DuplicateName {
            name: pair[0].name.clone(),
        });
    }
    Ok(sorted)
}

/// The exact byte sequence hashed for an already-validated, collation-ordered directory.
fn preimage(sorted: &[&CustodyEntry], collation_version: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(DOMAIN);
    out.extend_from_slice(&collation_version.to_le_bytes());
    out.extend_from_slice(&(sorted.len() as u64).to_le_bytes());
    for e in sorted {
        out.extend_from_slice(&(e.name.len() as u64).to_le_bytes());
        for unit in &e.name {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out.push(e.kind.byte());
        out.extend_from_slice(&e.size.to_le_bytes());
        out.extend_from_slice(&e.digest);
    }
    out
}

/// Manifest root of one directory under `collation_version` (see the module documentation for the exact
/// encoding and collation). Independent of the order of `entries`; an empty slice has a defined root.
/// Refused, with no root produced: an unknown collation version, a directory with a non-zero size, or two
/// entries with the same exact name.
pub fn manifest_root(
    entries: &[CustodyEntry],
    collation_version: u32,
) -> Result<[u8; 32], CustodyError> {
    let sorted = checked_order(entries, collation_version)?;
    Ok(Sha256::digest(preimage(&sorted, collation_version)).into())
}

/// One directory of an observed tree and its manifest root, or the reason it has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryRoot {
    /// Relative path of the directory; empty for the tree root.
    pub path: PathBuf,
    /// The root under [`COLLATION_V1`], or why no root exists. Never a partial root.
    pub root: Result<[u8; 32], &'static str>,
}

/// The exact name as UTF-16 code units. On Windows this is the native wide name, unpaired surrogates
/// included; elsewhere only names that are valid Unicode can be represented.
fn name_units(name: &std::ffi::OsStr) -> Option<Vec<u16>> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        Some(name.encode_wide().collect())
    }
    #[cfg(not(windows))]
    {
        name.to_str().map(|s| s.encode_utf16().collect())
    }
}

/// Manifest roots (`COLLATION_V1`) for every directory of an observed `tree`, from the entries actually read
/// and hashed in that observation (the file digest is the tree reader's SHA-256 of the default stream). A
/// directory has a root only if its listing was complete, every entry was read with a known size and, for a
/// file, a content digest, every subdirectory has a root of its own, and `report` holds no difference or
/// unknown at or under it. Metadata-class differences are the one exception: the root covers names, kinds,
/// sizes and content digests only, so they do not make it partial. Anything else yields `Err(reason)`.
/// Pure: reads nothing from the filesystem.
pub fn tree_manifest_roots(
    tree: &crate::tree::Tree,
    report: &crate::tree::Report,
) -> Vec<DirectoryRoot> {
    use crate::tree::{DifferenceKind, Kind};
    use std::collections::BTreeMap;
    let blockers: Vec<(&Path, &'static str)> = report
        .differences
        .iter()
        .filter(|d| !matches!(d.kind, DifferenceKind::Metadata(_)))
        .map(|d| (d.path.as_path(), "differing_entry"))
        .chain(
            report
                .unknowns
                .iter()
                .map(|u| (u.path.as_path(), "unknown_entry")),
        )
        .collect();
    let mut children: BTreeMap<&Path, Vec<(&Path, &crate::tree::Observation<crate::tree::Node>)>> =
        BTreeMap::new();
    for (path, node) in &tree.nodes {
        if let Some(parent) = path.parent() {
            children.entry(parent).or_default().push((path, node));
        }
    }
    let mut directories: Vec<&Path> = tree
        .nodes
        .iter()
        .filter(|(_, node)| matches!(node, Ok(n) if n.kind == Kind::Directory))
        .map(|(path, _)| path.as_path())
        .collect();
    // Children before parents: a directory's digest in its parent is its own root.
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let mut roots: BTreeMap<&Path, Result<[u8; 32], &'static str>> = BTreeMap::new();
    for dir in directories {
        let root = (|| {
            let Ok(node) = &tree.nodes[dir] else {
                unreachable!("only observed directories are listed");
            };
            if node.children.is_err() {
                return Err("listing_incomplete");
            }
            if let Some((_, reason)) = blockers.iter().find(|(path, _)| path.starts_with(dir)) {
                return Err(*reason);
            }
            let mut entries = Vec::new();
            for (path, node) in children.get(dir).map(Vec::as_slice).unwrap_or_default() {
                let node = node.as_ref().map_err(|_| "entry_not_read")?;
                let name = name_units(path.file_name().ok_or("name_not_representable")?)
                    .ok_or("name_not_representable")?;
                let (kind, size, digest) = match node.kind {
                    Kind::File => (
                        EntryKind::File,
                        node.size.map_err(|_| "size_unknown")?,
                        node.content.map_err(|_| "content_not_hashed")?,
                    ),
                    Kind::Directory => (
                        EntryKind::Directory,
                        0,
                        roots[path].map_err(|_| "descendant_not_rooted")?,
                    ),
                };
                entries.push(CustodyEntry {
                    name,
                    kind,
                    size,
                    digest,
                });
            }
            manifest_root(&entries, COLLATION_V1).map_err(|e| match e {
                CustodyError::DuplicateName { .. } => "duplicate_name",
                _ => "custody_refused",
            })
        })();
        roots.insert(dir, root);
    }
    roots
        .into_iter()
        .map(|(path, root)| DirectoryRoot {
            path: path.to_path_buf(),
            root,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str, kind: EntryKind, size: u64, fill: u8) -> CustodyEntry {
        CustodyEntry::from_utf8_name(name, kind, size, [fill; 32])
    }

    fn names_in_order(entries: &[CustodyEntry]) -> Vec<String> {
        checked_order(entries, COLLATION_V1)
            .unwrap()
            .iter()
            .map(|e| String::from_utf16_lossy(&e.name))
            .collect()
    }

    #[test]
    fn ordering_is_ordinal_over_raw_code_units_without_case_folding() {
        let entries = [
            named("alpha", EntryKind::File, 0, 1),
            named("Zeta", EntryKind::File, 0, 2),
            named("Alpha", EntryKind::File, 0, 3),
        ];
        // 'A' (0x41) < 'Z' (0x5A) < 'a' (0x61): uppercase sorts before lowercase, nothing is folded.
        assert_eq!(names_in_order(&entries), ["Alpha", "Zeta", "alpha"]);
    }

    #[test]
    fn surrogate_code_units_order_by_value_not_by_scalar_value() {
        // U+1F600 is 0xD83D 0xDE00 in UTF-16, which sorts before U+FFFD (0xFFFD) ordinally.
        let entries = [
            named("\u{FFFD}", EntryKind::File, 0, 1),
            named("\u{1F600}", EntryKind::File, 0, 2),
        ];
        assert_eq!(names_in_order(&entries), ["\u{1F600}", "\u{FFFD}"]);
    }

    #[test]
    fn length_prefix_is_encoded_before_every_name() {
        // "ab" + "c" and "a" + "bc" have identical name code units in sequence; only the length prefix tells
        // the entries apart. Removing the prefix changes these exact bytes and fails this test.
        let split_ab_c = [
            named("ab", EntryKind::File, 0, 7),
            named("c", EntryKind::File, 0, 7),
        ];
        let split_a_bc = [
            named("a", EntryKind::File, 0, 7),
            named("bc", EntryKind::File, 0, 7),
        ];
        let pa = preimage(
            &checked_order(&split_ab_c, COLLATION_V1).unwrap(),
            COLLATION_V1,
        );
        let pb = preimage(
            &checked_order(&split_a_bc, COLLATION_V1).unwrap(),
            COLLATION_V1,
        );
        assert_ne!(pa, pb);

        let mut expected = Vec::new();
        expected.extend_from_slice(DOMAIN);
        expected.extend_from_slice(&1u32.to_le_bytes()); // collation version
        expected.extend_from_slice(&1u64.to_le_bytes()); // entry count
        expected.extend_from_slice(&2u64.to_le_bytes()); // name length in code units
        expected.extend_from_slice(&[0x61, 0x00, 0x62, 0x00]); // "ab"
        expected.push(0x00); // file
        expected.extend_from_slice(&0u64.to_le_bytes()); // size
        expected.extend_from_slice(&[7u8; 32]);
        let single_ab = [named("ab", EntryKind::File, 0, 7)];
        assert_eq!(
            preimage(
                &checked_order(&single_ab, COLLATION_V1).unwrap(),
                COLLATION_V1
            ),
            expected
        );
    }

    mod tree_roots {
        use super::super::*;
        use crate::tree::{
            Difference, DifferenceKind, Field, Kind, Node, Report, Side, Tree, Unknown,
        };
        use std::collections::BTreeMap;

        fn node(kind: Kind, size: u64, fill: u8) -> Node {
            Node {
                kind,
                size: Ok(size),
                content: if kind == Kind::File {
                    Ok([fill; 32])
                } else {
                    Err("content_not_read")
                },
                metadata: BTreeMap::new(),
                children: Ok(()),
            }
        }

        /// `"" / a.txt, sub / b.txt`
        fn tree() -> Tree {
            let mut t = Tree::default();
            for (path, n) in [
                ("", node(Kind::Directory, 0, 0)),
                ("a.txt", node(Kind::File, 3, 1)),
                ("sub", node(Kind::Directory, 0, 0)),
                ("sub/b.txt", node(Kind::File, 5, 2)),
            ] {
                t.nodes.insert(PathBuf::from(path), Ok(n));
            }
            t
        }

        fn root_of(roots: &[DirectoryRoot], path: &str) -> Result<[u8; 32], &'static str> {
            roots
                .iter()
                .find(|r| r.path == Path::new(path))
                .unwrap()
                .root
        }

        fn file_mut<'a>(t: &'a mut Tree, path: &str) -> &'a mut Node {
            t.nodes.get_mut(Path::new(path)).unwrap().as_mut().unwrap()
        }

        #[test]
        fn complete_tree_roots_equal_the_manual_merkle_recursion() {
            let roots = tree_manifest_roots(&tree(), &Report::default());
            let sub = manifest_root(
                &[CustodyEntry::from_utf8_name(
                    "b.txt",
                    EntryKind::File,
                    5,
                    [2; 32],
                )],
                COLLATION_V1,
            )
            .unwrap();
            let top = manifest_root(
                &[
                    CustodyEntry::from_utf8_name("a.txt", EntryKind::File, 3, [1; 32]),
                    CustodyEntry::from_utf8_name("sub", EntryKind::Directory, 0, sub),
                ],
                COLLATION_V1,
            )
            .unwrap();
            assert_eq!(root_of(&roots, "sub"), Ok(sub));
            assert_eq!(root_of(&roots, ""), Ok(top));
            assert_eq!(roots.len(), 2);
        }

        #[test]
        fn one_changed_digest_changes_the_root() {
            let before = root_of(&tree_manifest_roots(&tree(), &Report::default()), "");
            let mut changed = tree();
            file_mut(&mut changed, "sub/b.txt").content = Ok([9; 32]);
            let after = root_of(&tree_manifest_roots(&changed, &Report::default()), "");
            assert!(before.is_ok() && after.is_ok());
            assert_ne!(before, after);
        }

        #[test]
        fn unreadable_unhashed_and_incomplete_entries_leave_no_root_and_propagate_upward() {
            let mut unhashed = tree();
            file_mut(&mut unhashed, "sub/b.txt").content = Err("byte_budget");
            let roots = tree_manifest_roots(&unhashed, &Report::default());
            assert_eq!(root_of(&roots, "sub"), Err("content_not_hashed"));
            assert_eq!(root_of(&roots, ""), Err("descendant_not_rooted"));

            let mut unreadable = tree();
            unreadable
                .nodes
                .insert(PathBuf::from("a.txt"), Err("access_denied"));
            let roots = tree_manifest_roots(&unreadable, &Report::default());
            assert!(
                root_of(&roots, "sub").is_ok(),
                "an unrelated sibling keeps its root"
            );
            assert_eq!(root_of(&roots, ""), Err("entry_not_read"));

            let mut partial = tree();
            file_mut(&mut partial, "sub").children = Err("incomplete_listing");
            let roots = tree_manifest_roots(&partial, &Report::default());
            assert_eq!(root_of(&roots, "sub"), Err("listing_incomplete"));
            assert_eq!(root_of(&roots, ""), Err("descendant_not_rooted"));
        }

        #[test]
        fn differences_and_unknowns_block_only_the_directories_that_hold_them() {
            let mut report = Report::default();
            report.differences.push(Difference {
                path: "sub/b.txt".into(),
                kind: DifferenceKind::Content,
            });
            let roots = tree_manifest_roots(&tree(), &report);
            assert_eq!(root_of(&roots, "sub"), Err("differing_entry"));
            assert_eq!(root_of(&roots, ""), Err("differing_entry"));

            let mut report = Report::default();
            report.unknowns.push(Unknown {
                path: "a.txt".into(),
                side: Side::Source,
                field: Field::Content,
                reason: "byte_budget",
            });
            let roots = tree_manifest_roots(&tree(), &report);
            assert!(root_of(&roots, "sub").is_ok(), "a.txt is not under sub");
            assert_eq!(root_of(&roots, ""), Err("unknown_entry"));

            // The root does not cover metadata, so a metadata-only difference leaves it whole.
            let mut report = Report::default();
            report.differences.push(Difference {
                path: "a.txt".into(),
                kind: DifferenceKind::Metadata(crate::tree::MetadataClass::LastWriteTime),
            });
            assert!(root_of(&tree_manifest_roots(&tree(), &report), "").is_ok());
        }

        #[cfg(windows)]
        #[test]
        fn native_wide_names_keep_unpaired_surrogates() {
            use std::{ffi::OsString, os::windows::ffi::OsStringExt};
            let wide = [0x61u16, 0xD800];
            let mut t = Tree::default();
            t.nodes
                .insert(PathBuf::new(), Ok(node(Kind::Directory, 0, 0)));
            t.nodes.insert(
                PathBuf::from(OsString::from_wide(&wide)),
                Ok(node(Kind::File, 1, 4)),
            );
            let expected = manifest_root(
                &[CustodyEntry {
                    name: wide.to_vec(),
                    kind: EntryKind::File,
                    size: 1,
                    digest: [4; 32],
                }],
                COLLATION_V1,
            )
            .unwrap();
            assert_eq!(
                root_of(&tree_manifest_roots(&t, &Report::default()), ""),
                Ok(expected)
            );
        }

        #[cfg(not(windows))]
        #[test]
        fn names_that_are_not_unicode_have_no_root_here() {
            use std::{ffi::OsString, os::unix::ffi::OsStringExt};
            let mut t = Tree::default();
            t.nodes
                .insert(PathBuf::new(), Ok(node(Kind::Directory, 0, 0)));
            t.nodes.insert(
                PathBuf::from(OsString::from_vec(vec![0x66, 0xFF])),
                Ok(node(Kind::File, 1, 4)),
            );
            assert_eq!(
                root_of(&tree_manifest_roots(&t, &Report::default()), ""),
                Err("name_not_representable")
            );
        }
    }
}
