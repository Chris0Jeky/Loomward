//! Doc 47 lane C4 (LW-038, #43): the copy manifest, the preflight refusal classifier, the canonical
//! manifest digest and the single-use approval bound to it.
//!
//! Pure: no I/O, no handle, no effect. The inputs are fresh native observations of the owner-selected
//! sources and of the destination parent (lanes C2 and C5 supply them); a catalogue node ID only ever
//! selects what to observe, it never reaches the digest (invariant 5). An [`Approval`] is consumed by
//! [`Approval::admit`], which yields the `started` record's digest and item count only when a freshly
//! rebuilt [`Preview`] has no refusal and the very same digest.
use crate::journal::{component_ok, MAX_ITEMS};
use crate::tree::Kind;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::HashSet;

/// Single-use approval lifetime (doc 47 section 9; assumption).
pub const APPROVAL_TTL_MS: i64 = 30 * 60 * 1000;
/// Named `$DATA` streams per file (doc 47 section 6).
pub const MAX_STREAMS: usize = 64;
/// Space kept free beside the need: the larger of 1 GiB and 1% of the volume (doc 47 section 7; assumption).
pub const MIN_RESERVE: u64 = 1 << 30;
/// Space counted per folder, the new one included (doc 47 section 7).
pub const FOLDER_BYTES: u64 = 4096;
/// Final path limit in UTF-16 units [W13].
pub const MAX_PATH_UNITS: usize = 32_767;
/// One name component limit in UTF-16 units [W13].
pub const MAX_COMPONENT_UNITS: usize = 255;

const REPARSE_POINT: u32 = 0x400;
const OFFLINE: u32 = 0x1000;
const RECALL_ON_OPEN: u32 = 0x4_0000;
const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
/// System, temporary, sparse, compressed, encrypted, integrity stream (doc 47 section 6, refused).
const UNSUPPORTED: u32 = 0x4 | 0x100 | 0x200 | 0x800 | 0x4000 | 0x8000;
/// `IO_REPARSE_TAG_CLOUD*`: 0x9000x01A for every x.
fn cloud_tag(tag: u32) -> bool {
    tag & !0xF000 == 0x9000_001A
}

/// Every refusal code of doc 47 section 7, in table order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    IdentityUnverified,
    CloudPlaceholder,
    ReparsePoint,
    UnsupportedAttribute,
    ExtendedAttributes,
    AlternateStreamsUnsupported,
    TooManyStreams,
    SpecialFile,
    SourceChanged,
    StagingNameTaken,
    StagingInUse,
    ChangedAfterVerification,
    SourceInUse,
    AccessDenied,
    InsufficientSpace,
    DestinationInsideSource,
    DestinationCloudSynced,
    DestinationUnsupportedVolume,
    DestinationProtected,
    CaseCollision,
    NameUnrepresentable,
    PathTooLong,
    PublishInUse,
    VerificationMismatch,
    GrantRevoked,
    Elevated,
    ApprovalExpired,
    DigestMismatch,
    ManifestLimit,
}

impl Code {
    pub const ALL: [Code; 29] = {
        use Code::*;
        [
            IdentityUnverified,
            CloudPlaceholder,
            ReparsePoint,
            UnsupportedAttribute,
            ExtendedAttributes,
            AlternateStreamsUnsupported,
            TooManyStreams,
            SpecialFile,
            SourceChanged,
            StagingNameTaken,
            StagingInUse,
            ChangedAfterVerification,
            SourceInUse,
            AccessDenied,
            InsufficientSpace,
            DestinationInsideSource,
            DestinationCloudSynced,
            DestinationUnsupportedVolume,
            DestinationProtected,
            CaseCollision,
            NameUnrepresentable,
            PathTooLong,
            PublishInUse,
            VerificationMismatch,
            GrantRevoked,
            Elevated,
            ApprovalExpired,
            DigestMismatch,
            ManifestLimit,
        ]
    };

    /// Checked at preview (P) or at start (S): refused before the operation's first effect, so before any
    /// write handle could exist. Everything else is a runtime code the executor (C5) journals.
    pub fn preflight(self) -> bool {
        use Code::*;
        !matches!(
            self,
            SpecialFile
                | StagingNameTaken
                | StagingInUse
                | ChangedAfterVerification
                | SourceInUse
                | AccessDenied
                | PublishInUse
                | VerificationMismatch
        )
    }
}

