//! The find-bar query grammar (docs/46 section 4.1, F1): free text in, structured filters out.
//!
//! Pure and total: every input parses, anything it cannot honour becomes an entry in `issues`
//! rather than a silent drop (invariant 4). It reads no clock, no catalogue and no file: relative
//! times stay symbolic until [`ParsedQuery::modified_bounds`] is given a `now`, and collection or
//! node names stay names (the caller resolves them to `collection_id` / `under_node_id`).
//!
//! The TypeScript mirror is `app/src/lib/search/grammar.ts`; both are held to the shared
//! `fixtures/v3/query-grammar-parity.json` (every case must produce the same JSON), and the
//! rules are written down in `contracts/v3/semantics.md` ("Query grammar").

use crate::dto::{SearchRequestKind, SearchTerm, SearchTermMode};
use crate::types::{BoundedStr, Bytes, Timestamp};
use serde::Serialize;

const KEYS: [&str; 6] = ["ext", "size", "modified", "kind", "in", "under"];
const MAX_TERMS: usize = 8;
const MAX_TERM_CHARS: usize = 64;
const MAX_EXTENSIONS: usize = 16;
const MAX_EXTENSION_CHARS: usize = 32;
const ISSUE_TOKEN_CHARS: usize = 64;
/// 1601-01-01T00:00:00Z, the oldest instant a Windows file time can name.
const EPOCH_1601_MS: i64 = -11_644_473_600_000;
const DAY_MS: i64 = 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryIssueCode {
    UnterminatedQuote,
    BadValue,
    NegatedFilter,
    TooManyTerms,
    TermTooLong,
    TooManyExtensions,
}

/// Something the grammar could not honour; the offending text is kept (cut to 64 characters).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueryIssue {
    pub code: QueryIssueCode,
    pub token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgoUnit {
    H,
    D,
    W,
    Mo,
    Y,
}

