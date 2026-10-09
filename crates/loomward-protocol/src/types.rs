//! Wire primitives shared by every DTO.
//!
//! Each type deserialises only values the JSON Schema accepts (and, where Rust is
//! stricter, says so): deserialising IS validating, so a malformed payload can never
//! reach a handler as a half-checked struct. Serialisation is lossless, so a valid
//! example round-trips to an equal `serde_json::Value`.

use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::hash::Hash;
use std::marker::PhantomData;
use std::ops::Deref;

/// Largest integer a JavaScript number holds exactly: the ceiling for every wire `Count`.
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// A value violated a contract bound. The message names the bound, never the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid(pub String);

impl Invalid {
    pub fn new(message: impl Into<String>) -> Self {
        Invalid(message.into())
    }
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Invalid {}

// ---------------------------------------------------------------------------------------
// Optional-field helpers (used through `#[serde(deserialize_with = ...)]`)
// ---------------------------------------------------------------------------------------

/// A required property whose value may be `null`. Serde would otherwise treat a missing
/// `Option` field as `None`; the schema lists it under `required`, so absent must fail.
pub fn required_nullable<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d)
}

/// An optional property that must not be `null` when present.
pub fn optional_present<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(d).map(Some)
}

/// An optional property that may also be `null`: absent is `None`, `null` is `Some(None)`.
pub fn optional_nullable<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

// ---------------------------------------------------------------------------------------
// Decimal-string integers (ADR-V3-04): Bytes and Generation
// ---------------------------------------------------------------------------------------

/// Exact unsigned decimal: `0` or no leading zero, at most 20 digits, and it must fit `u64`
/// (the schema pattern alone also admits 99999999999999999999; Rust is stricter).
fn parse_decimal(s: &str) -> Result<u64, Invalid> {
    let ok = !s.is_empty()
        && s.len() <= 20
        && s.bytes().all(|b| b.is_ascii_digit())
        && (s == "0" || !s.starts_with('0'));
    if !ok {
        return Err(Invalid::new(
            "expected an unsigned decimal string without leading zeros",
        ));
    }
    s.parse::<u64>()
        .map_err(|_| Invalid::new("decimal string exceeds u64"))
}

macro_rules! decimal_newtype {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        pub struct $name(pub u64);

        impl $name {
            pub fn get(self) -> u64 {
                self.0
            }
        }
        impl From<u64> for $name {
            fn from(v: u64) -> Self {
                Self(v)
            }
        }
        impl std::str::FromStr for $name {
            type Err = Invalid;
            fn from_str(s: &str) -> Result<Self, Invalid> {
                parse_decimal(s).map(Self)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(&self.0)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct V;
                impl Visitor<'_> for V {
                    type Value = u64;
                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("an unsigned decimal string (never a JSON number)")
                    }
                    fn visit_str<E: de::Error>(self, v: &str) -> Result<u64, E> {
                        parse_decimal(v).map_err(E::custom)
                    }
                }
                d.deserialize_str(V).map($name)
            }
        }
    };
}

decimal_newtype!(
    /// A byte quantity. Serialised as a decimal string, never a JSON number (ADR-V3-04).
    /// Unknown is `Option<Bytes>::None`, which serialises as `null`, never as zero.
    Bytes
);
decimal_newtype!(
    /// A catalogue generation counter, a decimal string like `Bytes`.
    Generation
);

// ---------------------------------------------------------------------------------------
// Bounded numbers
// ---------------------------------------------------------------------------------------

/// A bounded count: a JSON integer in `0..=2^53-1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Count(u64);

impl Count {
    pub const ZERO: Count = Count(0);

    pub fn new(v: u64) -> Result<Self, Invalid> {
        if v <= MAX_SAFE_INTEGER {
            Ok(Self(v))
        } else {
            Err(Invalid::new("count exceeds 2^53-1"))
        }
    }
    /// Clamps instead of failing; for engine totals that cannot realistically overflow.
    pub fn saturating(v: u64) -> Self {
        Self(v.min(MAX_SAFE_INTEGER))
    }
    pub fn get(self) -> u64 {
        self.0
    }
}

