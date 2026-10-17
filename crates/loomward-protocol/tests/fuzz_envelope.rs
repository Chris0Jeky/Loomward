//! Property fuzzing of the protocol ingress and its wire primitives (docs/45 row 6, #414):
//! `RequestEnvelope::from_slice` (the one entry point every adapter, HTTP and Tauri `lw_call`,
//! decodes through) and the string newtypes that carry cursors and idempotency keys.
//! Seeds: `contracts/v3/examples` (every command request, the envelope examples, the invalid ones).
//! Properties: never panics on arbitrary bytes or arbitrary JSON; every valid example is accepted and
//! re-serialises to an equal request; a mutated example is refused with one of the three typed
//! ingress codes or accepted as exactly the JSON it was given (no silent coercion, invariant 4).
#[path = "../../../fixtures/fuzz/support.rs"]
mod support;

use loomward_protocol::*;
use proptest::prelude::*;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::PathBuf;
use support::{apply, config, edits, few_edits};

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/v3/examples")
}

fn read(path: PathBuf) -> Value {
    serde_json::from_slice(&fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .unwrap()
}

/// One valid request envelope per command (payload = its request example), plus the envelope examples.
fn seeds() -> &'static [Value] {
    static S: std::sync::OnceLock<Vec<Value>> = std::sync::OnceLock::new();
    S.get_or_init(load_seeds)
}

fn load_seeds() -> Vec<Value> {
    let mut out = Vec::new();
    for c in Command::ALL {
        let payload = read(examples().join(format!("commands/{}.request.json", c.as_str())));
        out.push(json!({"protocol": "loomward/3", "request_id": "r_fuzz", "command": c.as_str(), "payload": payload}));
    }
    for name in [
        "request",
        "request-minimal",
        "request-declare-tier-precondition",
    ] {
        out.push(read(examples().join(format!("envelopes/{name}.json")))["value"].clone());
    }
    out
}

fn invalid_requests() -> Vec<Value> {
    let mut out = Vec::new();
    for e in fs::read_dir(examples().join("invalid")).unwrap() {
        let doc = read(e.unwrap().path());
        if doc["def"] == "RequestEnvelope" {
            out.push(doc["value"].clone());
        }
    }
    out
}

fn bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}

/// Numbers compare by value (`1` and `1.0` agree), as `decode_exact` does.
fn norm(v: &Value) -> Value {
    match v {
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 1e15 => json!(f as i64),
            _ => v.clone(),
        },
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), norm(v))).collect()),
        _ => v.clone(),
    }
}

fn typed_refusal(r: &Rejected) -> Result<(), TestCaseError> {
    prop_assert!(
        matches!(
            r.error.code,
            ErrorCode::InvalidRequest | ErrorCode::UnsupportedProtocol | ErrorCode::UnknownCommand
        ),
        "{:?}",
        r.error.code
    );
    if let Some(id) = &r.request_id {
        prop_assert!(RequestId::new(id.as_str()).is_ok());
    }
    Ok(())
}

/// Accepted means: valid, stable under a second pass, and the same JSON that came in.
fn accepted_is_exact(input: &Value, req: &RequestEnvelope) -> Result<(), TestCaseError> {
    prop_assert!(req.validate().is_ok());
    let out = serde_json::to_value(req).unwrap();
    prop_assert_eq!(norm(&out), norm(input));
    let again = RequestEnvelope::from_slice(&serde_json::to_vec(req).unwrap());
    prop_assert_eq!(again.as_ref().ok(), Some(req));
    Ok(())
}

fn count(v: &Value) -> usize {
    1 + match v {
        Value::Array(a) => a.iter().map(count).sum(),
        Value::Object(o) => o.values().map(count).sum(),
        _ => 0,
    }
}

