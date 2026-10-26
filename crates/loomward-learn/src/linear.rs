//! Hashed n-gram linear student (LW-139 stage 2, docs/51 section 4.8): the one linear candidate the
//! ablation compares against the naive Bayes baseline. Rust-only new work; Python stays a frozen oracle.
//!
//! Features are the existing student's tokens (`features::tokens`, no new path parser) plus character
//! trigrams of every name and context word, hashed into [`BUCKETS`] slots with 64-bit FNV-1a (a fixed,
//! documented hash; no `RandomState`). Collisions are summed, and each vector is L2-normalised.
//!
//! Model: multinomial logistic regression (softmax) trained by plain SGD with a fixed seed, epoch count
//! and learning rate. It was chosen over an averaged perceptron because softmax yields relative scores on
//! the same [0, 1] simplex as the naive Bayes student, so both share the abstention thresholds and the
//! selective-risk ranking, and the convex loss makes the result insensitive to the visiting order.
//!
//! Training takes `(features, label)` pairs with no provenance field: callers pass human labels only, and
//! a teacher label has no way in (invariant 3). Scores are relative and uncalibrated, never probabilities
//! (invariant 4); every prediction can abstain and serialises with `calibrated: false`.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::features::{tokens, Features};
use crate::student::{canonical_json, validate_labels, Prediction, Suggestion};
use crate::LearnError;

pub const ALGORITHM: &str = "hashed_ngram_softmax_v1";
/// Hash space per class. Small enough for dense weights (labels x 4096 x 8 bytes).
pub const BUCKETS: usize = 1 << 12;
pub const EPOCHS: usize = 30;
pub const LEARNING_RATE: f64 = 0.5;
pub const SEED: u64 = 139;
const MAX_EXAMPLES: usize = 10_000;
const TOP_KNOWN_FEATURES: usize = 12;

/// 64-bit FNV-1a over the UTF-8 bytes. Pinned by a test; changing it invalidates every stored model.
pub fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The unhashed feature strings: every student token, then `<prefix>3:` trigrams of `^word$`.
pub fn feature_strings(features: &Features) -> Vec<String> {
    let mut result = Vec::new();
    for token in tokens(features) {
        if let Some((prefix, word)) = token.split_once(':') {
            if prefix == "word" || prefix == "context" {
                let padded: Vec<char> = format!("^{word}$").chars().collect();
                for gram in padded.windows(3) {
                    result.push(format!("{prefix}3:{}", gram.iter().collect::<String>()));
                }
            }
        }
        result.push(token);
    }
    result
}

/// Sorted `(bucket, value)` pairs, L2-normalised. Empty when the features carry no tokens.
pub fn hashed(features: &Features) -> Vec<(usize, f64)> {
    let mut buckets: Vec<usize> = feature_strings(features)
        .iter()
        .map(|text| (fnv1a(text) % BUCKETS as u64) as usize)
        .collect();
    buckets.sort_unstable();
    let mut result: Vec<(usize, f64)> = Vec::new();
    for bucket in buckets {
        match result.last_mut() {
            Some((last, value)) if *last == bucket => *value += 1.0,
            _ => result.push((bucket, 1.0)),
        }
    }
    let norm = result
        .iter()
        .map(|(_, value)| value * value)
        .sum::<f64>()
        .sqrt();
    for (_, value) in &mut result {
        *value /= norm;
    }
    result
}

fn softmax(logits: &[f64]) -> Vec<f64> {
    let highest = logits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let exps: Vec<f64> = logits.iter().map(|logit| (logit - highest).exp()).collect();
    let total: f64 = exps.iter().sum();
    exps.iter().map(|value| value / total).collect()
}

#[derive(Debug, Clone)]
pub struct LinearStudent {
    labels: Vec<String>,
    /// `weights[class * BUCKETS + bucket]`.
    weights: Vec<f64>,
    bias: Vec<f64>,
    seen: BTreeSet<usize>,
    human_support: Vec<u64>,
    training_count: usize,
    model_id: String,
}

