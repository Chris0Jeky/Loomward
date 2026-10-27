//! Offline grouped temporal evaluation. All scores remain advice-only and uncalibrated.
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::features::{integer, validate_features};
use crate::student::{validate_labels, Prediction, Source, Student};
use crate::LearnError;

pub const SCOPE: &str = "synthetic smoke, not real-user accuracy";
pub const SCORE_NOTE: &str = "relative score, not a probability";

struct Item {
    id: String,
    logical: String,
    project: Option<String>,
    duplicate: Option<String>,
    time: u64,
    revision: u64,
    source: Source,
    label: String,
    teacher: Option<String>,
    features: Value,
}

struct Corpus {
    labels: Vec<String>,
    items: Vec<Item>,
}

fn text(value: &Value, key: &str) -> Result<String, LearnError> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 128 && !s.contains('\0'))
        .map(str::to_owned)
        .ok_or_else(|| LearnError::new(format!("Invalid {key}")))
}

impl Corpus {
    fn parse(value: &Value) -> Result<Self, LearnError> {
        if value["synthetic"] != true {
            return Err(LearnError::new(
                "Only explicitly synthetic corpora are accepted",
            ));
        }
        let labels = validate_labels(&value["labels"])?;
        let raw = value["items"]
            .as_array()
            .filter(|items| !items.is_empty() && items.len() <= 10_000)
            .ok_or_else(|| LearnError::new("Corpus needs 1..10000 items"))?;
        let mut ids = BTreeSet::new();
        let mut revisions = BTreeSet::new();
        let mut items = Vec::new();
        for row in raw {
            let source = match row["source"].as_str() {
                Some("human") => Source::Human,
                Some("teacher") => Source::Teacher,
                _ => return Err(LearnError::new("Unknown label provenance")),
            };
            let id = text(row, "id")?;
            let logical = text(row, "logical_id")?;
            let revision = integer(row.get("revision"), "revision", 1, 1_000_000_000)?;
            if !ids.insert(id.clone()) || !revisions.insert((logical.clone(), revision, source)) {
                return Err(LearnError::new("Duplicate item or logical revision/source"));
            }
            let label = text(row, "label")?;
            if !labels.contains(&label) {
                return Err(LearnError::new("Label outside taxonomy"));
            }
            let optional = |key| {
                if row[key].is_null() {
                    Ok(None)
                } else {
                    text(row, key).map(Some)
                }
            };
            let teacher = optional("teacher_label")?;
            if teacher
                .as_ref()
                .is_some_and(|label| !labels.contains(label))
                || (source == Source::Teacher && teacher.as_ref().is_some_and(|t| t != &label))
            {
                return Err(LearnError::new("Invalid separate teacher label"));
            }
            items.push(Item {
                id,
                logical,
                revision,
                source,
                label,
                teacher,
                project: optional("project")?,
                duplicate: optional("near_duplicate")?,
                time: integer(row.get("time"), "time", 0, (1 << 53) - 1)?,
                features: validate_features(&row["features"])?.to_value(),
            });
        }
        Ok(Self { labels, items })
    }

