//! The broker's chat profile for the `local` class (docs/46 section 8.1, F6 in part): one
//! OpenAI-compatible `POST <base>/chat/completions` to the owner's loopback endpoint, then exit.
//!
//! The endpoint comes from the owner's profile, never from a default. It must be `http` on
//! `127.0.0.1`, `[::1]` or `localhost`, and `localhost` must resolve to loopback addresses only;
//! the check runs on every call. No redirects are followed, no proxy is consulted, no credential
//! is sent, and the request and reply are bounded. A connection that never opened is
//! `unreachable` with `dispatched: false`; after the first write, every failure says it may have
//! been sent.

use loomward_ask::{
    BrokerReply, BrokerRequest, ChatReply, ErrorKind, FunctionCall, Limits, LocalEndpoint,
    ProviderError, ToolCall, MAX_REPLY_BYTES, MAX_REQUEST_BYTES,
};
use serde_json::Value;
use std::io::{ErrorKind as IoKind, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Header bytes allowed on top of the reply bound.
const MAX_HEAD_BYTES: usize = 16 * 1024;
const MAX_TOOL_CALLS: usize = 16;

fn err(kind: ErrorKind, message: impl Into<String>, dispatched: bool) -> ProviderError {
    ProviderError::new(kind, message, dispatched)
}

/// One round. Never panics on server input.
pub fn call(req: &BrokerRequest) -> BrokerReply {
    let started = Instant::now();
    let defaults = Limits::default();
    let ms = |v: u64, max: u64| Duration::from_millis(v.clamp(1, max));
    let (connect, first_byte, stall) = (
        ms(req.limits.connect_ms, defaults.connect_ms),
        ms(req.limits.first_byte_ms, defaults.first_byte_ms),
        ms(req.limits.stall_ms, defaults.stall_ms),
    );
    let deadline = started + ms(req.deadline_ms, 180_000);

    let endpoint = LocalEndpoint::parse(&req.endpoint)
        .ok_or_else(|| err(ErrorKind::NotLocal, "not a loopback http endpoint", false))?;
    if req.model.is_empty() || req.model.len() > 256 || loomward_ask::is_remote_model(&req.model) {
        return Err(err(ErrorKind::NotLocal, "not a local model name", false));
    }
    let addrs: Vec<SocketAddr> = (endpoint.host.trim_matches(['[', ']']), endpoint.port)
        .to_socket_addrs()
        .map_err(|_| err(ErrorKind::Unreachable, "the host did not resolve", false))?
        .collect();
    if addrs.is_empty() || addrs.iter().any(|a| !a.ip().is_loopback()) {
        return Err(err(
            ErrorKind::NotLocal,
            "the host is not loopback-only",
            false,
        ));
    }

    let body = loomward_ask::request_body(&req.model, &req.request);
    if body.len() > MAX_REQUEST_BYTES {
        return Err(err(ErrorKind::InvalidRequest, "request too large", false));
    }
    let head = format!(
        "POST {}/chat/completions HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        endpoint.path,
        endpoint.host,
        endpoint.port,
        body.len()
    );

    let mut stream = addrs
        .iter()
        .find_map(|a| TcpStream::connect_timeout(a, connect).ok())
        .ok_or_else(|| {
            err(
                ErrorKind::Unreachable,
                "nothing answered at the endpoint",
                false,
            )
        })?;
    // From here on bytes may have left: every failure is `dispatched`.
    let sent = |kind, m: &str| err(kind, m, true);
    let _ = stream.set_nodelay(true);
    stream
        .set_write_timeout(Some(stall))
        .and_then(|_| stream.write_all(head.as_bytes()))
        .and_then(|_| stream.write_all(body.as_bytes()))
        .map_err(|_| sent(ErrorKind::Unreachable, "the endpoint dropped the request"))?;

    let cap = MAX_REPLY_BYTES + MAX_HEAD_BYTES;
    let mut raw = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(sent(ErrorKind::Timeout, "round deadline passed"));
        }
        let wait = if raw.is_empty() { first_byte } else { stall };
        stream
            .set_read_timeout(Some(wait.min(deadline - now)))
            .map_err(|_| sent(ErrorKind::Timeout, "socket timeout unavailable"))?;
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.len() > cap {
                    return Err(sent(ErrorKind::Oversize, "reply exceeds the bound"));
                }
            }
            Err(e) if matches!(e.kind(), IoKind::WouldBlock | IoKind::TimedOut) => {
                let m = if raw.is_empty() {
                    "no first byte in time"
                } else {
                    "the reply stalled"
                };
                return Err(sent(ErrorKind::Timeout, m));
            }
            Err(e) if e.kind() == IoKind::Interrupted => {}
            // A reset after a complete Content-Length body still parses below.
            Err(_) if !raw.is_empty() => break,
            Err(_) => return Err(sent(ErrorKind::Unreachable, "the connection failed")),
        }
    }
    let body = parse_http(&raw).map_err(|(kind, m)| sent(kind, &m))?;
    parse_completion(&body).map_err(|m| sent(ErrorKind::InvalidReply, &m))
}

