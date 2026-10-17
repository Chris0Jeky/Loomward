//! Property fuzzing of the two native buffer decoders (docs/45 row 6, #414): the directory-buffer
//! decoder `enumerate::decode_buffer` and the change-notification decoder `watch::decode`.
//! Properties: never panics on arbitrary bytes; a valid chain round-trips exactly; a mutated valid
//! chain is refused with a typed error or accepted with entries that re-encode and decode to the
//! same entries (no silent coercion, invariant 4). `LOOMWARD_FUZZ_CASES` sets the case count.
#[path = "../../../fixtures/fuzz/support.rs"]
mod support;

use loomward_windows::enumerate::{decode_buffer, FileIdObs, Flow, IncompleteReason, Strategy};
use loomward_windows::watch::{self, Change};
use proptest::prelude::*;
use proptest::strategy::Strategy as _;
use support::{apply, apply_fields, config, edits, field_edit, overlap_after};

#[derive(Debug, Clone, PartialEq)]
struct Row {
    name: Vec<u16>,
    attributes: u32,
    tag: u32,
    eof: i64,
    alloc: i64,
    times: [i64; 4],
    id: [u8; 16],
}

/// What the sink saw: everything `RawEntry` carries, owned.
type Seen = (
    Vec<u16>,
    Option<FileIdObs>,
    u32,
    Option<u32>,
    u64,
    Option<u64>,
    [Option<i64>; 4],
);

fn header_len(strategy: Strategy) -> usize {
    if strategy == Strategy::Extended {
        88
    } else {
        104
    }
}

fn encode(rows: &[Row], strategy: Strategy) -> Vec<u8> {
    let header = header_len(strategy);
    let mut out = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let start = out.len();
        let size = (header + r.name.len() * 2).div_ceil(8) * 8;
        out.resize(start + size, 0);
        let b = &mut out[start..];
        let last = i + 1 == rows.len();
        let next = if last { 0 } else { size as u32 };
        b[0..4].copy_from_slice(&next.to_le_bytes());
        for (k, t) in r.times.iter().enumerate() {
            b[8 + k * 8..16 + k * 8].copy_from_slice(&t.to_le_bytes());
        }
        b[40..48].copy_from_slice(&r.eof.to_le_bytes());
        b[48..56].copy_from_slice(&r.alloc.to_le_bytes());
        b[56..60].copy_from_slice(&r.attributes.to_le_bytes());
        b[60..64].copy_from_slice(&((r.name.len() * 2) as u32).to_le_bytes());
        if strategy == Strategy::Extended {
            b[68..72].copy_from_slice(&r.tag.to_le_bytes());
            b[72..88].copy_from_slice(&r.id);
        } else {
            b[96..104].copy_from_slice(&r.id[..8]);
        }
        for (k, c) in r.name.iter().enumerate() {
            b[header + k * 2..header + k * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }
    }
    out
}

