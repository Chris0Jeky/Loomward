//! Admission of an owned reply after a root forget. These types are internal Rust state, not
//! wire fields. The adapter carries the pending reply across its blocking-work await and checks
//! it at the final handoff, just as an event stream checks its pending frame.

use crate::{Detail, ErrorBody, ErrorCode, EventDeliveryStamp, ResponseEnvelope};

impl ResponseEnvelope {
    /// Privacy can withdraw a retained success without claiming that its mutation did not run.
    /// Existing errors in the protected reply classes contain no root or volume snapshots.
    pub fn without_forgotten_result(self) -> Self {
        let Self::Ok(ok) = self else {
            return self;
        };
        let mut error = ErrorBody::new(
            ErrorCode::PermissionDenied,
            "This reply was withheld after a root forget. The action may have completed; read current state before acting again.",
            false,
        );
        error.detail = Some(
            Detail::new(std::collections::BTreeMap::from([(
                "reason".into(),
                serde_json::json!("forgotten_result"),
            )]))
            .expect("one bounded reason"),
        );
        Self::error(Some(ok.request_id), error, None)
    }
}

/// A response awaiting its adapter's final synchronous handoff. The stamp is captured before
/// the service call starts. A forget can then invalidate both an old cached clone and a result
/// built concurrently with its durable commit; the response itself adds no protocol field.
#[derive(Debug)]
#[must_use]
pub struct PendingResponse {
    envelope: ResponseEnvelope,
    stamp: Option<EventDeliveryStamp>,
}

impl PendingResponse {
    pub fn new(envelope: ResponseEnvelope, stamp: Option<EventDeliveryStamp>) -> Self {
        Self { envelope, stamp }
    }

    pub fn unrestricted(envelope: ResponseEnvelope) -> Self {
        Self::new(envelope, None)
    }

    /// Native admission: call after the blocking-worker await, immediately before returning the
    /// envelope to Tauri. Nothing holds a service mutex across the framework handoff.
    pub fn admit(self) -> ResponseEnvelope {
        if self.stamp.is_some_and(|stamp| !stamp.admit()) {
            self.envelope.without_forgotten_result()
        } else {
            self.envelope
        }
    }

    /// Serialize before HTTP admission so no payload work occurs after its final stamp check.
    pub fn serialize(self) -> SerializedResponse {
        let bytes = serde_json::to_vec(&self.envelope).expect("a response envelope serializes");
        let withheld = serde_json::to_vec(&self.envelope.without_forgotten_result())
            .expect("a privacy refusal envelope serializes");
        SerializedResponse {
            bytes,
            withheld,
            stamp: self.stamp,
        }
    }
}

/// One bounded HTTP reply, checked when the body is polled rather than when it is constructed.
#[derive(Debug)]
#[must_use]
pub struct SerializedResponse {
    bytes: Vec<u8>,
    withheld: Vec<u8>,
    stamp: Option<EventDeliveryStamp>,
}

impl SerializedResponse {
    pub fn admit(self) -> Vec<u8> {
        if self.stamp.is_some_and(|stamp| !stamp.admit()) {
            self.withheld
        } else {
            self.bytes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventDeliveryPolicy, RequestId, ResponseMeta};
    use serde_json::{json, Value};

    fn private_reply() -> ResponseEnvelope {
        let meta: ResponseMeta = serde_json::from_value(json!({
            "served_at":"2026-10-19T00:00:00Z", "elapsed_ms":0,
            "dataset_class":"synthetic", "budget_hit":false,
            "catalog_rev":"1", "state_rev":"1"
        }))
        .unwrap();
        ResponseEnvelope::ok_object(
            RequestId::new("private-reply").unwrap(),
            json!({"root":{"display_path":"SyntheticForgottenReply"}})
                .as_object()
                .unwrap()
                .clone(),
            meta,
        )
    }

    #[test]
    fn retained_privacy_response_admission_preserves_the_key_and_refuses_without_retry() {
        let policy = EventDeliveryPolicy::default();
        let pending = PendingResponse::new(private_reply(), Some(policy.stamp()));
        policy.invalidate();
        let ResponseEnvelope::Err(error) = pending.admit() else {
            panic!("old reply admitted")
        };
        assert_eq!(error.request_id.unwrap().as_str(), "private-reply");
        assert_eq!(error.error.code, ErrorCode::PermissionDenied);
        assert!(!error.error.retryable);
        assert_eq!(error.error.detail.unwrap()["reason"], "forgotten_result");
        assert!(!serde_json::to_string(&error.error.message)
            .unwrap()
            .contains("SyntheticForgottenReply"));
    }

    #[test]
    fn retained_privacy_encoded_reply_is_checked_after_serialization() {
        let policy = EventDeliveryPolicy::default();
        let encoded = PendingResponse::new(private_reply(), Some(policy.stamp())).serialize();
        policy.invalidate();
        let bytes = encoded.admit();
        assert!(!String::from_utf8(bytes.clone())
            .unwrap()
            .contains("SyntheticForgottenReply"));
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["request_id"], "private-reply");
        assert_eq!(result["error"]["detail"]["reason"], "forgotten_result");
        let current = PendingResponse::new(private_reply(), Some(policy.stamp())).admit();
        assert_eq!(current, private_reply());
    }
}
