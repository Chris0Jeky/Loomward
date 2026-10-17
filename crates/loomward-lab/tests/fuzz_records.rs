//! Property fuzzing of the lab's checked directory-buffer decoder `decode_records` (docs/45 row 6,
//! #414). Never panics; a valid FILE_ID_EXTD_DIR_INFO chain round-trips; a mutated chain is refused
//! with `InvalidInput` or accepted with records that re-encode and decode to the same records.
#[path = "../../../fixtures/fuzz/support.rs"]
mod support;

use loomward_lab::{decode_records, DirectoryRecord};
use proptest::prelude::*;
use std::io::ErrorKind;
use support::{apply, apply_fields, config, edits, field_edit, overlap_after};

fn encode(rows: &[DirectoryRecord]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let start = out.len();
        let size = (88 + r.name.len() * 2).div_ceil(8) * 8;
        out.resize(start + size, 0);
        let b = &mut out[start..];
        let next = if i + 1 == rows.len() { 0 } else { size as u32 };
        b[0..4].copy_from_slice(&next.to_le_bytes());
        b[40..48].copy_from_slice(&r.logical_bytes.to_le_bytes());
        b[48..56].copy_from_slice(&r.allocation_bytes.to_le_bytes());
        b[56..60].copy_from_slice(&r.attributes.to_le_bytes());
        b[60..64].copy_from_slice(&((r.name.len() * 2) as u32).to_le_bytes());
        b[68..72].copy_from_slice(&r.reparse_tag.to_le_bytes());
        b[72..88].copy_from_slice(&r.file_id);
        for (k, c) in r.name.iter().enumerate() {
            b[88 + k * 2..90 + k * 2].copy_from_slice(&c.to_le_bytes());
        }
    }
    out
}

