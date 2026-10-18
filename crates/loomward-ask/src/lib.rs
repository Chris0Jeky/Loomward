//! The Ask provider interface (LW-117, docs/46 section 8.1, ADR-V3-26).
//!
//! One [`Provider`] trait answers one model round. Its first implementation, [`BrokerProvider`],
//! spawns a fresh `loomward-broker` process per round (P2): the broker is the only component that
//! links an HTTP client, and this crate has no network code. A provider exists only for a
//! [`ProviderProfile`] the owner configured; nothing has a default endpoint, nothing is discovered
//! and nothing is downloaded. The `local` class is an `http` loopback endpoint whose model is not
//! remote; everything else is `cloud`, and a cloud provider cannot be opened without a consent
//! record and, until q-5's conditions hold (docs/46 F11), not at all.
//!
//! The tool loop ([`turn::ask`]) reaches the catalogue only through the service's grant-scoped
//! `search.query` and `node.inspect` as an `ask:<class>` client principal (LW-067, LW-069), and
//! records every outbound round with its disclosure scope.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
pub mod turn;

/// Largest request a round may send and largest reply it may receive (docs/46 section 8.1).
pub const MAX_REQUEST_BYTES: usize = 256 * 1024;
pub const MAX_REPLY_BYTES: usize = 256 * 1024;

/// One chat message in the OpenAI Chat Completions shape, sent as is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        #[serde(default)]
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "function")]
    pub kind: String,
    pub function: FunctionCall,
}

fn function() -> String {
    "function".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// JSON text, as the wire carries it; the tool loop parses and validates it.
    pub arguments: String,
}

/// One model round: the conversation so far and the tools offered (OpenAI tool schemas).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<serde_json::Value>,
}

/// The exact HTTP request body of one round. The broker writes these bytes and the turn loop
/// digests them for its outbound record, so the record cannot describe anything else (#560).
pub fn request_body(model: &str, req: &ChatRequest) -> String {
    let mut body = serde_json::json!({"model": model, "messages": req.messages, "stream": false});
    if !req.tools.is_empty() {
        body["tools"] = serde_json::Value::from(req.tools.clone());
    }
    body.to_string()
}

/// The model's reply to one round.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatReply {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Cloud class without a consent record for this exact profile.
    ConsentRequired,
    /// Cloud class with consent: not built until q-5's conditions hold (docs/46 F11).
    CloudUnavailable,
    /// The endpoint is not a loopback `http` endpoint, or the model is remote (`:cloud`).
    NotLocal,
    InvalidRequest,
    /// Nothing answered: refused, unresolvable or connect timeout. Never an empty answer.
    Unreachable,
    /// First byte, stall or round deadline passed after the request was sent.
    Timeout,
    /// A non-2xx status; redirects are refused, never followed.
    HttpStatus,
    Oversize,
    InvalidReply,
    /// The broker process could not run or did not answer in its own shape.
    BrokerFailed,
}

/// An honest round failure. `dispatched` says whether request bytes may have left Loomward:
/// it is false only when nothing was written to a connected socket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderError {
    pub kind: ErrorKind,
    pub message: String,
    pub dispatched: bool,
}

impl ProviderError {
    pub fn new(kind: ErrorKind, message: impl Into<String>, dispatched: bool) -> Self {
        Self {
            kind,
            message: message.into(),
            dispatched,
        }
    }
}

/// Answers one model round within `deadline`.
pub trait Provider {
    fn complete(&self, req: &ChatRequest, deadline: Duration) -> Result<ChatReply, ProviderError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderClass {
    Local,
    Cloud,
}

/// A loopback endpoint as the owner typed it: `http://127.0.0.1:<port>`, `http://[::1]:<port>` or
/// `http://localhost:<port>`, with an optional path prefix such as `/v1`. The port is required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEndpoint {
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl LocalEndpoint {
    /// Syntax only; the broker also resolves `localhost` and refuses any non-loopback address.
    pub fn parse(url: &str) -> Option<Self> {
        let rest = url.strip_prefix("http://")?;
        let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let (host, port) = authority.rsplit_once(':')?;
        if !matches!(host, "127.0.0.1" | "[::1]" | "localhost") {
            return None;
        }
        if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
        let path = path.trim_end_matches('/');
        let path_ok = path.len() <= 128
            && path
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
            && !path.split('/').any(|seg| seg == "..");
        path_ok.then(|| Self {
            host: host.into(),
            port,
            path: path.into(),
        })
    }
}

/// Owner-created (docs/46: in a native dialog on the desktop only). There is no default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub base_url: String,
    pub model: String,
}

/// A model a loopback server runs elsewhere: Ollama names them `name:cloud` or `name:tag-cloud`
/// (docs/46 section 8.1). Case-insensitive, and erring towards remote: a false "remote" only refuses.
/// A name with whitespace or control characters is never a valid local tag (`"m:cloud "` must not
/// slip past the suffix test), so it counts as remote too.
pub fn is_remote_model(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    m.ends_with(":cloud")
        || m.ends_with("-cloud")
        || model.chars().any(|c| c.is_whitespace() || c.is_control())
}

