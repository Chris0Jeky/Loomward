//! Weighted multinomial Naive Bayes with explicit abstention, ported from `class Student`.
//! Every fit is atomic: a rejected batch leaves the previous fitted state untouched.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::features::{integer, tokens, validate_features, Features};
use crate::LearnError;

pub const ALGORITHM: &str = "weighted_multinomial_nb_v1";
pub const DEFAULT_LABELS: [&str; 6] = [
    "Documents",
    "Finance",
    "Media",
    "Projects",
    "Models",
    "Archive",
];
const MAX_LABELS: usize = 32;
const MAX_EVENTS: usize = 10_000;
const MAX_VOCABULARY: usize = 100_000;
const MAX_ID_CHARS: usize = 128;
const MAX_REVISION: u64 = 1_000_000_000;
const TOP_KNOWN_FEATURES: usize = 12;
const SCORE_NOTE: &str =
    "Uncalibrated relative model scores, not a probability that a move is safe.";

/// Supervision source. Human labels weigh 1.0, teacher labels 0.2 (`WEIGHTS` in the reference).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Human,
    Teacher,
}

impl Source {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "human" => Some(Self::Human),
            "teacher" => Some(Self::Teacher),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Teacher => "teacher",
        }
    }

    pub fn weight(self) -> f64 {
        match self {
            Self::Human => 1.0,
            Self::Teacher => 0.2,
        }
    }
}

/// One normalised feedback event (the reference `r` dict).
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub event_id: String,
    pub item_id: String,
    pub revision: u64,
    pub source: Source,
    pub label: String,
    pub features: Features,
    pub retracted: bool,
}

impl Event {
    fn to_value(&self) -> Value {
        json!({
            "event_id": self.event_id,
            "item_id": self.item_id,
            "revision": self.revision,
            "source": self.source.as_str(),
            "label": self.label,
            "features": self.features.to_value(),
            "retracted": self.retracted,
        })
    }