/// A native object identity: volume serial number plus the 128-bit file ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Identity {
    pub volume: u64,
    pub file_id: [u8; 16],
}

/// One owner-selected source object as a fresh native listing observed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEntry {
    /// Exact native name relative to the selection's parent: the selected item's own name, then
    /// `/`-joined descendants. Never normalised.
    pub name: String,
    pub kind: Kind,
    pub identity: Identity,
    pub size: u64,
    pub creation_ft: i64,
    pub last_write_ft: i64,
    pub change_ft: i64,
    pub attributes: u32,
    pub reparse_tag: Option<u32>,
    pub ea_size: u32,
    /// Named `$DATA` streams: name (without colons) and size.
    pub streams: Vec<(String, u64)>,
    pub link_count: u32,
    /// The object carried its own (non-inherited) ACEs; reported, never copied.
    pub explicit_aces: bool,
    /// The source volume verifies identity (NTFS or ReFS).
    pub verified_volume: bool,
}

/// The owner-picked destination parent as a fresh native observation (doc 47 section 4 "Checks").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationFacts {
    /// The active grant the parent was picked under.
    pub grant: u64,
    pub parent: Identity,
    /// The parent and each of its ancestors up to the volume root.
    pub ancestors: Vec<Identity>,
    pub verified_volume: bool,
    pub cloud_synced: bool,
    /// Removable or network volume.
    pub unsupported_volume: bool,
    /// Loomward's state directory or a scan fence (#284, #286).
    pub protected: bool,
    pub case_sensitive: bool,
    /// Names already in the parent, for the " (2)" proposal.
    pub existing_names: Vec<String>,
    /// UTF-16 length of the parent's final path.
    pub parent_path_units: usize,
    pub available_bytes: u64,
    pub volume_bytes: u64,
    pub cluster_bytes: u64,
}

/// Session facts the classifier needs beside the observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub elevated: bool,
    /// Every source grant the selection came through is still active.
    pub grants_active: bool,
    /// H1 measured positive: named streams can be copied. Until then files with named streams are refused.
    pub named_streams: bool,
    /// `YYYY-MM-DD`, for the multi-item folder name.
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestItem {
    /// Exact name relative to the new folder, `/`-joined.
    pub name: String,
    pub kind: Kind,
    pub identity: Identity,
    pub size: u64,
    pub creation_ft: i64,
    pub last_write_ft: i64,
    pub change_ft: i64,
    pub attributes: u32,
    pub link_count: u32,
    pub streams: Vec<(String, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub grant: u64,
    pub parent: Identity,
    /// The one new folder created inside the parent (v1 never writes into an existing folder; Q3 open).
    pub folder_name: String,
}

/// The canonical manifest: items sorted by exact name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub destination: Destination,
    /// When exactly one non-empty folder is selected, the new folder is its copy: this is that folder
    /// (its times and attributes go to the new folder) and `items` are its descendants.
    pub root: Option<ManifestItem>,
    pub items: Vec<ManifestItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub code: Code,
    /// The manifest item name, or `None` for an operation-wide refusal.
    pub item: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub manifest: Manifest,
    /// Empty: the preview is a `draft` that can be approved. Otherwise it is `refused`; deselecting builds
    /// a new preview.
    pub refusals: Vec<Refused>,
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
    pub space_needed: u64,
    pub space_reserve: u64,
    pub same_volume: bool,
    pub explicit_acl_items: u64,
    pub hard_link_names: u64,
}