/// Status, headers and body of one HTTP/1.1 response. Redirects and non-2xx are errors.
fn parse_http(raw: &[u8]) -> Result<Vec<u8>, (ErrorKind, String)> {
    let invalid = |m: &str| (ErrorKind::InvalidReply, m.to_string());
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .filter(|p| *p <= MAX_HEAD_BYTES)
        .ok_or_else(|| invalid("no HTTP head"))?;
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| invalid("unreadable HTTP head"))?;
    let mut body = raw[split + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let status: u16 = lines
        .next()
        .and_then(|l| l.strip_prefix("HTTP/1.").and_then(|r| r.get(2..5)))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| invalid("no HTTP status"))?;
    if (300..400).contains(&status) {
        return Err((
            ErrorKind::HttpStatus,
            format!("HTTP {status}: redirects are refused"),
        ));
    }
    if !(200..300).contains(&status) {
        return Err((ErrorKind::HttpStatus, format!("HTTP {status}")));
    }
    let mut chunked = false;
    let mut length = None;
    for line in lines {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        } else if k == "content-length" {
            length = Some(
                v.parse::<usize>()
                    .map_err(|_| invalid("bad Content-Length"))?,
            );
        }
    }
    if chunked {
        body = dechunk(&body).ok_or_else(|| invalid("bad chunked body"))?;
    } else if let Some(n) = length {
        if body.len() < n {
            return Err(invalid("the reply ended early"));
        }
        body.truncate(n);
    }
    if body.len() > MAX_REPLY_BYTES {
        return Err((ErrorKind::Oversize, "reply exceeds the bound".into()));
    }
    Ok(body)
}

fn dechunk(mut raw: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = raw.windows(2).position(|w| w == b"\r\n")?;
        let size = std::str::from_utf8(&raw[..eol]).ok()?;
        let size = usize::from_str_radix(size.split(';').next()?.trim(), 16).ok()?;
        raw = &raw[eol + 2..];
        if size == 0 {
            return Some(out);
        }
        // Bound the size first: a hostile chunk size must not overflow `size + 2`.
        if size > MAX_REPLY_BYTES || raw.len() < size + 2 || out.len() + size > MAX_REPLY_BYTES {
            return None;
        }
        out.extend_from_slice(&raw[..size]);
        raw = &raw[size + 2..];
    }
}