/// Damages the `pick`-th node (pre-order) in place; returns whether it was reached.
fn damage_at(node: &mut Value, pick: &mut usize, how: u8, filler: &Value) -> bool {
    if *pick == 0 {
        match (how % 6, &mut *node) {
            (0, Value::Object(o)) => {
                o.insert("zz_unknown".into(), filler.clone());
            }
            (1, Value::Object(o)) if !o.is_empty() => {
                let k = o.keys().nth(how as usize % o.len()).unwrap().clone();
                o.remove(&k);
            }
            (2, Value::Object(o)) if !o.is_empty() => {
                let k = o.keys().nth(how as usize % o.len()).unwrap().clone();
                o.insert(k, filler.clone());
            }
            (3, Value::Array(a)) => a.push(filler.clone()),
            (4, Value::Array(a)) if !a.is_empty() => {
                a.remove(how as usize % a.len());
            }
            (_, n) => *n = filler.clone(),
        }
        return true;
    }
    *pick -= 1;
    match node {
        Value::Array(a) => a.iter_mut().any(|c| damage_at(c, pick, how, filler)),
        Value::Object(o) => o.values_mut().any(|c| damage_at(c, pick, how, filler)),
        _ => false,
    }
}

/// A structure-aware edit: pick a node by pre-order index and damage it.
fn damage(root: &mut Value, pick: usize, how: u8, filler: &Value) {
    let mut at = pick % count(root);
    damage_at(root, &mut at, how, filler);
}

fn filler() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        Just(json!(true)),
        Just(json!(0)),
        Just(json!(-1)),
        Just(json!(1.5)),
        Just(json!(1e300)),
        Just(json!(9007199254740993u64)),
        Just(json!(u64::MAX)),
        Just(json!("")),
        Just(json!("x".repeat(600))),
        Just(json!("\u{0}")),
        Just(json!("../..")),
        Just(json!([])),
        Just(json!([1, 2])),
        Just(json!({})),
        Just(json!({"a": {"b": {}}})),
        "[ -~]{0,20}".prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
    ]
}