fn units(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Not creatable as a Win32 name component: reserved device names, trailing dot or space, over 255
/// units, or a character `component_ok` (the journal's own rule) refuses.
pub fn name_unrepresentable(component: &str) -> bool {
    const DEVICES: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
    let stem = component
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let device = DEVICES.contains(&stem.as_str())
        || ["COM", "LPT"].iter().any(|p| {
            stem.strip_prefix(p).is_some_and(|n| {
                matches!(
                    n,
                    "1" | "2"
                        | "3"
                        | "4"
                        | "5"
                        | "6"
                        | "7"
                        | "8"
                        | "9"
                        | "\u{b9}"
                        | "\u{b2}"
                        | "\u{b3}"
                )
            })
        });
    device
        || !component_ok(component)
        || component.ends_with(['.', ' '])
        || units(component) > MAX_COMPONENT_UNITS
}

/// Case folding for collisions under a case-insensitive destination.
// ponytail: one-to-one Unicode uppercase approximates NTFS's $UpCase table (CompareStringOrdinal,
// ignore case); a native fold from C2 replaces it if a fixture ever disagrees.
fn fold(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut up = c.to_uppercase();
            match (up.next(), up.next()) {
                (Some(u), None) => u,
                _ => c,
            }
        })
        .collect()
}

fn round_up(n: u64, cluster: u64) -> u64 {
    n.div_ceil(cluster).saturating_mul(cluster)
}

fn propose_name(base: &str, dest: &DestinationFacts) -> String {
    let key = |s: &str| {
        if dest.case_sensitive {
            s.to_string()
        } else {
            fold(s)
        }
    };
    let taken: HashSet<String> = dest.existing_names.iter().map(|n| key(n)).collect();
    // `taken` is finite, so a free suffix exists within len + 2 tries.
    (1..)
        .map(|n| match n {
            1 => base.to_string(),
            n => format!("{base} ({n})"),
        })
        .find(|name| !taken.contains(&key(name)))
        .expect("a free name exists")
}

