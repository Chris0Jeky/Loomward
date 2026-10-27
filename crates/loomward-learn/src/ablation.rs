//! Stage 2 ablation (LW-139, docs/51 section 4.8): the naive Bayes baseline and the hashed linear student
//! on one grouped temporal split of categoriser records, per axis. The split and macro-F1 come from
//! `evaluation.rs`; only synthetic corpora are accepted, so every report says `synthetic: true` and is
//! never product accuracy. Teacher labels are excluded from training and from truth for both students.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::evaluation::{classification_metrics, split_manifest, SCOPE, SCORE_NOTE};
use crate::features::{Features, CONTEXT_LIMIT, EXTENSION_LIMIT, NAME_LIMIT};
use crate::linear::LinearStudent;
use crate::packet::{validate_corpus, LIFECYCLES, MEANINGS, RECREATABILITIES};
use crate::student::{Prediction, Student, ALGORITHM as NB_ALGORITHM};
use crate::LearnError;

pub const SCHEMA: &str = "loomward-ablation-v1";
/// Target coverages in per mille, so `ceil(coverage * n)` is exact integer arithmetic.
pub const COVERAGE_PER_MILLE: [usize; 4] = [500, 800, 900, 980];
pub const AXES: [(&str, &[&str]); 3] = [
    ("meaning", MEANINGS),
    ("lifecycle", LIFECYCLES),
    ("recreatability", RECREATABILITIES),
];

/// Whole tokens joined by spaces, stopping before the limit (in code points) would be crossed.
fn join_within<'a>(words: impl Iterator<Item = &'a str>, limit: usize) -> String {
    let mut text = String::new();
    for word in words {
        let extra = word.chars().count() + usize::from(!text.is_empty());
        if text.chars().count() + extra > limit {
            break;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(word);
    }
    text
}

/// The existing student's inputs from a validated packet: name tokens as the name, path markers as the
/// context, the extension with the most files, and logical bytes (unknown stays 0, so no size token).
pub fn packet_features(packet: &Value) -> Features {
    let strings = |pointer: &str| -> Vec<&str> {
        packet
            .pointer(pointer)
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    };
    let mut extension = "";
    let mut most = 0;
    for (index, entry) in packet["extensions"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let files = entry["files"].as_u64().unwrap_or(0);
        if index == 0 || files > most {
            extension = entry["ext"].as_str().unwrap_or("");
            most = files;
        }
    }
    Features {
        name: join_within(strings("/subject/name_tokens").into_iter(), NAME_LIMIT),
        extension: extension.chars().take(EXTENSION_LIMIT).collect(),
        context: join_within(strings("/markers").into_iter(), CONTEXT_LIMIT),
        size_bytes: packet["shape"]["logical_bytes"].as_u64().unwrap_or(0),
    }
}

/// The record's human label on one axis. Teacher labels are never read here.
fn human_label<'a>(record: &'a Value, axis: &str) -> Option<&'a str> {
    record["human_labels"]
        .as_array()?
        .iter()
        .find(|label| label["axis"] == axis)?["label"]
        .as_str()
}

// pub(crate) so the in-crate evaluation tests can pin the selective-risk
// ranking with hand-built scores; the computation itself is unchanged.
pub(crate) struct Scored {
    pub(crate) truth: String,
    pub(crate) prediction: Prediction,
    pub(crate) record_id: String,
}

pub(crate) fn model_report(
    name: &str,
    model_id: &str,
    labels: &[String],
    rows: &[Scored],
) -> Result<Value, LearnError> {
    let top = |row: &Scored| row.prediction.suggestions[0].label.clone();
    let pairs: Vec<(String, String)> = rows
        .iter()
        .map(|row| (row.truth.clone(), top(row)))
        .collect();
    let metrics = classification_metrics(labels, &pairs)?;
    // Rank by raw top score, then margin, then record ID: a ranking, not a probability.
    let mut ranked: Vec<&Scored> = rows.iter().collect();
    let key = |row: &Scored| {
        let s = &row.prediction.suggestions;
        (
            s[0].score,
            s[0].score - s.get(1).map_or(0.0, |second| second.score),
        )
    };
    ranked.sort_by(|left, right| {
        let (a, b) = (key(left), key(right));
        b.0.total_cmp(&a.0)
            .then(b.1.total_cmp(&a.1))
            .then_with(|| left.record_id.cmp(&right.record_id))
    });
    // Additional rows beyond the first at each repeated top score, including saturated scores.
    let top_score_tie_count = ranked
        .windows(2)
        .filter(|pair| key(pair[0]).0 == key(pair[1]).0)
        .count();
    let n = rows.len();
    let selective: Vec<Value> = COVERAGE_PER_MILLE
        .iter()
        .map(|per_mille| {
            let accepted = (per_mille * n).div_ceil(1000);
            let errors = ranked[..accepted]
                .iter()
                .filter(|row| top(row) != row.truth)
                .count();
            json!({"target_coverage": *per_mille as f64 / 1000.0, "accepted": accepted,
                "coverage": (n > 0).then(|| accepted as f64 / n as f64),
                "risk": (accepted > 0).then(|| errors as f64 / accepted as f64)})
        })
        .collect();
    let abstained = rows.iter().filter(|row| row.prediction.abstain).count();
    Ok(
        json!({"model": name, "model_id": model_id, "calibrated": false,
        "macro_f1": metrics["macro_f1"], "accuracy": metrics["accuracy"],
        "abstention_rate": (n > 0).then(|| abstained as f64 / n as f64),
        "top_score_tie_count": top_score_tie_count,
        "selective_risk": selective,
        "selective_ranking": "raw top score, then margin, then record id; ignores abstention; not a probability"}),
    )
}

