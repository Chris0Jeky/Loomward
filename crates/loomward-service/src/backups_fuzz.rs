//! Property fuzzing of the snapshot importer's file side (docs/45 row 6): `read_receipt` (the
//! owner-picked bytes) and `summary_document` (the `check --json` summary extraction), alone and
//! end to end through `backups.import` with a scripted file dialog. Synthetic bytes only.
//!
//! Properties: nothing panics; `read_receipt` returns exactly the file's bytes when it is at most
//! 256 KiB and `too_large` otherwise, never more than the budget, and refuses every arbitrary
//! non-existent or reserved name by a typed reason without hanging; `summary_document` returns a
//! trimmed single line of its input after which only whitespace follows (or the whole input when
//! it is one JSON value); an import is either `imported` or `refused` with a documented reason,
//! stores nothing when refused, and claims an integrity check only for a clean restic summary.
#[path = "../../../fixtures/fuzz/support.rs"]
mod support;

use super::*;
use crate::backups::summary_document;
use loomward_backup::MAX_INPUT_BYTES;
use proptest::prelude::*;
use std::time::{Duration, Instant};
use support::{apply, config, edits};

/// Generous for a debug build; a read that blocks on a device or a network name blows it.
const BOUND: Duration = Duration::from_secs(10);

fn sized() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        4 => prop::collection::vec(any::<u8>(), 0..512),
        1 => (prop::sample::select(vec![
            MAX_INPUT_BYTES - 1,
            MAX_INPUT_BYTES,
            MAX_INPUT_BYTES + 1,
            MAX_INPUT_BYTES + 4096,
            4 * MAX_INPUT_BYTES,
        ]), any::<u8>())
            .prop_map(|(n, b)| vec![b; n]),
    ]
}

proptest! {
    #![proptest_config(config(300))]

    #[test]
    fn the_reader_returns_exactly_the_bytes_or_too_large(bytes in sized()) {
        let tmp = temp_root();
        let path = write(tmp.path(), "receipt.json", &bytes);
        let started = Instant::now();
        let got = read_receipt(&path);
        prop_assert!(started.elapsed() < BOUND);
        if bytes.len() <= MAX_INPUT_BYTES {
            prop_assert_eq!(got.map(|(b, _)| b), Ok(bytes));
        } else {
            prop_assert_eq!(got.unwrap_err(), BackupImportRefusal::TooLarge);
        }
    }
}

proptest! {
    #![proptest_config(config(2000))]

    #[test]
    fn arbitrary_names_are_refused_by_a_typed_reason(
        name in prop_oneof![
            "[A-Za-z0-9 .:_~$-]{1,16}",
            prop::sample::select(vec!["CON", "con.json", "NUL", "AUX.txt", "COM1", "LPT9.json", "CONIN$",
                "CONOUT$", "receipt.json:stream", "receipt.json::$DATA", ".", "..", " ", "a.", "a "])
                .prop_map(String::from),
        ],
    ) {
        let tmp = temp_root();
        let started = Instant::now();
        // Nothing is written: every pick names a missing file, a device or a stream.
        let got = read_receipt(&tmp.path().join(&name));
        prop_assert!(started.elapsed() < BOUND);
        prop_assert!(
            matches!(got, Err(BackupImportRefusal::NotARegularFile | BackupImportRefusal::Unreadable)),
            "{:?} -> {:?}", name, got.map(|(b, t)| (b.len(), t))
        );
    }

    #[test]
    fn the_summary_is_the_input_or_its_last_non_empty_line(
        raw in prop_oneof![
            prop::collection::vec(any::<u8>(), 0..256),
            (edits(), 0usize..3).prop_map(|(e, lines)| {
                let mut doc = Vec::new();
                for _ in 0..lines {
                    doc.extend_from_slice(b"{\"message_type\":\"status\",\"percent_done\":0.5}\r\n\n");
                }
                doc.extend_from_slice(CLEAN_CHECK.as_bytes());
                apply(doc, &e)
            }),
        ],
    ) {
        let got = summary_document(&raw);
        if serde_json::from_slice::<Value>(&raw).is_ok() {
            prop_assert_eq!(got, &raw[..]);
        } else if got.is_empty() {
            prop_assert!(raw.split(|b| *b == b'\n').all(|l| l.trim_ascii().is_empty()));
        } else {
            // A borrowed subslice of the input: nothing is copied or invented.
            let start = (got.as_ptr() as usize).wrapping_sub(raw.as_ptr() as usize);
            prop_assert!(start <= raw.len() && got.len() <= raw.len() - start);
            prop_assert!(!got.contains(&b'\n'));
            prop_assert_eq!(got, got.trim_ascii());
            prop_assert!(raw[start + got.len()..].iter().all(u8::is_ascii_whitespace));
            prop_assert!(raw[..start].iter().rev().take_while(|b| **b != b'\n').all(u8::is_ascii_whitespace));
        }
    }
}

