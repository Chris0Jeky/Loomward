//! Archive custody manifest root (LW-136, docs/51 section 2.2): a Merkle-style root over the entries of one
//! directory, under a versioned collation. Pure: no I/O, no file or process effect, no authority.
//!
//! Each entry is `(name, kind, size, digest)`. A file's digest is its content digest; a directory's digest is
//! the root the caller computed for that directory's own entries (the Merkle recursion lives in the caller:
//! compute the child root first, then pass it as the parent's entry). The root of a directory is the SHA-256
//! of the following byte sequence, in this order, with no separators beyond the length prefixes given:
//!
//! 1. the domain tag `loomward/manifest-root/1` followed by one `0x00` byte;
//! 2. `collation_version` as a `u32`, little-endian (4 bytes);
//! 3. the entry count as a `u64`, little-endian (8 bytes);
//! 4. for each entry, in collation order (below):
//!    - the exact name's UTF-8 length in bytes as a `u64`, little-endian, then the exact name's UTF-8 bytes
//!      (never folded, so `Readme` and `readme` produce different roots);
//!    - the kind as one byte: `0x00` for a file, `0x01` for a directory;
//!    - the size as a `u64`, little-endian;
//!    - the 32-byte digest.
//!
//! Collation (the order in which entries are hashed). Each entry has a total sort key, compared in this order:
//! 1. the folded name, folded with `str::to_lowercase` (the standard library's Unicode lowercase mapping),
//!    compared as a sequence of UTF-16 code units, each compared as an unsigned 16-bit integer, a proper
//!    prefix sorting first;
//! 2. the exact name's UTF-8 bytes (this is the tiebreak that orders names differing only by case);
//! 3. the kind byte;
//! 4. the size;
//! 5. the digest bytes.
//!
//! Steps 3 to 5 only matter when two entries have the same exact name, which a filesystem does not produce;
//! they keep the root independent of input order even for malformed input. Duplicate exact names are not
//! rejected.
//!
//! The collation implemented here is collation version 1. `collation_version` is mixed into the hash, so the
//! same entries under two versions never share a root. This module contains only the collation above; a
//! different collation needs its own function, and the caller must pass the version that names the collation
//! it used. An empty directory has the root defined by steps 1 to 3 with an entry count of zero.
//!
//! Names are single path components. This function does not validate them; the caller supplies exact names.

use sha2::{Digest as _, Sha256};

const DOMAIN: &[u8] = b"loomward/manifest-root/1\0";

/// Kind of a directory entry, encoded as one byte in the root (`0x00` file, `0x01` directory).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
}

impl EntryKind {
    fn byte(self) -> u8 {
        match self {
            EntryKind::File => 0,
            EntryKind::Directory => 1,
        }
    }
}

/// One directory entry as the manifest root sees it. `digest` is the content digest for a file and the child
/// root for a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyEntry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub digest: [u8; 32],
}

/// Manifest root of one directory under `collation_version` (see the module documentation for the exact
/// encoding and collation). Independent of the order of `entries`; an empty slice has a defined root.
pub fn manifest_root(entries: &[CustodyEntry], collation_version: u32) -> [u8; 32] {
    let mut keyed: Vec<(Vec<u16>, &CustodyEntry)> = entries
        .iter()
        .map(|e| (e.name.to_lowercase().encode_utf16().collect(), e))
        .collect();
    keyed.sort_by(|(fa, a), (fb, b)| {
        fa.cmp(fb)
            .then_with(|| a.name.as_bytes().cmp(b.name.as_bytes()))
            .then_with(|| a.kind.byte().cmp(&b.kind.byte()))
            .then_with(|| a.size.cmp(&b.size))
            .then_with(|| a.digest.cmp(&b.digest))
    });

    let mut h = Sha256::new();
    h.update(DOMAIN);
    h.update(collation_version.to_le_bytes());
    h.update((keyed.len() as u64).to_le_bytes());
    for (_, e) in keyed {
        h.update((e.name.len() as u64).to_le_bytes());
        h.update(e.name.as_bytes());
        h.update([e.kind.byte()]);
        h.update(e.size.to_le_bytes());
        h.update(e.digest);
    }
    h.finalize().into()
}