/// Train both students per axis on the train split and score the evaluation split. The calibration split
/// is held back for a later calibration fit and is not scored here.
pub fn ablation(
    records: &Value,
    train_end: u64,
    calibration_end: u64,
) -> Result<Value, LearnError> {
    validate_corpus(records)?;
    // Canonical record-ID order keeps the order-sensitive SGD independent of corpus input order.
    let by_id: BTreeMap<&str, &Value> = records
        .as_array()
        .unwrap()
        .iter()
        .map(|record| (record["record_id"].as_str().unwrap(), record))
        .collect();
    // The split depends only on identity, group and time, so it runs on a one-label stand-in corpus.
    let items: Vec<Value> = by_id
        .iter()
        .map(|(id, record)| {
            json!({"id": id, "logical_id": id, "revision": 1, "project": record["packet"]["group_id"],
                "near_duplicate": null, "time": record["packet"]["observed_at"], "source": "human",
                "label": "Record", "teacher_label": null, "features": {}})
        })
        .collect();
    let manifest = split_manifest(
        &json!({"synthetic": true, "labels": ["Record"], "items": items}),
        train_end,
        calibration_end,
    )?;
    let roles: BTreeMap<&str, &str> = manifest["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["id"].as_str().unwrap(),
                entry["split"].as_str().unwrap(),
            )
        })
        .collect();
    let count = |role: &str| roles.values().filter(|found| **found == role).count();

    let mut axes = Vec::new();
    for (axis, taxonomy) in AXES {
        let labels: Vec<String> = taxonomy.iter().map(|label| (*label).to_owned()).collect();
        let labelled = |role: &str| -> Vec<(&str, Features, String)> {
            by_id
                .iter()
                .filter(|(id, _)| roles[**id] == role)
                .filter_map(|(id, record)| {
                    human_label(record, axis)
                        .map(|label| (*id, packet_features(&record["packet"]), label.to_owned()))
                })
                .collect()
        };
        let train = labelled("train");
        let evaluation = labelled("evaluation");

        let events: Vec<Value> = train
            .iter()
            .map(|(id, features, label)| {
                json!({"event_id": id, "item_id": id, "revision": 1, "source": "human",
                    "label": label, "features": features.to_value()})
            })
            .collect();
        let mut bayes = Student::new(&json!(labels))?;
        bayes.fit(&json!(events))?;
        let examples: Vec<(Features, String)> = train
            .iter()
            .map(|(_, features, label)| (features.clone(), label.clone()))
            .collect();
        let linear = LinearStudent::fit(&labels, &examples)?;

        let mut bayes_rows = Vec::new();
        let mut linear_rows = Vec::new();
        for (id, features, truth) in &evaluation {
            bayes_rows.push(Scored {
                truth: truth.clone(),
                prediction: bayes.predict(&features.to_value())?,
                record_id: (*id).to_owned(),
            });
            linear_rows.push(Scored {
                truth: truth.clone(),
                prediction: linear.predict(features),
                record_id: (*id).to_owned(),
            });
        }
        axes.push(json!({"axis": axis, "labels": labels, "train_count": train.len(),
            "evaluation_count": evaluation.len(), "models": [
                model_report(NB_ALGORITHM, bayes.model_id(), &labels, &bayes_rows)?,
                model_report(crate::linear::ALGORITHM, linear.model_id(), &labels, &linear_rows)?]}));
    }
    Ok(
        json!({"schema": SCHEMA, "synthetic": true, "scope": SCOPE, "score_note": SCORE_NOTE,
        "calibrated": false, "autonomy_allowed": false,
        "teacher_labels": "excluded from training and truth for both students (invariant 3)",
        "split": {"rule": "grouped temporal (evaluation::split_manifest), group = packet group_id",
            "train_end": train_end, "calibration_end": calibration_end, "train": count("train"),
            "calibration_unscored": count("calibration"), "evaluation": count("evaluation")},
        "axes": axes}),
    )
}
