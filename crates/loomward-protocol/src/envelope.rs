//! Request and response envelopes (docs/41 section 5.1).

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
        }
    }

    /// Decodes the payload into the command's DTO (strictly, see [`decode_exact`]). Unknown or
    /// malformed fields become an `invalid_request` error body, ready to put in a response.
    pub fn decode_payload<T: DeserializeOwned + Serialize>(&self) -> Result<T, ErrorBody> {
        decode_exact(Value::Object(self.payload.clone()))
    }

    /// Parses the raw bytes of a call. Anything that is not exactly a request envelope is an
    /// `invalid_request` error body.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, ErrorBody> {
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|e| ErrorBody::invalid_request(&e.to_string()))?;
        decode_exact(value)
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

fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x == y || x.as_f64() == y.as_f64(),
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
