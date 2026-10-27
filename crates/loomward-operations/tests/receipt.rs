use loomward_operations::{CopyReceipt, OpState, ReceiptLabel, ReceiptState};

#[test]
fn only_a_completed_read_back_earns_a_label() {
    let passed = CopyReceipt::from_copy(OpState::Committed, 3, 3, true);
    assert_eq!(passed.highest_label, Some(ReceiptLabel::ReadBack));
    assert_eq!(passed.state, ReceiptState::ReadBack);
    assert!(passed.archive_verified_at.is_none());
    for state in OpState::ALL {
        if state != OpState::Committed {
            let fault = CopyReceipt::from_copy(state, 3, 3, false);
            assert_eq!(fault.highest_label, None, "{state:?}");
        }
    }
    let mismatch = CopyReceipt::from_copy(OpState::Committed, 3, 2, true);
    assert_eq!(mismatch.highest_label, None);
    assert_eq!(mismatch.state, ReceiptState::PublishedUnverified);
}

#[test]
fn publish_and_fault_receipts_do_not_overclaim() {
    for state in [OpState::Publishing, OpState::NeedsAttention] {
        let receipt = CopyReceipt::from_copy(state, 3, 3, true);
        assert_eq!(receipt.state, ReceiptState::PublishedUnverified);
        assert_eq!(receipt.highest_label, None);
        assert!(receipt.explanation.contains("published_unverified"));
    }
    assert_eq!(
        CopyReceipt::from_copy(OpState::Publishing, 3, 3, false).state,
        ReceiptState::Unverified,
        "a publish intent is not publication"
    );
}

#[test]
fn receipt_strings_and_serialized_fields_never_use_the_forbidden_word() {
    for state in OpState::ALL {
        for published in [false, true] {
            let receipt = CopyReceipt::from_copy(state, 3, 3, published);
            assert!(!receipt.explanation.to_lowercase().contains("durable"));
            assert!(!serde_json::to_string(&receipt)
                .unwrap()
                .to_lowercase()
                .contains("durable"));
            assert!(receipt
                .explanation
                .contains("not power-loss tested on this disk"));
            assert!(!matches!(
                receipt.highest_label,
                Some(
                    ReceiptLabel::CrashQualified
                        | ReceiptLabel::PowerLossQualified
                        | ReceiptLabel::ArchiveVerified
                )
            ));
        }
    }
}

#[test]
fn service_agent_cli_and_app_share_the_read_back_fixture() {
    let status: serde_json::Value = serde_json::from_str(include_str!(
        "../../../contracts/v3/examples/dtos/copy-receipt.json"
    ))
    .unwrap();
    assert_eq!(
        serde_json::to_value(CopyReceipt::from_copy(OpState::Committed, 3, 3, true)).unwrap(),
        status["value"]
    );
}