/// Offsets and natural sizes of the records `encode` writes.
fn layout(rows: &[Row], strategy: Strategy) -> (Vec<usize>, Vec<usize>) {
    let header = header_len(strategy);
    let sizes: Vec<usize> = rows
        .iter()
        .map(|r| (header + r.name.len() * 2).div_ceil(8) * 8)
        .collect();
    let starts = sizes
        .iter()
        .scan(0, |at, s| {
            let start = *at;
            *at += s;
            Some(start)
        })
        .collect();
    (starts, sizes)
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// An independent reading of the layout rules, as two plain passes: the record spans first (each
/// `NextEntryOffset` a multiple of 8, at least a header, strictly inside the buffer; the last
/// record runs to the end), then each span's own fields. `Err` when any record is bad.
fn reference(buf: &[u8], strategy: Strategy) -> Result<Vec<Seen>, ()> {
    let header = header_len(strategy);
    let mut spans = Vec::new();
    let mut at = 0usize;
    loop {
        let rest = buf.get(at..).ok_or(())?;
        if rest.len() < header {
            return Err(());
        }
        let next = le32(rest, 0) as usize;
        if next == 0 {
            spans.push((at, buf.len()));
            break;
        }
        if next % 8 != 0 || next < header || next >= rest.len() {
            return Err(());
        }
        spans.push((at, at + next));
        at += next;
    }
    let mut out = Vec::new();
    for (start, end) in spans {
        let b = &buf[start..end];
        let name_len = le32(b, 60) as usize;
        if name_len == 0 || name_len % 2 != 0 || header + name_len > b.len() {
            return Err(());
        }
        let name: Vec<u16> = (0..name_len / 2)
            .map(|k| u16::from_le_bytes([b[header + 2 * k], b[header + 2 * k + 1]]))
            .collect();
        let i64_at = |n: usize| i64::from_le_bytes(b[n..n + 8].try_into().unwrap());
        if i64_at(40) < 0 || i64_at(48) < 0 || name.contains(&0) {
            return Err(());
        }
        if name == [46] || name == [46, 46] {
            continue;
        }
        let id = if strategy == Strategy::Extended {
            FileIdObs::Id128(b[72..88].try_into().unwrap())
        } else {
            FileIdObs::Id64(u64::from_le_bytes(b[96..104].try_into().unwrap()))
        };
        let attributes = le32(b, 56);
        out.push((
            name,
            id.nonzero(),
            attributes,
            (strategy == Strategy::Extended && attributes & 0x400 != 0).then(|| le32(b, 68)),
            i64_at(40) as u64,
            Some(i64_at(48) as u64),
            [
                Some(i64_at(8)),
                Some(i64_at(16)),
                Some(i64_at(24)),
                Some(i64_at(32)),
            ],
        ));
    }
    Ok(out)
}

fn decode(bytes: &[u8], strategy: Strategy) -> Result<Vec<Seen>, IncompleteReason> {
    let mut seen = Vec::new();
    decode_buffer(bytes, strategy, &mut |e| {
        seen.push((
            e.name.to_vec(),
            e.file_id,
            e.attributes,
            e.reparse_tag,
            e.end_of_file,
            e.allocation_size,
            [e.creation, e.last_access, e.last_write, e.change],
        ));
        Flow::Continue
    })?;
    Ok(seen)
}

/// The rows a decoder must report for `rows`: dot entries are skipped, nothing else.
fn expected(rows: &[Row], strategy: Strategy) -> Vec<Seen> {
    rows.iter()
        .filter(|r| r.name != [46] && r.name != [46, 46])
        .map(|r| {
            let id = if strategy == Strategy::Extended {
                FileIdObs::Id128(r.id)
            } else {
                FileIdObs::Id64(u64::from_le_bytes(r.id[..8].try_into().unwrap()))
            };
            (
                r.name.clone(),
                id.nonzero(),
                r.attributes,
                (strategy == Strategy::Extended && r.attributes & 0x400 != 0).then_some(r.tag),
                r.eof as u64,
                Some(r.alloc as u64),
                [
                    Some(r.times[0]),
                    Some(r.times[1]),
                    Some(r.times[2]),
                    Some(r.times[3]),
                ],
            )
        })
        .collect()
}

/// Re-encodes what a decoder accepted (a canonical chain with the same observable entries).
fn rows_of(seen: &[Seen]) -> Vec<Row> {
    seen.iter()
        .map(|s| {
            let mut id = [0u8; 16];
            match s.1 {
                Some(FileIdObs::Id128(v)) => id = v,
                Some(FileIdObs::Id64(v)) => id[..8].copy_from_slice(&v.to_le_bytes()),
                None => {}
            }
            Row {
                name: s.0.clone(),
                attributes: s.2,
                tag: s.3.unwrap_or(0),
                eof: s.4 as i64,
                alloc: s.5.unwrap() as i64,
                times: [
                    s.6[0].unwrap(),
                    s.6[1].unwrap(),
                    s.6[2].unwrap(),
                    s.6[3].unwrap(),
                ],
                id,
            }
        })
        .collect()
}

fn name() -> impl proptest::strategy::Strategy<Value = Vec<u16>> {
    prop_oneof![
        1 => Just(vec![46u16]),
        1 => Just(vec![46u16, 46]),
        8 => prop::collection::vec(1u16..=u16::MAX, 1..24),
    ]
}

fn row(strategy: Strategy) -> impl proptest::strategy::Strategy<Value = Row> {
    (
        name(),
        any::<u32>(),
        any::<u32>(),
        0i64..=i64::MAX,
        0i64..=i64::MAX,
        any::<[i64; 4]>(),
        any::<[u8; 16]>(),
    )
        .prop_map(move |(name, attributes, tag, eof, alloc, times, mut id)| {
            if strategy == Strategy::Both {
                id[8..].fill(0);
            }
            Row {
                name,
                attributes,
                tag,
                eof,
                alloc,
                times,
                id,
            }
        })
}

fn chain(strategy: Strategy) -> impl proptest::strategy::Strategy<Value = Vec<Row>> {
    prop::collection::vec(row(strategy), 1..6)
}

/// A chain for either buffer class.
fn class() -> impl proptest::strategy::Strategy<Value = (Strategy, Vec<Row>)> {
    prop_oneof![
        chain(Strategy::Extended).prop_map(|r| (Strategy::Extended, r)),
        chain(Strategy::Both).prop_map(|r| (Strategy::Both, r)),
    ]
}

proptest! {
    #![proptest_config(config(3000))]

    /// Records that overlap (a name reaching into the next record) are refused, never read.
    #[test]
    fn overlapping_records_are_refused((strategy, rows) in class(), at in any::<usize>()) {
        if rows.len() < 2 {
            return Ok(());
        }
        let (starts, sizes) = layout(&rows, strategy);
        let bytes = overlap_after(encode(&rows, strategy), &starts, &sizes, at % (rows.len() - 1));
        prop_assert!(decode(&bytes, strategy).is_err());
    }

    /// Header fields pushed to, just inside and just past their bounds, then compared with the reference.
    #[test]
    fn field_edited_chains_agree_with_the_reference(
        (strategy, rows) in class(),
        fe in prop::collection::vec(field_edit(vec![0, 40, 48, 60, 56, 68]), 1..3),
        e in proptest::option::weighted(0.3, edits()),
    ) {
        let (starts, sizes) = layout(&rows, strategy);
        let mut bytes = apply_fields(encode(&rows, strategy), &starts, &sizes, &fe);
        if let Some(e) = e {
            bytes = apply(bytes, &e);
        }
        prop_assert_eq!(decode(&bytes, strategy).map_err(|_| ()), reference(&bytes, strategy));
    }

    #[test]
    fn decode_buffer_never_panics_on_arbitrary_bytes(
        bytes in prop::collection::vec(any::<u8>(), 0..700),
        s in prop_oneof![Just(Strategy::Extended), Just(Strategy::Both), Just(Strategy::Find), Just(Strategy::Portable)],
    ) {
        let _ = decode(&bytes, s);
    }

    #[test]
    fn valid_chains_round_trip((strategy, rows) in class()) {
        let bytes = encode(&rows, strategy);
        prop_assert_eq!(decode(&bytes, strategy), Ok(expected(&rows, strategy)));
    }

    #[test]
    fn mutated_chains_are_refused_or_semantically_identical((strategy, rows) in class(), e in edits()) {
        let mutated = apply(encode(&rows, strategy), &e);
        prop_assert_eq!(decode(&mutated, strategy).map_err(|_| ()), reference(&mutated, strategy));
        match decode(&mutated, strategy) {
            Err(reason) => prop_assert_eq!(reason, IncompleteReason::Malformed),
            Ok(seen) => {
                for entry in &seen {
                    prop_assert!(!entry.0.is_empty() && !entry.0.contains(&0));
                    prop_assert!(entry.0 != [46] && entry.0 != [46, 46]);
                    prop_assert!(entry.4 <= i64::MAX as u64 && entry.5.unwrap() <= i64::MAX as u64);
                }
                // What was accepted means the same thing when written canonically and read back.
                if seen.is_empty() {
                    // Only dot entries survived; nothing to re-encode.
                } else {
                    let canonical = rows_of(&seen);
                    prop_assert_eq!(decode(&encode(&canonical, strategy), strategy), Ok(seen));
                }
            }
        }
    }
}

// ---- watch::decode: FILE_NOTIFY_INFORMATION chains ----

fn notify(entries: &[(u32, Vec<u16>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, (action, name)) in entries.iter().enumerate() {
        let start = out.len();
        let size = (12 + name.len() * 2).div_ceil(4) * 4;
        out.resize(start + size, 0);
        let last = i + 1 == entries.len();
        out[start..start + 4].copy_from_slice(&(if last { 0 } else { size as u32 }).to_le_bytes());
        out[start + 4..start + 8].copy_from_slice(&action.to_le_bytes());
        out[start + 8..start + 12].copy_from_slice(&((name.len() * 2) as u32).to_le_bytes());
        for (k, c) in name.iter().enumerate() {
            out[start + 12 + k * 2..start + 14 + k * 2].copy_from_slice(&c.to_le_bytes());
        }
    }
    out
}

fn segment() -> impl proptest::strategy::Strategy<Value = Vec<u16>> {
    // Plain segments only: no NUL, `/`, `:`, `\`, and never `.` or `..` (refused by contract).
    prop::collection::vec(
        any::<u16>().prop_filter("plain", |c| !matches!(*c, 0 | 47 | 58 | 92)),
        1..8,
    )
    .prop_filter("not a dot segment", |s| s != &[46] && s != &[46, 46])
}

fn relative_name() -> impl proptest::strategy::Strategy<Value = Vec<u16>> {
    prop::collection::vec(segment(), 1..4).prop_map(|parts| parts.join(&92u16))
}

proptest! {
    #![proptest_config(config(3000))]

    #[test]
    fn watch_decode_never_panics_on_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..300)) {
        let _ = watch::decode(&bytes);
    }

    #[test]
    fn watch_valid_chains_round_trip(entries in prop::collection::vec((1u32..=5, relative_name()), 1..6)) {
        let want: Vec<Change> = entries.iter().map(|(a, n)| Change::Name { action: *a, name: n.clone() }).collect();
        prop_assert_eq!(watch::decode(&notify(&entries)), Ok(want));
    }

    #[test]
    fn watch_mutated_chains_are_refused_or_names_stay_relative(
        entries in prop::collection::vec((1u32..=5, relative_name()), 1..6),
        e in edits(),
    ) {
        if let Ok(changes) = watch::decode(&apply(notify(&entries), &e)) {
            for c in changes {
                if let Change::Name { action, name } = c {
                    prop_assert!((1..=5).contains(&action));
                    // A hint is never an input path: no root, drive, stream, traversal or empty part.
                    prop_assert!(name.split(|c| *c == 92).all(|p| !p.is_empty() && p != [46] && p != [46, 46]
                        && !p.iter().any(|c| matches!(*c, 0 | 47 | 58))));
                }
            }
        }
    }
}
