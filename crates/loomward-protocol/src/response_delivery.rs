//! Admission of an owned reply after a root forget. These types are internal Rust state, not
//! wire fields. The adapter carries the pending reply across its blocking-work await and checks
//! it at the final handoff, just as an event stream checks its pending frame.

use crate::{Detail, ErrorBody, ErrorCode, ResponseDeliveryStamp, ResponseEnvelope};

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
    stamp: Option<ResponseDeliveryStamp>,
}

impl PendingResponse {
    pub fn new(envelope: ResponseEnvelope, stamp: Option<ResponseDeliveryStamp>) -> Self {
        // Protected error envelopes contain no private snapshot. A busy/deadline refusal must
        // keep its timing and transport meaning even while an unrelated forget is undecided.
        let stamp = if matches!(&envelope, ResponseEnvelope::Ok(_)) {
            stamp
        } else {
            None
        };
        Self { envelope, stamp }
    }

    pub fn unrestricted(envelope: ResponseEnvelope) -> Self {
        Self::new(envelope, None)
    }

    /// Synchronous admission outside service work. Async adapters use admit_async to wait for a
    /// pending forget without blocking their runtime. No service lock spans the handoff.
    pub fn admit(self) -> ResponseEnvelope {
        if self.stamp.is_some_and(|stamp| !stamp.admit()) {
            self.envelope.without_forgotten_result()
        } else {
            self.envelope
        }
    }

    /// Native admission after the blocking-worker await. Resolving the pause and checking the
    /// stamp happen in the same poll; no await follows that final decision before the handoff.
    pub async fn admit_async(self) -> ResponseEnvelope {
        if let Some(stamp) = self.stamp {
            if !stamp.admit_async().await {
                return self.envelope.without_forgotten_result();
            }
        }
        self.envelope
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
    stamp: Option<ResponseDeliveryStamp>,
}

impl SerializedResponse {
    pub fn admit(self) -> Vec<u8> {
        if self.stamp.is_some_and(|stamp| !stamp.admit()) {
            self.withheld
        } else {
            self.bytes
        }
    }

    /// Poll from the one-frame HTTP body. A pending forget yields instead of blocking the async
    /// runtime; the chosen bytes are handed on in the same poll as the final admission check.
    pub async fn admit_async(self) -> Vec<u8> {
        if let Some(stamp) = self.stamp {
            if !stamp.admit_async().await {
                return self.withheld;
            }
        }
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RequestId, ResponseDeliveryPolicy, ResponseMeta};
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
        let policy = ResponseDeliveryPolicy::default();
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
        let policy = ResponseDeliveryPolicy::default();
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

    #[test]
    fn retained_privacy_response_errors_do_not_wait_behind_an_unrelated_forget() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        let policy = ResponseDeliveryPolicy::default();
        let _pause = policy.suspend();
        let error = ResponseEnvelope::error(
            Some(RequestId::new("private-reply").unwrap()),
            ErrorBody::new(
                ErrorCode::DeadlineExceeded,
                "the read deadline expired",
                false,
            ),
            None,
        );
        let pending = PendingResponse::new(error.clone(), Some(policy.stamp()));
        let encoded = PendingResponse::new(error.clone(), Some(policy.stamp())).serialize();
        let mut native = std::pin::pin!(pending.admit_async());
        let mut http = std::pin::pin!(encoded.admit_async());
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(native.as_mut().poll(&mut cx), Poll::Ready(error.clone()));
        let Poll::Ready(bytes) = http.as_mut().poll(&mut cx) else {
            panic!("a safe error response was delayed behind forget")
        };
        assert_eq!(
            serde_json::from_slice::<ResponseEnvelope>(&bytes).unwrap(),
            error
        );
    }
}
