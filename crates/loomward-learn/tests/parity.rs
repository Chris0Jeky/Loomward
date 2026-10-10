//! Parity with the Python reference `python/loomward/learning.py`, through the fixtures exported by
//! `experiments/export_learning_fixtures.py` into `fixtures/learning-v3/`. Tokens and abstention reasons
//! must match exactly, floats within 1e-12 relative, and model IDs exactly.

use std::collections::BTreeSet;
use std::path::PathBuf;

use loomward_learn::features::{tokens, validate_features};
use loomward_learn::student::Student;
use serde_json::{json, Value};

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/learning-v3")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

/// Floats (JSON numbers) agree within 1e-12 relative; every other value must match exactly.
fn assert_close(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            let close =
                a == b || (a - b).abs() <= 1e-12 * a.abs().max(b.abs()) || (a - b).abs() <= 1e-300;
            assert!(close, "{path}: {a:?} != {b:?}");
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path}: array length");
            for (index, (x, y)) in a.iter().zip(b).enumerate() {
                assert_close(x, y, &format!("{path}[{index}]"));
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            let left: BTreeSet<&String> = a.keys().collect();
            let right: BTreeSet<&String> = b.keys().collect();
            assert_eq!(left, right, "{path}: object keys");
            for key in a.keys() {
                assert_close(&a[key], &b[key], &format!("{path}.{key}"));
            }
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

#[test]
fn token_sets_match_python_for_every_fixture_name() {
    let doc = fixture("tokens.json");
    let cases = doc["cases"].as_array().expect("cases");
    assert!(
        cases.len() >= 200,
        "fixture set has {} cases, brief requires 200",
        cases.len()
    );
    for case in cases {
        let id = case["id"].as_str().unwrap();
        let features = validate_features(&case["features"])
            .unwrap_or_else(|error| panic!("{id}: fixture features rejected: {error}"));
        let got: Vec<String> = tokens(&features).into_iter().collect();
        let want: Vec<String> = case["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(got, want, "{id}: tokens for {}", case["features"]);
    }
}

#[test]
fn student_fit_predict_and_serialisation_match_python() {
    let doc = fixture("student.json");
    for case in doc["cases"].as_array().expect("cases") {
        let id = case["id"].as_str().unwrap();
        let mut student = match (Student::new(&case["labels"]), case["labels_error"].as_str()) {
            (Err(error), Some(want)) => {
                assert_eq!(error.message(), want, "{id}: labels error");
                continue;
            }
            (Err(error), None) => panic!("{id}: unexpected labels error: {error}"),
            (Ok(_), Some(want)) => panic!("{id}: expected labels error {want:?}"),
            (Ok(student), None) => student,
        };

        match (
            student.fit(&case["input_events"]),
            case["fit_error"].as_str(),
        ) {
            (Ok(()), None) => {}
            (Err(error), Some(want)) => assert_eq!(error.message(), want, "{id}: fit error"),
            (Ok(()), Some(want)) => panic!("{id}: expected fit error {want:?}"),
            (Err(error), None) => panic!("{id}: unexpected fit error: {error}"),
        }

        if let Some(after) = case.get("after_fit") {
            let human: serde_json::Map<String, Value> = student
                .labels()
                .iter()
                .zip(student.human_support())
                .map(|(label, count)| (label.clone(), json!(count)))
                .collect();
            let mass: serde_json::Map<String, Value> = student
                .labels()
                .iter()
                .zip(student.mass())
                .map(|(label, value)| (label.clone(), json!(value)))
                .collect();
            let actual = json!({
                "labels": student.labels(),
                "model_id": student.model_id(),
                "training_count": student.training_count(),
                "human_support": human,
                "mass": mass,
                "vocabulary_size": student.vocabulary_len(),
                "stored_events": student.to_value()["events"].clone(),
            });
            assert_close(&actual, after, &format!("{id}.after_fit"));
        }

        for (index, prediction) in case["predictions"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let path = format!("{id}.predictions[{index}]");
            match (
                student.predict(&prediction["features"]),
                prediction.get("result"),
            ) {
                (Ok(result), Some(want)) => assert_close(&result.to_value(), want, &path),
                (Err(error), None) => {
                    assert_eq!(
                        error.message(),
                        prediction["error"].as_str().unwrap(),
                        "{path}"
                    )
                }
                (Ok(_), None) => panic!("{path}: expected {}", prediction["error"]),
                (Err(error), Some(_)) => panic!("{path}: unexpected error {error}"),
            }
        }

        if let Some(serialised) = case.get("serialised") {
            assert_close(&student.to_value(), serialised, &format!("{id}.serialised"));
            let restored = Student::from_value(serialised).expect("serialised model restores");
            assert_eq!(
                restored.model_id(),
                case["round_trip_model_id"].as_str().unwrap(),
                "{id}: round-trip model_id"
            );
        }
    }
}

#[test]
fn from_dict_matches_python_for_good_and_bad_documents() {
    let doc = fixture("student.json");
    for case in doc["from_dict"].as_array().expect("from_dict cases") {
        let id = case["id"].as_str().unwrap();
        let expect = &case["expect"];
        match Student::from_value(&case["input"]) {
            Ok(restored) => {
                assert!(expect.get("error").is_none(), "{id}: expected an error");
                assert_eq!(
                    restored.model_id(),
                    expect["model_id"].as_str().unwrap(),
                    "{id}"
                );
                assert_eq!(
                    restored.training_count() as u64,
                    expect["training_count"].as_u64().unwrap(),
                    "{id}"
                );
                assert_close(
                    &restored.to_value(),
                    &expect["serialised"],
                    &format!("{id}.serialised"),
                );
            }
            Err(error) => assert_eq!(error.message(), expect["error"].as_str().unwrap(), "{id}"),
        }
    }
}
