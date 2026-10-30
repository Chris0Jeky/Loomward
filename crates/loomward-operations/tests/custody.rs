//! LW-136 / LW-875: the per-directory manifest root (`custody` module). Pure function tests: no I/O.

use loomward_operations::{manifest_root, CustodyEntry, EntryKind};

fn entry(name: &str, kind: EntryKind, size: u64, fill: u8) -> CustodyEntry {
    CustodyEntry {
        name: name.to_owned(),
        kind,
        size,
        digest: [fill; 32],
    }
}

fn hex(root: [u8; 32]) -> String {
    root.iter().map(|b| format!("{b:02x}")).collect()
}

/// Mixed-case, non-ASCII and supplementary-plane names. The golden roots below pin the UTF-16 collation: in
/// UTF-16 the U+1F600 surrogate pair (0xD83D) sorts before U+FFFD (0xFFFD), the reverse of UTF-8 byte order.
fn fixture() -> Vec<CustodyEntry> {
    vec![
        entry("\u{FFFD}", EntryKind::File, 1, 0x11),
        entry("\u{1F600}", EntryKind::File, 2, 0x22),
        entry("Zeta", EntryKind::Directory, 0, 0x33),
        entry("alpha", EntryKind::File, 10, 0x44),
        entry("Alpha", EntryKind::File, 10, 0x55),
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
const FIXTURE_V1: &str = "0176108cc615cf5d3306674848ffde7a04c7f5f99a412e933660f16298263b79";
const FIXTURE_V2: &str = "44d65749dd2b2fa33567ea4d047db7c6f3ea6abda35948fc1cef30b438b02f40";

#[test]
fn empty_directory_has_defined_golden_root() {
    assert_eq!(hex(manifest_root(&[], 1)), EMPTY_V1);
}

#[test]
fn golden_roots_pin_the_encoding_and_utf16_collation() {
    assert_eq!(hex(manifest_root(&fixture(), 1)), FIXTURE_V1);
    assert_eq!(hex(manifest_root(&fixture(), 2)), FIXTURE_V2);
}

#[test]
fn root_is_independent_of_input_order() {
    let base = manifest_root(&fixture(), 1);
    let mut items = fixture();
    let mut seen = 0usize;
    for_each_permutation(&mut items, 0, &mut |p| {
        seen += 1;
        assert_eq!(manifest_root(p, 1), base, "order {seen} changed the root");
    });
    assert_eq!(seen, 120);
}

#[test]
fn duplicate_exact_names_are_ordered_deterministically() {
    let pair = || {
        vec![
            entry("x", EntryKind::File, 1, 0x01),
            entry("x", EntryKind::File, 1, 0x02),
        ]
    };
    let base = manifest_root(&pair(), 1);
    let mut items = pair();
    for_each_permutation(&mut items, 0, &mut |p| {
        assert_eq!(manifest_root(p, 1), base)
    });
}

#[test]
fn one_byte_content_digest_difference_changes_the_root() {
    let base = manifest_root(&fixture(), 1);
    let mut changed = fixture();
    changed[2].digest[31] ^= 0x01;
    assert_ne!(manifest_root(&changed, 1), base);
}

#[test]
fn rename_changes_the_root() {
    let base = manifest_root(&fixture(), 1);
    let mut renamed = fixture();
    renamed[3].name = "alphb".to_owned();
    assert_ne!(manifest_root(&renamed, 1), base);
}

#[test]
fn case_only_rename_changes_the_root() {
    let base = manifest_root(&[entry("Readme", EntryKind::File, 1, 0x01)], 1);
    assert_ne!(
        manifest_root(&[entry("readme", EntryKind::File, 1, 0x01)], 1),
        base
    );
}

#[test]
fn kind_flip_changes_the_root() {
    let base = manifest_root(&fixture(), 1);
    let mut flipped = fixture();
    flipped[2].kind = EntryKind::File;
    assert_ne!(manifest_root(&flipped, 1), base);
}

#[test]
fn collation_version_changes_the_root() {
    assert_ne!(manifest_root(&fixture(), 1), manifest_root(&fixture(), 2));
    assert_ne!(manifest_root(&[], 1), manifest_root(&[], 2));
}

#[test]
fn length_prefix_stops_concatenation_collisions() {
    let d = [0x07; 32];
    let split_ab_c = vec![
        CustodyEntry {
            name: "ab".into(),
            kind: EntryKind::File,
            size: 0,
            digest: d,
        },
        CustodyEntry {
            name: "c".into(),
            kind: EntryKind::File,
            size: 0,
            digest: d,
        },
    ];
    let split_a_bc = vec![
        CustodyEntry {
            name: "a".into(),
            kind: EntryKind::File,
            size: 0,
            digest: d,
        },
        CustodyEntry {
            name: "bc".into(),
            kind: EntryKind::File,
            size: 0,
            digest: d,
        },
    ];
    assert_ne!(manifest_root(&split_ab_c, 1), manifest_root(&split_a_bc, 1));
}

#[test]
fn child_root_propagates_into_the_parent() {
    // A directory entry carries its child's root as its digest, so a one-byte change deep in the tree
    // reaches the parent root.
    let parent_of = |child_root: [u8; 32]| {
        manifest_root(
            &[CustodyEntry {
                name: "sub".into(),
                kind: EntryKind::Directory,
                size: 0,
                digest: child_root,
            }],
            1,
        )
    };
    let child_a = manifest_root(&[entry("f", EntryKind::File, 3, 0x09)], 1);
    let child_b = manifest_root(&[entry("f", EntryKind::File, 3, 0x08)], 1);
    assert_ne!(child_a, child_b);
    assert_ne!(parent_of(child_a), parent_of(child_b));
}