/// Build the preview: the canonical manifest of the selection plus every preflight (P) refusal.
pub fn preview(entries: Vec<SourceEntry>, dest: &DestinationFacts, ctx: &Context) -> Preview {
    let mut refusals = Vec::new();
    let mut refuse = |code, item: Option<&str>| {
        refusals.push(Refused {
            code,
            item: item.map(str::to_string),
        })
    };
    if ctx.elevated {
        refuse(Code::Elevated, None);
    }
    if !ctx.grants_active {
        refuse(Code::GrantRevoked, None);
    }
    if !dest.verified_volume {
        refuse(Code::IdentityUnverified, None);
    }
    if dest.cloud_synced {
        refuse(Code::DestinationCloudSynced, None);
    }
    if dest.unsupported_volume {
        refuse(Code::DestinationUnsupportedVolume, None);
    }
    if dest.protected {
        refuse(Code::DestinationProtected, None);
    }
    if entries.is_empty() || entries.len() > MAX_ITEMS as usize {
        refuse(Code::ManifestLimit, None);
    }

    // The single selected folder with descendants becomes the new folder itself.
    let tops: HashSet<&str> = entries
        .iter()
        .map(|e| e.name.split('/').next().unwrap_or(""))
        .collect();
    let single = (tops.len() == 1 && entries.len() > 1)
        .then(|| {
            entries
                .iter()
                .find(|e| !e.name.contains('/') && e.kind == Kind::Directory)
        })
        .flatten()
        .map(|e| e.name.clone());
    let base = single
        .clone()
        .unwrap_or_else(|| format!("Loomward copy {}", ctx.date));
    let folder_name = propose_name(&base, dest);
    if name_unrepresentable(&folder_name) {
        refuse(Code::NameUnrepresentable, None);
    }

    let mut root = None;
    let mut items = Vec::with_capacity(entries.len());
    let (mut files, mut folders, mut bytes, mut need) = (0u64, 0u64, 0u64, FOLDER_BYTES);
    let (mut acl, mut links, mut same_volume) = (0u64, 0u64, false);
    let cluster = dest.cluster_bytes.max(1);
    let inside: HashSet<Identity> = dest.ancestors.iter().copied().collect();
    for e in entries {
        let shown = e.name.clone();
        let item = Some(shown.as_str());
        if !e.verified_volume {
            refuse(Code::IdentityUnverified, item);
        }
        let tag = e.reparse_tag.unwrap_or(0);
        let cloud = e.attributes & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0
            || (e.reparse_tag.is_some() && cloud_tag(tag));
        if cloud {
            refuse(Code::CloudPlaceholder, item);
        } else if e.attributes & REPARSE_POINT != 0 || e.reparse_tag.is_some() {
            refuse(Code::ReparsePoint, item);
        }
        if e.attributes & UNSUPPORTED != 0 {
            refuse(Code::UnsupportedAttribute, item);
        }
        if e.ea_size > 0 {
            refuse(Code::ExtendedAttributes, item);
        }
        if !e.streams.is_empty() && (e.kind == Kind::Directory || !ctx.named_streams) {
            refuse(Code::AlternateStreamsUnsupported, item);
        } else if e.streams.len() > MAX_STREAMS {
            refuse(Code::TooManyStreams, item);
        }
        if e.kind == Kind::Directory && inside.contains(&e.identity) {
            refuse(Code::DestinationInsideSource, item);
        }
        if e.name.split('/').any(name_unrepresentable)
            || e.streams.iter().any(|(s, _)| name_unrepresentable(s))
        {
            refuse(Code::NameUnrepresentable, item);
        }
        same_volume |= e.identity.volume == dest.parent.volume;
        acl += u64::from(e.explicit_aces);
        match e.kind {
            Kind::Directory => folders += 1,
            Kind::File => {
                files += 1;
                links += u64::from(e.link_count > 1);
                bytes = bytes.saturating_add(e.size);
                need = need.saturating_add(round_up(e.size, cluster));
                for (_, s) in &e.streams {
                    bytes = bytes.saturating_add(*s);
                    need = need.saturating_add(round_up(*s, cluster));
                }
            }
        }
        let name = match &single {
            Some(top) if e.name == *top => None,
            Some(top) => Some(e.name[top.len() + 1..].to_string()),
            None => Some(e.name),
        };
        let item = ManifestItem {
            name: name.clone().unwrap_or(shown),
            kind: e.kind,
            identity: e.identity,
            size: e.size,
            creation_ft: e.creation_ft,
            last_write_ft: e.last_write_ft,
            change_ft: e.change_ft,
            attributes: e.attributes,
            link_count: e.link_count,
            streams: e.streams,
        };
        match name {
            Some(_) => items.push(item),
            None => root = Some(item),
        }
    }
    // The new folder is already counted; a stripped root is that folder.
    need = need.saturating_add(FOLDER_BYTES.saturating_mul(folders - u64::from(root.is_some())));
    items.sort_by(|a, b| a.name.cmp(&b.name));

    let fold_key = |s: &str| {
        if dest.case_sensitive {
            s.to_string()
        } else {
            fold(s)
        }
    };
    let mut seen = HashSet::new();
    for it in &items {
        if !seen.insert(fold_key(&it.name)) {
            refuse(Code::CaseCollision, Some(&it.name));
        }
        if dest.parent_path_units + 1 + units(&folder_name) + 1 + units(&it.name) > MAX_PATH_UNITS {
            refuse(Code::PathTooLong, Some(&it.name));
        }
    }
    let reserve = MIN_RESERVE.max(dest.volume_bytes / 100);
    if dest.available_bytes < need.saturating_add(reserve) {
        refuse(Code::InsufficientSpace, None);
    }
    Preview {
        manifest: Manifest {
            destination: Destination {
                grant: dest.grant,
                parent: dest.parent,
                folder_name,
            },
            root,
            items,
        },
        refusals,
        files,
        folders,
        bytes,
        space_needed: need,
        space_reserve: reserve,
        same_volume,
        explicit_acl_items: acl,
        hard_link_names: links,
    }
}

