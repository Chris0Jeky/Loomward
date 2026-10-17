//! Property fuzzing of the backup receipt normaliser (the parse core of `backups.import`, the
//! snapshot importer) `normalise_backup_receipt` (docs/45 row 6, #414). Seeds: the synthetic
//! restic `snapshots --json` and `check --json` documents of `receipts.rs`.
//! Properties: never panics on arbitrary bytes or arbitrary source fields; valid receipts are
//! accepted with the snapshot they name; a mutated receipt or check summary is refused with a typed
//! error or accepted only when an independent reading of the bytes agrees (no silent coercion,
//! invariant 4); `reported_recovery_evidence` never holds without every predicate behind it.
#[path = "../../../fixtures/fuzz/support.rs"]
mod support;

use loomward_backup::*;
use proptest::prelude::*;
use serde_json::{json, Value};
use support::{apply, config, edits};

fn id(c: char) -> String {
    c.to_string().repeat(64)
}

fn file(path: &str) -> FileVersion {
    FileVersion {
        path: path.into(),
        version: id('a'),
    }
}

fn observation() -> Observation {
    Observation {
        repository_id: "synthetic-repository".into(),
        snapshot_id: id('b'),
        observed_at: 990,
        exit_code: 0,
    }
}

fn check() -> Vec<u8> {
    br#"{"message_type":"summary","num_errors":0,"broken_packs":[],"suggest_repair_index":false,"suggest_prune":false}"#.to_vec()
}

fn source() -> BackupSource {
    let files = vec![file("fixture/document.txt"), file("fixture/notes.txt")];
    BackupSource {
        repository_id: "synthetic-repository".into(),
        snapshot_id: id('b'),
        observed_at: 990,
        now: 1000,
        max_age_seconds: 60,
        topology: Topology::IndependentTarget,
        credentials_valid_until: Some(2000),
        required: files.clone(),
        snapshot_files: files.clone(),
        required_metadata: vec![MetadataClass::Acl],
        integrity: Some(IntegrityObservation {
            observation: observation(),
            raw: check(),
            read_all_data: true,
        }),
        restore: Some(RestoreObservation {
            observation: observation(),
            complete: true,
            files: files
                .into_iter()
                .map(|file| RestoredFile {
                    file,
                    compared_metadata: vec![MetadataClass::Acl],
                })
                .collect(),
        }),
    }
}

fn snapshots() -> Vec<u8> {
    serde_json::to_vec(
        &json!([{"id":id('b'),"time":"2026-01-01T00:00:00Z","paths":["/synthetic-fixture"],
        "tree":id('c'),"hostname":"synthetic-host","username":"synthetic-owner","uid":0,"gid":0,
        "summary":{"total_files_processed":2,"total_bytes_processed":32}}]),
    )
    .unwrap()
}

/// Every predicate the headline flag claims, re-derived from the evidence alone.
fn headline_has_its_predicates(
    e: &RecoveryEvidence,
    s: &BackupSource,
) -> Result<(), TestCaseError> {
    if e.reported_recovery_evidence {
        prop_assert!(e.refusals.is_empty());
        prop_assert!(e.snapshot_taken && e.integrity_checked && e.integrity_read_all_data);
        prop_assert!(e.restore_drill_succeeded);
        prop_assert_eq!(e.topology, Topology::IndependentTarget);
        prop_assert!(s.credentials_valid_until.is_some_and(|t| t > s.now));
        prop_assert_eq!(&e.restore_coverage, &s.required);
        for age in [
            e.observation_age_seconds,
            e.integrity_age_seconds,
            e.restore_age_seconds,
        ]
        .into_iter()
        .flatten()
        {
            prop_assert!(age <= s.max_age_seconds);
        }
    } else {
        prop_assert!(!e.refusals.is_empty());
    }
    prop_assert_eq!(e.snapshot_taken, e.snapshot_time.is_some());
    if !e.snapshot_taken {
        prop_assert!(e.refusals.contains(&Refusal::SnapshotMissing));
        prop_assert!(!e.integrity_checked && !e.restore_drill_succeeded);
    }
    Ok(())
}

