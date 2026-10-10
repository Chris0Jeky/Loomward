//! Request and response envelopes (docs/41 section 5.1).

use crate::command::Command;
use crate::dto::{Anchor, TreeSliceRequest};
use crate::error::{ErrorBody, ErrorCode};
use crate::types::{
    optional_present, required_nullable, CommandName, ConstFalse, ConstTrue, Count, DatasetClass,
    Generation, Int, Protocol, RequestId, Timestamp,
};
use serde::de::{self, DeserializeOwned};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

/// A JSON object: command payloads and results are always objects.
pub type Payload = Map<String, Value>;

/// One call. Unknown fields fail closed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestEnvelope {
    pub protocol: Protocol,
    pub request_id: RequestId,
    pub command: CommandName,
    pub payload: Payload,
    #[serde(
        default,
        deserialize_with = "optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub deadline_ms: Option<Int<1, 60000>>,
    #[serde(
        default,
        deserialize_with = "optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub expected_generation: Option<Generation>,
    /// Revision precondition for owner-state mutations (`volumes.declare_tier`,
    /// `collections.update_members`); a mismatch is `stale_generation`.
    #[serde(
        default,
        deserialize_with = "optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub expected_state_rev: Option<Generation>,
}

impl RequestEnvelope {
    pub fn new(request_id: RequestId, command: CommandName, payload: Payload) -> Self {
        Self {
            protocol: Protocol,
            request_id,
            command,
            payload,
            deadline_ms: None,
            expected_generation: None,
            expected_state_rev: None,
        }
    }

    /// Complete-envelope check (semantics.md sections 1 and 5): a known command whose payload is
    /// exactly its request type, `expected_state_rev` only where a state precondition exists, and
    /// `expected_generation` only on commands anchored in exactly one root.
    pub fn validate(&self) -> Result<Command, ErrorBody> {
        let command = Command::from_name(&self.command)
            .ok_or_else(|| ErrorBody::new(ErrorCode::UnknownCommand, "unknown command", false))?;
        command.validate_request(&self.payload)?;
        let takes_precondition = matches!(
            command,
            Command::VolumesDeclareTier | Command::CollectionsUpdateMembers
        );
        if self.expected_state_rev.is_some() && !takes_precondition {
            return Err(ErrorBody::invalid_request(
                "expected_state_rev is not accepted by this command",
            ));
        }
        if self.expected_generation.is_some() && !self.single_root_anchor(command)? {
            return Err(ErrorBody::invalid_request(
                "expected_generation is only accepted by commands anchored in exactly one root",
            ));
        }
        Ok(command)
    }

    /// Whether `command` (whose payload already validated) is anchored in exactly one root:
    /// `tree.*`, `node.inspect` and `stats.breakdown`, but not a `tree.slice` of the whole atlas.
    fn single_root_anchor(&self, command: Command) -> Result<bool, ErrorBody> {
        Ok(match command {
            Command::TreeChildren
            | Command::TreePath
            | Command::NodeInspect
            | Command::StatsBreakdown => true,
            Command::TreeSlice => {
                let slice: TreeSliceRequest = self.decode_payload()?;
                !matches!(slice.anchor, Anchor::Atlas {})
            }
            _ => false,
        })
    }

    /// Typed decode of the payload after [`RequestEnvelope::validate`] (strictly, see
    /// [`decode_exact`]). `from_slice` has already validated, so `T` is the command's request type.
    /// Unknown or malformed fields become an `invalid_request` error body.
    pub fn decode_payload<T: DeserializeOwned + Serialize>(&self) -> Result<T, ErrorBody> {
        decode_exact(Value::Object(self.payload.clone()))
    }

