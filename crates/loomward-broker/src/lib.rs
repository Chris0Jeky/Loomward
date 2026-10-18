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

fn remaining(
    deadline: Instant,
    limit: Duration,
    dispatched: bool,
) -> Result<Duration, ProviderError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .map(|left| left.min(limit))
        .ok_or_else(|| err(ErrorKind::Timeout, "round deadline passed", dispatched))
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

    let mut connected = None;
    for addr in addrs {
        let wait = remaining(deadline, connect, false)?;
        if let Ok(stream) = TcpStream::connect_timeout(&addr, wait) {
            connected = Some(stream);
            break;
        }
    }
    let stream = connected.ok_or_else(|| {
        err(
            if Instant::now() >= deadline {
                ErrorKind::Timeout
            } else {
                ErrorKind::Unreachable
            },
            "nothing answered at the endpoint",
            false,
        )
    })?;
    exchange(
        stream,
        head.as_bytes(),
        body.as_bytes(),
        first_byte,
        stall,
        deadline,
    )
}

fn exchange(
    mut stream: TcpStream,
    head: &[u8],
    body: &[u8],
    first_byte: Duration,
    stall: Duration,
    deadline: Instant,
) -> BrokerReply {
    let _ = stream.set_nodelay(true);
    let mut dispatched = false;
    for mut bytes in [head, body] {
        while !bytes.is_empty() {
            let wait = remaining(deadline, stall, dispatched)?;
            stream
                .set_write_timeout(Some(wait))
                .map_err(|_| err(ErrorKind::Timeout, "socket timeout unavailable", dispatched))?;
            // A failed write can still have sent bytes.
            dispatched = true;
            match stream.write(bytes) {
                Ok(n) if n > 0 => bytes = &bytes[n..],
                Err(e) if e.kind() == IoKind::Interrupted => continue,
                Err(e) if matches!(e.kind(), IoKind::WouldBlock | IoKind::TimedOut) => {
                    return Err(err(ErrorKind::Timeout, "the request stalled", true));
                }
                _ => {
                    return Err(err(
                        ErrorKind::Unreachable,
                        "the endpoint dropped the request",
                        true,
                    ))
                }
            }
        }
    }
    let sent = |kind, m: &str| err(kind, m, true);

    let cap = MAX_REPLY_BYTES + MAX_HEAD_BYTES;
    let mut raw = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let wait = if raw.is_empty() { first_byte } else { stall };
        stream
            .set_read_timeout(Some(remaining(deadline, wait, true)?))
            .map_err(|_| sent(ErrorKind::Timeout, "socket timeout unavailable"))?;
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.len() > cap {
                    return Err(sent(ErrorKind::Oversize, "reply exceeds the bound"));
                }
                if let Some(body) = parse_http(&raw, false).map_err(|(kind, m)| sent(kind, &m))? {
                    return parse_completion(&body).map_err(|m| sent(ErrorKind::InvalidReply, &m));
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
            Err(_) => return Err(sent(ErrorKind::Unreachable, "the connection failed")),
        }
    }
    let body = parse_http(&raw, true)
        .map_err(|(kind, m)| sent(kind, &m))?
        .ok_or_else(|| sent(ErrorKind::InvalidReply, "the reply ended early"))?;
    parse_completion(&body).map_err(|m| sent(ErrorKind::InvalidReply, &m))
}

/// Status, headers and body of one HTTP/1.1 response. Redirects and non-2xx are errors.
fn parse_http(raw: &[u8], eof: bool) -> Result<Option<Vec<u8>>, (ErrorKind, String)> {
    let invalid = |m: &str| (ErrorKind::InvalidReply, m.to_string());
    let split = match raw.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(p) if p <= MAX_HEAD_BYTES => p,
        None if !eof && raw.len() <= MAX_HEAD_BYTES + 3 => return Ok(None),
        _ => return Err(invalid("no HTTP head")),
    };
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
        match dechunk(&body).map_err(|_| invalid("bad chunked body"))? {
            Some(decoded) => body = decoded,
            None if !eof => return Ok(None),
            None => return Err(invalid("the reply ended early")),
        }
    } else if let Some(n) = length {
        if n > MAX_REPLY_BYTES {
            return Err((ErrorKind::Oversize, "reply exceeds the bound".into()));
        }
        if body.len() < n {
            return if eof {
                Err(invalid("the reply ended early"))
            } else {
                Ok(None)
            };
        }
        body.truncate(n);
    } else if !eof {
        return Ok(None);
    }
    if body.len() > MAX_REPLY_BYTES {
        return Err((ErrorKind::Oversize, "reply exceeds the bound".into()));
    }
    Ok(Some(body))
}

