//! Shared by every `fuzz_*.rs` integration test (docs/45 row 6, #414): `#[path]`-included, never a crate.
//! Property-based fuzzing with `proptest`, because the pinned toolchain is stable MSVC and `cargo-fuzz`
//! needs nightly. Default case counts keep a target under a few seconds; `LOOMWARD_FUZZ_CASES=100000`
//! turns the same targets into a long bounded run. Seeds come from the existing fixtures.
#![allow(dead_code)]
use proptest::prelude::*;

/// `LOOMWARD_FUZZ_CASES` when set to a positive integer, else `default`.
pub fn cases(default: u32) -> u32 {
    std::env::var("LOOMWARD_FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

/// No regression files in the tree: a failing input is printed in full and minimised.
pub fn config(default: u32) -> ProptestConfig {
    ProptestConfig {
        cases: cases(default),
        failure_persistence: None,
        max_shrink_iters: 4096,
        ..ProptestConfig::default()
    }
}

/// One edit of a valid byte string. Positions are reduced modulo the current length.
#[derive(Debug, Clone)]
pub enum Edit {
    Set(usize, u8),
    /// Overwrite four bytes with a boundary-ish word (sizes, offsets, lengths).
    Word(usize, u32),
    Truncate(usize),
    Insert(usize, Vec<u8>),
    Delete(usize, usize),
    Dup(usize, usize),
}

const WORDS: [u32; 12] = [
    0,
    1,
    2,
    7,
    8,
    87,
    88,
    89,
    104,
    0x7fff_ffff,
    0x8000_0000,
    0xffff_ffff,
];

pub fn edit() -> impl Strategy<Value = Edit> {
    let at = any::<usize>();
    prop_oneof![
        2 => (at.clone(), any::<u8>()).prop_map(|(a, b)| Edit::Set(a, b)),
        2 => (at.clone(), prop::sample::select(b"a0_-9Zz1 ".to_vec())).prop_map(|(a, b)| Edit::Set(a, b)),
        3 => (at.clone(), prop_oneof![prop::sample::select(WORDS.to_vec()), any::<u32>()])
            .prop_map(|(a, w)| Edit::Word(a, w)),
        1 => at.clone().prop_map(Edit::Truncate),
        1 => (at.clone(), prop::collection::vec(any::<u8>(), 0..16)).prop_map(|(a, v)| Edit::Insert(a, v)),
        1 => (at.clone(), 0usize..32).prop_map(|(a, n)| Edit::Delete(a, n)),
        1 => (at, 0usize..64).prop_map(|(a, n)| Edit::Dup(a, n)),
    ]
}

pub fn edits() -> impl Strategy<Value = Vec<Edit>> {
    prop::collection::vec(edit(), 1..8)
}

/// One or two edits: most results stay near the valid input, which is where acceptance is decided.
pub fn few_edits() -> impl Strategy<Value = Vec<Edit>> {
    prop::collection::vec(edit(), 1..3)
}

pub fn apply(mut bytes: Vec<u8>, edits: &[Edit]) -> Vec<u8> {
    for e in edits {
        let n = bytes.len();
        match e {
            Edit::Set(a, b) if n > 0 => bytes[a % n] = *b,
            Edit::Word(a, w) if n >= 4 => {
                let at = a % (n - 3);
                bytes[at..at + 4].copy_from_slice(&w.to_le_bytes());
            }
            Edit::Truncate(a) => bytes.truncate(a % (n + 1)),
            Edit::Insert(a, v) => {
                let at = a % (n + 1);
                bytes.splice(at..at, v.iter().copied());
            }
            Edit::Delete(a, len) if n > 0 => {
                let at = a % n;
                bytes.drain(at..(at + len).min(n));
            }
            Edit::Dup(a, len) if n > 0 => {
                let at = a % n;
                let chunk = bytes[at..(at + len).min(n)].to_vec();
                bytes.splice(at..at, chunk);
            }
            _ => {}
        }
    }
    bytes
}

/// A header-field edit of one record in a chain of records: `starts[rec % len]` is the record's
/// offset, `field` a byte offset inside its header. The value is a boundary-ish u32 or the
/// record's natural size, one step under or one step over, so offsets land on, just inside and
/// just past their bound.
#[derive(Debug, Clone)]
pub struct FieldEdit {
    pub rec: usize,
    pub field: usize,
    pub pick: u8,
    pub value: u32,
}

pub fn field_edit(fields: Vec<usize>) -> impl Strategy<Value = FieldEdit> {
    (
        any::<usize>(),
        prop::sample::select(fields),
        any::<u8>(),
        prop_oneof![prop::sample::select(WORDS.to_vec()), any::<u32>()],
    )
        .prop_map(|(rec, field, pick, value)| FieldEdit {
            rec,
            field,
            pick,
            value,
        })
}

/// Applies `edits` where records start at `starts` and have natural sizes `sizes`.
pub fn apply_fields(
    mut bytes: Vec<u8>,
    starts: &[usize],
    sizes: &[usize],
    edits: &[FieldEdit],
) -> Vec<u8> {
    for e in edits {
        let i = e.rec % starts.len();
        let (at, size) = (starts[i] + e.field, sizes[i] as u32);
        let v = match e.pick % 4 {
            0 => e.value,
            1 => size,
            2 => size.wrapping_sub(8),
            _ => size.wrapping_add(8),
        };
        if at + 4 <= bytes.len() {
            bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
    bytes
}

/// Pulls the records after record `i` eight bytes closer and shortens record `i`'s
/// `NextEntryOffset` to match, so its declared name now reaches into the next record
/// (a legal-looking chain whose records overlap). `i` must not be the last record.
pub fn overlap_after(mut bytes: Vec<u8>, starts: &[usize], sizes: &[usize], i: usize) -> Vec<u8> {
    let (start, size) = (starts[i], sizes[i]);
    bytes.drain(start + size - 8..start + size);
    bytes[start..start + 4].copy_from_slice(&((size - 8) as u32).to_le_bytes());
    bytes
}