fn layout(rows: &[DirectoryRecord]) -> (Vec<usize>, Vec<usize>) {
    let sizes: Vec<usize> = rows
        .iter()
        .map(|r| (88 + r.name.len() * 2).div_ceil(8) * 8)
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

fn le32(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize
}

/// An independent reading of the rules: the record spans first (`NextEntryOffset` a multiple of 8,
/// at least the record's own header plus name, strictly inside the buffer; the last runs to the
/// end), then each span's fields. `Err` when any record is bad.
fn reference(buf: &[u8]) -> Result<Vec<DirectoryRecord>, ()> {
    let mut spans = Vec::new();
    let mut at = 0usize;
    loop {
        let rest = buf.get(at..).ok_or(())?;
        if rest.len() < 88 {
            return Err(());
        }
        let next = le32(rest, 0);
        let own = 88 + le32(rest, 60);
        if next == 0 {
            spans.push((at, buf.len()));
            break;
        }
        if next % 8 != 0 || next < own || next >= rest.len() {
            return Err(());
        }
        spans.push((at, at + next));
        at += next;
    }
    let mut out = Vec::new();
    for (start, end) in spans {
        let b = &buf[start..end];
        let name_len = le32(b, 60);
        let u64_at = |n: usize| u64::from_le_bytes(b[n..n + 8].try_into().unwrap());
        if name_len == 0
            || name_len % 2 != 0
            || 88 + name_len > b.len()
            || u64_at(40) > i64::MAX as u64
            || u64_at(48) > i64::MAX as u64
        {
            return Err(());
        }
        let name: Vec<u16> = (0..name_len / 2)
            .map(|k| u16::from_le_bytes([b[88 + 2 * k], b[89 + 2 * k]]))
            .collect();
        if name.iter().any(|&c| matches!(c, 0 | 47 | 58 | 92)) {
            return Err(());
        }
        out.push(DirectoryRecord {
            name,
            logical_bytes: u64_at(40),
            allocation_bytes: u64_at(48),
            attributes: le32(b, 56) as u32,
            reparse_tag: le32(b, 68) as u32,
            file_id: b[72..88].try_into().unwrap(),
        });
    }
    Ok(out)
}

fn decode(bytes: &[u8]) -> Result<Vec<DirectoryRecord>, ErrorKind> {
    let mut rows = Vec::new();
    decode_records(bytes, |r| {
        rows.push(r);
        Ok(())
    })
    .map_err(|e| e.kind())?;
    Ok(rows)
}

fn record() -> impl Strategy<Value = DirectoryRecord> {
    (
        prop::collection::vec(
            any::<u16>().prop_filter("plain", |c| !matches!(*c, 0 | 47 | 58 | 92)),
            1..24,
        ),
        0u64..=i64::MAX as u64,
        0u64..=i64::MAX as u64,
        any::<u32>(),
        any::<u32>(),
        any::<[u8; 16]>(),
    )
        .prop_map(
            |(name, logical_bytes, allocation_bytes, attributes, reparse_tag, file_id)| {
                DirectoryRecord {
                    name,
                    logical_bytes,
                    allocation_bytes,
                    attributes,
                    reparse_tag,
                    file_id,
                }
            },
        )
}

fn chain() -> impl Strategy<Value = Vec<DirectoryRecord>> {
    prop::collection::vec(record(), 1..6)
}

proptest! {
    #![proptest_config(config(3000))]

    #[test]
    fn never_panics_on_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..700)) {
        let _ = decode(&bytes);
    }

    #[test]
    fn valid_chains_round_trip(rows in chain()) {
        prop_assert_eq!(decode(&encode(&rows)), Ok(rows));
    }

    /// Records that overlap (a name reaching into the next record) are refused, never read.
    #[test]
    fn overlapping_records_are_refused(rows in prop::collection::vec(record(), 2..6), at in any::<usize>()) {
        let (starts, sizes) = layout(&rows);
        let bytes = overlap_after(encode(&rows), &starts, &sizes, at % (rows.len() - 1));
        prop_assert!(decode(&bytes).is_err());
    }

    /// Header fields pushed to, just inside and just past their bounds, then compared with the reference.
    #[test]
    fn field_edited_chains_agree_with_the_reference(
        rows in chain(),
        fe in prop::collection::vec(field_edit(vec![0, 40, 48, 60, 56, 68]), 1..3),
        e in proptest::option::weighted(0.3, edits()),
    ) {
        let (starts, sizes) = layout(&rows);
        let mut bytes = apply_fields(encode(&rows), &starts, &sizes, &fe);
        if let Some(e) = e {
            bytes = apply(bytes, &e);
        }
        prop_assert_eq!(decode(&bytes).map_err(|_| ()), reference(&bytes));
    }

    #[test]
    fn mutated_chains_are_refused_or_semantically_identical(rows in chain(), e in edits()) {
        let mutated = apply(encode(&rows), &e);
        prop_assert_eq!(decode(&mutated).map_err(|_| ()), reference(&mutated));
        match decode(&mutated) {
            Err(kind) => prop_assert_eq!(kind, ErrorKind::InvalidInput),
            Ok(seen) => {
                prop_assert!(!seen.is_empty());
                for r in &seen {
                    prop_assert!(!r.name.is_empty());
                    prop_assert!(!r.name.iter().any(|c| matches!(*c, 0 | 47 | 58 | 92)));
                    prop_assert!(r.logical_bytes <= i64::MAX as u64 && r.allocation_bytes <= i64::MAX as u64);
                }
                prop_assert_eq!(decode(&encode(&seen)), Ok(seen));
            }
        }
    }

    /// A failing consumer stops the walk and its error is reported, never swallowed.
    #[test]
    fn emit_errors_stop_the_walk(rows in chain(), stop_at in 0usize..6) {
        let mut seen = 0usize;
        let result = decode_records(&encode(&rows), |_| {
            seen += 1;
            if seen > stop_at { Err(std::io::Error::other("stop")) } else { Ok(()) }
        });
        if stop_at < rows.len() {
            prop_assert!(result.is_err());
            prop_assert_eq!(seen, stop_at + 1);
        } else {
            prop_assert!(result.is_ok());
            prop_assert_eq!(seen, rows.len());
        }
    }
}
