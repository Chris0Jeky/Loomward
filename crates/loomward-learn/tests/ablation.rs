use loomward_learn::ablation::{ablation, packet_features, COVERAGE_PER_MILLE};
use loomward_learn::features::Features;
use loomward_learn::linear::{feature_strings, fnv1a, hashed, LinearStudent, BUCKETS};
use loomward_learn::packet::validate_corpus;
use serde_json::{json, Value};

const BASE_TIME: u64 = 1_800_000_000;
const GROUPS: u64 = 60;
const TRAIN_END: u64 = BASE_TIME + 35 * 1000;
const CALIBRATION_END: u64 = BASE_TIME + 44 * 1000;

fn templates() -> Vec<Value> {
    serde_json::from_str(include_str!(
        "../../../fixtures/categoriser-v1/records.json"
    ))
    .unwrap()
}

fn next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state >> 33
}

/// A deterministic synthetic corpus built from the fixture templates: each group is one synthetic
/// project at one time tick, with 2-4 records whose name words and markers are perturbed.
fn synthetic(seed: u64) -> Value {
    const NOISE: [&str; 8] = [
        "sample", "old", "new", "misc", "data", "copy", "work", "temp",
    ];
    let templates = templates();
    let mut state = seed;
    let mut records = Vec::new();
    for group in 0..GROUPS {
        for index in 0..2 + next(&mut state) % 3 {
            let template = &templates[(next(&mut state) % templates.len() as u64) as usize];
            let mut record = template.clone();
            // One word from this class, sometimes one from another class (confusion), plus noise.
            let own = template["packet"]["subject"]["name_tokens"]
                .as_array()
                .unwrap();
            let pick = own[1 + (next(&mut state) as usize) % (own.len() - 1)]
                .as_str()
                .unwrap();
            let mut words = std::collections::BTreeSet::from([pick.to_owned()]);
            if next(&mut state) % 5 == 0 {
                let other = &templates[(next(&mut state) % templates.len() as u64) as usize];
                let tokens = other["packet"]["subject"]["name_tokens"]
                    .as_array()
                    .unwrap();
                words.insert(tokens.last().unwrap().as_str().unwrap().to_owned());
            }
            words.insert(NOISE[(next(&mut state) % 8) as usize].to_owned());
            let markers: Vec<Value> = template["packet"]["markers"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|_| next(&mut state) % 3 != 0)
                .cloned()
                .collect();
            let packet = &mut record["packet"];
            packet["subject"]["name_tokens"] = json!(words);
            packet["markers"] = json!(markers);
            packet["packet_id"] = json!(format!("packet-{group}-{index}"));
            packet["group_id"] = json!(format!("group-{group}"));
            packet["observed_at"] = json!(BASE_TIME + group * 1000);
            record["record_id"] = json!(format!("record-{group}-{index}"));
            records.push(record);
        }
    }
    let corpus = json!(records);
    validate_corpus(&corpus).unwrap();
    corpus
}

fn features(name: &str, context: &str) -> Features {
    Features {
        name: name.into(),
        extension: String::new(),
        context: context.into(),
        size_bytes: 0,
    }
}

#[test]
fn feature_hash_is_pinned() {
    // The published FNV-1a 64 test vector for "a", then one pinned feature bucket.
    assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a("word:cargo"), 0xfded_5612_5b74_2a27);
    assert_eq!(fnv1a("word:cargo") % BUCKETS as u64, 2599);
    let strings = feature_strings(&features("cargo", ""));
    assert_eq!(
        strings,
        [
            "word3:^ca",
            "word3:car",
            "word3:arg",
            "word3:rgo",
            "word3:go$",
            "word:cargo"
        ]
    );
    assert!(hashed(&features("cargo", ""))
        .iter()
        .any(|(bucket, _)| *bucket == 2599));
}

#[test]
fn empty_feature_set_abstains_without_calibration() {
    let labels: Vec<String> = ["generated_build", "source_project"]
        .map(String::from)
        .to_vec();
    let examples: Vec<_> = (0..4)
        .flat_map(|_| {
            [
                (features("target build", "cargo"), labels[0].clone()),
                (features("src code", "git"), labels[1].clone()),
            ]
        })
        .collect();
    let model = LinearStudent::fit(&labels, &examples).unwrap();
    let prediction = model.predict(&features("", ""));
    assert!(hashed(&features("", "")).is_empty());
    assert!(prediction.abstain);
    assert!(prediction.reasons.contains(&"unfamiliar_features"));
    assert_eq!(prediction.to_value()["calibrated"], false);
    assert_eq!(model.to_value()["calibrated"], false);
    let untrained = LinearStudent::fit(&labels, &[])
        .unwrap()
        .predict(&features("build", ""));
    assert!(untrained.reasons.contains(&"no_training_examples"));
}