impl From<u32> for Count {
    fn from(v: u32) -> Self {
        Self(u64::from(v))
    }
}

impl Serialize for Count {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(self.0)
    }
}

impl<'de> Deserialize<'de> for Count {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Count::new(u64::deserialize(d)?).map_err(de::Error::custom)
    }
}

/// An integer bounded to `MIN..=MAX` (limits, depths, tiers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Int<const MIN: i64, const MAX: i64>(i64);

impl<const MIN: i64, const MAX: i64> Int<MIN, MAX> {
    pub fn new(v: i64) -> Result<Self, Invalid> {
        if (MIN..=MAX).contains(&v) {
            Ok(Self(v))
        } else {
            Err(Invalid::new(format!("integer outside {MIN}..={MAX}")))
        }
    }
    pub fn get(self) -> i64 {
        self.0
    }
}

impl<const MIN: i64, const MAX: i64> TryFrom<i64> for Int<MIN, MAX> {
    type Error = Invalid;
    fn try_from(v: i64) -> Result<Self, Invalid> {
        Self::new(v)
    }
}

impl<const MIN: i64, const MAX: i64> Serialize for Int<MIN, MAX> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i64(self.0)
    }
}

impl<'de, const MIN: i64, const MAX: i64> Deserialize<'de> for Int<MIN, MAX> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(i64::deserialize(d)?).map_err(de::Error::custom)
    }
}

/// A tier: 0 is fastest, 9 slowest.
pub type Tier = Int<0, 9>;

