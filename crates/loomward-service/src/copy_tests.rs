//! docs/47 lane C4: the copy commands exist in the contract but no adapter reaches an effect. Every one
//! answers `capability_unavailable` (`copy_effect_gated`); a personal approval over HTTP is refused by
//! its own rule (`copy_personal_approval_http`, G6). Session capabilities stay false.
use super::*;
use std::path::PathBuf;

struct NoDialogs;
impl NativeDialogs for NoDialogs {
    fn pick_folder(&self) -> Option<PathBuf> {
        panic!("a copy command opened the folder picker")
    }
    fn confirm_disclosure(&self, _: &DisclosureSummary) -> Result<bool, ErrorBody> {
        panic!("a copy command opened the disclosure dialog")
    }
}

fn example(command: Command) -> Value {
    let path = format!(
        "{}/../../contracts/v3/examples/commands/{}.request.json",
        env!("CARGO_MANIFEST_DIR"),
        command.as_str()
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn every_copy_command_is_gated_on_every_adapter_and_dataset() {
    let copy: Vec<_> = Command::ALL
        .iter()
        .copied()
        .filter(|c| c.as_str().starts_with("copy."))
        .collect();
    assert_eq!(copy.len(), 6);
    for dataset in [DatasetClass::Synthetic, DatasetClass::Personal] {
        let tmp = temp_root();
        let svc = open(tmp.path(), dataset);
        let tauri = CallContext::tauri(Arc::new(NoDialogs));
        for ctx in [CallContext::http(), tauri] {
            let (hello, _) = ok(
                "session.hello",
                svc.call(request("session.hello", json!({})), &ctx),
            );
            let effects = hello["capabilities"]["effects"].as_object().unwrap();
            assert!(effects.values().all(|v| v == &json!(false)), "{effects:?}");
            for &c in &copy {
                let e = err(svc.call(request(c.as_str(), example(c)), &ctx));
                assert_eq!(e.code, ErrorCode::CapabilityUnavailable, "{}", c.as_str());
                assert!(!e.retryable);
                let personal_http = c == Command::CopyApprove
                    && ctx.adapter == loomward_protocol::Adapter::Http
                    && dataset == DatasetClass::Personal;
                let want = if personal_http {
                    "copy_personal_approval_http"
                } else {
                    "copy_effect_gated"
                };
                assert_eq!(reason(&e), Some(want), "{} {dataset:?}", c.as_str());
            }
        }
    }
}