impl ProviderProfile {
    /// `local` needs a loopback `http` endpoint and a model that is not remote: a loopback Ollama
    /// serves `:cloud` models that run elsewhere (docs/46 section 8.1, remote-model trap).
    pub fn class(&self) -> ProviderClass {
        if LocalEndpoint::parse(&self.base_url).is_some() && !is_remote_model(&self.model) {
            ProviderClass::Local
        } else {
            ProviderClass::Cloud
        }
    }

    /// Bound into every round's record and every consent: a changed endpoint or model is a new
    /// profile.
    pub fn digest(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.base_url.as_bytes());
        h.update([0]);
        h.update(self.model.as_bytes());
        format!("sha256:{:x}", h.finalize())
    }

    /// The client principal the service scopes this profile's tool calls to.
    pub fn principal(&self) -> &'static str {
        match self.class() {
            ProviderClass::Local => "ask:local",
            ProviderClass::Cloud => "ask:cloud",
        }
    }
}

/// The owner's consent record for one cloud profile (digest-bound).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudConsent {
    pub profile_digest: String,
}

/// The only way to a provider. Local: a broker per round. Cloud: refused without a consent
/// record for this exact profile, and refused with one until the cloud class exists.
pub fn open(
    profile: &ProviderProfile,
    broker: PathBuf,
    consent: Option<&CloudConsent>,
) -> Result<BrokerProvider, ProviderError> {
    match profile.class() {
        ProviderClass::Local => Ok(BrokerProvider {
            exe: broker,
            profile: profile.clone(),
            limits: Limits::default(),
        }),
        ProviderClass::Cloud => {
            if consent.is_none_or(|c| c.profile_digest != profile.digest()) {
                return Err(ProviderError::new(
                    ErrorKind::ConsentRequired,
                    "a cloud provider needs the owner's consent record for this profile",
                    false,
                ));
            }
            Err(ProviderError::new(
                ErrorKind::CloudUnavailable,
                "the cloud class waits on q-5's conditions (broker canaries, names-only projection, sensitive_data)",
                false,
            ))
        }
    }
}

/// Per-round network limits for the local class (docs/46 section 8.1). The broker clamps each to
/// these defaults; tests may only shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub connect_ms: u64,
    pub first_byte_ms: u64,
    pub stall_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            connect_ms: 2_000,
            first_byte_ms: 90_000,
            stall_ms: 30_000,
        }
    }
}

/// What the engine writes to a broker's stdin: one round, then the broker exits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerRequest {
    pub endpoint: String,
    pub model: String,
    pub limits: Limits,
    pub deadline_ms: u64,
    pub request: ChatRequest,
}

/// The broker's one stdout line.
pub type BrokerReply = Result<ChatReply, ProviderError>;

/// A local profile's provider: spawns `exe chat` once per round with a minimal environment, writes
/// one [`BrokerRequest`], reads one [`BrokerReply`] and kills the process at the deadline.
#[derive(Debug, Clone)]
pub struct BrokerProvider {
    exe: PathBuf,
    profile: ProviderProfile,
    pub limits: Limits,
}