macro_rules! float_newtype {
    ($(#[$m:meta])* $name:ident, $what:literal, $ok:expr) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
        pub struct $name(f64);

        impl $name {
            pub fn new(v: f64) -> Result<Self, Invalid> {
                let ok: fn(f64) -> bool = $ok;
                if v.is_finite() && ok(v) {
                    Ok(Self(v))
                } else {
                    Err(Invalid::new(concat!("expected ", $what)))
                }
            }
            pub fn get(self) -> f64 {
                self.0
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_f64(self.0)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                $name::new(f64::deserialize(d)?).map_err(de::Error::custom)
            }
        }
    };
}

float_newtype!(
    /// A ratio in `0..=1`. Scores are uncalibrated relative values, not probabilities.
    Fraction,
    "a number in 0..=1",
    |v| (0.0..=1.0).contains(&v)
);
float_newtype!(
    /// A display-only non-negative number such as a per-second rate. Never authority.
    Rate,
    "a non-negative number",
    |v| v >= 0.0
);
float_newtype!(
    /// The `min_share` cutoff of a slice request: `0..=0.1`.
    MinShare,
    "a number in 0..=0.1",
    |v| (0.0..=0.1).contains(&v)
);

// ---------------------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------------------

/// A string of `MIN..=MAX` Unicode scalar values (JSON Schema counts code points).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BoundedStr<const MIN: usize, const MAX: usize>(String);

/// Free text up to `MAX` characters.
pub type Text<const MAX: usize> = BoundedStr<0, MAX>;

impl<const MIN: usize, const MAX: usize> BoundedStr<MIN, MAX> {
    pub fn new(v: impl Into<String>) -> Result<Self, Invalid> {
        let v = v.into();
        let n = v.chars().count();
        if (MIN..=MAX).contains(&n) {
            Ok(Self(v))
        } else {
            Err(Invalid::new(format!(
                "string length outside {MIN}..={MAX} characters"
            )))
        }
    }
    /// Cuts to `MAX` characters. For display text the engine builds itself (names, notes);
    /// a lossy cut is flagged elsewhere (`name_lossy`), never silent for identifiers.
    pub fn truncated(v: &str) -> Self {
        Self(v.chars().take(MAX).collect())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<const MIN: usize, const MAX: usize> Deref for BoundedStr<MIN, MAX> {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl<const MIN: usize, const MAX: usize> TryFrom<&str> for BoundedStr<MIN, MAX> {
    type Error = Invalid;
    fn try_from(v: &str) -> Result<Self, Invalid> {
        Self::new(v)
    }
}

impl<const MIN: usize, const MAX: usize> Serialize for BoundedStr<MIN, MAX> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de, const MIN: usize, const MAX: usize> Deserialize<'de> for BoundedStr<MIN, MAX> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(d)?).map_err(de::Error::custom)
    }
}

/// A string newtype checked by a predicate that mirrors the schema `pattern`.
macro_rules! string_newtype {
    ($(#[$m:meta])* $name:ident, $what:literal, $check:expr) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, Invalid> {
                let v = value.into();
                let check: fn(&str) -> bool = $check;
                if check(&v) {
                    Ok(Self(v))
                } else {
                    Err(Invalid::new(concat!("expected ", $what)))
                }
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }
        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl TryFrom<&str> for $name {
            type Error = Invalid;
            fn try_from(v: &str) -> Result<Self, Invalid> {
                Self::new(v)
            }
        }
        impl TryFrom<String> for $name {
            type Error = Invalid;
            fn try_from(v: String) -> Result<Self, Invalid> {
                Self::new(v)
            }
        }
        impl std::str::FromStr for $name {
            type Err = Invalid;
            fn from_str(v: &str) -> Result<Self, Invalid> {
                Self::new(v)
            }
        }
        impl From<$name> for String {
            fn from(v: $name) -> String {
                v.0
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.0)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Self::new(String::deserialize(d)?).map_err(de::Error::custom)
            }
        }
    };
}

fn slug_len(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn prefixed(s: &str, prefix: &str) -> bool {
    s.strip_prefix(prefix)
        .is_some_and(|rest| slug_len(rest, 1, 64))
}

macro_rules! id_newtype {
    ($(#[$m:meta])* $name:ident, $prefix:literal) => {
        string_newtype!(
            $(#[$m])*
            $name,
            "an opaque prefixed identifier",
            |s| prefixed(s, $prefix)
        );
    };
}

id_newtype!(
    /// Root grant id.
    RootId, "rt_");
id_newtype!(
    /// Volume id.
    VolumeId, "vo_");
id_newtype!(
    /// Opaque, session-tagged catalogue row id. Identifies a row for viewing only; never an
    /// effect operand (invariant 5).
    NodeId, "nd_");
id_newtype!(
    /// Job id.
    JobId, "jb_");
id_newtype!(
    /// Root or disclosure grant id.
    GrantId, "gr_");
id_newtype!(
    /// Collection id.
    CollectionId, "co_");
id_newtype!(
    /// Saved placement simulation id.
    ProposalId, "pp_");
id_newtype!(
    /// Telemetry subscription id.
    SubscriptionId, "sb_");
id_newtype!(
    /// Teacher payload preview id.
    PreviewId, "tp_");
id_newtype!(
    /// Process reference; encodes pid plus start time so PID reuse cannot alias. Observation only.
    ProcessRef, "pc_");
id_newtype!(
    /// Student model id.
    ModelId, "md_");
id_newtype!(
    /// Feedback event id.
    EventId, "fe_");
id_newtype!(
    /// Placement candidate group id.
    GroupId, "cg_");

string_newtype!(
    /// Caller-chosen request correlation id.
    RequestId,
    "1-64 characters of [A-Za-z0-9_-]",
    |s| slug_len(s, 1, 64)
);

string_newtype!(
    /// Dotted command name, `^[a-z]+(\.[a-z_]+)+$`, at most 64 characters. Carried as a string on
    /// the wire so an unknown command yields `unknown_command` rather than a parse failure.
    CommandName,
    "a dotted lowercase command name",
    |s| {
        let mut parts = s.split('.');
        let first_ok = parts
            .next()
            .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase()));
        let mut rest = 0;
        let rest_ok = parts.all(|p| {
            rest += 1;
            !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        });
        s.len() <= 64 && first_ok && rest_ok && rest > 0
    }
);

string_newtype!(
    /// Opaque keyset cursor, bound server-side to command, query, anchor, generation and session.
    Cursor,
    "1-512 characters of [A-Za-z0-9_-]",
    |s| slug_len(s, 1, 512)
);

string_newtype!(
    /// `sha256:` followed by 64 lowercase hex digits.
    Digest,
    "sha256: followed by 64 lowercase hex digits",
    |s| s
        .strip_prefix("sha256:")
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
);

string_newtype!(
    /// A taxonomy label: a letter, then up to 63 of letters, digits, space, `_`, `-`.
    Label,
    "a taxonomy label",
    |s| {
        let mut chars = s.chars();
        chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && s.len() <= 64
            && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-'))
    }
);

string_newtype!(
    /// Client-chosen idempotency key for a feedback event.
    ClientEventId,
    "8-64 characters of [A-Za-z0-9_-]",
    |s| slug_len(s, 8, 64)
);

string_newtype!(
    /// A request-local teacher item handle, `i00` to `i99`.
    TeacherHandle,
    "a handle like i07",
    |s| {
        let b = s.as_bytes();
        b.len() == 3 && b[0] == b'i' && b[1].is_ascii_digit() && b[2].is_ascii_digit()
    }
);

string_newtype!(
    /// Lowercase hex file id, 16 to 32 digits.
    FileIdHex,
    "16-32 lowercase hex digits",
    |s| (16..=32).contains(&s.len()) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
);

string_newtype!(
    /// A machine code like `volume_offline`.
    FactCode,
    "1-64 characters of [a-z_]",
    |s| (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
);

string_newtype!(
    /// A collection name: 1 to 64 characters, no control characters.
    CollectionName,
    "1-64 characters without control characters",
    |s| (1..=64).contains(&s.chars().count()) && s.chars().all(|c| c >= '\u{20}')
);

fn two(b: &[u8]) -> Option<u32> {
    match b {
        [a @ b'0'..=b'9', c @ b'0'..=b'9'] => Some(u32::from(a - b'0') * 10 + u32::from(c - b'0')),
        _ => None,
    }
}

/// `YYYY-MM-DDTHH:MM:SS[.f{1,9}]Z`, a real calendar date and time of day. UTC only: the
/// contract says RFC 3339 UTC, so numeric offsets are refused rather than normalised.
fn is_utc_timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 20 || b.len() > 30 || b[b.len() - 1] != b'Z' {
        return false;
    }
    let (Some(c), Some(yy), Some(mo), Some(d), Some(h), Some(mi), Some(se)) = (
        two(&b[0..2]),
        two(&b[2..4]),
        two(&b[5..7]),
        two(&b[8..10]),
        two(&b[11..13]),
        two(&b[14..16]),
        two(&b[17..19]),
    ) else {
        return false;
    };
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return false;
    }
    let year = c * 100 + yy;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    let fraction = &b[19..b.len() - 1];
    let fraction_ok = fraction.is_empty()
        || (fraction[0] == b'.'
            && (2..=10).contains(&fraction.len())
            && fraction[1..].iter().all(u8::is_ascii_digit));
    (1..=days).contains(&d) && h < 24 && mi < 60 && se < 60 && fraction_ok
}

string_newtype!(
    /// RFC 3339 UTC timestamp. Display and chronology only; never identity (invariant 5).
    Timestamp,
    "an RFC 3339 UTC timestamp like 2026-10-09T12:00:00Z",
    is_utc_timestamp
);

// ---------------------------------------------------------------------------------------
// Constants: a property that has exactly one legal value
// ---------------------------------------------------------------------------------------

/// The JSON `false` and nothing else. Every effect capability is this type, so a response
/// claiming an effect is available cannot even be deserialised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ConstFalse;

/// The JSON `true` and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ConstTrue;

macro_rules! const_bool {
    ($name:ident, $value:literal) => {
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_bool($value)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                if bool::deserialize(d)? == $value {
                    Ok($name)
                } else {
                    Err(de::Error::custom(concat!(
                        "expected the constant ",
                        stringify!($value)
                    )))
                }
            }
        }
    };
}
const_bool!(ConstFalse, false);
const_bool!(ConstTrue, true);