proptest! {
    #![proptest_config(config(2500))]

    #[test]
    fn never_panics_on_arbitrary_bytes(raw in prop::collection::vec(any::<u8>(), 0..400)) {
        if let Err(r) = RequestEnvelope::from_slice(&raw) { typed_refusal(&r)?; }
    }

    #[test]
    fn mutated_example_bytes_are_refused_or_exact(seed in 0usize..64, e in prop_oneof![edits(), few_edits(), few_edits()]) {
        let seeds = seeds();
        let input = &seeds[seed % seeds.len()];
        let mutated = apply(bytes(input), &e);
        match RequestEnvelope::from_slice(&mutated) {
            Err(r) => typed_refusal(&r)?,
            Ok(req) => {
                // Accepted bytes parse (leniently) to the very JSON the request serialises back to.
                let seen: Value = serde_json::from_slice(&mutated).expect("accepted bytes are JSON");
                accepted_is_exact(&seen, &req)?;
            }
        }
    }

    #[test]
    fn damaged_example_trees_are_refused_or_exact(seed in 0usize..64, pick in any::<usize>(), how in any::<u8>(), f in filler(), second in proptest::option::of((any::<usize>(), any::<u8>(), filler()))) {
        let seeds = seeds();
        let mut v = seeds[seed % seeds.len()].clone();
        damage(&mut v, pick, how, &f);
        if let Some((p, h, g)) = second { damage(&mut v, p, h, &g); }
        match RequestEnvelope::from_slice(&bytes(&v)) {
            Err(r) => typed_refusal(&r)?,
            Ok(req) => accepted_is_exact(&v, &req)?,
        }
    }

    /// The envelope's own fields, where the precondition and bound rules live.
    #[test]
    fn envelope_fields_are_refused_or_exact(
        seed in 0usize..64,
        id in prop_oneof![Just(json!("r_1")), Just(json!("")), Just(json!("a b")), Just(json!("x".repeat(65))), Just(json!(7)), Just(Value::Null)],
        protocol in prop_oneof![8 => Just(json!("loomward/3")), 1 => Just(json!("loomward/2")), 1 => Just(json!("loomward/3 ")), 1 => Just(Value::Null)],
        deadline in proptest::option::of(prop_oneof![Just(json!(0)), Just(json!(1)), Just(json!(60000)), Just(json!(60001)), Just(json!(-1)), Just(json!(1.5)), Just(json!(2000.0)), Just(json!("5")), Just(Value::Null)]),
        generation in proptest::option::of(prop_oneof![Just(json!("12")), Just(json!("0")), Just(json!("012")), Just(json!(12)), Just(Value::Null), Just(json!("18446744073709551615")), Just(json!("18446744073709551616"))]),
        state_rev in proptest::option::of(prop_oneof![Just(json!("3")), Just(json!("03")), Just(json!(3)), Just(Value::Null)]),
    ) {
        let seeds = seeds();
        let mut v = seeds[seed % seeds.len()].clone();
        let o = v.as_object_mut().unwrap();
        o.insert("request_id".into(), id);
        o.insert("protocol".into(), protocol);
        for (k, x) in [("deadline_ms", deadline), ("expected_generation", generation), ("expected_state_rev", state_rev)] {
            match x { Some(x) => { o.insert(k.into(), x); } None => { o.remove(k); } }
        }
        match RequestEnvelope::from_slice(&bytes(&v)) {
            Err(r) => typed_refusal(&r)?,
            Ok(req) => accepted_is_exact(&v, &req)?,
        }
    }

    /// serde would read a derived struct from a positional array; the contract forbids it.
    #[test]
    fn positional_arrays_are_refused(seed in 0usize..64, tail in prop::collection::vec(filler(), 0..3)) {
        let seeds = seeds();
        let o = seeds[seed % seeds.len()].as_object().unwrap();
        let mut a: Vec<Value> = ["protocol", "request_id", "command", "payload"].iter().map(|k| o[*k].clone()).collect();
        a.extend(tail);
        let r = RequestEnvelope::from_slice(&bytes(&Value::Array(a)));
        prop_assert!(r.is_err());
        typed_refusal(&r.unwrap_err())?;
    }

    #[test]
    fn arbitrary_json_is_never_accepted_as_something_else(v in any_json()) {
        match RequestEnvelope::from_slice(&bytes(&v)) {
            Err(r) => typed_refusal(&r)?,
            Ok(req) => accepted_is_exact(&v, &req)?,
        }
    }

    #[test]
    fn a_repeated_member_at_any_depth_is_refused(seed in 0usize..64, key in "[a-z_]{1,8}") {
        // Text-level: serde_json::Value cannot hold a duplicate, so splice one into the raw bytes.
        let seeds = seeds();
        let text = String::from_utf8(bytes(&seeds[seed % seeds.len()])).unwrap();
        for (at, _) in text.match_indices('{') {
            let spliced = format!("{}\"{key}\":0,\"{key}\":1,{}", &text[..=at], &text[at + 1..]);
            let spliced = spliced.replace(",}", "}");
            let r = RequestEnvelope::from_slice(spliced.as_bytes());
            prop_assert!(r.is_err(), "accepted a repeated member at byte {at}");
            typed_refusal(&r.unwrap_err())?;
        }
    }
}