    fn manifest(&self, train_end: u64, calibration_end: u64) -> Result<Value, LearnError> {
        if train_end >= calibration_end || calibration_end > (1 << 53) - 1 {
            return Err(LearnError::new("Temporal cutoffs must be ordered"));
        }
        // Union every declared relation; a duplicate linking two projects joins both projects.
        let mut parent: Vec<usize> = (0..self.items.len()).collect();
        let mut links = BTreeMap::new();
        for (index, item) in self.items.iter().enumerate() {
            for (kind, key) in [
                ("logical", Some(&item.logical)),
                ("project", item.project.as_ref()),
                ("duplicate", item.duplicate.as_ref()),
            ] {
                if let Some(key) = key {
                    if let Some(previous) = links.insert((kind, key), index) {
                        let a = root(&mut parent, index);
                        let b = root(&mut parent, previous);
                        parent[a] = b;
                    }
                }
            }
        }
        let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for index in 0..self.items.len() {
            groups
                .entry(root(&mut parent, index))
                .or_default()
                .push(index);
        }
        let split = |time| {
            if time <= train_end {
                "train"
            } else if time <= calibration_end {
                "calibration"
            } else {
                "evaluation"
            }
        };
        let mut entries = Vec::new();
        for indices in groups.values() {
            let group_key = indices.iter().map(|i| &self.items[*i].id).min().unwrap();
            let role = split(self.items[indices[0]].time);
            if indices.iter().any(|i| split(self.items[*i].time) != role) {
                return Err(LearnError::new("Linked group spans a temporal cutoff"));
            }
            for index in indices {
                let item = &self.items[*index];
                entries.push(json!({"id": item.id, "logical_id": item.logical,
                    "revision": item.revision, "project": item.project,
                    "near_duplicate": item.duplicate, "group_key": group_key,
                    "time": item.time, "source": item.source.as_str(), "label": item.label,
                    "teacher_label": item.teacher, "split": role,
                    "holdout_truth": role != "train" && item.source == Source::Human}));
            }
        }
        entries.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        Ok(json!({"schema": "loomward-evaluation-v1", "scope": SCOPE,
            "time_unit": "synthetic ordered tick", "train_end": train_end,
            "calibration_end": calibration_end, "entries": entries}))
    }

    fn validate(&self, manifest: &Value) -> Result<(), LearnError> {
        let train_end = integer(manifest.get("train_end"), "train_end", 0, (1 << 53) - 1)?;
        let calibration_end = integer(
            manifest.get("calibration_end"),
            "calibration_end",
            0,
            (1 << 53) - 1,
        )?;
        if self.manifest(train_end, calibration_end)? != *manifest {
            return Err(LearnError::new(
                "Manifest membership, grouping, time or provenance changed",
            ));
        }
        Ok(())
    }

    fn predictions(&self, manifest: &Value) -> Result<(Student, Vec<Row<'_>>), LearnError> {
        self.validate(manifest)?;
        let roles: BTreeMap<&str, &Value> = manifest["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e["id"].as_str().unwrap(), e))
            .collect();
        let events: Vec<Value> = self
            .items
            .iter()
            .filter(|i| roles[i.id.as_str()]["split"] == "train")
            .map(|i| {
                json!({"event_id": i.id, "item_id": i.logical, "revision": i.revision,
                "source": i.source.as_str(), "label": i.label, "features": i.features})
            })
            .collect();
        let mut student = Student::new(&json!(self.labels))?;
        student.fit(&json!(events))?;
        let rows = self
            .items
            .iter()
            .filter(|i| roles[i.id.as_str()]["split"] != "train")
            .map(|item| {
                let entry = roles[item.id.as_str()];
                Ok(Row {
                    item,
                    group: entry["group_key"].as_str().unwrap().to_owned(),
                    role: entry["split"].as_str().unwrap().to_owned(),
                    prediction: student.predict(&item.features)?,
                })
            })
            .collect::<Result<Vec<_>, LearnError>>()?;
        Ok((student, rows))
    }
}

fn root(parent: &mut [usize], mut index: usize) -> usize {
    while parent[index] != index {
        parent[index] = parent[parent[index]];
        index = parent[index];
    }
    index
}

/// Refuses linked groups spanning cutoffs rather than moving old observations into future holdouts.
pub fn split_manifest(
    corpus: &Value,
    train_end: u64,
    calibration_end: u64,
) -> Result<Value, LearnError> {
    Corpus::parse(corpus)?.manifest(train_end, calibration_end)
}

/// Rechecks every item, group, temporal assignment and provenance before a manifest can be used.
pub fn validate_manifest(corpus: &Value, manifest: &Value) -> Result<(), LearnError> {
    Corpus::parse(corpus)?.validate(manifest)
}

struct Row<'a> {
    item: &'a Item,
    group: String,
    role: String,
    prediction: Prediction,
}