fn check_strategy() -> impl Strategy<Value = Option<Vec<u8>>> {
    proptest::option::of(prop_oneof![
        2 => edits().prop_map(|e| apply(CLEAN_CHECK.as_bytes().to_vec(), &e)),
        1 => sized(),
        1 => Just(format!("{{\"message_type\":\"status\"}}\n{CLEAN_CHECK}\n").into_bytes()),
    ])
}

fn snapshot_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        3 => edits().prop_map(|e| apply(snapshots().into_bytes(), &e)),
        1 => sized(),
        1 => Just(snapshots().into_bytes()),
    ]
}

/// A clean restic check summary, read independently of the normaliser.
fn clean_summary(raw: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(summary_document(raw)) else {
        return false;
    };
    v["message_type"] == "summary"
        && v["num_errors"] == 0
        && v.get("broken_packs")
            .is_none_or(|p| p.is_null() || p.as_array().is_some_and(Vec::is_empty))
        && v.get("suggest_repair_index").is_none_or(|b| *b == false)
        && v.get("suggest_prune").is_none_or(|b| *b == false)
}

#[test]
fn imports_of_arbitrary_receipts_are_imported_or_refused_by_name() {
    use proptest::test_runner::TestRunner;
    let tmp = temp_root();
    let svc = personal(tmp.path());
    let mut runner = TestRunner::new(config(300));
    let seen: [std::cell::Cell<usize>; 3] = Default::default();
    runner
        .run(
            &(snapshot_strategy(), check_strategy()),
            |(snaps, check)| {
                rearm(&svc);
                // Ages are recomputed on every read; the stored records are what must not change.
                let records = || {
                    list(&svc)
                        .iter()
                        .map(|e| (e["evidence_id"].clone(), e["imported_at"].clone()))
                        .collect::<Vec<_>>()
                };
                let before = records();
                let snaps_path = write(tmp.path(), "snapshots.json", &snaps);
                let mut picks = vec![Some(snaps_path)];
                if let Some(check) = &check {
                    picks.push(Some(write(tmp.path(), "check.json", check)));
                }
                let files = Files::new(picks);
                let started = Instant::now();
                let r = import(
                    &svc,
                    &files,
                    payload("synthetic-nas", None, None, check.is_some()),
                );
                prop_assert!(started.elapsed() < BOUND);
                let (result, _) = ok("backups.import", r);
                match result["outcome"].as_str() {
                    Some("refused") => {
                        seen[0].set(seen[0].get() + 1);
                        let why: BackupImportRefusal =
                            serde_json::from_value(result["refusal"].clone()).unwrap();
                        prop_assert!(!matches!(
                            why,
                            BackupImportRefusal::SyntheticSession
                                | BackupImportRefusal::NotARegularFile
                        ));
                        prop_assert_eq!(&result["evidence"], &Value::Null);
                        prop_assert_eq!(records(), before, "a refusal stored evidence");
                        let oversized = snaps.len() > MAX_INPUT_BYTES
                            || check.as_ref().is_some_and(|c| c.len() > MAX_INPUT_BYTES);
                        prop_assert_eq!(why == BackupImportRefusal::TooLarge, oversized);
                    }
                    Some("imported") => {
                        seen[1].set(seen[1].get() + 1);
                        prop_assert!(snaps.len() <= MAX_INPUT_BYTES);
                        prop_assert!(serde_json::from_slice::<Value>(&snaps).is_ok());
                        let e = &result["evidence"];
                        if e["integrity_checked"] == true {
                            seen[2].set(seen[2].get() + 1);
                            prop_assert!(check.as_deref().is_some_and(clean_summary), "{:?}", e);
                        }
                        prop_assert_eq!(&e["integrity_read_all_data"], &json!(false));
                    }
                    other => prop_assert!(false, "outcome {:?}", other),
                }
                Ok(())
            },
        )
        .unwrap();
    // The generator must reach every branch, or the properties above prove nothing.
    let [refused, imported, checked] = seen.map(|c| c.get());
    assert!(
        refused > 0 && imported > 0 && checked > 0,
        "refused {refused}, imported {imported}, checked {checked}"
    );
}
