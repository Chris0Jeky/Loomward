//! `loomward-broker chat`: reads one `BrokerRequest` on stdin, makes one call, writes one
//! `BrokerReply` line on stdout and exits. Spawned per model round by `loomward-ask`.

use loomward_ask::{BrokerReply, BrokerRequest, ErrorKind, ProviderError, MAX_REQUEST_BYTES};
use std::io::{Read, Write};

fn main() {
    let reply: BrokerReply = if std::env::args().nth(1).as_deref() != Some("chat") {
        Err(ProviderError::new(
            ErrorKind::InvalidRequest,
            "usage: loomward-broker chat",
            false,
        ))
    } else {
        let mut input = Vec::new();
        let limit = (MAX_REQUEST_BYTES + 8192) as u64;
        match std::io::stdin().take(limit + 1).read_to_end(&mut input) {
            Ok(n) if n as u64 <= limit => serde_json::from_slice::<BrokerRequest>(&input)
                .map_err(|_| {
                    ProviderError::new(ErrorKind::InvalidRequest, "unreadable request", false)
                })
                .and_then(|req| loomward_broker::call(&req)),
            _ => Err(ProviderError::new(
                ErrorKind::InvalidRequest,
                "request too large or unreadable",
                false,
            )),
        }
    };
    let mut out = serde_json::to_vec(&reply).expect("serialisable");
    out.push(b'\n');
    let _ = std::io::stdout().write_all(&out);
}