struct Canon(Sha256);
impl Canon {
    fn u64(&mut self, n: u64) {
        self.0.update(n.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.0.update(s.as_bytes());
    }
    fn id(&mut self, id: Identity) {
        self.u64(id.volume);
        self.0.update(id.file_id);
    }
    fn item(&mut self, it: &ManifestItem, full: bool) {
        self.str(&it.name);
        self.0.update([matches!(it.kind, Kind::Directory) as u8]);
        if !full {
            return;
        }
        self.id(it.identity);
        self.u64(it.size);
        self.u64(it.creation_ft as u64);
        self.u64(it.last_write_ft as u64);
        self.u64(it.change_ft as u64);
        self.u64(u64::from(it.attributes));
        self.u64(u64::from(it.link_count));
        self.u64(it.streams.len() as u64);
        for (name, size) in &it.streams {
            self.str(name);
            self.u64(*size);
        }
    }
}

impl Manifest {
    fn hash(&self, full: bool) -> [u8; 32] {
        let mut c = Canon(Sha256::new());
        c.0.update(if full {
            b"loomward/copy-manifest/1\0".as_slice()
        } else {
            b"loomward/copy-shape/1\0".as_slice()
        });
        c.u64(self.destination.grant);
        c.id(self.destination.parent);
        c.str(&self.destination.folder_name);
        match &self.root {
            Some(r) => {
                c.0.update([1]);
                c.item(r, full);
            }
            None => c.0.update([0]),
        }
        c.u64(self.items.len() as u64);
        for it in &self.items {
            c.item(it, full);
        }
        c.0.finalize().into()
    }

    /// Canonical SHA-256 over destination (grant, parent identity, new folder name), membership (exact
    /// names, never normalised, in sorted order), bytes, identities and the preserved metadata the
    /// executor will compare. Integers little-endian, strings length-prefixed UTF-8.
    pub fn digest(&self) -> [u8; 32] {
        self.hash(true)
    }

    /// Destination plus membership only: tells `source_changed` (same shape, other bytes or identities)
    /// apart from `digest_mismatch` (another selection or destination).
    fn shape(&self) -> [u8; 32] {
        self.hash(false)
    }
}

/// The owner's consent, bound to one manifest digest, its destination grant and the grant epoch it
/// was given under. Fields are private: an approval exists only through [`approve`], and
/// [`Approval::admit`] consumes it, so it is single use by construction.
#[derive(Debug, PartialEq, Eq)]
pub struct Approval {
    digest: [u8; 32],
    shape: [u8; 32],
    grant: u64,
    grants_epoch: u64,
    approved_at_ms: i64,
    expires_at_ms: i64,
}

/// What `start` hands the journal (`Store::start`): the consumed approval's digest and item count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admitted {
    pub digest: [u8; 32],
    pub items: u32,
}

/// `draft -> approved`: the owner confirmed `shown`, the digest rendered from the stored manifest.
pub fn approve(
    preview: &Preview,
    shown: [u8; 32],
    grants_epoch: u64,
    now_ms: i64,
) -> Result<Approval, Code> {
    if let Some(r) = preview.refusals.first() {
        return Err(r.code);
    }
    let digest = preview.manifest.digest();
    if shown != digest {
        return Err(Code::DigestMismatch);
    }
    Ok(Approval {
        digest,
        shape: preview.manifest.shape(),
        grant: preview.manifest.destination.grant,
        grants_epoch,
        approved_at_ms: now_ms,
        expires_at_ms: now_ms.saturating_add(APPROVAL_TTL_MS),
    })
}

impl Approval {
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }
    pub fn destination_grant(&self) -> u64 {
        self.grant
    }
    pub fn approved_at_ms(&self) -> i64 {
        self.approved_at_ms
    }
    pub fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }

    /// `approved -> preparing`, consuming the approval. `fresh` is the preview rebuilt from new native
    /// observations at start (S): it must carry no refusal and exactly the approved digest. A moved grant
    /// epoch (a revocation or emergency stop since approval) refuses as `grant_revoked`.
    pub fn admit(self, fresh: &Preview, grants_epoch: u64, now_ms: i64) -> Result<Admitted, Code> {
        if grants_epoch != self.grants_epoch {
            return Err(Code::GrantRevoked);
        }
        if now_ms >= self.expires_at_ms {
            return Err(Code::ApprovalExpired);
        }
        if let Some(r) = fresh.refusals.first() {
            return Err(r.code);
        }
        if fresh.manifest.digest() != self.digest {
            return Err(if fresh.manifest.shape() == self.shape {
                Code::SourceChanged
            } else {
                Code::DigestMismatch
            });
        }
        let items = u32::try_from(fresh.manifest.items.len())
            .ok()
            .filter(|n| (1..=MAX_ITEMS).contains(n))
            .ok_or(Code::ManifestLimit)?;
        Ok(Admitted {
            digest: self.digest,
            items,
        })
    }
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