    /// The adapter entry point (semantics.md section 1): parses the raw bytes of a call and runs
    /// the checks in ingress order. The body must be one JSON object without a repeated member at
    /// any depth, else `invalid_request` with no `request_id`. A `protocol` other than
    /// `loomward/3` is `unsupported_protocol` with the `request_id` echoed when it is well formed.
    /// Then the envelope must decode exactly, name a known command (`unknown_command`), carry
    /// exactly that command's payload and use only preconditions the command accepts
    /// ([`RequestEnvelope::validate`]). The returned [`Rejected`] is ready to become a response.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, Rejected> {
        let value = parse_strict(bytes)
            .map_err(|message| Rejected::new(None, ErrorBody::invalid_request(&message)))?;
        let id = value
            .get("request_id")
            .and_then(Value::as_str)
            .and_then(|s| RequestId::new(s).ok());
        if let Some(protocol) = value.get("protocol").and_then(Value::as_str) {
            if protocol != Protocol::VALUE {
                let message = format!("this service speaks {}", Protocol::VALUE);
                let error = ErrorBody::new(ErrorCode::UnsupportedProtocol, &message, false);
                return Err(Rejected::new(id, error));
            }
        }
        let request: Self = decode_exact(value).map_err(|e| Rejected::new(id, e))?;
        request
            .validate()
            .map_err(|e| Rejected::new(Some(request.request_id.clone()), e))?;
        Ok(request)
    }
}

/// A call refused before it reached a command, with what the error response needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Rejected {
    /// The request's id when the body was parseable and the id well formed, else `null`.
    pub request_id: Option<RequestId>,
    pub error: ErrorBody,
}

impl Rejected {
    pub fn new(request_id: Option<RequestId>, error: ErrorBody) -> Self {
        Self { request_id, error }
    }

    /// The error response to send (no `meta`: nothing was served).
    pub fn into_response(self) -> ResponseEnvelope {
        ResponseEnvelope::error(self.request_id, self.error, None)
    }
}

/// Parses one JSON document, refusing a repeated object member at any depth. `serde_json`
/// collapses duplicates silently into the last value, so a plain parse cannot enforce the rule.
/// Nesting is bounded by `serde_json`'s recursion limit (128).
fn parse_strict(bytes: &[u8]) -> Result<Value, String> {
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let Strict(value) = Strict::deserialize(&mut de).map_err(|e| e.to_string())?;
    de.end().map_err(|e| e.to_string())?;
    Ok(value)
}

struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> de::Visitor<'de> for V {
            type Value = Strict;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Strict, E> {
                Ok(Strict(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Strict, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| Strict(Value::Number(n)))
                    .ok_or_else(|| E::custom("number is not finite"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Strict, E> {
                Ok(Strict(Value::String(v.to_owned())))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Strict, A::Error> {
                let mut items = Vec::new();
                while let Some(Strict(item)) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Strict(Value::Array(items)))
            }
            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Strict, A::Error> {
                let mut out = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if out.contains_key(&key) {
                        return Err(de::Error::custom(format!("duplicate member `{key}`")));
                    }
                    let Strict(value) = map.next_value()?;
                    out.insert(key, value);
                }
                Ok(Strict(Value::Object(out)))
            }
        }
        d.deserialize_any(V)
    }
}

/// Deserialises `value` as `T` and refuses anything the contract would not itself produce.
///
/// Serde-derived structs also accept a JSON array as a positional tuple, which the schema forbids;
/// requiring the decoded value to serialise back to the input (numbers compared by value, so
/// `1` and `1.0` agree) closes that and any similar leniency. Every adapter decodes through here.
pub fn decode_exact<T: DeserializeOwned + Serialize>(value: Value) -> Result<T, ErrorBody> {
    let decoded: T = serde_json::from_value(value.clone())
        .map_err(|e| ErrorBody::invalid_request(&e.to_string()))?;
    let canonical =
        serde_json::to_value(&decoded).map_err(|e| ErrorBody::invalid_request(&e.to_string()))?;
    if same_json(&canonical, &value) {
        Ok(decoded)
    } else {
        Err(ErrorBody::invalid_request(
            "value is not in the contract's canonical shape",
        ))
    }
}

fn as_i128(n: &serde_json::Number) -> Option<i128> {
    n.as_i64()
        .map(i128::from)
        .or_else(|| n.as_u64().map(i128::from))
}