/// Provider roots: plain segments (a `.` or `..` segment is refused by contract, tested apart).
fn paths_re() -> impl Strategy<Value = String> {
    r"([/\\][A-Za-z0-9_ -]{1,10}){1,4}"
}

fn label() -> impl Strategy<Value = String> {
    "[A-Za-z0-9:.+-]{1,64}"
}

fn snapshot_entry(id: String) -> impl Strategy<Value = Value> {
    (label(), prop::collection::vec(paths_re(), 1..4)).prop_map(move |(time, paths)| {
        json!({"id": id, "time": time, "paths": paths, "hostname": "synthetic-host", "uid": 0})
    })
}

/// A receipt whose bound snapshot sits among 0..3 unrelated ones: bytes and the time it names.
fn valid_receipt() -> impl Strategy<Value = (Vec<u8>, String)> {
    let digit = prop::sample::select(vec![
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'c', 'd', 'e', 'f',
    ]);
    (
        prop::collection::btree_set(digit, 0..3),
        0usize..4,
        label(),
        prop::collection::vec(paths_re(), 1..4),
    )
        .prop_flat_map(|(others, at, time, paths)| {
            let mut entries: Vec<_> = others
                .into_iter()
                .map(|c| snapshot_entry(id(c)).boxed())
                .collect();
            let at = at.min(entries.len());
            let bound = json!({"id": id('b'), "time": time, "paths": paths});
            entries.insert(at, Just(bound).boxed());
            entries.prop_map(move |v| (serde_json::to_vec(&v).unwrap(), time.clone()))
        })
}

/// An independent reading of a snapshots document: the time of the one entry bound to `b`.
fn bound_time(raw: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(raw).ok()?;
    let all = v.as_array()?;
    let hit = all
        .iter()
        .filter(|e| e.get("id").and_then(Value::as_str) == Some(&id('b')))
        .collect::<Vec<_>>();
    (hit.len() == 1).then_some(())?;
    hit[0].get("time")?.as_str().map(str::to_owned)
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        "[ -~]{0,70}".prop_map(Value::from),
        Just(Value::from(id('b'))),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(Value::from),
            prop::collection::vec(("id|time|paths|[a-z]{1,4}", inner), 0..5)
                .prop_map(|kv| Value::Object(kv.into_iter().collect())),
        ]
    })
}