impl AgoUnit {
    /// `mo` is 30 days and `y` is 365 days; calendar months and years are not worth a clock.
    fn ms(self) -> i64 {
        match self {
            AgoUnit::H => 3_600_000,
            AgoUnit::D => DAY_MS,
            AgoUnit::W => 7 * DAY_MS,
            AgoUnit::Mo => 30 * DAY_MS,
            AgoUnit::Y => 365 * DAY_MS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ago {
    pub n: u32,
    pub unit: AgoUnit,
}

/// One side of a modified-time range, inclusive: a fixed instant or "this long before now".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeBound {
    At(Timestamp),
    Ago(Ago),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParsedQuery {
    pub terms: Vec<SearchTerm>,
    pub extensions: Vec<String>,
    pub min_bytes: Option<Bytes>,
    pub max_bytes: Option<Bytes>,
    pub modified_after: Option<TimeBound>,
    pub modified_before: Option<TimeBound>,
    pub kind: SearchRequestKind,
    pub in_collection: Option<String>,
    pub under: Option<String>,
    pub issues: Vec<QueryIssue>,
}

impl ParsedQuery {
    /// The modified range as request timestamps, with relative bounds taken from `now_ms`
    /// (milliseconds since the Unix epoch). A relative bound older than 1601 is clamped to 1601.
    pub fn modified_bounds(&self, now_ms: i64) -> (Option<Timestamp>, Option<Timestamp>) {
        let resolve = |b: &Option<TimeBound>| match b {
            None => None,
            Some(TimeBound::At(t)) => Some(t.clone()),
            Some(TimeBound::Ago(a)) => {
                let ms = now_ms
                    .saturating_sub(i64::from(a.n) * a.unit.ms())
                    .max(EPOCH_1601_MS);
                format_ms(ms).and_then(|s| Timestamp::new(s).ok())
            }
        };
        (
            resolve(&self.modified_after),
            resolve(&self.modified_before),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

fn is_sep(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn raw(cs: &[char]) -> String {
    cs.iter().take(ISSUE_TOKEN_CHARS).collect()
}

fn split_cmp(v: &str) -> (Cmp, &str) {
    for (p, c) in [
        (">=", Cmp::Ge),
        ("<=", Cmp::Le),
        (">", Cmp::Gt),
        ("<", Cmp::Lt),
        ("=", Cmp::Eq),
    ] {
        if let Some(rest) = v.strip_prefix(p) {
            return (c, rest);
        }
    }
    (Cmp::Eq, v)
}

/// Reads from `from` to the closing quote (or the end). Returns the text, the index after the
/// quote and whether a quote closed it.
fn read_quoted(cs: &[char], from: usize) -> (String, usize, bool) {
    match cs[from..].iter().position(|&c| c == '"') {
        Some(n) => (cs[from..from + n].iter().collect(), from + n + 1, true),
        None => (cs[from..].iter().collect(), cs.len(), false),
    }
}

pub fn parse_query(input: &str) -> ParsedQuery {
    let cs: Vec<char> = input.chars().collect();
    let mut q = ParsedQuery {
        terms: Vec::new(),
        extensions: Vec::new(),
        min_bytes: None,
        max_bytes: None,
        modified_after: None,
        modified_before: None,
        kind: SearchRequestKind::Any,
        in_collection: None,
        under: None,
        issues: Vec::new(),
    };
    let mut i = 0;
    while i < cs.len() {
        if is_sep(cs[i]) {
            i += 1;
            continue;
        }
        let start = i;
        let negated = cs[i] == '-' && i + 1 < cs.len() && !is_sep(cs[i + 1]);
        if negated {
            i += 1;
        }
        if cs[i] == '"' {
            let (text, next, closed) = read_quoted(&cs, i + 1);
            i = next;
            if !closed {
                q.issue(QueryIssueCode::UnterminatedQuote, &cs[start..i]);
            }
            q.term(text, SearchTermMode::Substring, negated, &cs[start..i]);
            continue;
        }
        // A word runs to the next separator, unless it is `key:"quoted value"`.
        let mut end = i;
        while end < cs.len() && !is_sep(cs[end]) && cs[end] != ':' {
            end += 1;
        }
        let key: String = cs[i..end].iter().collect::<String>().to_ascii_lowercase();
        if end < cs.len() && cs[end] == ':' && KEYS.contains(&key.as_str()) {
            let (value, next) = if cs.get(end + 1) == Some(&'"') {
                let (v, n, closed) = read_quoted(&cs, end + 2);
                if !closed {
                    q.issue(QueryIssueCode::UnterminatedQuote, &cs[start..n]);
                }
                (v, n)
            } else {
                let mut n = end + 1;
                while n < cs.len() && !is_sep(cs[n]) {
                    n += 1;
                }
                (cs[end + 1..n].iter().collect(), n)
            };
            i = next;
            if negated {
                q.issue(QueryIssueCode::NegatedFilter, &cs[start..i]);
            } else {
                q.filter(&key, &value, &cs[start..i]);
            }
            continue;
        }
        while end < cs.len() && !is_sep(cs[end]) {
            end += 1;
        }
        let word: String = cs[i..end].iter().collect();
        q.term(word, SearchTermMode::Token, negated, &cs[start..end]);
        i = end;
    }
    q
}

impl ParsedQuery {
    fn issue(&mut self, code: QueryIssueCode, token: &[char]) {
        self.issues.push(QueryIssue {
            code,
            token: raw(token),
        });
    }

    fn term(&mut self, text: String, mode: SearchTermMode, negated: bool, token: &[char]) {
        if text.is_empty() {
            return;
        }
        let Some(text) = (text.chars().count() <= MAX_TERM_CHARS)
            .then(|| BoundedStr::<1, 64>::new(text).ok())
            .flatten()
        else {
            return self.issue(QueryIssueCode::TermTooLong, token);
        };
        if self.terms.len() == MAX_TERMS {
            return self.issue(QueryIssueCode::TooManyTerms, token);
        }
        self.terms.push(SearchTerm {
            text,
            mode,
            negated,
        });
    }

    fn filter(&mut self, key: &str, value: &str, token: &[char]) {
        let ok = match key {
            "ext" => self.ext(value, token),
            "size" => self.size(value),
            "modified" => self.modified(value),
            "kind" => match value.to_ascii_lowercase().as_str() {
                "file" => {
                    self.kind = SearchRequestKind::File;
                    true
                }
                "dir" => {
                    self.kind = SearchRequestKind::Dir;
                    true
                }
                _ => false,
            },
            "in" | "under" if !value.is_empty() => {
                let slot = if key == "in" {
                    &mut self.in_collection
                } else {
                    &mut self.under
                };
                *slot = Some(value.to_string());
                true
            }
            _ => false,
        };
        if !ok {
            self.issue(QueryIssueCode::BadValue, token);
        }
    }

    /// `ext:pdf;docx`: a set, accumulated over repeated tokens. All-or-nothing per token.
    fn ext(&mut self, value: &str, token: &[char]) -> bool {
        let mut set = Vec::new();
        for part in value.split(';').filter(|p| !p.is_empty()) {
            let e = part.strip_prefix('.').unwrap_or(part).to_ascii_lowercase();
            let bad = |c: char| c.is_control() || c.is_whitespace() || "\\/:*?\"<>|;".contains(c);
            if e.is_empty() || e.chars().count() > MAX_EXTENSION_CHARS || e.chars().any(bad) {
                return false;
            }
            set.push(e);
        }
        if set.is_empty() {
            return false;
        }
        for e in set {
            if self.extensions.contains(&e) {
                continue;
            }
            if self.extensions.len() == MAX_EXTENSIONS {
                self.issue(QueryIssueCode::TooManyExtensions, token);
                break;
            }
            self.extensions.push(e);
        }
        true
    }

    /// `size:>100mb`. Bounds are inclusive on the wire, so `>` and `<` move by one byte.
    /// A later token replaces the same side.
    fn size(&mut self, value: &str) -> bool {
        let (cmp, rest) = split_cmp(value);
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        let (num, unit) = rest.split_at(digits);
        let shift = match unit.to_ascii_lowercase().as_str() {
            "" | "b" => 0,
            "kb" => 10,
            "mb" => 20,
            "gb" => 30,
            "tb" => 40,
            _ => return false,
        };
        if num.is_empty() || num.len() > 20 {
            return false;
        }
        let Some(v) = num
            .parse::<u128>()
            .ok()
            .map(|n| n << shift)
            .filter(|n| *n <= u128::from(u64::MAX))
            .map(|n| n as u64)
        else {
            return false;
        };
        // An exclusive bound that cannot be named (above u64::MAX, below zero) is a bad value.
        let (min, max): (Option<u64>, Option<u64>) = match cmp {
            Cmp::Gt => match v.checked_add(1) {
                Some(m) => (Some(m), None),
                None => return false,
            },
            Cmp::Ge => (Some(v), None),
            Cmp::Lt => match v.checked_sub(1) {
                Some(m) => (None, Some(m)),
                None => return false,
            },
            Cmp::Le => (None, Some(v)),
            Cmp::Eq => (Some(v), Some(v)),
        };
        self.min_bytes = min.map(Bytes).or(self.min_bytes);
        self.max_bytes = max.map(Bytes).or(self.max_bytes);
        true
    }

    /// `modified:2025`, `modified:2025-03`, `modified:2025-03-14`, `modified:<30d`. Relative
    /// values compare the age: `<30d` is newer than 30 days, `>30d` older. A later token
    /// replaces the same side.
    fn modified(&mut self, value: &str) -> bool {
        let (cmp, rest) = split_cmp(value);
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        let unit = match rest[digits..].to_ascii_lowercase().as_str() {
            "h" => Some(AgoUnit::H),
            "d" => Some(AgoUnit::D),
            "w" => Some(AgoUnit::W),
            "mo" => Some(AgoUnit::Mo),
            "y" => Some(AgoUnit::Y),
            _ => None,
        };
        if let Some(unit) = unit {
            let Ok(n) = rest[..digits].parse::<u32>() else {
                return false;
            };
            if digits > 5 || n == 0 {
                return false;
            }
            let ago = Some(TimeBound::Ago(Ago { n, unit }));
            match cmp {
                Cmp::Lt | Cmp::Le => self.modified_after = ago,
                Cmp::Gt | Cmp::Ge => self.modified_before = ago,
                Cmp::Eq => return false,
            }
            return true;
        }
        let Some((start, end)) = date_span(rest) else {
            return false;
        };
        let (after, before) = match cmp {
            Cmp::Eq => (Some(start), Some(end)),
            Cmp::Gt => (Some(end + 1), None),
            Cmp::Ge => (Some(start), None),
            Cmp::Lt => (None, Some(start - 1)),
            Cmp::Le => (None, Some(end)),
        };
        let at = |ms: Option<i64>| -> Result<Option<TimeBound>, ()> {
            match ms {
                None => Ok(None),
                Some(ms) => format_ms(ms)
                    .and_then(|s| Timestamp::new(s).ok())
                    .map(|t| Some(TimeBound::At(t)))
                    .ok_or(()),
            }
        };
        let (Ok(after), Ok(before)) = (at(after), at(before)) else {
            return false;
        };
        self.modified_after = after.or(self.modified_after.take());
        self.modified_before = before.or(self.modified_before.take());
        true
    }
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `YYYY`, `YYYY-MM` or `YYYY-MM-DD` (UTC) as its inclusive millisecond span.
fn date_span(s: &str) -> Option<(i64, i64)> {
    let b = s.as_bytes();
    let num = |a: usize, z: usize| -> Option<i64> {
        let part = s.get(a..z)?;
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let y = num(0, 4)?;
    if !(1601..=9999).contains(&y) {
        return None;
    }
    let (m, d, whole_year, whole_month) = match b.len() {
        4 => (1, 1, true, false),
        7 if b[4] == b'-' => (num(5, 7)?, 1, false, true),
        10 if b[4] == b'-' && b[7] == b'-' => (num(5, 7)?, num(8, 10)?, false, false),
        _ => return None,
    };
    if !(1..=12).contains(&m) || !(1..=days_in_month(y, m)).contains(&d) {
        return None;
    }
    let start = days_from_civil(y, m, d) * DAY_MS;
    let days = if whole_year {
        days_from_civil(y + 1, 1, 1) - days_from_civil(y, 1, 1)
    } else if whole_month {
        days_in_month(y, m)
    } else {
        1
    };
    Some((start, start + days * DAY_MS - 1))
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ`, or `None` outside years 0..=9999.
fn format_ms(ms: i64) -> Option<String> {
    let (y, m, d) = civil_from_days(ms.div_euclid(DAY_MS));
    if !(0..=9999).contains(&y) {
        return None;
    }
    let t = ms.rem_euclid(DAY_MS);
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        t / 3_600_000,
        t / 60_000 % 60,
        t / 1000 % 60,
        t % 1000
    ))
}