/// Unconditional top-one metrics; abstention utility is reported separately. Empty denominators are null.
pub fn classification_metrics(
    labels: &[String],
    pairs: &[(String, String)],
) -> Result<Value, LearnError> {
    validate_labels(&json!(labels))?;
    if pairs
        .iter()
        .any(|(truth, predicted)| !labels.contains(truth) || !labels.contains(predicted))
    {
        return Err(LearnError::new("Metric label outside taxonomy"));
    }
    let mut per_label = Vec::new();
    let mut f1 = Vec::new();
    for label in labels {
        let tp = pairs
            .iter()
            .filter(|(t, p)| t == label && p == label)
            .count();
        let support = pairs.iter().filter(|(t, _)| t == label).count();
        let predicted = pairs.iter().filter(|(_, p)| p == label).count();
        let score = ratio(2 * tp, support + predicted);
        if let Some(score) = score {
            f1.push(score);
        }
        per_label.push(
            json!({"label": label, "support": support, "predicted": predicted,
            "precision": ratio(tp, predicted), "recall": ratio(tp, support), "f1": score}),
        );
    }
    Ok(
        json!({"count": pairs.len(), "accuracy": ratio(pairs.iter().filter(|(t, p)| t == p).count(), pairs.len()),
        "macro_f1": mean(&f1), "macro_labels": f1.len(), "per_label": per_label,
        "macro_policy": "exclude labels with neither truth nor prediction; undefined rates are null"}),
    )
}

fn ratio(n: usize, d: usize) -> Option<f64> {
    if d == 0 {
        None
    } else {
        Some(n as f64 / d as f64)
    }
}
fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}
fn top<'a>(row: &'a Row<'_>) -> &'a str {
    &row.prediction.suggestions[0].label
}
fn teacher(item: &Item) -> Option<&str> {
    item.teacher
        .as_deref()
        .or_else(|| (item.source == Source::Teacher).then_some(item.label.as_str()))
}