/// `choices[0].message` of a Chat Completions reply: content and tool calls, strictly.
fn parse_completion(body: &[u8]) -> Result<ChatReply, String> {
    let v: Value = serde_json::from_slice(body).map_err(|_| "the reply is not JSON".to_string())?;
    let msg = v
        .pointer("/choices/0/message")
        .and_then(Value::as_object)
        .ok_or("the reply has no choices[0].message")?;
    let content = match msg.get("content") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("content is not text".into()),
    };
    let raw_calls = match msg.get("tool_calls") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(a)) if a.len() <= MAX_TOOL_CALLS => a.as_slice(),
        Some(_) => return Err("tool_calls is not a bounded list".into()),
    };
    let mut tool_calls = Vec::with_capacity(raw_calls.len());
    for (i, c) in raw_calls.iter().enumerate() {
        let f = c.get("function").ok_or("a tool call has no function")?;
        let name = f
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| (1..=64).contains(&n.len()))
            .ok_or("a tool call has no name")?;
        // OpenAI sends a JSON string; some local servers send the object itself.
        let arguments = match f.get("arguments") {
            Some(Value::String(s)) => s.clone(),
            Some(o @ Value::Object(_)) => o.to_string(),
            None | Some(Value::Null) => "{}".into(),
            Some(_) => return Err("tool arguments are not an object".into()),
        };
        let id = c
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| (1..=64).contains(&id.len()))
            .map_or_else(|| format!("call_{}", i + 1), str::to_owned);
        tool_calls.push(ToolCall {
            id,
            kind: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments,
            },
        });
    }
    Ok(ChatReply {
        content,
        tool_calls,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_hostile_chunk_size_is_refused_without_panicking() {
        assert_eq!(dechunk(b"ffffffffffffffff\r\nab\r\n0\r\n\r\n"), None);
        assert_eq!(dechunk(b"2\r\nab\r\n0\r\n\r\n"), Some(b"ab".to_vec()));
    }

    #[test]
    fn http_bodies_are_framed_and_statuses_are_honest() {
        let ok = |raw: &str| parse_http(raw.as_bytes());
        assert_eq!(
            ok("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}junk").unwrap(),
            b"{}"
        );
        assert_eq!(
            ok("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{\"\r\n3;x=1\r\na\":\r\n2\r\n1}\r\n0\r\n\r\n")
                .unwrap(),
            br#"{"a":1}"#
        );
        assert_eq!(ok("HTTP/1.0 200 OK\r\n\r\n{}").unwrap(), b"{}");
        assert_eq!(
            ok("HTTP/1.1 302 Found\r\nLocation: http://x/\r\n\r\n")
                .unwrap_err()
                .0,
            ErrorKind::HttpStatus
        );
        assert_eq!(ok("HTTP/1.1 503 Busy\r\n\r\n").unwrap_err().1, "HTTP 503");
        assert_eq!(
            ok("HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n{}")
                .unwrap_err()
                .0,
            ErrorKind::InvalidReply
        );
        assert_eq!(ok("garbage").unwrap_err().0, ErrorKind::InvalidReply);
        assert_eq!(
            ok("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n")
                .unwrap_err()
                .0,
            ErrorKind::InvalidReply
        );
    }

    #[test]
    fn completions_are_read_strictly() {
        let p = |v: Value| parse_completion(v.to_string().as_bytes());
        let r = p(
            json!({"choices": [{"message": {"content": null, "tool_calls": [
            {"function": {"name": "find", "arguments": {"text": "a"}}},
            {"id": "x", "function": {"name": "describe", "arguments": "{\"handle\":\"h1\"}"}}]}}]}),
        )
        .unwrap();
        assert_eq!(r.tool_calls[0].id, "call_1");
        assert_eq!(r.tool_calls[0].function.arguments, r#"{"text":"a"}"#);
        assert_eq!(r.tool_calls[1].id, "x");
        assert!(p(json!({"choices": []})).is_err());
        assert!(p(json!({"choices": [{"message": {"content": 5}}]})).is_err());
        let many: Vec<Value> = (0..17)
            .map(|_| json!({"function": {"name": "f"}}))
            .collect();
        assert!(p(json!({"choices": [{"message": {"tool_calls": many}}]})).is_err());
        assert!(parse_completion(b"not json").is_err());
    }
}