fn dechunk(mut raw: &[u8]) -> Result<Option<Vec<u8>>, ()> {
    let mut out = Vec::new();
    loop {
        let Some(eol) = raw.windows(2).position(|w| w == b"\r\n") else {
            return Ok(None);
        };
        let size = std::str::from_utf8(&raw[..eol]).map_err(|_| ())?;
        let size = size.split(';').next().ok_or(())?;
        if size.is_empty() || !size.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(());
        }
        let size = usize::from_str_radix(size, 16).map_err(|_| ())?;
        raw = &raw[eol + 2..];
        if size == 0 {
            loop {
                let Some(eol) = raw.windows(2).position(|w| w == b"\r\n") else {
                    return Ok(None);
                };
                if eol == 0 {
                    return Ok(Some(out));
                }
                let trailer = &raw[..eol];
                let colon = trailer.iter().position(|b| *b == b':').ok_or(())?;
                if colon == 0
                    || !trailer[..colon]
                        .iter()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(b))
                    || trailer[colon + 1..]
                        .iter()
                        .any(|b| b.is_ascii_control() && *b != b'\t')
                {
                    return Err(());
                }
                raw = &raw[eol + 2..];
            }
        }
        // Bound the size first: a hostile chunk size must not overflow `size + 2`.
        if size > MAX_REPLY_BYTES || out.len() + size > MAX_REPLY_BYTES {
            return Err(());
        }
        if raw.len() < size + 2 {
            return Ok(None);
        }
        if &raw[size..size + 2] != b"\r\n" {
            return Err(());
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
    use std::net::TcpListener;

    #[test]
    fn socket_timeouts_are_capped_by_the_remaining_round_budget() {
        let limit = Duration::from_secs(1);
        assert_eq!(
            remaining(Instant::now() + limit * 10, limit, false).unwrap(),
            limit
        );
        let left = remaining(Instant::now() + limit, limit * 10, true).unwrap();
        assert!(!left.is_zero() && left <= limit);
        for dispatched in [false, true] {
            let error = remaining(Instant::now() - limit, limit, dispatched).unwrap_err();
            assert_eq!(
                (error.kind, error.dispatched),
                (ErrorKind::Timeout, dispatched)
            );
        }
    }

    #[test]
    fn an_expired_round_sends_nothing_after_connecting() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        let error = exchange(
            stream,
            b"headers",
            b"body",
            Duration::from_secs(1),
            Duration::from_secs(1),
            Instant::now() - Duration::from_secs(1),
        )
        .unwrap_err();
        let mut bytes = Vec::new();
        peer.read_to_end(&mut bytes).unwrap();
        assert!(bytes.is_empty(), "expired request was sent: {bytes:?}");
        assert_eq!((error.kind, error.dispatched), (ErrorKind::Timeout, false));
    }

    #[test]
    fn malformed_chunk_delimiters_and_truncated_trailers_are_refused() {
        let completion = br#"{"choices":[{"message":{"content":"ok"}}]}"#;
        for (separator, terminal) in [
            ("XX", "0\r\n"),
            ("XX", "0\r\n\r\n"),
            ("\r\n", "0\r\n"),
            ("\r\n", "0\r\nX-Trace: synthetic\r\n"),
        ] {
            let raw = format!(
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}{separator}{terminal}",
                completion.len(),
                std::str::from_utf8(completion).unwrap()
            );
            assert!(
                parse_http(raw.as_bytes(), true).is_err(),
                "accepted {raw:?}"
            );
        }
    }

    #[test]
    fn framed_bodies_wait_for_every_byte_including_the_trailer_terminator() {
        for raw in [
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1;x=1\r\n{\r\n1\r\n}\r\n0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\nX-Trace: synthetic\r\nOther: value\r\n\r\n",
        ] {
            for end in 0..raw.len() {
                let partial = &raw.as_bytes()[..end];
                assert_eq!(parse_http(partial, false), Ok(None), "prefix {end} of {raw:?}");
                assert!(parse_http(partial, true).is_err(), "accepted truncated prefix {end}");
            }
            assert_eq!(parse_http(raw.as_bytes(), false), Ok(Some(b"{}".to_vec())));
        }
    }

    #[test]
    fn close_delimited_bodies_require_eof() {
        let raw = b"HTTP/1.0 200 OK\r\n\r\n{}";
        assert_eq!(parse_http(raw, false), Ok(None));
        assert_eq!(parse_http(raw, true), Ok(Some(b"{}".to_vec())));
    }

    #[test]
    fn a_hostile_chunk_size_is_refused_without_panicking() {
        assert!(dechunk(b"ffffffffffffffff\r\nab\r\n0\r\n\r\n").is_err());
        assert_eq!(dechunk(b"2\r\nab\r\n0\r\n\r\n"), Ok(Some(b"ab".to_vec())));
    }

    #[test]
    fn http_bodies_are_framed_and_statuses_are_honest() {
        let ok = |raw: &str| parse_http(raw.as_bytes(), true).map(|body| body.unwrap());
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
