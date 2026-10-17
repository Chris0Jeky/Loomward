//! The scripted fake provider and the provider contract suite (LW-117 acceptance). The same
//! [`contract`] runs against the fake here and against the broker over a loopback stub in
//! `loomward-broker`'s tests, so the fake cannot drift from the real adapter.

use crate::ToolCall;
use crate::{ChatReply, ChatRequest, ErrorKind, FunctionCall, Message, Provider, ProviderError};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

/// What the model server does for one round.
#[derive(Debug, Clone)]
pub enum Upstream {
    Reply(ChatReply),
    /// Nothing listening.
    Down,
    /// Fake only: a scripted failure.
    Fail(ProviderError),
}

/// Answers rounds from a script and records every request it was given.
#[derive(Debug, Default)]
pub struct ScriptedProvider {
    script: Mutex<VecDeque<Upstream>>,
    pub seen: Mutex<Vec<ChatRequest>>,
}

impl ScriptedProvider {
    pub fn new(script: Vec<Upstream>) -> Self {
        Self {
            script: Mutex::new(script.into()),
            seen: Mutex::default(),
        }
    }
}

impl Provider for ScriptedProvider {
    fn complete(&self, req: &ChatRequest, _: Duration) -> Result<ChatReply, ProviderError> {
        let next = self.script.lock().unwrap().pop_front();
        match next {
            Some(Upstream::Down) => Err(ProviderError::new(
                ErrorKind::Unreachable,
                "connection refused",
                false,
            )),
            Some(Upstream::Fail(e)) => {
                if e.dispatched {
                    self.seen.lock().unwrap().push(req.clone());
                }
                Err(e)
            }
            Some(Upstream::Reply(r)) => {
                self.seen.lock().unwrap().push(req.clone());
                Ok(r)
            }
            None => Err(ProviderError::new(
                ErrorKind::InvalidReply,
                "script exhausted",
                true,
            )),
        }
    }
}

pub fn text(s: &str) -> ChatReply {
    ChatReply {
        content: Some(s.into()),
        tool_calls: vec![],
    }
}

pub fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        kind: "function".into(),
        function: FunctionCall {
            name: name.into(),
            arguments: arguments.into(),
        },
    }
}

pub fn calls(c: Vec<ToolCall>) -> ChatReply {
    ChatReply {
        content: None,
        tool_calls: c,
    }
}

/// The behaviour every provider owes the tool loop. `provider_for` builds a provider whose model
/// server follows the script.
pub fn contract(provider_for: &dyn Fn(Vec<Upstream>) -> Box<dyn Provider>) {
    let req = ChatRequest {
        messages: vec![
            Message::System {
                content: "s".into(),
            },
            Message::User {
                content: "which report is largest? \u{202E}txt.exe".into(),
            },
        ],
        tools: vec![
            serde_json::json!({"type": "function", "function": {"name": "find",
            "parameters": {"type": "object"}}}),
        ],
    };
    let t = Duration::from_secs(20);
    // A text answer comes back verbatim.
    let p = provider_for(vec![Upstream::Reply(text("h1 is largest. ünïcode"))]);
    assert_eq!(p.complete(&req, t).unwrap(), text("h1 is largest. ünïcode"));
    // Tool calls keep their id, name and exact argument text, in order.
    let tc = calls(vec![
        call("call_1", "find", r#"{"text":"report"}"#),
        call("call_2", "describe", r#"{"handle":"h1"}"#),
    ]);
    let p = provider_for(vec![Upstream::Reply(tc.clone())]);
    assert_eq!(p.complete(&req, t).unwrap(), tc);
    // A server that is not there is unreachable, not an empty answer, and nothing was sent.
    let p = provider_for(vec![Upstream::Down]);
    let e = p.complete(&req, t).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unreachable, "{e:?}");
    assert!(!e.dispatched);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_meets_the_provider_contract() {
        contract(&|script| Box::new(ScriptedProvider::new(script)));
    }
}