/// Equal by value without f64 collisions: two integers compare as integers, an integer and a
/// float agree only when the float is exactly that integer, two floats compare as floats.
fn same_number(x: &serde_json::Number, y: &serde_json::Number) -> bool {
    let exactly = |float: &serde_json::Number, int: i128| {
        float
            .as_f64()
            .is_some_and(|f| f.fract() == 0.0 && f.abs() < 1.0e18 && f as i128 == int)
    };
    match (as_i128(x), as_i128(y)) {
        (Some(a), Some(b)) => a == b,
        (Some(a), None) => exactly(y, a),
        (None, Some(b)) => exactly(x, b),
        (None, None) => x.as_f64() == y.as_f64(),
    }
}

fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => same_number(x, y),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_json(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_json(v, w)))
        }
        _ => a == b,
    }
}

/// Facts about how a call was served.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseMeta {
    pub served_at: Timestamp,
    pub elapsed_ms: Count,
    pub dataset_class: DatasetClass,
    pub budget_hit: bool,
    /// Catalogue revision the response was read or committed at; `None` when no catalogue was involved.
    #[serde(deserialize_with = "required_nullable")]
    pub catalog_rev: Option<Generation>,
    /// `state.db` revision the response was read or committed at; `None` when it touched no state.
    #[serde(deserialize_with = "required_nullable")]
    pub state_rev: Option<Generation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseOk {
    pub protocol: Protocol,
    pub request_id: RequestId,
    pub ok: ConstTrue,
    pub result: Payload,
    pub meta: ResponseMeta,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseError {
    pub protocol: Protocol,
    /// `null` when the request was too malformed to carry an id.
    #[serde(deserialize_with = "required_nullable")]
    pub request_id: Option<RequestId>,
    pub ok: ConstFalse,
    pub error: ErrorBody,
    #[serde(
        default,
        deserialize_with = "optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub meta: Option<ResponseMeta>,
}

/// The answer to one call. Always delivered as HTTP 200 or an `invoke` result: transport
/// faults are the only non-200 cases.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ResponseEnvelope {
    Ok(ResponseOk),
    Err(ResponseError),
}

impl ResponseOk {
    /// Complete-envelope check: `result` must be exactly the result type of `command`.
    pub fn validate_for(&self, command: Command) -> Result<(), ErrorBody> {
        command.validate_result(&self.result)
    }
}

impl ResponseEnvelope {
    pub fn ok_object(request_id: RequestId, result: Payload, meta: ResponseMeta) -> Self {
        Self::Ok(ResponseOk {
            protocol: Protocol,
            request_id,
            ok: ConstTrue,
            result,
            meta,
        })
    }

    /// Serialises `result` and wraps it. A result that is not a JSON object is a service bug
    /// and becomes an `internal_error` response instead of a malformed envelope.
    pub fn ok<T: Serialize>(request_id: RequestId, result: &T, meta: ResponseMeta) -> Self {
        match serde_json::to_value(result) {
            Ok(Value::Object(map)) => Self::ok_object(request_id, map, meta),
            _ => Self::error(
                Some(request_id),
                ErrorBody::new(
                    ErrorCode::InternalError,
                    "result was not a JSON object",
                    false,
                ),
                Some(meta),
            ),
        }
    }

    pub fn error(
        request_id: Option<RequestId>,
        error: ErrorBody,
        meta: Option<ResponseMeta>,
    ) -> Self {
        Self::Err(ResponseError {
            protocol: Protocol,
            request_id,
            ok: ConstFalse,
            error,
            meta,
        })
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok(_))
    }
}

impl<'de> Deserialize<'de> for ResponseEnvelope {
    /// Dispatches on `ok` so a malformed body reports the real field error, not
    /// "did not match any variant".
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(d)?;
        let ok = value
            .get("ok")
            .and_then(Value::as_bool)
            .ok_or_else(|| de::Error::custom("response needs a boolean `ok`"))?;
        if ok {
            serde_json::from_value(value)
                .map(Self::Ok)
                .map_err(de::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(Self::Err)
                .map_err(de::Error::custom)
        }
    }
}
