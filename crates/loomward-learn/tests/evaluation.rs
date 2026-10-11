use loomward_learn::evaluation::{
    classification_metrics, evaluate, fit_isotonic, split_manifest, validate_manifest, SCOPE,
    SCORE_NOTE,
};
use serde_json::{json, Value};

fn corpus() -> Value {
    serde_json::from_str(include_str!("../../../fixtures/evaluation/synthetic.json")).unwrap()
}
fn manifest(c: &Value) -> Value {
    split_manifest(c, 10, 20).unwrap()
}
fn entry<'a>(m: &'a Value, id: &str) -> &'a Value {
    m["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}

#[test]
fn linked_groups_are_transitive_and_temporal_traps_are_refused() {
    let c = corpus();
    let m = manifest(&c);
    let a = entry(&m, "train-documents-0");
    assert_eq!(a["group_key"], entry(&m, "train-documents-2")["group_key"]);
    assert_eq!(
        entry(&m, "train-finance-0")["group_key"],
        entry(&m, "train-finance-revision")["group_key"]
    );
    for (key, train_index) in [("logical_id", 0), ("project", 1), ("near_duplicate", 1)] {
        let mut trap = c.clone();
        trap["items"][14][key] = trap["items"][train_index][key].clone();
        trap["items"][14]["revision"] = json!(3);
        assert!(
            split_manifest(&trap, 10, 20)
                .unwrap_err()
                .message()
                .contains("spans"),
            "{key}"
        );
    }
    let mut reversed = c.clone();
    reversed["items"].as_array_mut().unwrap().reverse();
    assert_eq!(manifest(&reversed), m);
    assert!(split_manifest(&c, 20, 10).is_err());
}

#[test]
fn supplied_leaking_manifest_is_rejected_before_fit_or_report() {
    let c = corpus();
    let mut m = manifest(&c);
    for e in m["entries"].as_array_mut().unwrap() {
        if e["id"] == "train-finance-revision" {
            e["split"] = json!("evaluation");
        }
    }
    assert!(validate_manifest(&c, &m).is_err());
    assert!(evaluate(&c, &m, 100, 25).is_err());
    assert!(fit_isotonic(&c, &m, "calibration").is_err());
}

#[test]
fn provenance_is_explicit_and_teacher_never_becomes_holdout_truth() {
    let c = corpus();
    let m = manifest(&c);
    assert_eq!(entry(&m, "eval-teacher")["holdout_truth"], false);
    assert_eq!(entry(&m, "eval-a")["holdout_truth"], true);
    let r = evaluate(&c, &m, 100, 25).unwrap();
    assert_eq!(r["human_holdout_utility"]["metrics"]["count"], 6);
    assert_eq!(r["teacher_accuracy"]["metrics"]["count"], 6);
    assert_eq!(r["student_imitation"]["metrics"]["count"], 7);
    assert_eq!(
        r["calibration"]["evaluation_raw_score_diagnostics"]["count"],
        6
    );
    assert_eq!(
        fit_isotonic(&c, &m, "calibration").unwrap()["human_count"],
        4
    );
    let mut teacher_only = c.clone();
    for row in teacher_only["items"].as_array_mut().unwrap() {
        if row["time"].as_u64().unwrap() > 10 {
            row["source"] = json!("teacher");
            row["teacher_label"] = Value::Null;
        }
    }
    let m = manifest(&teacher_only);
    let r = evaluate(&teacher_only, &m, 100, 25).unwrap();
    assert!(r["human_holdout_utility"]["metrics"]["accuracy"].is_null());
    assert!(r["calibration"]["evaluation_raw_score_diagnostics"]["brier"]["estimate"].is_null());
    assert!(fit_isotonic(&teacher_only, &m, "calibration").is_err());
}

#[test]
fn macro_metrics_match_a_worked_confusion_matrix_and_unknowns_stay_null() {
    let labels = vec!["Documents".into(), "Finance".into(), "Absent".into()];
    let pairs = vec![
        ("Documents".into(), "Documents".into()),
        ("Documents".into(), "Finance".into()),
        ("Finance".into(), "Finance".into()),
    ];
    let m = classification_metrics(&labels, &pairs).unwrap();
    assert!((m["macro_f1"].as_f64().unwrap() - 2.0 / 3.0).abs() < 1e-12);
    assert_eq!(m["per_label"][0]["precision"], 1.0);
    assert_eq!(m["per_label"][0]["recall"], 0.5);
    assert_eq!(m["per_label"][1]["precision"], 0.5);
    assert_eq!(m["per_label"][1]["recall"], 1.0);
    assert!(m["per_label"][2]["f1"].is_null());
    assert!(classification_metrics(&labels, &[]).unwrap()["macro_f1"].is_null());
    assert!(classification_metrics(&labels, &[("Wrong".into(), "Finance".into())]).is_err());
}

#[test]
fn report_preserves_abstention_thresholds_and_missing_correction_observations() {
    let c = corpus();
    let m = manifest(&c);
    let r = evaluate(&c, &m, 100, 25).unwrap();
    let utility = &r["human_holdout_utility"];
    let accepted = r["predictions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| {
            p["split"] == "evaluation"
                && p["source"] == "human"
                && p["prediction"]["abstain"] == false
        })
        .count();
    assert_eq!(utility["operating_point"]["accepted"], accepted);
    assert_eq!(utility["selective"][1]["accepted"], accepted);
    assert!(accepted > 0 && accepted < 6);
    let selective = utility["selective"].as_array().unwrap();
    assert!(selective
        .windows(2)
        .all(|s| s[0]["accepted"].as_u64() >= s[1]["accepted"].as_u64()));
    assert!(utility["correction_rate"]["value"].is_null());
    assert_eq!(r["score_note"], SCORE_NOTE);
    assert_eq!(r["scope"], SCOPE);
    assert_eq!(r["calibrated"], false);
    assert_ne!(
        r["teacher_accuracy"]["metrics"]["accuracy"],
        r["student_imitation"]["metrics"]["accuracy"]
    );
}