fn any_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        "[ -~]{0,20}".prop_map(Value::from),
        Just(json!("loomward/3")),
        Just(json!("session.hello")),
    ];
    leaf.prop_recursive(3, 24, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::from),
            prop::collection::vec(
                (
                    prop_oneof![
                        Just("protocol".to_string()),
                        Just("request_id".to_string()),
                        Just("command".to_string()),
                        Just("payload".to_string()),
                        "[a-z_]{1,6}".prop_map(|s| s)
                    ],
                    inner
                ),
                0..6
            )
            .prop_map(|kv| Value::Object(kv.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

#[test]
fn every_seed_is_accepted_and_every_invalid_example_is_refused() {
    let seeds = seeds();
    assert!(seeds.len() > Command::ALL.len());
    for v in seeds {
        let req =
            RequestEnvelope::from_slice(&bytes(v)).unwrap_or_else(|r| panic!("{v}: {:?}", r.error));
        assert_eq!(norm(&serde_json::to_value(&req).unwrap()), norm(v));
    }
    let invalid = invalid_requests();
    assert!(!invalid.is_empty());
    for v in invalid {
        assert!(RequestEnvelope::from_slice(&bytes(&v)).is_err(), "{v}");
    }
}

// ---- wire primitives behind cursors and idempotency keys ----

fn slug(s: &str, lo: usize, hi: usize) -> bool {
    (lo..=hi).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn decimal(s: &str) -> Option<u64> {
    let ok = !s.is_empty()
        && s.len() <= 20
        && s.bytes().all(|b| b.is_ascii_digit())
        && (s == "0" || !s.starts_with('0'));
    ok.then(|| s.parse().ok()).flatten()
}

/// Strings near the accepting set: slug characters mostly, with the odd off-grammar byte.
fn nearly_slug() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[A-Za-z0-9_-]{0,70}",
        1 => "[A-Za-z0-9_-]{0,70}[ -~\u{80}-\u{2fff}][A-Za-z0-9_-]{0,5}",
        1 => "[0-9]{0,22}",
        1 => any::<String>(),
        1 => "(sha256:)?[0-9a-g]{60,66}",
    ]
}

macro_rules! accepts_exactly {
    ($ty:ty, $s:expr, $oracle:expr) => {{
        let s: &str = $s;
        let parsed = serde_json::from_value::<$ty>(Value::String(s.to_owned()));
        let via_new = <$ty>::new(s);
        prop_assert_eq!(parsed.is_ok(), $oracle, "{}: {:?}", stringify!($ty), s);
        prop_assert_eq!(via_new.is_ok(), $oracle);
        if let Ok(v) = parsed {
            // Accepted means unchanged: the wire string comes back byte for byte.
            prop_assert_eq!(
                serde_json::to_value(&v).unwrap(),
                Value::String(s.to_owned())
            );
        }
    }};
}

proptest! {
    #![proptest_config(config(6000))]

    #[test]
    fn cursor_and_key_newtypes_accept_exactly_their_grammar(s in nearly_slug()) {
        accepts_exactly!(Cursor, &s, slug(&s, 1, 512));
        accepts_exactly!(ClientEventId, &s, slug(&s, 8, 64));
        accepts_exactly!(RequestId, &s, slug(&s, 1, 64));
        accepts_exactly!(StreamEpoch, &s, slug(&s, 8, 64));
        accepts_exactly!(SnapshotId, &s, s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        accepts_exactly!(Digest, &s, s.strip_prefix("sha256:").is_some_and(|h| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))));
    }

    #[test]
    fn decimal_counters_are_exact_strings_never_numbers(s in nearly_slug(), n in any::<u64>()) {
        let want = decimal(&s);
        prop_assert_eq!(serde_json::from_value::<Generation>(Value::String(s.clone())).ok().map(|g| g.get()), want);
        prop_assert_eq!(serde_json::from_value::<Bytes>(Value::String(s.clone())).ok().map(|g| g.get()), want);
        // A JSON number is never a decimal-string counter, whatever its value.
        prop_assert!(serde_json::from_value::<Generation>(json!(n)).is_err());
        prop_assert!(serde_json::from_value::<Bytes>(json!(n)).is_err());
        // And a u64 round-trips through its string.
        let g: Generation = serde_json::from_value(Value::String(n.to_string())).unwrap();
        prop_assert_eq!(serde_json::to_value(g).unwrap(), Value::String(n.to_string()));
    }
}
