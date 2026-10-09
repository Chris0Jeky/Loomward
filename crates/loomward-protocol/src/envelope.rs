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

    /// Decodes the payload into the command's DTO. Unknown or malformed fields become an
    /// `invalid_request` error body, ready to put in a response.
    pub fn decode_payload<T: DeserializeOwned>(&self) -> Result<T, ErrorBody> {
        serde_json::from_value(Value::Object(self.payload.clone()))
            .map_err(|e| ErrorBody::invalid_request(&e.to_string()))
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