#[test]
fn calibration_fit_refuses_evaluation_and_train_and_is_independent_of_evaluation_labels() {
    let c = corpus();
    let m = manifest(&c);
    for split in ["evaluation", "train", "unknown"] {
        assert!(fit_isotonic(&c, &m, split).is_err());
    }
    let fit = fit_isotonic(&c, &m, "calibration").unwrap();
    let mut changed = c.clone();
    for row in changed["items"].as_array_mut().unwrap() {
        if row["time"].as_u64().unwrap() > 20 {
            row["label"] = json!("Documents");
            row["teacher_label"] = Value::Null;
        }
    }
    assert_eq!(
        fit_isotonic(&changed, &manifest(&changed), "calibration").unwrap(),
        fit
    );
    assert_eq!(fit["calibrated"], false);
    let blocks = fit["blocks"].as_array().unwrap();
    assert!(blocks
        .windows(2)
        .all(|b| b[0]["fitted_correctness"].as_f64() <= b[1]["fitted_correctness"].as_f64()));
}

#[test]
fn raw_diagnostics_have_deterministic_group_bootstrap_intervals() {
    let c = corpus();
    let m = manifest(&c);
    let r = evaluate(&c, &m, 100, 25).unwrap();
    assert_eq!(evaluate(&c, &m, 100, 25).unwrap(), r);
    let d = &r["calibration"]["evaluation_raw_score_diagnostics"];
    assert_eq!(d["independent_groups"], 5);
    for metric in [&d["brier"], &d["ece"]] {
        let interval = metric["bootstrap_95_percent_interval"].as_array().unwrap();
        assert!(interval[0].as_f64().unwrap() <= interval[1].as_f64().unwrap());
        assert_eq!(metric["defined_replicates"], 100);
    }
    assert_ne!(
        evaluate(&c, &m, 100, 26).unwrap()["calibration"],
        r["calibration"]
    );
    assert_eq!(d["reliability_bins"].as_array().unwrap().len(), 10);
    assert_eq!(
        d["reliability_bins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["count"]["estimate"].as_f64().unwrap())
            .sum::<f64>(),
        6.0
    );
    assert!(d["reliability_bins"][0]["accuracy"]["estimate"].is_null());
    assert!(evaluate(&c, &m, 0, 25).is_err());
}

#[test]
fn corpus_boundary_rejects_invalid_and_non_synthetic_records() {
    let c = corpus();
    for (key, value) in [
        ("source", json!("preference")),
        ("time", json!(-1)),
        ("label", json!("Unknown")),
        ("revision", json!(0)),
        ("logical_id", json!("")),
    ] {
        let mut invalid = c.clone();
        invalid["items"][0][key] = value;
        assert!(split_manifest(&invalid, 10, 20).is_err(), "{key}");
    }
    let mut invalid = c.clone();
    invalid["synthetic"] = json!(false);
    assert!(split_manifest(&invalid, 10, 20).is_err());
    let mut duplicate = c.clone();
    duplicate["items"]
        .as_array_mut()
        .unwrap()
        .push(c["items"][0].clone());
    assert!(split_manifest(&duplicate, 10, 20).is_err());
}