/// The integer `1` and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ConstOne;

impl Serialize for ConstOne {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(1)
    }
}
impl<'de> Deserialize<'de> for ConstOne {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if u64::deserialize(d)? == 1 {
            Ok(ConstOne)
        } else {
            Err(de::Error::custom("expected the constant 1"))
        }
    }
}

/// The number `0.2`: the fixed weight of a teacher label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ConstWeight;

impl Serialize for ConstWeight {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(0.2)
    }
}
impl<'de> Deserialize<'de> for ConstWeight {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if f64::deserialize(d)? == 0.2 {
            Ok(ConstWeight)
        } else {
            Err(de::Error::custom("expected the constant 0.2"))
        }
    }
}

/// A unit type that is exactly one JSON string.
macro_rules! const_str {
    ($(#[$m:meta])* $vis:vis $name:ident = $lit:literal) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
        $vis struct $name;

        impl $name {
            pub const VALUE: &'static str = $lit;
        }
        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str($lit)
            }
        }
        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct V;
                impl ::serde::de::Visitor<'_> for V {
                    type Value = ();
                    fn expecting(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                        f.write_str(concat!("the constant \"", $lit, "\""))
                    }
                    fn visit_str<E: ::serde::de::Error>(self, v: &str) -> Result<(), E> {
                        if v == $lit { Ok(()) } else { Err(E::invalid_value(::serde::de::Unexpected::Str(v), &self)) }
                    }
                }
                d.deserialize_str(V).map(|()| $name)
            }
        }
    };
}
#[allow(unused_imports)]
pub(crate) use const_str;

