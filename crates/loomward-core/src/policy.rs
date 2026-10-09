//! Capability evaluation is independent of statistical or LLM scores.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    ObserveMetadata,
    InspectContents,
    WriteFeedback,
    MoveFile,
    DeleteFile,
    RenameFile,
    ControlProcess,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub scoped_grant: bool,
    pub content_read_consent: bool,
    pub current_identity_verified: bool,
    pub protected: bool,
    pub sensitive: bool,
    pub reparse_or_placeholder: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Decision {
    pub allowed: bool,
    pub reason: &'static str,
}

/// Pure policy sketch, not a substitute for native handle-based validation.
/// Mutation capabilities are disabled unconditionally in this prototype.
pub fn evaluate(capability: Capability, evidence: &Evidence) -> Decision {
    use Capability::*;
    let reason = match capability {
        MoveFile | DeleteFile | RenameFile | ControlProcess => Some("mutation_not_implemented"),
        _ if !evidence.scoped_grant => Some("scope_not_granted"),
        InspectContents if !evidence.content_read_consent => Some("content_consent_required"),
        InspectContents if !evidence.current_identity_verified => Some("identity_not_current"),
        InspectContents
            if evidence.protected || evidence.sensitive || evidence.reparse_or_placeholder =>
        {
            Some("excluded_content")
        }
        WriteFeedback if evidence.sensitive => Some("sensitive_training_excluded"),
        _ => None,
    };
    Decision {
        allowed: reason.is_none(),
        reason: reason.unwrap_or("bounded_capability_permitted"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_grant_denies() {
        assert!(!evaluate(Capability::ObserveMetadata, &Evidence::default()).allowed);
    }
    #[test]
    fn explicit_scope_allows_metadata_without_reading_contents() {
        let e = Evidence {
            scoped_grant: true,
            ..Default::default()
        };
        assert!(evaluate(Capability::ObserveMetadata, &e).allowed);
        assert!(!evaluate(Capability::InspectContents, &e).allowed);
    }
    #[test]
    fn even_all_grants_cannot_enable_mutations() {
        let e = Evidence {
            scoped_grant: true,
            content_read_consent: true,
            current_identity_verified: true,
            ..Default::default()
        };
        for c in [
            Capability::MoveFile,
            Capability::DeleteFile,
            Capability::RenameFile,
            Capability::ControlProcess,
        ] {
            assert_eq!(evaluate(c, &e).reason, "mutation_not_implemented");
        }
    }
    #[test]
    fn protected_placeholder_never_hashes() {
        let e = Evidence {
            scoped_grant: true,
            content_read_consent: true,
            current_identity_verified: true,
            reparse_or_placeholder: true,
            ..Default::default()
        };
        assert!(!evaluate(Capability::InspectContents, &e).allowed);
    }
    #[test]
    fn model_confidence_is_not_a_policy_field() {
        assert!(
            serde_json::from_str::<Evidence>(r#"{"scoped_grant":true,"confidence":1.0}"#).is_err()
        );
    }
}
