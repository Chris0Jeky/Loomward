//! Feature validation and tokenisation, ported from `validate_features`, `integer` and `tokens`.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use unicode_general_category::{get_general_category, GeneralCategory};

use crate::LearnError;

pub const NAME_LIMIT: usize = 256;
pub const EXTENSION_LIMIT: usize = 32;
pub const CONTEXT_LIMIT: usize = 512;
/// Only the first words of a name or context are tokenised.
const WORD_CAP: usize = 64;
/// `2**53 - 1`, the largest size the reference accepts.
pub const SIZE_MAX: u64 = (1 << 53) - 1;
const FEATURE_KEYS: [&str; 4] = ["name", "extension", "context", "size_bytes"];

/// Validated metadata. Raw content never appears here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Features {
    pub name: String,
    pub extension: String,
    pub context: String,
    pub size_bytes: u64,
}

impl Features {
    pub fn to_value(&self) -> Value {
        json!({
            "name": self.name,
            "extension": self.extension,
            "context": self.context,
            "size_bytes": self.size_bytes,
        })
    }
}

/// Mirror of the reference `integer(value, name, minimum, maximum)`: only JSON integers in range pass.
pub(crate) fn integer(
    value: Option<&Value>,
    name: &str,
    minimum: u64,
    maximum: u64,
) -> Result<u64, LearnError> {
    let parsed = match value {
        Some(Value::Number(number)) => number.as_u64(),
        _ => None,
    };
    match parsed {
        Some(found) if (minimum..=maximum).contains(&found) => Ok(found),
        _ => Err(LearnError::new(format!(
            "{name} must be an integer in [{minimum}, {maximum}]"
        ))),
    }
}

pub fn validate_features(value: &Value) -> Result<Features, LearnError> {
    let object = match value.as_object() {
        Some(object)
            if object
                .keys()
                .all(|key| FEATURE_KEYS.contains(&key.as_str())) =>
        {
            object
        }
        _ => {
            return Err(LearnError::new(
                "Only bounded name, extension, context and size metadata are permitted",
            ))
        }
    };
    let text = |key: &str, limit: usize| -> Result<String, LearnError> {
        let candidate = match object.get(key) {
            None => Some(String::new()),
            Some(Value::String(found)) => Some(found.clone()),
            Some(_) => None,
        };
        match candidate {
            Some(found) if found.chars().count() <= limit && !found.contains('\0') => Ok(found),
            _ => Err(LearnError::new(format!("Invalid {key}"))),
        }
    };
    let name = text("name", NAME_LIMIT)?;
    let extension = text("extension", EXTENSION_LIMIT)?;
    let context = text("context", CONTEXT_LIMIT)?;
    let size_bytes = match object.get("size_bytes") {
        None => 0,
        some => integer(some, "size_bytes", 0, SIZE_MAX)?,
    };
    Ok(Features {
        name,
        extension,
        context,
        size_bytes,
    })
}

/// Python `str.casefold()`: full case folding (CaseFolding C+F), applied per code point.
pub fn casefold(text: &str) -> String {
    caseless::default_case_fold_str(text)
}

/// Python `\w` minus underscore, i.e. `[^\W_]`: general categories L* and N*.
fn is_word_character(character: char) -> bool {
    matches!(
        get_general_category(character),
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::ModifierLetter
            | GeneralCategory::OtherLetter
            | GeneralCategory::DecimalNumber
            | GeneralCategory::LetterNumber
            | GeneralCategory::OtherNumber
    )
}

/// Maximal runs of word characters, as `re.findall(r'[^\W_]+', text)` returns them.
pub fn word_runs(text: &str) -> Vec<String> {
    let mut runs = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if is_word_character(character) {
            current.push(character);
        } else if !current.is_empty() {
            runs.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

/// Python `str.isdecimal()` for a non-empty run: every character is Nd.
fn is_decimal(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|c| get_general_category(c) == GeneralCategory::DecimalNumber)
}

/// `int(math.log2(size)) // 4` for a positive size, computed with binary floats exactly as Python does.
/// Binary floats round `log2(2**k - 1)` up to `k` for large `k`, and the reference inherits that.
pub fn size_bucket(size_bytes: u64) -> Option<u64> {
    if size_bytes == 0 {
        None
    } else {
        Some((size_bytes as f64).log2() as u64 / 4)
    }
}

/// The token set of the reference `tokens(features)`, sorted by code point as Python's `sorted` does.
pub fn tokens(features: &Features) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    for (text, prefix) in [(&features.name, "word"), (&features.context, "context")] {
        for word in word_runs(&casefold(text)).into_iter().take(WORD_CAP) {
            if word.chars().count() > 1 && !is_decimal(&word) {
                result.insert(format!("{prefix}:{word}"));
            }
        }
    }
    if !features.extension.is_empty() {
        let folded = casefold(&features.extension);
        result.insert(format!("extension:{}", folded.trim_start_matches('.')));
    }
    if let Some(bucket) = size_bucket(features.size_bytes) {
        result.insert(format!("size_bucket:{bucket}"));
    }
    result
}