fn risk_at_coverage(rows: &[&Row<'_>], threshold: f64) -> Value {
    let accepted: Vec<_> = rows
        .iter()
        .filter(|r| {
            let p = &r.prediction;
            let margin = p.suggestions[0].score - p.suggestions.get(1).map_or(0.0, |s| s.score);
            // Preserve all non-score abstention guards while sweeping only the top-score threshold.
            p.reasons.iter().all(|r| *r == "ambiguous_model_scores")
                && p.feature_coverage >= 0.35
                && margin >= 0.2
                && p.suggestions[0].score >= threshold
        })
        .collect();
    json!({"threshold": threshold, "margin_floor": 0.2, "feature_coverage_floor": 0.35,
        "accepted": accepted.len(), "coverage": ratio(accepted.len(), rows.len()),
        "error": ratio(accepted.iter().filter(|r| top(r) != r.item.label).count(), accepted.len())})
}

// Metrics vector: multiclass Brier, top-label ECE, then count/mean-score/accuracy for ten bins.
fn calibration_values(rows: &[&Row<'_>]) -> Vec<Option<f64>> {
    let mut counts = [0usize; 10];
    let mut scores = [0.0; 10];
    let mut correct = [0usize; 10];
    let mut brier = Vec::new();
    for row in rows {
        let score = row.prediction.suggestions[0].score;
        let bin = ((score * 10.0) as usize).min(9);
        counts[bin] += 1;
        scores[bin] += score;
        correct[bin] += usize::from(top(row) == row.item.label);
        brier.push(
            row.prediction
                .suggestions
                .iter()
                .map(|s| (s.score - f64::from(s.label == row.item.label)).powi(2))
                .sum(),
        );
    }
    let ece = (!rows.is_empty()).then(|| {
        (0..10)
            .map(|i| (scores[i] - correct[i] as f64).abs() / rows.len() as f64)
            .sum::<f64>()
    });
    let mut values = vec![mean(&brier), ece];
    for i in 0..10 {
        values.extend([
            Some(counts[i] as f64),
            (counts[i] > 0).then(|| scores[i] / counts[i] as f64),
            ratio(correct[i], counts[i]),
        ]);
    }
    values
}

fn calibration_report(rows: &[&Row<'_>], replicates: usize, seed: u64) -> Value {
    let estimates = calibration_values(rows);
    let mut groups: BTreeMap<&str, Vec<&Row<'_>>> = BTreeMap::new();
    for row in rows {
        groups.entry(&row.group).or_default().push(row);
    }
    let groups: Vec<_> = groups.into_values().collect();
    let mut samples = vec![Vec::new(); estimates.len()];
    let mut state = seed;
    if !groups.is_empty() {
        for _ in 0..replicates {
            let mut resampled = Vec::new();
            for _ in 0..groups.len() {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                resampled.extend(&groups[((state >> 32) as usize) % groups.len()]);
            }
            for (sample, value) in samples.iter_mut().zip(calibration_values(&resampled)) {
                if let Some(value) = value {
                    sample.push(value);
                }
            }
        }
    }
    let metric = |index: usize| {
        let mut sample = samples[index].clone();
        sample.sort_by(f64::total_cmp);
        let interval = if sample.is_empty() {
            Value::Null
        } else {
            json!([
                sample[((sample.len() - 1) as f64 * 0.025).floor() as usize],
                sample[((sample.len() - 1) as f64 * 0.975).ceil() as usize]
            ])
        };
        json!({"estimate": estimates[index], "bootstrap_95_percent_interval": interval,
            "defined_replicates": sample.len()})
    };
    json!({"scope": SCOPE, "score_note": SCORE_NOTE, "count": rows.len(), "independent_groups": groups.len(),
        "brier_definition": "mean sum over taxonomy of (relative score - one-hot human truth)^2",
        "ece_definition": "top-label correctness, ten equal-width raw-score bins",
        "brier": metric(0), "ece": metric(1),
        "reliability_bins": (0..10).map(|i| json!({"lower": i as f64 / 10.0,
            "upper": (i + 1) as f64 / 10.0, "upper_inclusive": i == 9,
            "count": metric(2 + 3*i), "mean_score": metric(3 + 3*i), "accuracy": metric(4 + 3*i)})).collect::<Vec<_>>(),
        "bootstrap": {"method": "percentile, resample whole linked groups with replacement",
            "replicates": replicates, "seed": seed, "empty_bins": "null; intervals use only defined replicates",
            "warning": "Small synthetic samples can give degenerate intervals; no real-user inference"}})
}

/// Fit a top-score/correctness isotonic candidate on human calibration rows only. Never a promotion gate.
pub fn fit_isotonic(corpus: &Value, manifest: &Value, split: &str) -> Result<Value, LearnError> {
    if split != "calibration" {
        return Err(LearnError::new(
            "Calibration fit requires calibration split, never evaluation or train",
        ));
    }
    let corpus = Corpus::parse(corpus)?;
    let (student, rows) = corpus.predictions(manifest)?;
    let mut points: Vec<_> = rows
        .iter()
        .filter(|r| r.role == split && r.item.source == Source::Human)
        .map(|r| {
            (
                r.prediction.suggestions[0].score,
                f64::from(top(r) == r.item.label),
            )
        })
        .collect();
    if points.is_empty() {
        return Err(LearnError::new("No human calibration truth"));
    }
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut blocks: Vec<(f64, f64, f64, usize)> = Vec::new();
    for (score, correct) in &points {
        blocks.push((*score, *score, *correct, 1));
        while blocks.len() > 1 {
            let n = blocks.len();
            let a = blocks[n - 2];
            let b = blocks[n - 1];
            if a.1 != b.0 && a.2 / a.3 as f64 <= b.2 / b.3 as f64 {
                break;
            }
            blocks.pop();
            blocks[n - 2] = (a.0, b.1, a.2 + b.2, a.3 + b.3);
        }
    }
    Ok(
        json!({"scope": SCOPE, "fit_split": split, "model_id": student.model_id(),
        "human_count": points.len(), "status": "candidate, gate NOT verified", "calibrated": false,
        "score_note": SCORE_NOTE, "target": "top-label correctness, not action safety",
        "blocks": blocks.iter().map(|(low, high, correct, count)| json!({"low": low,
            "high": high, "fitted_correctness": correct / *count as f64, "count": count})).collect::<Vec<_>>() }),
    )
}

/// Train only on manifest train rows, then report separate teacher, imitation and human evidence.
pub fn evaluate(
    corpus: &Value,
    manifest: &Value,
    replicates: usize,
    seed: u64,
) -> Result<Value, LearnError> {
    if !(100..=2000).contains(&replicates) {
        return Err(LearnError::new("Bootstrap needs 100..2000 replicates"));
    }
    let corpus = Corpus::parse(corpus)?;
    let (student, rows) = corpus.predictions(manifest)?;
    let evaluation: Vec<_> = rows.iter().filter(|r| r.role == "evaluation").collect();
    let human: Vec<_> = evaluation
        .iter()
        .copied()
        .filter(|r| r.item.source == Source::Human)
        .collect();
    // A teacher label may arrive as its own provenance record (same logical ID and revision);
    // a label carried on the human record itself takes precedence.
    let separate: BTreeMap<_, _> = evaluation
        .iter()
        .filter(|r| r.item.source == Source::Teacher)
        .map(|r| {
            (
                (r.item.logical.as_str(), r.item.revision),
                r.item.label.as_str(),
            )
        })
        .collect();
    let teacher_pairs: Vec<_> = human
        .iter()
        .filter_map(|r| {
            teacher(r.item)
                .or_else(|| {
                    separate
                        .get(&(r.item.logical.as_str(), r.item.revision))
                        .copied()
                })
                .map(|t| (r.item.label.clone(), t.to_owned()))
        })
        .collect();
    let imitation: Vec<_> = evaluation
        .iter()
        .filter_map(|r| teacher(r.item).map(|t| (t.to_owned(), top(r).to_owned())))
        .collect();
    let utility: Vec<_> = human
        .iter()
        .map(|r| (r.item.label.clone(), top(r).to_owned()))
        .collect();
    let accepted: Vec<_> = human.iter().filter(|r| !r.prediction.abstain).collect();
    let predictions: Vec<_> = rows
        .iter()
        .map(|r| {
            json!({"id": r.item.id, "group_key": r.group,
        "split": r.role, "source": r.item.source.as_str(), "prediction": r.prediction.to_value()})
        })
        .collect();
    Ok(
        json!({"schema": "loomward-evaluation-v1", "scope": SCOPE, "score_note": SCORE_NOTE,
        "calibrated": false, "autonomy_allowed": false, "model_id": student.model_id(),
        "training_count": student.training_count(), "manifest": manifest, "predictions": predictions,
        "teacher_accuracy": {"scope": SCOPE, "truth": "human evaluation only", "metrics": classification_metrics(&corpus.labels, &teacher_pairs)?},
        "student_imitation": {"scope": SCOPE, "truth": "teacher agreement, NOT human truth", "metrics": classification_metrics(&corpus.labels, &imitation)?},
        "human_holdout_utility": {"scope": SCOPE, "truth": "human evaluation only", "metrics": classification_metrics(&corpus.labels, &utility)?,
            "operating_point": {"accepted": accepted.len(), "coverage": ratio(accepted.len(), human.len()),
                "error": ratio(accepted.iter().filter(|r| top(r) != r.item.label).count(), accepted.len())},
            "selective": ([0.5, 0.8, 0.9, 0.98].map(|t| risk_at_coverage(&human, t))),
            "correction_rate": {"value": null, "status": "unobserved; needs human correction interactions"},
            "interruption_cost": {"value": null, "status": "unobserved; needs human interaction timing"}},
        "calibration": {"evaluation_raw_score_diagnostics": calibration_report(&human, replicates, seed),
            "map_status": "No gated calibrated map; optional candidate fit uses separate calibration split"}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ablation::{model_report, Scored};
    use crate::student::Suggestion;

    #[test]
    fn calibration_matches_hand_computed_brier_ece_and_bin_edges() {
        let corpus = Corpus::parse(
            &json!({"synthetic": true, "labels": ["Documents", "Finance"],
            "items": [{"id": "a", "logical_id": "a", "revision": 1, "time": 21,
                "source": "human", "label": "Documents", "features": {}},
                {"id": "b", "logical_id": "b", "revision": 1, "time": 22,
                "source": "human", "label": "Finance", "features": {}}]}),
        )
        .unwrap();
        let row = |index: usize, score: f64| Row {
            item: &corpus.items[index],
            group: index.to_string(),
            role: "evaluation".into(),
            prediction: Prediction {
                model_id: "synthetic".into(),
                suggestions: vec![
                    Suggestion {
                        label: "Documents".into(),
                        score,
                    },
                    Suggestion {
                        label: "Finance".into(),
                        score: 1.0 - score,
                    },
                ],
                abstain: false,
                reasons: vec![],
                feature_coverage: 1.0,
                known_features: vec![],
                review_priority: 0.0,
            },
        };
        let a = row(0, 0.8);
        let b = row(1, 0.6);
        let values = calibration_values(&[&a, &b]);
        assert!((values[0].unwrap() - 0.4).abs() < 1e-12);
        assert!((values[1].unwrap() - 0.4).abs() < 1e-12);
        assert_eq!(values[2 + 3 * 8], Some(1.0));
        assert_eq!(values[4 + 3 * 8], Some(1.0));
        assert_eq!(values[4 + 3 * 6], Some(0.0));
        let perfect = row(0, 1.0);
        let edge = calibration_values(&[&perfect]);
        assert_eq!(edge[0], Some(0.0));
        assert_eq!(edge[1], Some(0.0));
        assert_eq!(edge[2 + 3 * 9], Some(1.0));
        assert!(calibration_values(&[])[0].is_none());
    }

    /// One hand-built scored prediction: truth "yes", predicted "yes" when
    /// correct and "no" otherwise, with the given top and second scores.
    fn scored(id: &str, top: f64, second: f64, correct: bool) -> Scored {
        let predicted = if correct { "yes" } else { "no" }.to_owned();
        let other = if predicted == "yes" { "no" } else { "yes" }.to_owned();
        Scored {
            truth: "yes".to_owned(),
            prediction: Prediction {
                model_id: "synthetic".into(),
                suggestions: vec![
                    Suggestion {
                        label: predicted,
                        score: top,
                    },
                    Suggestion {
                        label: other,
                        score: second,
                    },
                ],
                abstain: false,
                reasons: vec![],
                feature_coverage: 1.0,
                known_features: vec![],
                review_priority: 0.0,
            },
            record_id: id.to_owned(),
        }
    }

    fn risk_at_coverage_points(rows: &[Scored]) -> Value {
        let labels = vec!["yes".to_owned(), "no".to_owned()];
        model_report("test", "test-model", &labels, rows).unwrap()["selective_risk"].clone()
    }

    #[test]
    fn risk_at_coverage_matches_hand_computed_top_slice_values() {
        // Caller expectation (issue #931 item 2): ten predictions ranked by top
        // score descending; coverage c keeps the top ceil(c*n); risk = errors/kept.
        // Exact slice rule as implemented: accepted = (per_mille * n).div_ceil(1000)
        // for COVERAGE_PER_MILLE = [500, 800, 900, 980], so n = 10 keeps 5, 8, 9, 10.
        // The top five are correct and the bottom five wrong with uniform margins,
        // so the ranking is purely by top score.
        let scores = [0.95, 0.90, 0.85, 0.80, 0.75, 0.70, 0.65, 0.60, 0.55, 0.50];
        let rows: Vec<Scored> = scores
            .iter()
            .enumerate()
            .map(|(i, score)| scored(&format!("r{i:02}"), *score, score - 0.1, i < 5))
            .collect();
        let points = risk_at_coverage_points(&rows);
        let points = points.as_array().unwrap();
        assert_eq!(points.len(), 4);
        let expected = [
            (0.5, 5u64, 0.5, 0.0),
            (0.8, 8, 0.8, 0.375),
            (0.9, 9, 0.9, 4.0 / 9.0),
            (0.98, 10, 1.0, 0.5),
        ];
        for (point, (target, accepted, coverage, risk)) in points.iter().zip(expected) {
            assert!(
                (point["target_coverage"].as_f64().unwrap() - target).abs() < 1e-12,
                "{point}"
            );
            assert_eq!(point["accepted"].as_u64().unwrap(), accepted);
            assert!(
                (point["coverage"].as_f64().unwrap() - coverage).abs() < 1e-12,
                "{point}"
            );
            assert!(
                (point["risk"].as_f64().unwrap() - risk).abs() < 1e-12,
                "{point}"
            );
        }
    }

    #[test]
    fn risk_at_coverage_breaks_score_ties_by_record_id() {
        // Current behaviour: the ranking key is (top score desc, margin desc,
        // record id asc). Equal scores and equal margins fall through to the
        // record id, not the input order.
        let rows: Vec<Scored> = (0..10)
            .rev()
            .map(|i| scored(&format!("r{i:02}"), 1.0, 0.0, i < 5))
            .collect();
        let points = risk_at_coverage_points(&rows);
        let points = points.as_array().unwrap();
        assert_eq!(points[0]["accepted"].as_u64().unwrap(), 5);
        assert!((points[0]["risk"].as_f64().unwrap() - 0.0).abs() < 1e-12);
        assert!((points[3]["risk"].as_f64().unwrap() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn report_counts_duplicate_top_scores_including_saturation() {
        let labels = vec!["yes".to_owned(), "no".to_owned()];
        let rows = vec![
            scored("a", 1.0, 0.0, true),
            scored("b", 1.0, 0.0, false),
            scored("c", 1.0, 0.0, true),
            scored("d", 0.8, 0.2, false),
            scored("e", 0.8, 0.1, true),
            scored("f", 0.7, 0.3, false),
        ];
        let report = |rows: &[Scored]| model_report("test", "test-model", &labels, rows).unwrap();
        // Two additional saturated rows and one additional row scoring 0.8.
        assert_eq!(report(&rows)["top_score_tie_count"], 3);
        assert_eq!(report(&[])["top_score_tie_count"], 0);
        assert_eq!(report(&rows[..1])["top_score_tie_count"], 0);
        assert_eq!(report(&rows[4..])["top_score_tie_count"], 0);
    }

    #[test]
    fn risk_at_coverage_prefers_larger_margin_before_record_id() {
        // Current-behaviour characterisation: with equal top scores the larger
        // margin ranks first, ahead of the record-id tie-break. r04 has the
        // smaller id but the smaller margin and is wrong; r05 is correct with
        // margin 0.30, so the top five stay error-free only under margin order.
        let rows = vec![
            scored("r00", 0.95, 0.85, true),
            scored("r01", 0.90, 0.80, true),
            scored("r02", 0.85, 0.75, true),
            scored("r03", 0.80, 0.70, true),
            scored("r04", 0.70, 0.60, false),
            scored("r05", 0.70, 0.40, true),
            scored("r06", 0.65, 0.55, false),
            scored("r07", 0.60, 0.50, false),
            scored("r08", 0.55, 0.45, false),
            scored("r09", 0.50, 0.40, false),
        ];
        let points = risk_at_coverage_points(&rows);
        let points = points.as_array().unwrap();
        assert_eq!(points[0]["accepted"].as_u64().unwrap(), 5);
        assert!((points[0]["risk"].as_f64().unwrap() - 0.0).abs() < 1e-12);
    }

    #[test]
    fn risk_at_coverage_empty_and_unknown_label_edges() {
        // Empty boundary: nothing is kept, so coverage and risk stay null.
        let points = risk_at_coverage_points(&[]);
        for point in points.as_array().unwrap() {
            assert_eq!(point["accepted"].as_u64().unwrap(), 0);
            assert!(point["coverage"].is_null());
            assert!(point["risk"].is_null());
        }
        // A single correct row: ceil keeps it at every target, including 50%.
        let points = risk_at_coverage_points(&[scored("r00", 0.9, 0.8, true)]);
        for point in points.as_array().unwrap() {
            assert_eq!(point["accepted"].as_u64().unwrap(), 1);
            assert!((point["coverage"].as_f64().unwrap() - 1.0).abs() < 1e-12);
            assert!((point["risk"].as_f64().unwrap() - 0.0).abs() < 1e-12);
        }
        // Error path: a truth label outside the taxonomy is refused.
        let labels = vec!["yes".to_owned(), "no".to_owned()];
        let mut bad = vec![scored("r00", 0.9, 0.8, true)];
        bad[0].truth = "maybe".to_owned();
        assert!(model_report("test", "test-model", &labels, &bad)
            .unwrap_err()
            .message()
            .contains("outside taxonomy"));
    }
}
