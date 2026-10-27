use crate::{
    json,
    protocol::{error, Protocol},
};
use serde_json::Value;
use std::io::{self, BufRead, Read, Write};
use std::sync::mpsc::{self, SyncSender};

const MAX_FRAME: u64 = 65_536;
const MAX_OUTPUT: usize = 262_144;
const MAX_DRAIN: usize = 1_048_576;

enum Frame {
    Json(Vec<u8>),
    Oversized { stop: bool },
    Io(io::Error),
}

fn read_frames(mut source: impl BufRead, tx: SyncSender<Frame>) {
    loop {
        let mut raw = Vec::new();
        match source
            .by_ref()
            .take(MAX_FRAME + 1)
            .read_until(b'\n', &mut raw)
        {
            Ok(0) => break,
            Err(e) => {
                let _ = tx.send(Frame::Io(e));
                break;
            }
            Ok(_) => {}
        }
        let frame = if raw.len() as u64 > MAX_FRAME {
            let mut drained = raw.len();
            let mut stop = false;
            while raw.last() != Some(&b'\n') {
                raw.clear();
                match source
                    .by_ref()
                    .take(MAX_FRAME + 1)
                    .read_until(b'\n', &mut raw)
                {
                    Ok(0) => break,
                    Err(e) => {
                        let _ = tx.send(Frame::Io(e));
                        return;
                    }
                    Ok(n) => drained += n,
                }
                if drained > MAX_DRAIN {
                    stop = true;
                    break;
                }
            }
            Frame::Oversized { stop }
        } else {
            Frame::Json(raw)
        };
        let stop = matches!(frame, Frame::Oversized { stop: true });
        if tx.send(frame).is_err() || stop {
            break;
        }
    }
}

fn encode(response: &Value) -> Vec<u8> {
    let mut response = response.clone();
    crate::text::escape(&mut response);
    let encoded = response.to_string().into_bytes();
    if encoded.len() > MAX_OUTPUT {
        error(-32603, "Response size budget exceeded", response.get("id"))
            .to_string()
            .into_bytes()
    } else {
        encoded
    }
}

pub fn serve(mut protocol: Protocol) -> io::Result<()> {
    // A bounded reader allows idle shutdown even with stdin open or a partial frame pending.
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("lw-mcp-stdin".into())
        .spawn(move || {
            read_frames(io::stdin().lock(), tx);
        })?;
    let stdout = io::stdout();
    let mut sink = stdout.lock();
    loop {
        protocol.host.tick();
        let frame = match protocol.host.time_left() {
            Some(left) => match rx.recv_timeout(left) {
                Ok(frame) => frame,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(frame) => frame,
                Err(_) => break,
            },
        };
        protocol.host.tick();
        let stop = matches!(frame, Frame::Oversized { stop: true });
        let response = match frame {
            Frame::Json(raw) => match json::parse(&raw) {
                Ok(message) => protocol.handle(message),
                Err(_) => Some(error(
                    -32700,
                    "Malformed, duplicate-key, nonfinite or excessively nested JSON",
                    None,
                )),
            },
            Frame::Oversized { .. } => {
                Some(error(-32600, "Request frame exceeds 65536 bytes", None))
            }
            Frame::Io(error) => return Err(error),
        };
        if let Some(response) = response {
            sink.write_all(&encode(&response))?;
            sink.write_all(b"\n")?;
            sink.flush()?;
        }
        if stop {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn structured_content_without_a_text_block_encodes_without_panicking() {
        for content in [None, Some(json!([]))] {
            let mut response = json!({
                "jsonrpc": "2.0", "id": "structured-only",
                "result": {"structuredContent": {"name": "synthetic\u{202e}"}}
            });
            if let Some(content) = content {
                response["result"]["content"] = content;
            }
            let encoded: Value = serde_json::from_slice(&encode(&response)).unwrap();
            assert_eq!(encoded["id"], response["id"]);
            assert_eq!(
                encoded["result"]["structuredContent"]["name"],
                "synthetic\\u202E"
            );
            assert_eq!(
                encoded["result"].get("content"),
                response["result"].get("content")
            );
        }
    }

    #[test]
    fn output_budget_refuses_large_responses_and_preserves_the_id() {
        let response =
            json!({"jsonrpc":"2.0","id":"bounded","result":{"text":"x".repeat(MAX_OUTPUT)}});
        let encoded = encode(&response);
        assert!(encoded.len() < MAX_OUTPUT);
        let v: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(v["id"], "bounded");
        assert_eq!(v["error"]["code"], -32603);
    }

    #[test]
    fn frame_cap_includes_newline_and_bounded_drain_stops_the_reader() {
        let (tx, rx) = mpsc::sync_channel(4);
        let mut raw = vec![b' '; MAX_FRAME as usize - 1];
        raw.push(b'\n');
        raw.extend(vec![b' '; MAX_DRAIN + MAX_FRAME as usize]);
        read_frames(io::Cursor::new(raw), tx);
        assert!(matches!(rx.recv().unwrap(), Frame::Json(_)));
        assert!(matches!(
            rx.recv().unwrap(),
            Frame::Oversized { stop: true }
        ));
        assert!(rx.recv().is_err());
    }
}