impl Provider for BrokerProvider {
    fn complete(&self, req: &ChatRequest, deadline: Duration) -> Result<ChatReply, ProviderError> {
        let started = Instant::now();
        let body = serde_json::to_vec(&BrokerRequest {
            endpoint: self.profile.base_url.clone(),
            model: self.profile.model.clone(),
            limits: self.limits,
            deadline_ms: deadline.as_millis().try_into().unwrap_or(u64::MAX),
            request: req.clone(),
        })
        .map_err(|e| ProviderError::new(ErrorKind::InvalidRequest, e.to_string(), false))?;
        if body.len() > MAX_REQUEST_BYTES + 4096 {
            return Err(ProviderError::new(
                ErrorKind::InvalidRequest,
                "round exceeds the request bound",
                false,
            ));
        }
        let mut cmd = Command::new(&self.exe);
        cmd.arg("chat")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // Winsock needs SystemRoot to load its providers; nothing else is inherited.
        if let Some(root) = std::env::var_os("SystemRoot") {
            cmd.env("SystemRoot", root);
        }
        let broker_failed =
            |m: String, dispatched| ProviderError::new(ErrorKind::BrokerFailed, m, dispatched);
        let mut child = cmd
            .spawn()
            .map_err(|e| broker_failed(format!("broker did not start: {e}"), false))?;
        let mut stdin = child.stdin.take().expect("piped");
        let mut stdout = child.stdout.take().expect("piped");
        let reader = std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = (&mut stdout)
                .take((MAX_REPLY_BYTES * 2) as u64)
                .read_to_end(&mut out);
            out
        });
        // A broker that never drains stdin must not hold this thread past `kill_at`: the write runs
        // aside and the deadline loop below owns the child.
        let (wrote_tx, wrote_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = wrote_tx.send(stdin.write_all(&body).is_ok());
        });
        // The broker enforces its own deadline; the job is killed shortly after it.
        let kill_at = started + deadline + Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < kill_at => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    // A write still blocked (a grandchild may hold the pipe open) did not finish:
                    // that thread is left to end when the pipe closes, never joined here.
                    let wrote = wrote_rx.try_recv().unwrap_or(false);
                    // Whatever the broker sent is unknown: never auto-retried.
                    return Err(ProviderError::new(
                        ErrorKind::Timeout,
                        "round deadline passed; outcome unknown",
                        wrote,
                    ));
                }
            }
        }
        let wrote = wrote_rx.recv().unwrap_or(false);
        let out = reader.join().unwrap_or_default();
        let line = out.split(|b| *b == b'\n').next().unwrap_or_default();
        serde_json::from_slice::<BrokerReply>(line)
            .unwrap_or_else(|_| Err(broker_failed("broker reply unreadable".into(), wrote)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(url: &str, model: &str) -> ProviderProfile {
        ProviderProfile {
            base_url: url.into(),
            model: model.into(),
        }
    }

    /// A broker stand-in that never reads stdin, only sleeps.
    fn deaf_broker(dir: &std::path::Path) -> PathBuf {
        #[cfg(windows)]
        let (name, body) = (
            "deaf.cmd",
            "@%SystemRoot%\\System32\\ping.exe -n 60 127.0.0.1 >nul\r\n",
        );
        #[cfg(not(windows))]
        let (name, body) = ("deaf.sh", "#!/bin/sh\nexec sleep 60\n");
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn a_broker_that_never_reads_stdin_is_killed_at_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let p = open(
            &profile("http://127.0.0.1:11434/v1", "m"),
            deaf_broker(dir.path()),
            None,
        )
        .unwrap();
        // Far more than any pipe buffer holds, so write_all blocks.
        let req = ChatRequest {
            messages: vec![Message::User {
                content: "q".repeat(200 * 1024),
            }],
            tools: vec![],
        };
        let t = Instant::now();
        let e = p.complete(&req, Duration::from_millis(300)).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        assert_eq!(e.kind, ErrorKind::Timeout, "{}", e.message);
        // The body never reached a reader, so nothing provably left the machine.
        assert!(!e.dispatched);
    }

    #[test]
    fn only_an_explicit_loopback_http_endpoint_with_a_local_model_is_local() {
        for url in [
            "http://127.0.0.1:11434",
            "http://127.0.0.1:11434/v1",
            "http://localhost:1234/v1/",
            "http://[::1]:8080",
        ] {
            assert_eq!(
                profile(url, "qwen3:4b").class(),
                ProviderClass::Local,
                "{url}"
            );
        }
        for url in [
            "https://127.0.0.1:11434",
            "http://127.0.0.1",
            "http://127.0.0.1:",
            "http://127.0.0.1:0",
            "http://127.0.0.2:11434",
            "http://localhost.example.com:80",
            "http://user@127.0.0.1:80",
            "http://127.0.0.1:80/../x",
            "http://127.0.0.1:80/v1?x=1",
            "http://0.0.0.0:11434",
            "http://192.168.1.2:11434",
            "",
        ] {
            assert_eq!(profile(url, "m").class(), ProviderClass::Cloud, "{url}");
        }
        for model in [
            "gpt-oss:120b:cloud",
            "gpt-oss:120b-cloud",
            "qwen3-coder:480b-CLOUD",
            "m:Cloud",
            "gpt-oss:120b-cloud ",
            "m:cloud\t",
            " m:cloud",
            "a b",
            "m\u{7}",
        ] {
            assert_eq!(
                profile("http://127.0.0.1:11434", model).class(),
                ProviderClass::Cloud,
                "{model}"
            );
        }
        assert_eq!(
            profile("http://127.0.0.1:11434", "llama3.2:3b").class(),
            ProviderClass::Local
        );
    }

    #[test]
    fn a_cloud_profile_never_opens_and_needs_its_own_consent_record() {
        let cloud = profile("https://api.example.invalid/v1", "m");
        let other = profile("https://api.example.invalid/v1", "m2");
        let exe = PathBuf::from("unused");
        let kind = |c: Option<&CloudConsent>| open(&cloud, exe.clone(), c).unwrap_err();
        assert_eq!(kind(None).kind, ErrorKind::ConsentRequired);
        let wrong = CloudConsent {
            profile_digest: other.digest(),
        };
        assert_eq!(kind(Some(&wrong)).kind, ErrorKind::ConsentRequired);
        let right = CloudConsent {
            profile_digest: cloud.digest(),
        };
        let e = kind(Some(&right));
        assert_eq!(e.kind, ErrorKind::CloudUnavailable);
        assert!(!e.dispatched);
        assert!(open(&profile("http://127.0.0.1:9", "m"), exe, None).is_ok());
    }

    #[test]
    fn the_profile_digest_changes_with_endpoint_or_model() {
        let a = profile("http://127.0.0.1:1/v1", "m");
        assert_ne!(a.digest(), profile("http://127.0.0.1:2/v1", "m").digest());
        assert_ne!(a.digest(), profile("http://127.0.0.1:1/v1", "n").digest());
        assert_ne!(
            profile("http://127.0.0.1:1/v", "1m").digest(),
            profile("http://127.0.0.1:1/v1", "m").digest()
        );
    }
}
