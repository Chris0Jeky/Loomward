//! LW-136 / LW-875: the per-directory manifest root (`custody` module). Pure function tests: no I/O.

use loomward_operations::{manifest_root, CustodyEntry, CustodyError, EntryKind};

fn entry(name: &str, kind: EntryKind, size: u64, fill: u8) -> CustodyEntry {
    CustodyEntry::from_utf8_name(name, kind, size, [fill; 32])
}

fn raw(units: &[u16], kind: EntryKind, size: u64, fill: u8) -> CustodyEntry {
    CustodyEntry {
        name: units.to_vec(),
        kind,
        size,
        digest: [fill; 32],
    }
}

fn root(entries: &[CustodyEntry]) -> [u8; 32] {
    manifest_root(entries, 1).expect("valid fixture")
}

fn hex(root: [u8; 32]) -> String {
    root.iter().map(|b| format!("{b:02x}")).collect()
}

/// Mixed-case, non-ASCII, supplementary-plane and unpaired-surrogate names. The golden root pins the ordinal
/// UTF-16 collation: 0xD800 (unpaired) < 0xD83D (U+1F600 high surrogate) < 0xFFFD, and "Alpha" < "Zeta" < "alpha".
fn fixture() -> Vec<CustodyEntry> {
    vec![
        raw(&[0xFFFD], EntryKind::File, 1, 0x11),
        raw(&[0xD83D, 0xDE00], EntryKind::File, 2, 0x22),
        entry("Zeta", EntryKind::Directory, 0, 0x33),
        entry("alpha", EntryKind::File, 10, 0x44),
        entry("Alpha", EntryKind::File, 10, 0x55),
        raw(&[0xD800], EntryKind::File, 3, 0x66),
    ]
}

fn for_each_permutation(
    items: &mut Vec<CustodyEntry>,
    k: usize,
    visit: &mut dyn FnMut(&[CustodyEntry]),
) {
    if k == items.len() {
        visit(items);
        return;
    }
    for i in k..items.len() {
        items.swap(k, i);
        for_each_permutation(items, k + 1, visit);
        items.swap(k, i);
    }
}

const EMPTY_V1: &str = "4ba86c7fa60e2f9eb1b5bd2b78fd134b48a8f60c7d1b9175d6e1bd5f4f7bd48e";
const FIXTURE_V1: &str = "c1202870b9d6808aeff1733777943a383f37e1d32e98141ebc43253bfb78bf48";

#[test]
fn empty_directory_has_defined_golden_root() {
    assert_eq!(hex(root(&[])), EMPTY_V1);
}

#[test]
fn golden_root_pins_the_encoding_and_ordinal_collation() {
    assert_eq!(hex(root(&fixture())), FIXTURE_V1);
}

#[test]
fn unknown_collation_versions_are_refused() {
    for version in [0u32, 2, u32::MAX] {
        assert_eq!(
            manifest_root(&fixture(), version),
            Err(CustodyError::UnknownCollation { version })
        );
    }
    assert!(manifest_root(&[], 1).is_ok());
}

#[test]
fn root_is_independent_of_input_order() {
    let base = root(&fixture());
    let mut items = fixture();
    let mut seen = 0usize;
    for_each_permutation(&mut items, 0, &mut |p| {
        seen += 1;
        assert_eq!(root(p), base, "order {seen} changed the root");
    });
    assert_eq!(seen, 720);
}

#[test]
fn duplicate_exact_names_are_refused() {
    let entries = [
        entry("x", EntryKind::File, 1, 0x01),
        entry("y", EntryKind::File, 1, 0x02),
        entry("x", EntryKind::File, 1, 0x02),
    ];
    assert_eq!(
        manifest_root(&entries, 1),
        Err(CustodyError::DuplicateName {
            name: "x".encode_utf16().collect()
        })
    );
}

#[test]
fn duplicate_check_is_exact_so_case_variants_are_not_duplicates() {
    let entries = [
        entry("Readme", EntryKind::File, 1, 0x01),
        entry("readme", EntryKind::File, 1, 0x01),
    ];
    assert!(manifest_root(&entries, 1).is_ok());
}

#[test]
fn directory_with_non_zero_size_is_refused() {
    let entries = [entry("sub", EntryKind::Directory, 1, 0x01)];
    assert_eq!(
        manifest_root(&entries, 1),
        Err(CustodyError::DirectorySize {
            name: "sub".encode_utf16().collect(),
            size: 1,
        })
    );
}

#[test]
fn file_size_is_hashed_and_any_value_is_allowed() {
    let base = root(&[entry("f", EntryKind::File, 0, 0x01)]);
    assert_ne!(root(&[entry("f", EntryKind::File, 1, 0x01)]), base);
}

#[test]
fn one_byte_content_digest_difference_changes_the_root() {
    let base = root(&fixture());
    let mut changed = fixture();
    changed[2].digest[31] ^= 0x01;
    assert_ne!(root(&changed), base);
}

#[test]
fn rename_changes_the_root() {
    let base = root(&fixture());
    let mut renamed = fixture();
    renamed[3] = entry("alphb", EntryKind::File, 10, 0x44);
    assert_ne!(root(&renamed), base);
}

#[test]
fn case_only_rename_changes_the_root() {
    let base = root(&[entry("Readme", EntryKind::File, 1, 0x01)]);
    assert_ne!(root(&[entry("readme", EntryKind::File, 1, 0x01)]), base);
}

#[test]
fn kind_flip_changes_the_root() {
    let base = root(&fixture());
    let mut flipped = fixture();
    flipped[2].kind = EntryKind::File;
    assert_ne!(root(&flipped), base);
}

#[test]
fn unpaired_surrogate_names_hash_distinctly() {
    let lone_high = root(&[raw(&[0xD800], EntryKind::File, 0, 0x01)]);
    let lone_low = root(&[raw(&[0xDC00], EntryKind::File, 0, 0x01)]);
    // The lossy text form of both unpaired surrogates is U+FFFD, so a lossy name would collapse them.
    let replacement = root(&[entry("\u{FFFD}", EntryKind::File, 0, 0x01)]);
    assert_ne!(lone_high, lone_low);
    assert_ne!(lone_high, replacement);
    assert_ne!(lone_low, replacement);
}

#[test]
fn child_root_propagates_into_the_parent() {
    // A directory entry carries its child's root as its digest, so a one-byte change deep in the tree
    // reaches the parent root.
    let parent_of = |child_root: [u8; 32]| {
        root(&[CustodyEntry {
            name: "sub".encode_utf16().collect(),
            kind: EntryKind::Directory,
            size: 0,
            digest: child_root,
        }])
    };
    let child_a = root(&[entry("f", EntryKind::File, 3, 0x09)]);
    let child_b = root(&[entry("f", EntryKind::File, 3, 0x08)]);
    assert_ne!(child_a, child_b);
    assert_ne!(parent_of(child_a), parent_of(child_b));
}
