//! Behaviour the fixture set cannot reach cheaply: batch limits, the vocabulary budget and its rollback,
//! and the byte-level JSON the model ID is hashed from. Expected strings come from CPython 3.14.

use loomward_learn::features::{size_bucket, word_runs};
use loomward_learn::student::{canonical_json, Student};
use serde_json::{json, Value};

fn event(id: usize, name: &str, context: &str) -> Value {
    json!({
        "event_id": format!("e{id}"),
        "item_id": format!("i{id}"),
        "label": "Documents",
        "source": "human",
        "revision": 1,
        "retracted": false,
        "features": {"name": name, "context": context},
    })
}

/// Four lowercase letters, a bijection from 0..26**4.
fn four_letters(index: usize) -> String {
    (0..4)
        .rev()
        .map(|power| char::from(b'a' + ((index / 26usize.pow(power)) % 26) as u8))
        .collect()
}

fn words(start: usize, count: usize) -> String {
    (start..start + count)
        .map(four_letters)
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn canonical_json_escapes_like_python_ensure_ascii() {
    let value = json!({"b": "e\u{e9}\u{7f}\n\"", "a": "\u{1F600}", "c": "\u{1}\t\\"});
    let bs = '\\';
    // CPython output for this value is pinned in the expected string below.
    let expected = format!(
        "{{\"a\":\"{bs}ud83d{bs}ude00\",\"b\":\"e{bs}u00e9{bs}u007f{bs}n{bs}\"\",\"c\":\"{bs}u0001{bs}t{bs}{bs}\"}}"
    );
    assert_eq!(canonical_json(&value), expected);
}

#[test]
fn size_buckets_use_binary_float_log2() {
    assert_eq!(size_bucket(0), None);
    assert_eq!(size_bucket(1), Some(0));
    assert_eq!(size_bucket(16), Some(1));
    // log2(2**52 - 1) is 52.0 in binary floats, so the bucket is 13, not the exact 12.
    assert_eq!(size_bucket((1 << 52) - 1), Some(13));
    assert_eq!(size_bucket((1 << 53) - 1), Some(13));
}

#[test]
fn word_runs_keep_letters_and_numbers_only() {
    assert_eq!(word_runs("a_b-c d"), vec!["a", "b", "c", "d"]);
    assert_eq!(word_runs("cafe\u{301}"), vec!["cafe"]);
    assert_eq!(word_runs("\u{939}\u{93f}"), vec!["\u{939}"]);
    assert_eq!(word_runs("\u{661}\u{662}x"), vec!["\u{661}\u{662}x"]);
}

#[test]
fn fit_rejects_more_than_ten_thousand_events_and_keeps_state() {
    let mut student = Student::new(&Value::Null).expect("default taxonomy");
    let before = student.model_id().to_owned();
    let batch = Value::Array(vec![Value::Null; 10_001]);
    let error = student.fit(&batch).expect_err("batch over the limit");
    assert_eq!(
        error.message(),
        "At most 10000 feedback events per prototype training run"
    );
    assert_eq!(student.model_id(), before);
}

#[test]
fn vocabulary_budget_rejects_the_batch_and_keeps_the_previous_model() {
    let mut student = Student::new(&Value::Null).expect("default taxonomy");
    student
        .fit(&json!([event(0, "invoice march", "")]))
        .expect("small batch fits");
    let before = student.model_id().to_owned();
    // 115 distinct terms per event (51 name words and 64 context words); 870 events pass 100 000.
    let batch: Vec<Value> = (0..900)
        .map(|index| event(index, &words(index * 115, 51), &words(index * 115 + 51, 64)))
        .collect();
    let error = student
        .fit(&Value::Array(batch))
        .expect_err("vocabulary over budget");
    assert_eq!(error.message(), "Prototype vocabulary budget exceeded");
    assert_eq!(student.model_id(), before);
    assert_eq!(student.training_count(), 1);
}
