//! F1 (docs/46 section 4.1): the Rust parser against the fixture shared with the TypeScript
//! mirror (`app/tests/unit/grammar.test.ts` reads the same file).

use loomward_protocol::query_grammar::parse_query;
use serde_json::Value;
use std::path::Path;

fn fixture() -> Value {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/v3/query-grammar-parity.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn every_shared_case_parses_to_the_expected_json() {
    let doc = fixture();
    let cases = doc["cases"].as_array().unwrap();
    assert!(cases.len() >= 90, "the fixture is the contract");
    for c in cases {
        let input = c["input"].as_str().unwrap();
        let got = serde_json::to_value(parse_query(input)).unwrap();
        assert_eq!(got, c["expected"], "{}: {input:?}", c["name"]);
    }
}

#[test]
fn modified_bounds_resolve_relative_times_from_now() {
    let doc = fixture();
    let now = doc["now_ms"].as_i64().unwrap();
    for b in doc["bounds"].as_array().unwrap() {
        let input = b["input"].as_str().unwrap();
        let (after, before) = parse_query(input).modified_bounds(now);
        let show = |t: Option<loomward_protocol::Timestamp>| t.map(|t| t.as_str().to_string());
        assert_eq!(
            show(after).as_deref(),
            b["after"].as_str(),
            "{input}: after"
        );
        assert_eq!(
            show(before).as_deref(),
            b["before"].as_str(),
            "{input}: before"
        );
    }
}

/// Total: no prefix of any fixture input panics, and a parse never yields a term the wire
/// contract would refuse.
#[test]
fn every_prefix_parses_and_its_output_is_wire_valid() {
    let doc = fixture();
    for c in doc["cases"].as_array().unwrap() {
        let input: Vec<char> = c["input"].as_str().unwrap().chars().collect();
        for n in 0..=input.len() {
            let prefix: String = input[..n].iter().collect();
            let q = parse_query(&prefix);
            let v = serde_json::to_value(&q.terms).unwrap();
            let back: Vec<loomward_protocol::SearchTerm> = serde_json::from_value(v).unwrap();
            assert_eq!(back, q.terms);
            assert!(q.terms.len() <= 8 && q.extensions.len() <= 16);
        }
    }
}