    /// Everything except the event ID, as the reference compares same-revision duplicates.
    fn same_content(&self, other: &Self) -> bool {
        self.item_id == other.item_id
            && self.revision == other.revision
            && self.source == other.source
            && self.label == other.label
            && self.features == other.features
            && self.retracted == other.retracted
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub label: String,
    pub score: f64,
}

/// The reference `predict` result. Scores are relative and never calibrated.
#[derive(Debug, Clone, PartialEq)]
pub struct Prediction {
    pub model_id: String,
    pub suggestions: Vec<Suggestion>,
    pub abstain: bool,
    pub reasons: Vec<&'static str>,
    pub feature_coverage: f64,
    pub known_features: Vec<String>,
    pub review_priority: f64,
}

impl Prediction {
    pub fn to_value(&self) -> Value {
        let suggestions: Vec<Value> = self
            .suggestions
            .iter()
            .map(|suggestion| json!({"label": suggestion.label, "score": suggestion.score}))
            .collect();
        json!({
            "model_id": self.model_id,
            "suggestions": suggestions,
            "abstain": self.abstain,
            "reasons": self.reasons,
            "feature_coverage": self.feature_coverage,
            "known_features": self.known_features,
            "review_priority": self.review_priority,
            "calibrated": false,
            "autonomy_allowed": false,
            "score_note": SCORE_NOTE,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Student {
    labels: Vec<String>,
    events: Vec<Event>,
    counts: Vec<BTreeMap<String, f64>>,
    mass: Vec<f64>,
    human_support: Vec<u64>,
    vocabulary: BTreeSet<String>,
    training_count: usize,
    model_id: String,
}

/// Mirror of `validate_labels`. Accepts a JSON array of 1..=32 short, unique ASCII names.
pub fn validate_labels(value: &Value) -> Result<Vec<String>, LearnError> {
    let items = match value {
        Value::Array(items) if (1..=MAX_LABELS).contains(&items.len()) => items,
        _ => return Err(LearnError::new("Taxonomy must have 1..32 labels")),
    };
    let mut labels = Vec::with_capacity(items.len());
    for item in items {
        match item.as_str() {
            Some(name) if is_short_name(name) => labels.push(name.to_owned()),
            _ => {
                return Err(LearnError::new(
                    "Labels must be short names, not paths or commands",
                ))
            }
        }
    }
    let distinct: BTreeSet<&str> = labels.iter().map(String::as_str).collect();
    if distinct.len() != labels.len() {
        return Err(LearnError::new("Duplicate label"));
    }
    Ok(labels)
}

/// `[A-Za-z][A-Za-z0-9 _-]{0,63}` in full.
fn is_short_name(name: &str) -> bool {
    name.is_ascii()
        && (1..=64).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-'))
}

/// `null` selects the default taxonomy, as `Student(None)` does in the reference.
fn resolve_labels(value: &Value) -> Result<Vec<String>, LearnError> {
    if value.is_null() {
        Ok(DEFAULT_LABELS
            .iter()
            .map(|label| (*label).to_owned())
            .collect())
    } else {
        validate_labels(value)
    }
}

/// A string whose length, counted in code points, lies in 1..=limit.
fn bounded_text(value: Option<&Value>, limit: usize) -> Option<String> {
    match value {
        Some(Value::String(text)) if (1..=limit).contains(&text.chars().count()) => {
            Some(text.clone())
        }
        _ => None,
    }
}

fn parse_event(raw: &Value, labels: &[String]) -> Result<Event, LearnError> {
    let object = raw
        .as_object()
        .ok_or_else(|| LearnError::new("Feedback event must be an object"))?;
    let source = object
        .get("source")
        .and_then(Value::as_str)
        .and_then(Source::parse)
        .ok_or_else(|| LearnError::new("Unknown supervision source"))?;
    let (item_id, event_id) = match (
        bounded_text(object.get("item_id"), MAX_ID_CHARS),
        bounded_text(object.get("event_id"), MAX_ID_CHARS),
    ) {
        (Some(item_id), Some(event_id)) => (item_id, event_id),
        _ => return Err(LearnError::new("Invalid item or event ID")),
    };
    let revision = integer(object.get("revision"), "revision", 1, MAX_REVISION)?;
    let label = object
        .get("label")
        .and_then(Value::as_str)
        .filter(|label| labels.iter().any(|known| known.as_str() == *label))
        .ok_or_else(|| LearnError::new("Label is outside the taxonomy"))?;
    let retracted = match object.get("retracted") {
        None => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => return Err(LearnError::new("retracted must be boolean")),
    };
    let features = validate_features(object.get("features").unwrap_or(&Value::Null))?;
    Ok(Event {
        event_id,
        item_id,
        revision,
        source,
        label: label.to_owned(),
        features,
        retracted,
    })
}

/// Validate and deduplicate a batch, then choose the training set. Returns (normalised, selected).
fn normalise(labels: &[String], events: &Value) -> Result<(Vec<Event>, Vec<Event>), LearnError> {
    let batch = match events {
        Value::Array(batch) if batch.len() <= MAX_EVENTS => batch,
        _ => {
            return Err(LearnError::new(
                "At most 10000 feedback events per prototype training run",
            ))
        }
    };
    let mut normalised: Vec<Event> = Vec::new();
    let mut seen_ids: BTreeMap<String, Event> = BTreeMap::new();
    let mut latest: BTreeMap<(String, Source), Event> = BTreeMap::new();
    for raw in batch {
        let event = parse_event(raw, labels)?;
        if let Some(previous) = seen_ids.get(&event.event_id) {
            if *previous != event {
                return Err(LearnError::new("Conflicting feedback event ID"));
            }
            continue;
        }
        seen_ids.insert(event.event_id.clone(), event.clone());
        normalised.push(event.clone());
        let key = (event.item_id.clone(), event.source);
        if let Some(previous) = latest.get(&key) {
            if previous.revision == event.revision && !previous.same_content(&event) {
                return Err(LearnError::new("Conflicting feedback at the same revision"));
            }
        }
        let supersedes = latest
            .get(&key)
            .is_none_or(|previous| event.revision > previous.revision);
        if supersedes {
            latest.insert(key, event);
        }
    }
    // Per item: the human event if there is one, otherwise the teacher event. A retracted choice drops
    // the item, so an explicit human retraction also hides an older teacher label.
    let items: BTreeSet<String> = latest.keys().map(|(item, _)| item.clone()).collect();
    let mut selected = Vec::new();
    for item in items {
        let human = latest.get(&(item.clone(), Source::Human));
        let chosen = human.or_else(|| latest.get(&(item.clone(), Source::Teacher)));
        if let Some(event) = chosen {
            if !event.retracted {
                selected.push(event.clone());
            }
        }
    }
    Ok((normalised, selected))
}

fn model_id(labels: &[String], selected: &[Event]) -> String {
    let document = json!({
        "labels": labels,
        "selected": selected.iter().map(Event::to_value).collect::<Vec<Value>>(),
    });
    let digest = Sha256::digest(canonical_json(&document).as_bytes());
    digest
        .iter()
        .take(8)
        .fold(String::with_capacity(16), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// `json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True, allow_nan=False)`.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => out.push_str(&number.to_string()),
        Value::String(text) => write_json_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_json_string(key, out);
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
    }
}

/// Python's ASCII-only JSON string escaping: printable ASCII verbatim, short escapes for the usual
/// controls, and lowercase `\uXXXX` per UTF-16 code unit for everything else (DEL included).
fn write_json_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(character),
            _ => {
                let mut units = [0u16; 2];
                for unit in character.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

/// Python `round(value, 6)`: correctly rounded, ties to even (Rust's `{:.6}` rounds the same way).
fn round6(value: f64) -> f64 {
    format!("{value:.6}").parse().unwrap_or(value)
}

impl Student {
    /// `Student(labels)`: the taxonomy, then an empty fit. `null` selects the default taxonomy.
    pub fn new(labels: &Value) -> Result<Self, LearnError> {
        Self::from_parts(resolve_labels(labels)?, Vec::new(), Vec::new())
    }

    fn from_parts(
        labels: Vec<String>,
        events: Vec<Event>,
        selected: Vec<Event>,
    ) -> Result<Self, LearnError> {
        let slots = labels.len();
        let mut counts: Vec<BTreeMap<String, f64>> = vec![BTreeMap::new(); slots];
        let mut mass = vec![0.0_f64; slots];
        let mut human_support = vec![0_u64; slots];
        let mut vocabulary = BTreeSet::new();
        for event in &selected {
            let weight = event.source.weight();
            let terms = tokens(&event.features);
            vocabulary.extend(terms.iter().cloned());
            if vocabulary.len() > MAX_VOCABULARY {
                return Err(LearnError::new("Prototype vocabulary budget exceeded"));
            }
            let slot = labels
                .iter()
                .position(|label| *label == event.label)
                .ok_or_else(|| LearnError::new("Label is outside the taxonomy"))?;
            for term in terms {
                *counts[slot].entry(term).or_insert(0.0) += weight;
            }
            mass[slot] += weight;
            if event.source == Source::Human {
                human_support[slot] += 1;
            }
        }
        let model_id = model_id(&labels, &selected);
        Ok(Self {
            labels,
            events,
            counts,
            mass,
            human_support,
            vocabulary,
            training_count: selected.len(),
            model_id,
        })
    }

    /// `fit(events)`: replaces the fitted state only when the whole batch validates.
    pub fn fit(&mut self, events: &Value) -> Result<(), LearnError> {
        let (normalised, selected) = normalise(&self.labels, events)?;
        *self = Self::from_parts(self.labels.clone(), normalised, selected)?;
        Ok(())
    }

    pub fn predict(&self, features: &Value) -> Result<Prediction, LearnError> {
        let features = validate_features(features)?;
        let all_terms = tokens(&features);
        let known: Vec<String> = all_terms
            .iter()
            .filter(|term| self.vocabulary.contains(*term))
            .cloned()
            .collect();
        let coverage = known.len() as f64 / all_terms.len().max(1) as f64;
        let vocabulary_size = self.vocabulary.len().max(1) as f64;
        let total_mass = self.mass.iter().fold(0.0, |sum, value| sum + value);
        let label_count = self.labels.len() as f64;

        let mut logits = Vec::with_capacity(self.labels.len());
        for (mass, counts) in self.mass.iter().zip(&self.counts) {
            let prior = (mass + 1.0) / (total_mass + label_count);
            let denominator = counts.values().fold(0.0, |sum, value| sum + value) + vocabulary_size;
            let mut likelihood = 0.0;
            for term in &known {
                let count = counts.get(term).copied().unwrap_or(0.0);
                likelihood += ((count + 1.0) / denominator).ln();
            }
            logits.push(prior.ln() + likelihood);
        }
        let highest = logits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let exps: Vec<f64> = logits.iter().map(|logit| (logit - highest).exp()).collect();
        let normaliser = exps.iter().fold(0.0, |sum, value| sum + value);
        let mut suggestions: Vec<Suggestion> = self
            .labels
            .iter()
            .zip(&exps)
            .map(|(label, value)| Suggestion {
                label: label.clone(),
                score: value / normaliser,
            })
            .collect();
        suggestions.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.label.cmp(&right.label))
        });

        let first = &suggestions[0];
        let margin = first.score - suggestions.get(1).map_or(0.0, |second| second.score);
        let entropy_sum = suggestions.iter().fold(0.0, |sum, item| {
            sum + item.score * item.score.max(1e-15).ln()
        });
        let entropy = -entropy_sum / (suggestions.len().max(2) as f64).ln();
        let first_slot = self
            .labels
            .iter()
            .position(|label| *label == first.label)
            .unwrap_or(0);

        let mut reasons = Vec::new();
        if self.training_count == 0 {
            reasons.push("no_training_examples");
        }
        let total_human: u64 = self.human_support.iter().sum();
        if total_human < 4 || self.human_support[first_slot] < 2 {
            reasons.push("insufficient_human_support");
        }
        if known.is_empty() || coverage < 0.35 {
            reasons.push("unfamiliar_features");
        }
        if first.score < 0.8 || margin < 0.2 {
            reasons.push("ambiguous_model_scores");
        }

        let flag = if reasons.is_empty() { 0.0 } else { 1.0 };
        let review_priority = (0.5 * entropy + 0.35 * (1.0 - coverage) + 0.15 * flag).min(1.0);
        Ok(Prediction {
            model_id: self.model_id.clone(),
            suggestions,
            abstain: !reasons.is_empty(),
            reasons,
            feature_coverage: round6(coverage),
            known_features: known.into_iter().take(TOP_KNOWN_FEATURES).collect(),
            review_priority: round6(review_priority),
        })
    }

    /// `to_dict()`: the normalised events and the taxonomy, enough to rebuild the same model.
    pub fn to_value(&self) -> Value {
        json!({
            "schema_version": 1,
            "algorithm": ALGORITHM,
            "labels": self.labels,
            "events": self.events.iter().map(Event::to_value).collect::<Vec<Value>>(),
            "model_id": self.model_id,
            "calibrated": false,
        })
    }

    /// `from_dict(value)`: rebuilds by refitting the stored events. The stored model_id is not trusted.
    pub fn from_value(value: &Value) -> Result<Self, LearnError> {
        let object = value.as_object();
        let version_ok = matches!(
            object.and_then(|map| map.get("schema_version")),
            Some(Value::Number(number)) if number.as_u64() == Some(1)
        );
        let algorithm_ok = object
            .and_then(|map| map.get("algorithm"))
            .and_then(Value::as_str)
            == Some(ALGORITHM);
        let object = match object {
            Some(map) if version_ok && algorithm_ok => map,
            _ => return Err(LearnError::new("Unsupported model format")),
        };
        let mut student = Self::new(object.get("labels").unwrap_or(&Value::Null))?;
        student.fit(object.get("events").unwrap_or(&Value::Null))?;
        Ok(student)
    }

    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn training_count(&self) -> usize {
        self.training_count
    }

    /// Human events per label, in taxonomy order.
    pub fn human_support(&self) -> &[u64] {
        &self.human_support
    }

    /// Weighted event mass per label, in taxonomy order.
    pub fn mass(&self) -> &[f64] {
        &self.mass
    }

    pub fn vocabulary_len(&self) -> usize {
        self.vocabulary.len()
    }
}