#[test]
fn linear_model_beats_chance_on_a_separable_set() {
    let words = ["alpha", "bravo", "charlie", "delta"];
    let labels: Vec<String> = ["generated_build", "source_project", "archive", "installer"]
        .map(String::from)
        .to_vec();
    let mut train = Vec::new();
    for round in 0..6 {
        for (class, word) in words.iter().enumerate() {
            train.push((
                features(&format!("{word} item{round}"), ""),
                labels[class].clone(),
            ));
        }
    }
    let model = LinearStudent::fit(&labels, &train).unwrap();
    let mut correct = 0;
    for (class, word) in words.iter().enumerate() {
        let prediction = model.predict(&features(&format!("{word} fresh"), ""));
        correct += usize::from(prediction.suggestions[0].label == labels[class]);
    }
    // Chance is 1 of 4; a working model separates all four.
    assert_eq!(correct, 4);
}

#[test]
fn training_and_report_are_deterministic() {
    let corpus = synthetic(7);
    let first =
        serde_json::to_string(&ablation(&corpus, TRAIN_END, CALIBRATION_END).unwrap()).unwrap();
    let second =
        serde_json::to_string(&ablation(&corpus, TRAIN_END, CALIBRATION_END).unwrap()).unwrap();
    assert_eq!(first, second);
    let mut reversed = corpus.clone();
    reversed.as_array_mut().unwrap().reverse();
    let third =
        serde_json::to_string(&ablation(&reversed, TRAIN_END, CALIBRATION_END).unwrap()).unwrap();
    assert_eq!(first, third);

    let labels: Vec<String> = ["generated_build", "source_project"]
        .map(String::from)
        .to_vec();
    let examples: Vec<_> = corpus
        .as_array()
        .unwrap()
        .iter()
        .take(40)
        .map(|record| {
            (
                packet_features(&record["packet"]),
                labels[record["record_id"].as_str().unwrap().len() % 2].clone(),
            )
        })
        .collect();
    let a = LinearStudent::fit(&labels, &examples).unwrap().to_value();
    let b = LinearStudent::fit(&labels, &examples).unwrap().to_value();
    assert_eq!(
        serde_json::to_vec(&a).unwrap(),
        serde_json::to_vec(&b).unwrap()
    );
}

#[test]
fn teacher_labels_never_reach_training_or_truth() {
    let corpus = synthetic(11);
    let baseline = ablation(&corpus, TRAIN_END, CALIBRATION_END).unwrap();
    // Every record gets teacher labels that disagree with its human labels on all three axes.
    let mut taught = corpus.clone();
    for record in taught.as_array_mut().unwrap() {
        record["teacher_labels"] = json!([
            {"axis": "meaning", "label": "download"},
            {"axis": "lifecycle", "label": "unknown"},
            {"axis": "recreatability", "label": "unknown"}
        ]);
    }
    validate_corpus(&taught).unwrap();
    assert_eq!(
        ablation(&taught, TRAIN_END, CALIBRATION_END).unwrap(),
        baseline
    );
    // Removing human labels does change it, so the comparison above is not vacuous.
    let mut stripped = corpus.clone();
    for record in stripped.as_array_mut().unwrap().iter_mut().step_by(2) {
        record["human_labels"] = json!([]);
    }
    assert_ne!(
        ablation(&stripped, TRAIN_END, CALIBRATION_END).unwrap(),
        baseline
    );
}

#[test]
fn report_is_synthetic_and_coverage_points_are_monotone() {
    let report = ablation(&synthetic(7), TRAIN_END, CALIBRATION_END).unwrap();
    assert_eq!(report["synthetic"], true);
    assert_eq!(report["calibrated"], false);
    let axes = report["axes"].as_array().unwrap();
    assert_eq!(axes.len(), 3);
    for axis in axes {
        assert!(axis["evaluation_count"].as_u64().unwrap() > 0);
        for model in axis["models"].as_array().unwrap() {
            let points = model["selective_risk"].as_array().unwrap();
            assert_eq!(points.len(), COVERAGE_PER_MILLE.len());
            for pair in points.windows(2) {
                assert!(pair[0]["target_coverage"].as_f64() < pair[1]["target_coverage"].as_f64());
                assert!(pair[0]["accepted"].as_u64() <= pair[1]["accepted"].as_u64());
                assert!(pair[0]["coverage"].as_f64() <= pair[1]["coverage"].as_f64());
            }
            for point in points {
                assert!(
                    point["coverage"].as_f64().unwrap()
                        >= point["target_coverage"].as_f64().unwrap()
                );
            }
            assert!(model["macro_f1"].is_number());
            assert!(model["abstention_rate"].is_number());
        }
    }
    // The linear student must beat chance on the synthetic meaning axis (not product accuracy).
    let meaning = &axes[0]["models"][1];
    assert!(meaning["accuracy"].as_f64().unwrap() > 1.0 / 15.0);
    if std::env::var_os("LOOMWARD_ABLATION_PRINT").is_some() {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    }
}

#[test]
fn linked_groups_spanning_the_cutoff_are_refused() {
    let mut corpus = synthetic(7);
    // Move one record of a train group past the train cutoff while keeping its group.
    corpus[0]["packet"]["observed_at"] = json!(CALIBRATION_END + 1);
    assert!(ablation(&corpus, TRAIN_END, CALIBRATION_END)
        .unwrap_err()
        .message()
        .contains("spans"));
}