proptest! {
    #![proptest_config(config(4000))]

    #[test]
    fn never_panics_on_arbitrary_bytes(raw in prop::collection::vec(any::<u8>(), 0..600)) {
        match normalise_backup_receipt(&raw, &source()) {
            Ok(e) => headline_has_its_predicates(&e, &source())?,
            Err(err) => prop_assert!(matches!(err, ReceiptError::InvalidJson | ReceiptError::InvalidReceipt)),
        }
    }

    #[test]
    fn arbitrary_json_never_panics_and_never_invents_a_snapshot(v in json_value()) {
        let raw = serde_json::to_vec(&v).unwrap();
        match normalise_backup_receipt(&raw, &source()) {
            Ok(e) => {
                headline_has_its_predicates(&e, &source())?;
                prop_assert_eq!(e.snapshot_time, bound_time(&raw));
            }
            Err(err) => prop_assert!(matches!(err, ReceiptError::InvalidJson | ReceiptError::InvalidReceipt)),
        }
    }

    #[test]
    fn valid_receipts_are_accepted_with_the_snapshot_they_name((raw, time) in valid_receipt()) {
        let e = normalise_backup_receipt(&raw, &source()).unwrap();
        prop_assert!(e.snapshot_taken);
        prop_assert_eq!(e.snapshot_time.as_deref(), Some(time.as_str()));
        prop_assert!(e.reported_recovery_evidence, "{:?}", e);
        // Pure: the same bytes give the same evidence.
        prop_assert_eq!(normalise_backup_receipt(&raw, &source()).unwrap(), e);
    }

    #[test]
    fn mutated_receipts_are_refused_or_read_the_same_way((raw, _) in valid_receipt(), e in edits()) {
        let mutated = apply(raw, &e);
        match normalise_backup_receipt(&mutated, &source()) {
            Ok(ev) => {
                headline_has_its_predicates(&ev, &source())?;
                // Accepted bytes mean what an independent reader says they mean.
                prop_assert_eq!(ev.snapshot_time, bound_time(&mutated));
            }
            Err(err) => prop_assert!(matches!(err, ReceiptError::InvalidJson | ReceiptError::InvalidReceipt)),
        }
    }

    /// A provider root with a traversal, empty or control part is refused on the bound snapshot only:
    /// one odd unrelated snapshot cannot hide the bound one, and the bound one cannot hide a bad path.
    #[test]
    fn hostile_provider_paths_are_refused_on_the_bound_snapshot_only(
        bad in prop::sample::select(vec![
            "..".to_string(), ".".to_string(), "/a/../b".to_string(), "a\\.\\b".to_string(),
            "/a/./b".to_string(), String::new(), "a\u{7}b".to_string(), "a\nb".to_string(), "x".repeat(1025),
        ]),
        good in paths_re(),
        bound_is_bad in any::<bool>(),
    ) {
        let entry = |id: String, path: &str| json!({"id": id, "time": "t", "paths": [path]});
        let (bound, other) = if bound_is_bad { (&bad, &good) } else { (&good, &bad) };
        let raw = serde_json::to_vec(&json!([entry(id('b'), bound), entry(id('c'), other)])).unwrap();
        let got = normalise_backup_receipt(&raw, &source());
        if bound_is_bad {
            prop_assert_eq!(got.unwrap_err(), ReceiptError::InvalidReceipt);
        } else {
            prop_assert!(got.unwrap().snapshot_taken);
        }
    }

    #[test]
    fn a_repeated_member_or_id_is_refused(extra in "[a-z]{1,6}") {
        let dup_key = format!(
            r#"[{{"id":"{}","time":"t","paths":["/p"],"{extra}":1,"{extra}":2}}]"#, id('b'));
        prop_assert_eq!(normalise_backup_receipt(dup_key.as_bytes(), &source()).unwrap_err(), ReceiptError::InvalidJson);
        let one = format!(r#"{{"id":"{}","time":"t","paths":["/p"]}}"#, id('b'));
        let dup_id = format!("[{one},{one}]");
        prop_assert_eq!(normalise_backup_receipt(dup_id.as_bytes(), &source()).unwrap_err(), ReceiptError::InvalidReceipt);
    }

    #[test]
    fn mutated_check_summaries_never_fake_an_integrity_pass(raw in prop::collection::vec(any::<u8>(), 0..200), e in edits(), seed in any::<bool>()) {
        let bytes = if seed { apply(check(), &e) } else { raw };
        let mut s = source();
        s.integrity.as_mut().unwrap().raw = bytes.clone();
        let ev = normalise_backup_receipt(&snapshots(), &s).unwrap();
        headline_has_its_predicates(&ev, &s)?;
        if ev.integrity_checked {
            let v: Value = serde_json::from_slice(&bytes).expect("an accepted summary is JSON");
            prop_assert_eq!(v["message_type"].as_str(), Some("summary"));
            prop_assert_eq!(v["num_errors"].as_i64(), Some(0));
            prop_assert!(v["suggest_prune"] != json!(true) && v["suggest_repair_index"] != json!(true));
            prop_assert!(v["broken_packs"].is_null() || v["broken_packs"].as_array().is_some_and(|a| a.is_empty()));
        }
    }
}

// ---- the supplied source side: bounded scalars and relative, device-free paths ----

fn hostile_text() -> impl Strategy<Value = String> {
    prop_oneof![
        "[ -~]{0,70}",
        Just("..".to_string()),
        Just("a/../b".to_string()),
        Just("C:\\x".to_string()),
        Just("a\\b".to_string()),
        Just("CON".to_string()),
        Just("con .txt".to_string()),
        Just("nul.txt".to_string()),
        Just("COM1".to_string()),
        Just("LPT\u{b2}".to_string()),
        Just("dir/file.".to_string()),
        Just("dir/ file".to_string()),
        Just("a\u{0}b".to_string()),
        Just("a:b".to_string()),
        Just("a/./b".to_string()),
        Just("/abs".to_string()),
        Just("x//y".to_string()),
        Just(id('a')),
        Just(String::new()),
        any::<String>(),
    ]
}

/// Mostly the valid baseline with a few hostile fields, so accepted and refused sources both occur.
fn hostile_source() -> impl Strategy<Value = BackupSource> {
    let some = |t: BoxedStrategy<u64>| prop::option::weighted(0.25, t);
    (
        prop::option::weighted(0.2, hostile_text()),
        prop::option::weighted(
            0.4,
            prop::collection::vec((hostile_text(), hostile_text()), 0..5),
        ),
        some(any::<u64>().boxed()),
        some(any::<u64>().boxed()),
        some(any::<u64>().boxed()),
        prop::option::weighted(0.4, prop::option::of(any::<u64>())),
        prop::option::weighted(
            0.4,
            prop_oneof![
                Just(Topology::IndependentTarget),
                Just(Topology::SameVolume),
                Just(Topology::Unknown)
            ],
        ),
    )
        .prop_map(|(repo, req, observed_at, now, max_age, creds, topology)| {
            let mut s = source();
            s.repository_id = repo.unwrap_or(s.repository_id);
            if let Some(req) = req {
                s.required = req
                    .into_iter()
                    .map(|(path, version)| FileVersion { path, version })
                    .collect();
            }
            s.observed_at = observed_at.unwrap_or(s.observed_at);
            s.now = now.unwrap_or(s.now);
            s.max_age_seconds = max_age.unwrap_or(s.max_age_seconds);
            s.credentials_valid_until = creds.unwrap_or(s.credentials_valid_until);
            s.topology = topology.unwrap_or(s.topology);
            s
        })
}

proptest! {
    #![proptest_config(config(4000))]

    #[test]
    fn hostile_sources_are_refused_or_clean(s in hostile_source()) {
        match normalise_backup_receipt(&snapshots(), &s) {
            Err(err) => prop_assert_eq!(err, ReceiptError::InvalidSource),
            Ok(ev) => {
                headline_has_its_predicates(&ev, &s)?;
                prop_assert!(!s.required.is_empty());
                for f in &s.required {
                    // No silent coercion of a hostile path or version into an accepted one.
                    prop_assert!(!f.path.is_empty() && f.path.len() <= 1024);
                    prop_assert!(!f.path.contains('\\') && !f.path.contains(':') && !f.path.starts_with('/'));
                    prop_assert!(f.path.split('/').all(|p| !p.is_empty() && p != "." && p != ".."));
                    prop_assert!(!f.path.chars().any(char::is_control));
                    prop_assert!(f.version.len() == 64 && f.version.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
                }
                prop_assert!(s.repository_id.len() <= 64 && repository_label(&s.repository_id));
                prop_assert!(s.now <= 253_402_300_799 && s.observed_at <= 253_402_300_799);
            }
        }
    }
}

#[test]
fn the_seed_documents_are_a_clean_baseline() {
    let e = normalise_backup_receipt(&snapshots(), &source()).unwrap();
    assert!(e.reported_recovery_evidence, "{e:?}");
}
