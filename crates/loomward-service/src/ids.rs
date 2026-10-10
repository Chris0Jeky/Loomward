//! Opaque node IDs (docs/41 section 6.3): `nd_` + base64url of
//! `kind | row_id | born_run | instance tag | HMAC tag`. The HMAC-SHA256 tag covers the kind, row,
//! incarnation and the full catalogue instance under a per-session key that is never persisted, so
//! an ID is bound to the session and the catalogue instance (`semantics.md` section 5): after a
//! restart it is `not_found`. An ID only selects a row to view; it never authorises anything
//! (invariant 5).

use loomward_catalog::NodeKey;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const TAG: usize = 16;
const BODY: usize = 1 + 8 + 8 + 4;

/// RFC 2104 HMAC-SHA256.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let pad = |byte: u8| block.map(|b| b ^ byte);
    let inner = Sha256::new()
        .chain_update(pad(0x36))
        .chain_update(message)
        .finalize();
    Sha256::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub fn b64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 2);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

pub fn b64url_decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    for chunk in text.as_bytes().chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            let v = ALPHABET.iter().position(|a| a == c)? as u32;
            n |= v << (18 - 6 * i);
        }
        for i in 0..chunk.len() - 1 {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    // Canonical only: re-encoding must give the same text (no stray low bits).
    (b64url(&out) == text).then_some(out)
}

/// Why a structurally valid node ID did not open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not ours: malformed, forged, or under another key. `not_found` with no detail (no oracle).
    Foreign,
    /// Ours, minted for an earlier catalogue instance (`catalog_instance_changed`).
    InstanceChanged,
}

/// Seals and opens node IDs for one session key and the current catalogue instance.
pub struct NodeIds {
    key: [u8; 32],
}

fn kind_byte(key: NodeKey) -> (u8, i64) {
    match key {
        NodeKey::Atlas => (0, 0),
        NodeKey::Volume(id) => (1, id),
        NodeKey::Dir(id) => (2, id),
        NodeKey::File(id) => (3, id),
        NodeKey::Other(id) => (4, id),
    }
}

fn instance_tag(instance: &str) -> [u8; 4] {
    let d = Sha256::digest(instance.as_bytes());
    [d[0], d[1], d[2], d[3]]
}

fn tag_hex(instance: &str) -> String {
    instance_tag(instance)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Root and volume IDs: `prefix` + catalogue row + `_` + the catalogue instance tag. A rebuilt
/// catalogue reassigns row numbers, so an ID minted for another instance must never select the
/// row that now has its number (a retried revocation, an old tier declaration).
pub fn row_id(prefix: &str, row: i64, instance: &str) -> String {
    format!("{prefix}{row}_{}", tag_hex(instance))
}

/// The row a [`row_id`] names in `instance`. Any other spelling is [`Refusal::Foreign`].
pub fn open_row_id(prefix: &str, id: &str, instance: &str) -> Result<i64, Refusal> {
    let (row, tag) = id
        .strip_prefix(prefix)
        .and_then(|r| r.split_once('_'))
        .ok_or(Refusal::Foreign)?;
    let n = row
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == row)
        .ok_or(Refusal::Foreign)?;
    if tag.len() != 8 || !tag.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(Refusal::Foreign);
    }
    if tag != tag_hex(instance) {
        return Err(Refusal::InstanceChanged);
    }
    Ok(n)
}