const_str!(
    /// The protocol tag, `loomward/3`.
    pub Protocol = "loomward/3"
);
const_str!(
    /// The only v0.3 teacher: GPT-6.1 Sol at medium effort via the owner's local Codex CLI.
    /// A cloud recipient, so every disclosure names it.
    pub TeacherRecipient = "codex_cli_gpt_6_1_sol"
);

// ---------------------------------------------------------------------------------------
// Closed vocabularies the service trait needs
// ---------------------------------------------------------------------------------------

/// One catalogue file per class; a session serves exactly one (ADR-V3-15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetClass {
    Synthetic,
    Personal,
}

/// A metadata field a teacher disclosure may carry. Never content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeacherField {
    Name,
    Extension,
    Context,
    SizeBucket,
}

/// Which transport a call arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Adapter {
    Tauri,
    Http,
}

// ---------------------------------------------------------------------------------------
// Bounded collections
// ---------------------------------------------------------------------------------------

/// A JSON array of `MIN..=MAX` items; the bound is enforced while streaming, before the
/// excess is allocated.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundedVec<T, const MIN: usize, const MAX: usize>(Vec<T>);

impl<T, const MIN: usize, const MAX: usize> BoundedVec<T, MIN, MAX> {
    pub fn new(items: Vec<T>) -> Result<Self, Invalid> {
        if (MIN..=MAX).contains(&items.len()) {
            Ok(Self(items))
        } else {
            Err(Invalid::new(format!("array length outside {MIN}..={MAX}")))
        }
    }
    pub fn into_vec(self) -> Vec<T> {
        self.0
    }
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }
}

impl<T, const MAX: usize> Default for BoundedVec<T, 0, MAX> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T, const MIN: usize, const MAX: usize> Deref for BoundedVec<T, MIN, MAX> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T, const MIN: usize, const MAX: usize> TryFrom<Vec<T>> for BoundedVec<T, MIN, MAX> {
    type Error = Invalid;
    fn try_from(items: Vec<T>) -> Result<Self, Invalid> {
        Self::new(items)
    }
}

impl<T, const MIN: usize, const MAX: usize> IntoIterator for BoundedVec<T, MIN, MAX> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T, const MIN: usize, const MAX: usize> IntoIterator for &'a BoundedVec<T, MIN, MAX> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T: Serialize, const MIN: usize, const MAX: usize> Serialize for BoundedVec<T, MIN, MAX> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de, T: Deserialize<'de>, const MIN: usize, const MAX: usize> Deserialize<'de>
    for BoundedVec<T, MIN, MAX>
{
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T, const MIN: usize, const MAX: usize>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const MIN: usize, const MAX: usize> Visitor<'de> for V<T, MIN, MAX> {
            type Value = Vec<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "an array of {MIN}..={MAX} items")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
                let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(MAX).min(1024));
                while let Some(item) = seq.next_element()? {
                    if items.len() == MAX {
                        return Err(de::Error::custom(format!("array longer than {MAX} items")));
                    }
                    items.push(item);
                }
                Ok(items)
            }
        }
        let items = d.deserialize_seq(V::<T, MIN, MAX>(PhantomData))?;
        Self::new(items).map_err(de::Error::custom)
    }
}