impl LinearStudent {
    /// Train on human-labelled examples only. Deterministic: same input, byte-identical model.
    pub fn fit(labels: &[String], examples: &[(Features, String)]) -> Result<Self, LearnError> {
        validate_labels(&json!(labels))?;
        if examples.len() > MAX_EXAMPLES {
            return Err(LearnError::new("At most 10000 training examples"));
        }
        let classes = labels.len();
        let mut rows = Vec::with_capacity(examples.len());
        let mut human_support = vec![0_u64; classes];
        let mut seen = BTreeSet::new();
        for (features, label) in examples {
            let class = labels
                .iter()
                .position(|known| known == label)
                .ok_or_else(|| LearnError::new("Label is outside the taxonomy"))?;
            let vector = hashed(features);
            seen.extend(vector.iter().map(|(bucket, _)| *bucket));
            human_support[class] += 1;
            rows.push((vector, class));
        }
        let mut weights = vec![0.0_f64; classes * BUCKETS];
        let mut bias = vec![0.0_f64; classes];
        let mut order: Vec<usize> = (0..rows.len()).collect();
        let mut state = SEED;
        for _ in 0..EPOCHS {
            // Fisher-Yates with the LCG `evaluation.rs` uses for its bootstrap.
            for index in (1..order.len()).rev() {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                order.swap(index, ((state >> 33) as usize) % (index + 1));
            }
            for &row in &order {
                let (vector, truth) = &rows[row];
                let scores = softmax(&logits(&weights, &bias, vector, classes));
                for (class, score) in scores.iter().enumerate() {
                    let gradient = score - f64::from(class == *truth);
                    bias[class] -= LEARNING_RATE * gradient;
                    for (bucket, value) in vector {
                        weights[class * BUCKETS + bucket] -= LEARNING_RATE * gradient * value;
                    }
                }
            }
        }
        let document = json!({
            "algorithm": ALGORITHM, "labels": labels,
            "examples": examples.iter().map(|(features, label)| json!([features.to_value(), label])).collect::<Vec<_>>(),
        });
        let digest = Sha256::digest(canonical_json(&document).as_bytes());
        let model_id = digest.iter().take(8).fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
        Ok(Self {
            labels: labels.to_vec(),
            weights,
            bias,
            seen,
            human_support,
            training_count: examples.len(),
            model_id,
        })
    }

    /// The same abstention guards and thresholds as the naive Bayes student (`student.rs`).
    pub fn predict(&self, features: &Features) -> Prediction {
        let vector = hashed(features);
        let known: Vec<&(usize, f64)> = vector
            .iter()
            .filter(|(bucket, _)| self.seen.contains(bucket))
            .collect();
        let coverage = known.len() as f64 / vector.len().max(1) as f64;
        let scores = softmax(&logits(
            &self.weights,
            &self.bias,
            &vector,
            self.labels.len(),
        ));
        let mut suggestions: Vec<Suggestion> = self
            .labels
            .iter()
            .zip(&scores)
            .map(|(label, score)| Suggestion {
                label: label.clone(),
                score: *score,
            })
            .collect();
        suggestions.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.label.cmp(&right.label))
        });
        let first = &suggestions[0];
        let margin = first.score - suggestions.get(1).map_or(0.0, |second| second.score);
        let first_slot = self
            .labels
            .iter()
            .position(|label| *label == first.label)
            .unwrap_or(0);
        let mut reasons = Vec::new();
        if self.training_count == 0 {
            reasons.push("no_training_examples");
        }
        if self.human_support.iter().sum::<u64>() < 4 || self.human_support[first_slot] < 2 {
            reasons.push("insufficient_human_support");
        }
        if known.is_empty() || coverage < 0.35 {
            reasons.push("unfamiliar_features");
        }
        if first.score < 0.8 || margin < 0.2 {
            reasons.push("ambiguous_model_scores");
        }
        let entropy = -suggestions
            .iter()
            .map(|item| item.score * item.score.max(1e-15).ln())
            .sum::<f64>()
            / (suggestions.len().max(2) as f64).ln();
        let flag = if reasons.is_empty() { 0.0 } else { 1.0 };
        let known_features = tokens(features)
            .into_iter()
            .filter(|token| {
                self.seen
                    .contains(&((fnv1a(token) % BUCKETS as u64) as usize))
            })
            .take(TOP_KNOWN_FEATURES)
            .collect();
        Prediction {
            model_id: self.model_id.clone(),
            suggestions,
            abstain: !reasons.is_empty(),
            reasons,
            feature_coverage: coverage,
            known_features,
            review_priority: (0.5 * entropy + 0.35 * (1.0 - coverage) + 0.15 * flag).min(1.0),
        }
    }

    /// Hyperparameters and the non-zero weights in (class, bucket) order. Never claims calibration.
    pub fn to_value(&self) -> Value {
        let weights: Vec<Value> = self
            .weights
            .iter()
            .enumerate()
            .filter(|(_, weight)| **weight != 0.0)
            .map(|(index, weight)| json!([index / BUCKETS, index % BUCKETS, weight]))
            .collect();
        json!({
            "schema_version": 1, "algorithm": ALGORITHM, "buckets": BUCKETS, "epochs": EPOCHS,
            "learning_rate": LEARNING_RATE, "seed": SEED, "labels": self.labels,
            "model_id": self.model_id, "training_count": self.training_count,
            "human_support": self.human_support, "bias": self.bias, "weights": weights,
            "calibrated": false,
        })
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn labels(&self) -> &[String] {
        &self.labels
    }
}

fn logits(weights: &[f64], bias: &[f64], vector: &[(usize, f64)], classes: usize) -> Vec<f64> {
    (0..classes)
        .map(|class| {
            bias[class]
                + vector
                    .iter()
                    .map(|(bucket, value)| weights[class * BUCKETS + bucket] * value)
                    .sum::<f64>()
        })
        .collect()
}