impl NodeIds {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// A fresh key from the OS random source, held in memory only.
    pub fn session() -> Self {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).expect("OS random source");
        Self::new(key)
    }

    fn mac(&self, body: &[u8], instance: &str) -> [u8; 32] {
        let mut message = b"loomward/node/1\0".to_vec();
        message.extend_from_slice(body);
        message.extend_from_slice(instance.as_bytes());
        hmac_sha256(&self.key, &message)
    }

    /// `nd_` + 50 base64url characters.
    pub fn seal(&self, key: NodeKey, born_run: i64, instance: &str) -> String {
        let (kind, row) = kind_byte(key);
        let mut body = Vec::with_capacity(BODY + TAG);
        body.push(kind);
        body.extend_from_slice(&row.to_be_bytes());
        body.extend_from_slice(&born_run.to_be_bytes());
        body.extend_from_slice(&instance_tag(instance));
        let mac = self.mac(&body, instance);
        body.extend_from_slice(&mac[..TAG]);
        format!("nd_{}", b64url(&body))
    }

    /// The row and incarnation an ID names, once its tag verifies against `instance`. The caller
    /// still checks that the row exists with that `born_run`.
    pub fn open(&self, id: &str, instance: &str) -> Result<(NodeKey, i64), Refusal> {
        let raw = id
            .strip_prefix("nd_")
            .and_then(b64url_decode)
            .filter(|r| r.len() == BODY + TAG)
            .ok_or(Refusal::Foreign)?;
        let (body, tag) = raw.split_at(BODY);
        if body[17..21] != instance_tag(instance) {
            // The earlier instance is unknown, so its MAC cannot be checked: the reason is given
            // for any ID shaped like ours. It leaks nothing (the tag is a public hash prefix)
            // and never opens a row.
            return Err(if body[0] <= 4 {
                Refusal::InstanceChanged
            } else {
                Refusal::Foreign
            });
        }
        if !bool::from(self.mac(body, instance)[..TAG].ct_eq(tag)) {
            return Err(Refusal::Foreign);
        }
        let row = i64::from_be_bytes(body[1..9].try_into().unwrap());
        let born = i64::from_be_bytes(body[9..17].try_into().unwrap());
        let key = match body[0] {
            0 if row == 0 => NodeKey::Atlas,
            1 => NodeKey::Volume(row),
            2 => NodeKey::Dir(row),
            3 => NodeKey::File(row),
            4 => NodeKey::Other(row),
            _ => return Err(Refusal::Foreign),
        };
        Ok((key, born))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn hmac_matches_rfc4231() {
        // Test case 1 and 2 of RFC 4231.
        assert_eq!(
            hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Test case 6: a key longer than the block is hashed first.
        assert_eq!(
            hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn base64url_round_trips_and_is_canonical() {
        for n in 0..40u8 {
            let bytes: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(b64url_decode(&b64url(&bytes)).unwrap(), bytes);
        }
        assert_eq!(b64url(b"\xfb\xff"), "-_8");
        assert!(
            b64url_decode("-_9").is_none(),
            "stray low bits are not canonical"
        );
        assert!(b64url_decode("A").is_none());
        assert!(b64url_decode("ab+/").is_none());
    }

    #[test]
    fn row_ids_are_bound_to_their_catalogue_instance() {
        let id = row_id("rt_", 5, "inst-a");
        assert!(loomward_protocol::RootId::new(&id).is_ok());
        assert!(loomward_protocol::VolumeId::new(row_id("vo_", i64::MAX, "inst-a")).is_ok());
        assert_eq!(open_row_id("rt_", &id, "inst-a"), Ok(5));
        // Minted for another instance: refused with the reason, never row 5 of this one.
        assert_eq!(
            open_row_id("rt_", &id, "inst-b"),
            Err(Refusal::InstanceChanged)
        );
        let tag = &id[id.len() - 8..];
        for bad in [
            "rt_5".to_string(),
            format!("rt_05_{tag}"),
            format!("rt_+5_{tag}"),
            format!("rt_0_{tag}"),
            format!("rt_-1_{tag}"),
            "rt_5_ABCDEF12".to_string(),
            format!("rt_5_{tag}0"),
            format!("vo_5_{tag}"),
        ] {
            assert_eq!(
                open_row_id("rt_", &bad, "inst-a"),
                Err(Refusal::Foreign),
                "{bad}"
            );
        }
    }

    #[test]
    fn node_ids_seal_open_and_refuse_tampering() {
        let ids = NodeIds::new([7; 32]);
        let id = ids.seal(NodeKey::File(42), 9, "inst-a");
        assert!(id.len() <= 67 && loomward_protocol::NodeId::new(&id).is_ok());
        assert_eq!(ids.open(&id, "inst-a"), Ok((NodeKey::File(42), 9)));
        // Another key never opens it.
        assert_eq!(
            NodeIds::new([8; 32]).open(&id, "inst-a"),
            Err(Refusal::Foreign)
        );
        // A rebuilt catalogue reports the instance change, not the row.
        assert_eq!(ids.open(&id, "inst-b"), Err(Refusal::InstanceChanged));
        // Every single-bit flip in the body or tag is refused.
        let raw = b64url_decode(&id[3..]).unwrap();
        for i in 0..raw.len() {
            if (17..21).contains(&i) {
                continue; // instance tag bytes: covered by the instance case above
            }
            let mut forged = raw.clone();
            forged[i] ^= 1;
            let forged = format!("nd_{}", b64url(&forged));
            assert_eq!(
                ids.open(&forged, "inst-a"),
                Err(Refusal::Foreign),
                "byte {i}"
            );
        }
        // The catalogue's internal references are not node IDs.
        assert_eq!(ids.open("nd_file_42", "inst-a"), Err(Refusal::Foreign));
        assert_eq!(ids.open("nd_", "inst-a"), Err(Refusal::Foreign));
    }
}