/// A `BoundedVec` whose items are pairwise distinct (`uniqueItems`).
#[derive(Debug, Clone, PartialEq)]
pub struct UniqueVec<T, const MIN: usize, const MAX: usize>(BoundedVec<T, MIN, MAX>);

impl<T: Eq + Hash, const MIN: usize, const MAX: usize> UniqueVec<T, MIN, MAX> {
    pub fn new(items: Vec<T>) -> Result<Self, Invalid> {
        let bounded = BoundedVec::new(items)?;
        let mut seen = HashSet::with_capacity(bounded.len());
        if bounded.iter().all(|item| seen.insert(item)) {
            Ok(Self(bounded))
        } else {
            Err(Invalid::new("array items must be unique"))
        }
    }
}

impl<T, const MIN: usize, const MAX: usize> UniqueVec<T, MIN, MAX> {
    pub fn into_vec(self) -> Vec<T> {
        self.0.into_vec()
    }
}

impl<T, const MAX: usize> Default for UniqueVec<T, 0, MAX> {
    fn default() -> Self {
        Self(BoundedVec::default())
    }
}

impl<T, const MIN: usize, const MAX: usize> Deref for UniqueVec<T, MIN, MAX> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T: Eq + Hash, const MIN: usize, const MAX: usize> TryFrom<Vec<T>> for UniqueVec<T, MIN, MAX> {
    type Error = Invalid;
    fn try_from(items: Vec<T>) -> Result<Self, Invalid> {
        Self::new(items)
    }
}

impl<T: Serialize, const MIN: usize, const MAX: usize> Serialize for UniqueVec<T, MIN, MAX> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de, T: Deserialize<'de> + Eq + Hash, const MIN: usize, const MAX: usize> Deserialize<'de>
    for UniqueVec<T, MIN, MAX>
{
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(BoundedVec::<T, MIN, MAX>::deserialize(d)?.into_vec()).map_err(de::Error::custom)
    }
}

/// A JSON object with at most `MAX` properties (`maxProperties`).
#[derive(Debug, Clone, PartialEq)]
pub struct BoundedMap<K: Ord, V, const MAX: usize>(BTreeMap<K, V>);

impl<K: Ord, V, const MAX: usize> BoundedMap<K, V, MAX> {
    pub fn new(map: BTreeMap<K, V>) -> Result<Self, Invalid> {
        if map.len() <= MAX {
            Ok(Self(map))
        } else {
            Err(Invalid::new(format!(
                "object has more than {MAX} properties"
            )))
        }
    }
    pub fn into_map(self) -> BTreeMap<K, V> {
        self.0
    }
}

impl<K: Ord, V, const MAX: usize> Default for BoundedMap<K, V, MAX> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}

impl<K: Ord, V, const MAX: usize> Deref for BoundedMap<K, V, MAX> {
    type Target = BTreeMap<K, V>;
    fn deref(&self) -> &BTreeMap<K, V> {
        &self.0
    }
}

impl<K: Ord + Serialize, V: Serialize, const MAX: usize> Serialize for BoundedMap<K, V, MAX> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de, K: Ord + Deserialize<'de>, V: Deserialize<'de>, const MAX: usize> Deserialize<'de>
    for BoundedMap<K, V, MAX>
{
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(BTreeMap::deserialize(d)?).map_err(de::Error::custom)
    }
}

/// A free-form JSON object with at most 16 properties (`ErrorBody.detail`).
pub type Detail = BoundedMap<String, serde_json::Value, 16>;

/// A type with no values: the element of an array that must stay empty
/// (`ProcessExplanation.available_actions`, invariant 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Never {}
