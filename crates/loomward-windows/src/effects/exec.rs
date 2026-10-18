//! Lane C5 (docs/47 section 11, LW-040 #46): the copy executor on lab roots. It wires the C2 primitives
//! in this module's parent, the C3 journal and the C4 manifest. Every effect has its intent journaled
//! first; cleanup authority comes only from journaled identities (never a name). The service runs it
//! for synthetic sessions on registered lab roots only; personal sessions never reach it (AGENTS.md
//! invariant 1, the C8 gate stays closed).
use super::*;
use loomward_operations::{
    tree::Kind, Decision, Disk, Manifest, ObjectId, Observed, OpState, Record, Refusal, Replay,
    Store,
};
use serde::{Deserialize, Serialize};
use std::os::windows::ffi::OsStrExt;
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};

/// Everything a run needs, written to `op_plan` after `started` and before any effect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    /// The service's wire ID for this operation (`cp_...`), so it survives a restart.
    #[serde(default)]
    pub operation_id: String,
    pub source_root: PathBuf,
    pub source_root_id: [u8; 16],
    pub source_root_volume: u64,
    /// Handle-derived source binding; legacy plans without it cannot reopen native content.
    #[serde(default)]
    pub source_root_guid: Option<u128>,
    pub source_root_attributes: u32,
    pub destination: PathBuf,
    pub destination_key: ObjectKey,
    /// The destination grant's verified root: (volume serial, file ID). The pinned destination's
    /// ancestry must contain it at preview and again at run (lab roots only, invariant 2).
    pub destination_root: (u64, [u8; 16]),
    /// The service's grant numbers for the source and destination roots: revoking either fences the
    /// run (docs/47 section 5). Empty for plans journaled before #526 and in the lab suite.
    #[serde(default)]
    pub grants: Vec<i64>,
    pub manifest: Manifest,
    /// Per manifest item, in order: the path relative to `source_root` and the observed identity whose
    /// signature `Source::open` re-checks through pinned handles.
    pub sources: Vec<(PathBuf, identity::ObservedIdentity)>,
}

impl Plan {
    fn root_identity(&self) -> enumerate::OpenedIdentity {
        enumerate::OpenedIdentity {
            id: Some(enumerate::FileIdObs::Id128(self.source_root_id)),
            basis: enumerate::IdBasis::PostOpen,
            volume_guid: self.source_root_guid,
            volume_serial: Some(self.source_root_volume),
            attributes: self.source_root_attributes,
            reparse_tag: Some(0),
        }
    }
}

/// What `prepare` needs beside the paths: the session facts of C4's `Context` and the grants.
pub struct Request<'a> {
    /// Granted source root and the selected items relative to it (one shared parent folder).
    pub source_root: &'a Path,
    pub selected: &'a [PathBuf],
    /// The destination parent (a folder under a grant) and that grant's ID.
    pub destination: &'a Path,
    pub destination_grant: u64,
    pub grants_active: bool,
    /// Loomward's state directory or a scan fence contains the destination.
    pub protected: bool,
    pub date: &'a str,
    /// The destination grant's verified root identity: (volume serial, file ID).
    pub destination_root: (u64, [u8; 16]),
}

/// The pinned destination lies under the given root object (by identity, never by path).
fn within(dest: &Destination, root: (u64, [u8; 16])) -> io::Result<bool> {
    let serial = root.0.to_string();
    Ok(dest
        .ancestry()?
        .iter()
        .any(|k| k.file_id == root.1 && k.volume_serial == serial))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_string())
}

fn parse<T: std::str::FromStr>(text: &str) -> io::Result<T> {
    text.parse()
        .map_err(|_| invalid("unparseable native value"))
}

fn filesystem(path: &Path) -> io::Result<String> {
    let mut root = vec![0u16; 32768];
    let wide = win::wide(path)?;
    if unsafe { GetVolumePathNameW(wide.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut fs = [0u16; 32];
    if unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            fs.as_mut_ptr(),
            fs.len() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(String::from_utf16_lossy(
        &fs[..fs.iter().position(|c| *c == 0).unwrap_or(fs.len())],
    ))
}

/// Build the preview and the plan from FRESH native observations (docs/47 C4 inputs). Read-only: no
/// handle with write access, no content read. Called at `copy.preview` and again at `copy.start`,
/// whose digest must equal the approved one.
pub fn prepare(request: &Request) -> io::Result<(loomward_operations::Preview, Plan)> {
    prepare_with_limit(request, loomward_operations::MAX_ITEMS as usize)
}

fn prepare_with_limit(
    request: &Request,
    limit: usize,
) -> io::Result<(loomward_operations::Preview, Plan)> {
    use loomward_operations::{DestinationFacts, Identity, SourceEntry};
    let (_, root) = <enumerate::NativeSource as enumerate::DirSource>::open_root(
        &enumerate::NativeSource::default(),
        request.source_root,
    )
    .map_err(|_| invalid("source root refused"))?;
    let Some(enumerate::FileIdObs::Id128(root_id)) = root.id else {
        return Err(invalid("identity_unverified"));
    };
    let parent = request
        .selected
        .first()
        .and_then(|p| p.parent())
        .ok_or_else(|| invalid("empty selection"))?;
    if request.selected.iter().any(|p| p.parent() != Some(parent)) {
        return Err(invalid("the selection must share one parent folder"));
    }
    let source_fs_ok = enumerate::verifies_identity(&filesystem(request.source_root)?);
    let identity_of = |o: &identity::ObjectKey| -> io::Result<Identity> {
        Ok(Identity {
            volume: parse(&o.volume_serial)?,
            file_id: o.file_id,
        })
    };
    let mut entries = Vec::new();
    let mut observed: HashMap<String, (PathBuf, identity::ObservedIdentity)> = HashMap::new();
    if request.selected.len() > limit {
        return Err(invalid("manifest_limit"));
    }
    let mut pending: Vec<(PathBuf, String)> = request
        .selected
        .iter()
        .map(|p| {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| invalid("name_unrepresentable"))?;
            Ok((p.clone(), name.to_string()))
        })
        .collect::<io::Result<_>>()?;
    while let Some((relative, name)) = pending.pop() {
        if entries.len() >= limit {
            return Err(invalid("manifest_limit"));
        }
        let path = request.source_root.join(&relative);
        let o = identity::observe(&path)?;
        let probe = probe(&path)?;
        if probe.object != o.object {
            return Err(invalid("source_changed"));
        }
        let directory = o.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        let reparse = o.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
        if directory && !reparse {
            for child in std::fs::read_dir(&path)? {
                // Include the current directory, which has not entered `entries` yet.
                if entries.len() + pending.len() + 1 >= limit {
                    return Err(invalid("manifest_limit"));
                }
                let child = child?.file_name();
                let child = child
                    .to_str()
                    .ok_or_else(|| invalid("name_unrepresentable"))?;
                pending.push((relative.join(child), format!("{name}/{child}")));
            }
        }
        let mut streams = probe.streams;
        streams.sort();
        entries.push(SourceEntry {
            name: name.clone(),
            kind: if directory {
                Kind::Directory
            } else {
                Kind::File
            },
            identity: identity_of(&o.object)?,
            size: if directory { 0 } else { parse(&o.size)? },
            creation_ft: parse(&o.object.creation_time)?,
            last_write_ft: parse(&o.last_write_time)?,
            change_ft: parse(&o.change_time)?,
            attributes: o.attributes,
            reparse_tag: (o.object.reparse_tag != 0).then_some(o.object.reparse_tag),
            ea_size: probe.ea_size,
            streams,
            link_count: o.link_count,
            explicit_aces: probe.explicit_aces,
            verified_volume: source_fs_ok
                && o.object.quality == identity::IdentityQuality::FileId128,
        });
        observed.insert(name, (relative, o));
    }

    let dest_observed = identity::observe(request.destination)?;
    let dest = Destination::open(request.destination, &dest_observed.object)?;
    if !within(&dest, request.destination_root)? {
        return Err(invalid("destination_outside_grant"));
    }
    let mut ancestors = dest
        .ancestry()?
        .iter()
        .map(identity_of)
        .collect::<io::Result<Vec<_>>>()?;
    ancestors.dedup();
    let space = crate::volumes::space(request.destination)?;
    let existing_names = std::fs::read_dir(request.destination)?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<io::Result<Vec<_>>>()?;
    let facts = DestinationFacts {
        grant: request.destination_grant,
        parent: identity_of(&dest_observed.object)?,
        ancestors,
        verified_volume: enumerate::verifies_identity(&filesystem(request.destination)?),
        cloud_synced: dest.cloud_synced()?,
        unsupported_volume: crate::volumes::is_removable(request.destination)?,
        protected: request.protected,
        case_sensitive: dest.case_sensitive()?,
        existing_names,
        parent_path_units: request.destination.as_os_str().encode_wide().count(),
        available_bytes: parse(&space.free_bytes)?,
        volume_bytes: parse(&space.capacity_bytes)?,
        cluster_bytes: space.cluster_bytes.unwrap_or(4096),
    };
    drop(dest);
    let ctx = loomward_operations::Context {
        elevated: enumerate::running_elevated(),
        grants_active: request.grants_active,
        // H1 measured PASS on NTFS (evidence/v3/copy-primitives.md).
        named_streams: true,
        date: request.date.to_string(),
    };
    let preview = loomward_operations::preview(entries, &facts, &ctx);
    let top = preview.manifest.root.as_ref().map(|r| r.name.clone());
    let full = |name: &str| match &top {
        Some(top) => format!("{top}/{name}"),
        None => name.to_string(),
    };
    let sources = preview
        .manifest
        .items
        .iter()
        .map(|i| observed.get(&full(&i.name)).cloned())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| invalid("manifest and observation disagree"))?;
    let plan = Plan {
        operation_id: String::new(),
        source_root: request.source_root.to_path_buf(),
        source_root_id: root_id,
        source_root_volume: root
            .volume_serial
            .ok_or_else(|| invalid("no volume serial"))?,
        source_root_guid: root.volume_guid,
        source_root_attributes: root.attributes,
        destination: request.destination.to_path_buf(),
        destination_key: dest_observed.object,
        destination_root: request.destination_root,
        grants: Vec::new(),
        manifest: preview.manifest.clone(),
        sources,
    };
    Ok((preview, plan))
}

/// Test seams at journal boundaries. `Crash` stops the run on the spot with no further journal record
/// or effect, as a killed process would (the OS cache survives: crash consistency, not power loss).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    StagingCreated,
    /// Created and identity-committed: the batch's first item.
    Identities(u32),
    /// Default and named streams written and flushed, before metadata: the item.
    Written(u32),
    /// Verified and recorded: the batch's first item.
    Verified(u32),
    BeforePublish,
    AfterPublish,
    /// Before the item's preallocation and first write (lane C6 fault seam).
    Writing(u32),
    /// Before an item's create step, after the batch intent is durable.
    Creating(u32),
    /// Before a named stream's create step.
    CreatingStream(u32),
    /// Between an item create step's durable stop check and its create call (#670).
    Checked(u32),
    /// Between a named stream create step's durable stop check and its create call (#670).
    CheckedStream(u32),
    /// Between the staging step's durable stop check and the staging folder's creation (#672).
    CheckedStaging,
    /// Between the rename step's durable stop check and the publish rename (#672).
    CheckedPublish,
    /// Between a metadata step's durable stop check and its set call: a file, a folder or the staging
    /// folder (#672).
    CheckedMetadata,
    /// Between a cleanup removal's journal check and the removal (#672).
    CheckedRemoval,
    /// After the item cancellation check, before the potentially blocking source open.
    OpeningSource(u32),
    /// Inside a stream's chunk loop, between effect steps: the bytes handed to the writer so far.
    Chunk(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Crash,
    /// Lane C6 injected fault: the effect at this point fails with this OS error, as if the OS call had
    /// returned it (no small lab volume on the test hosts for a real ERROR_DISK_FULL). Honoured at
    /// `Point::Writing` only, where the error takes the same path as a failed preallocation or write.
    Fail(i32),
}

/// Lane C6 kill points (docs/47 section 10), test builds only: with `LOOMWARD_COPY_CRASH_AT=N` the Nth
/// boundary terminates the process on the spot (`TerminateProcess`: no unwinding, no destructor, no
/// handle closed or flushed by Loomward); `LOOMWARD_COPY_CRASH_TRACE=<file>` appends `N<TAB>label` per
/// boundary. Production builds compile every call to nothing and read no environment variable.
#[cfg(test)]
pub(crate) mod crash {
    use std::{
        io::Write,
        sync::{
            atomic::{AtomicU64, Ordering},
            OnceLock,
        },
    };
    pub const EXIT: u32 = 86;
    static COUNT: AtomicU64 = AtomicU64::new(0);
    type Config = (Option<u64>, Option<std::path::PathBuf>);
    fn config() -> &'static Config {
        static CONFIG: OnceLock<Config> = OnceLock::new();
        CONFIG.get_or_init(|| {
            (
                std::env::var("LOOMWARD_COPY_CRASH_AT")
                    .ok()
                    .and_then(|v| v.parse().ok()),
                std::env::var_os("LOOMWARD_COPY_CRASH_TRACE").map(Into::into),
            )
        })
    }
    pub fn hit(label: &str) {
        let (target, trace) = config();
        if target.is_none() && trace.is_none() {
            return;
        }
        let n = COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(path) = trace {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .expect("crash trace");
            writeln!(f, "{n}\t{label}").expect("crash trace");
        }
        if *target == Some(n) {
            use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
            unsafe { TerminateProcess(GetCurrentProcess(), EXIT) };
        }
    }
    /// The journal record's `kind`, with the target state for a transition.
    pub fn record(when: &str, record: &loomward_operations::Record) {
        let v = serde_json::to_value(record).expect("record");
        let kind = v["kind"].as_str().unwrap_or("?");
        match v["to"].as_str() {
            Some(to) => hit(&format!("{when} {kind}:{to}")),
            None => hit(&format!("{when} {kind}")),
        }
    }
}

#[inline]
fn hit(label: &str) {
    #[cfg(test)]
    crash::hit(label);
    #[cfg(not(test))]
    let _ = label;
}

type Hook = Box<dyn Fn(Point, &Control) -> Flow + Send + Sync>;

/// One run's controls: the owner's cancel, the staging-removal policy, and the revocation fence with
/// its admission lock (docs/47 section 5).
pub struct Control {
    cancel: AtomicBool,
    /// docs/47 Q1 (`loomward-copy-staging-removal`, OPEN, agent-hq#461). Off by default: cancel and
    /// abandon stop and leave the staging folder for the owner (option b). On: Loomward removes its own
    /// journaled staging by identity, and a first verification mismatch is redone.
    pub removal: bool,
    hook: Option<Hook>,
    /// Set before journaling a stop, so a failed append cannot leave admission open.
    fenced: AtomicBool,
    /// The per-run admission lock: every effect step holds it from its fence read until its one
    /// bounded OS call returns, so a revocation waits for at most the step in flight.
    admission: Mutex<()>,
    /// Set inside the rename step when the rename succeeded: the run is on the published side.
    renamed: AtomicBool,
}

/// What an acknowledged revocation says about the run (docs/47 section 5, revocation step 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ack {
    /// Already terminal or cancelled: nothing fenced.
    Settled,
    /// No effect step starts after the acknowledgement; the run cancels before publish.
    StoppedBeforePublish,
    /// The rename had completed: the run confirms and commits, annotated `grant_revoked`.
    Published,
    /// The step in flight did not return within the wait (stuck I/O); the fence is durable and every
    /// later step still reads it.
    PendingInFlightStep,
    /// The run had a publish intent and the destination could not be observed after the fence, so
    /// which side of the rename it ended on is unknown (invariant 4). The fence is durable; retry.
    PublishUnobserved,
}

/// docs/47 section 5 revocation step 3 (assumption: 30 s, to be measured).
const FENCE_WAIT: Duration = Duration::from_secs(30);

impl Control {
    pub fn new(removal: bool) -> Self {
        Control {
            cancel: AtomicBool::new(false),
            removal,
            hook: None,
            fenced: AtomicBool::new(false),
            admission: Mutex::new(()),
            renamed: AtomicBool::new(false),
        }
    }
    pub fn with_hook(
        mut self,
        hook: impl Fn(Point, &Control) -> Flow + Send + Sync + 'static,
    ) -> Self {
        self.hook = Some(Box::new(hook));
        self
    }
    /// Honoured at every item and every 1 MiB chunk, until publishing starts.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    fn at(&self, point: Point) -> Result<(), Stop> {
        match self
            .hook
            .as_ref()
            .map_or(Flow::Continue, |h| h(point, self))
        {
            Flow::Continue => Ok(()),
            Flow::Crash => Err(Stop::Crashed),
            Flow::Fail(code) => {
                debug_assert!(matches!(point, Point::Writing(_)), "fault at {point:?}");
                Err(io_refusal(&io::Error::from_raw_os_error(code)))
            }
        }
    }
    /// Start one effect step: take the admission lock, then read the fence. Hold the guard until the
    /// step's single OS call has returned. A fenced run starts no further step.
    fn admit(&self) -> Result<std::sync::MutexGuard<'_, ()>, Stop> {
        let step = self.admission.lock().unwrap_or_else(|e| e.into_inner());
        if self.fenced.load(Ordering::SeqCst) {
            return Err(Stop::Cancel);
        }
        Ok(step)
    }
    /// Raise the in-memory fence, then wait up to `wait` for the step
    /// in flight. `Some(renamed)` once acknowledged; `None` while that step has not returned.
    fn fence(&self, wait: Duration) -> Option<bool> {
        self.fenced.store(true, Ordering::SeqCst);
        let until = std::time::Instant::now() + wait;
        loop {
            match self.admission.try_lock() {
                Ok(_) | Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Some(self.renamed.load(Ordering::SeqCst))
                }
                Err(std::sync::TryLockError::WouldBlock) if std::time::Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                Err(_) => return None,
            }
        }
    }
    /// A revocation or emergency stop has fenced this run.
    pub fn fenced(&self) -> bool {
        self.fenced.load(Ordering::SeqCst)
    }
    /// Sleep up to `wait`, ending early once the owner cancels or a fence lands. False when cancelled.
    fn pause(&self, wait: Duration) -> bool {
        let until = std::time::Instant::now() + wait;
        while std::time::Instant::now() < until
            && !self.cancel.load(Ordering::SeqCst)
            && !self.fenced()
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        !self.cancel.load(Ordering::SeqCst)
    }
    fn check(&self) -> Result<(), Stop> {
        if self.cancel.load(Ordering::SeqCst) {
            Err(Stop::Cancel)
        } else {
            Ok(())
        }
    }
}

/// Where a run, cleanup or recovery left the operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub state: OpState,
    /// The last refusal journaled, with its manifest item when it is an item refusal.
    pub refusal: Option<(Refusal, Option<u32>)>,
    /// The staging folder's name in the destination parent when it was left for the owner (Q1 default).
    pub staging_left: Option<String>,
    pub items_total: u32,
    pub items_verified: u32,
    /// Staging tokens intended but never identity-committed: a folder under such a name may exist,
    /// empty and unjournaled. Never adopted or removed; reported for the owner.
    pub leftover_staging: Vec<String>,
    /// Staging-relative manifest names observed without journaled identities; never removed.
    pub unjournaled_items: Vec<String>,
}

#[derive(Debug)]
pub enum Fault {
    /// The journal refused a record or could not be written: no effect follows.
    Journal(String),
    /// A test crash point fired.
    Crashed,
}

enum Stop {
    Refused(Refusal, Option<u32>),
    /// Journal and disk disagree, or an error with no section 7 code: `needs_attention`, touch nothing.
    Attention(Option<Refusal>),
    Cancel,
    /// Another control changed the journal: observe its decision without settling it again.
    Superseded,
    /// Published, but the post-publish check could not read the tree now: stay `publishing` for
    /// recovery to confirm, as `confirm` does for an unverifiable read.
    Unconfirmed,
    Crashed,
    Journal(String),
}

/// A sharing, lock or access failure another process can clear: retried, never a finding about the copy.
fn transient(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

fn oid(key: &ObjectKey) -> ObjectId {
    ObjectId {
        file_id: key.file_id,
        creation_ft: key.creation_time.parse().unwrap_or(i64::MIN),
    }
}

/// What a `Cancellable` read reports once the owner cancels or a fence lands. Never `Interrupted`:
/// `read_exact` retries that forever (review of #544).
#[derive(Debug)]
struct Stopped;
impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("copy stopped by cancel or fence")
    }
}
impl std::error::Error for Stopped {}

fn io_refusal(e: &io::Error) -> Stop {
    match e.raw_os_error() {
        Some(80 | 183) => Stop::Refused(Refusal::StagingNameTaken, None),
        Some(5) => Stop::Refused(Refusal::AccessDenied, None),
        Some(32 | 33) => Stop::Refused(Refusal::StagingInUse, None),
        Some(39 | 112) => Stop::Refused(Refusal::InsufficientSpace, None),
        _ if e.get_ref().is_some_and(|r| r.is::<Stopped>()) => Stop::Cancel,
        _ => Stop::Attention(None),
    }
}

fn source_refusal(reason: duplicate_reader::Reason) -> Stop {
    use duplicate_reader::Reason as R;
    Stop::Refused(
        match reason {
            R::SharingViolation => Refusal::SourceInUse,
            R::AccessDenied => Refusal::AccessDenied,
            R::ChangedDuringRead | R::InvalidPath => Refusal::SourceChanged,
            R::ReparsePoint => Refusal::ReparsePoint,
            R::Placeholder | R::Offline => Refusal::CloudPlaceholder,
            R::SpecialFile => Refusal::SpecialFile,
            R::Unsupported => Refusal::UnsupportedAttribute,
            _ => return Stop::Attention(None),
        },
        None,
    )
}

fn with_item(stop: Stop, item: u32) -> Stop {
    match stop {
        Stop::Refused(code, _) => Stop::Refused(code, Some(item)),
        other => other,
    }
}

/// Reads the source in the writer's chunks, stopping at the next chunk once the owner cancels or a
/// fence lands. `step` is the effect step in progress (docs/47 section 5): taken at construction for
/// the preallocation, then retaken before each chunk's read, so that chunk's write (and the final
/// flush) runs under it; an in-memory fence stops the next chunk.
///
/// A detached worker's `Control` never sees a reopened service's fence, so each step also re-reads the
/// durable stop (#672). That check is not held across the write: holding the store lock for a
/// multi-GB file would stall every journal reader and writer. Residual window, bounded: a fence another
/// store acknowledges just after a step's check is followed by at most that step's effect, either the
/// preallocation (a size reservation, no data) or one chunk write (at most 1 MiB, `BLOCK`), and, after
/// the last chunk, the flush of bytes already written. No further chunk starts.
struct Cancellable<'a, R> {
    reader: &'a mut R,
    control: &'a Control,
    journal: &'a Journal<'a>,
    /// Bytes handed to the writer so far (a lane C6 mid-write kill point per chunk).
    read: u64,
    step: Option<std::sync::MutexGuard<'a, ()>>,
    /// Why the last read stopped, when the journal said so: the writer only sees an `io::Error`.
    stopped: Option<Stop>,
}
impl<'a, R> Cancellable<'a, R> {
    fn new(
        reader: &'a mut R,
        control: &'a Control,
        journal: &'a Journal<'a>,
    ) -> Result<Self, Stop> {
        let step = control.admit()?;
        control.check()?;
        journal.check_copying()?;
        Ok(Cancellable {
            reader,
            control,
            journal,
            read: 0,
            step: Some(step),
            stopped: None,
        })
    }
    /// The stop behind a failed write: the journal's, else `or` of the I/O error.
    fn stop(&mut self, e: &io::Error, or: impl FnOnce(&io::Error) -> Stop) -> Stop {
        self.stopped.take().unwrap_or_else(|| or(e))
    }
}
impl<R: Read> Read for Cancellable<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.step = None;
        // A test seam between chunk steps; a crash or fault here is not modelled.
        let _ = self.control.at(Point::Chunk(self.read));
        if self.control.cancel.load(Ordering::SeqCst) {
            return Err(io::Error::other(Stopped));
        }
        #[cfg(test)]
        crash::hit(&format!("write@{}", self.read));
        self.step = Some(
            self.control
                .admit()
                .map_err(|_| io::Error::other(Stopped))?,
        );
        if let Err(stop) = self.journal.check_copying() {
            self.stopped = Some(stop);
            return Err(io::Error::other(Stopped));
        }
        let n = self.reader.read(buffer)?;
        self.read += n as u64;
        Ok(n)
    }
}

const SOURCE_BUDGET: duplicate_reader::Budget = duplicate_reader::Budget {
    bytes: u64::MAX,
    time: Duration::from_secs(24 * 3600),
};
const PUBLISH_TRIES: u32 = 10;
const PUBLISH_WAIT: Duration = Duration::from_secs(3);

fn preserved(item: &loomward_operations::ManifestItem) -> BasicMetadata {
    BasicMetadata {
        creation_time: item.creation_ft,
        last_write_time: item.last_write_ft,
        attributes: item.attributes & PRESERVED,
    }
}

/// A staging-relative name split into its pinned ancestor path and its last component.
type Pinned<'n> = (Vec<(&'n str, ObjectKey)>, &'n str);
fn path_of<'n>(name: &'n str, keys: &HashMap<String, ObjectKey>) -> Result<Pinned<'n>, Stop> {
    let parts: Vec<&str> = name.split('/').collect();
    let mut path = Vec::with_capacity(parts.len() - 1);
    for k in 0..parts.len() - 1 {
        let key = keys
            .get(&parts[..=k].join("/"))
            .ok_or(Stop::Attention(None))?;
        path.push((parts[k], key.clone()));
    }
    Ok((path, parts[parts.len() - 1]))
}

struct Journal<'a> {
    store: &'a Mutex<Store>,
    op: u64,
    epoch: u64,
}
impl Journal<'_> {
    fn log(&self, record: Record) -> Result<(), Stop> {
        #[cfg(test)]
        crash::record("before", &record);
        #[cfg(test)]
        let after = record.clone();
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .append(self.op, self.epoch, record)
            .map_err(|e| Stop::Journal(format!("{e:?}")))?;
        #[cfg(test)]
        crash::record("after", &after);
        Ok(())
    }
    fn replay(&self) -> Result<Replay, Stop> {
        replay(self.store, self.op).map_err(|e| Stop::Journal(format!("{e:?}")))
    }

    fn check_copying(&self) -> Result<(), Stop> {
        self.while_copying(|| Ok(()))
    }

    fn while_copying<T>(&self, effect: impl FnOnce() -> Result<T, Stop>) -> Result<T, Stop> {
        self.while_in(OpState::Copying, effect)
    }

    /// A detached worker's Control is not the reopened service's Control: read the durable stop, and
    /// keep the database write lock until `effect` returns, so a fence appended by another store
    /// waits for the effect instead of landing between the check and it (#670, #672). Keep `effect` to
    /// one bounded create, rename, metadata or removal call (chunk writes only check, see
    /// `Cancellable`): another store's fence waits up to its busy timeout, then fails unacknowledged.
    // ponytail: a call stuck in the OS also holds this store's mutex, so a same-store fence waits for it
    // rather than answering `PendingInFlightStep`; split the lock from the connection (#672 item 3, open).
    fn while_in<T>(
        &self,
        state: OpState,
        effect: impl FnOnce() -> Result<T, Stop>,
    ) -> Result<T, Stop> {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .while_current(self.op, state, self.epoch, effect)
            .map_err(|e| Stop::Journal(e.to_string()))?
            .map_err(|e| Stop::Journal(format!("{e:?}")))?
            .unwrap_or(Err(Stop::Superseded))
    }
}

fn replay(store: &Mutex<Store>, op: u64) -> Result<Replay, Fault> {
    store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replay(op)
        .map_err(|e| Fault::Journal(e.to_string()))?
        .map_err(|e| Fault::Journal(format!("{e:?}")))
}

/// The operation's status as its journal replays (the receipt fields the service reports).
pub fn status(store: &Mutex<Store>, op: u64) -> Result<Outcome, Fault> {
    status_observing(store, op, false)
}

/// Status with no resident worker: re-observe interrupted runs without changing their journal.
pub fn status_idle(store: &Mutex<Store>, op: u64) -> Result<Outcome, Fault> {
    status_observing(store, op, true)
}

fn status_observing(store: &Mutex<Store>, op: u64, idle: bool) -> Result<Outcome, Fault> {
    let r = replay(store, op)?;
    let rows = store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .rows(op)
        .map_err(|e| Fault::Journal(e.to_string()))?;
    let mut state = r.state;
    let mut unjournaled_items = Vec::new();
    // Recovery cannot journal needs_attention from every state (e.g. preparing).
    // Re-observe those waiting states, and recorded attention, so later status reads keep the damage visible.
    if !state.is_terminal()
        && (idle
            || state == OpState::NeedsAttention
            || !state.can_transition(OpState::NeedsAttention))
    {
        let plan = store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .plan(op)
            .map_err(|e| Fault::Journal(e.to_string()))?
            .and_then(|p| serde_json::from_str::<Plan>(&p).ok());
        if let Some(plan) = plan {
            let disk = PathDisk::new(&plan, &r);
            let decision = loomward_operations::recover(op, &rows, &disk).decision;
            if matches!(decision, Decision::NeedsAttention(_))
                || (idle
                    && (disk.observation_failed.get()
                        || matches!(decision, Decision::Confirm { .. })))
            {
                state = OpState::NeedsAttention;
            }
            if state == OpState::NeedsAttention {
                if let Some(staging) = r.staging {
                    unjournaled_items = r
                        .items
                        .iter()
                        .filter(|i| i.id.is_none())
                        .filter_map(|i| i.name.as_ref())
                        .filter(|n| disk.in_staging(staging, n).is_some())
                        .cloned()
                        .collect();
                }
            }
        }
    }
    let mut refusal = None;
    for row in &rows {
        match serde_json::from_str::<loomward_operations::Entry>(&row.body).map(|e| e.record) {
            Ok(Record::ItemFailed { item, code }) => refusal = Some((code, Some(item))),
            Ok(Record::Transition {
                code: Some(code), ..
            }) => refusal = Some((code, None)),
            Ok(Record::Fenced { .. }) => refusal = Some((Refusal::GrantRevoked, None)),
            _ => {}
        }
    }
    Ok(Outcome {
        state,
        refusal,
        staging_left: r
            .staging_left
            .then(|| r.staging_tokens.last().cloned())
            .flatten(),
        items_total: r.items.len() as u32,
        items_verified: r.items.iter().filter(|i| i.verified).count() as u32,
        leftover_staging: match r.staging {
            Some(_) => r.staging_tokens[..r.staging_tokens.len() - 1].to_vec(),
            None => r.staging_tokens.clone(),
        },
        unjournaled_items,
    })
}

type StreamDigests = Vec<(String, [u8; 32], u64)>;

#[derive(Clone)]
struct Written {
    item: usize,
    digest: [u8; 32],
    streams: Vec<(String, [u8; 32], u64)>,
}

struct Runner<'a, 'd> {
    journal: Journal<'a>,
    plan: &'a Plan,
    control: &'a Control,
    staging: Staging<'d>,
    /// Current identity of every staged directory, by staging-relative name.
    keys: HashMap<String, ObjectKey>,
    /// Current identity of every verified file.
    files: HashMap<usize, ObjectKey>,
    /// Deny-write holds on default streams from verification until `committed` (docs/47 section 4;
    /// H2 positive for these), each with the named-stream digests it was verified with.
    holds: Vec<(VerifiedFile, StreamDigests)>,
    /// Named-stream holds: whether one blocks the ancestor rename depends on the host and location (H2:
    /// it blocked under the profile %TEMP%, not under a volume-root folder), so they are released just
    /// before publish regardless and the streams re-hashed through the default holds after.
    stream_holds: Vec<VerifiedFile>,
}

/// Run a started operation from `preparing` to `committed`, or to where a refusal, a cancel or the
/// journal stops it. The caller has journaled `started` (consuming the approval) and the plan.
pub fn run(store: &Mutex<Store>, op: u64, control: &Control) -> Result<Outcome, Fault> {
    let plan: Plan = {
        let text = store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .plan(op)
            .map_err(|e| Fault::Journal(e.to_string()))?
            .ok_or_else(|| Fault::Journal("no plan".into()))?;
        serde_json::from_str(&text).map_err(|e| Fault::Journal(e.to_string()))?
    };
    let r = replay(store, op)?;
    if r.state != OpState::Preparing || r.staging.is_some() || !r.staging_tokens.is_empty() {
        return Err(Fault::Journal("run starts only a fresh operation".into()));
    }
    let journal = Journal {
        store,
        op,
        epoch: r.epoch,
    };
    let stopped = match Destination::open(&plan.destination, &plan.destination_key) {
        Err(e) => {
            // Revalidation failed before any effect: `preparing -> refused`. An access, sharing or
            // space error keeps its own code; anything else means the name no longer reaches the
            // pinned object (renamed, replaced or removed): `destination_changed` (#526).
            let code = match io_refusal(&e) {
                Stop::Refused(code, _) if code != Refusal::StagingNameTaken => code,
                _ if e.kind() == io::ErrorKind::PermissionDenied => Refusal::AccessDenied,
                _ => Refusal::DestinationChanged,
            };
            journal.log(Record::Transition {
                to: OpState::Refused,
                code: Some(code),
            })
        }
        // The pinned parent must still sit under its grant's root (a swapped ancestor is refused).
        Ok(destination) if !within(&destination, plan.destination_root).unwrap_or(false) => journal
            .log(Record::Transition {
                to: OpState::Refused,
                code: Some(Refusal::DestinationChanged),
            }),
        Ok(destination) => copy(&journal, &plan, control, &destination),
    };
    finish(store, op, &plan, control, stopped)
}

fn copy(journal: &Journal, plan: &Plan, control: &Control, dest: &Destination) -> Result<(), Stop> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let token = format!("~lw-staging-{}-{nanos:x}", journal.op);
    journal.log(Record::StagingIntent {
        token: token.clone(),
    })?;
    // No staging identity exists yet, so the run cannot block or wait: `preparing -> refused` with the
    // cause (a token-named folder the OS may have left is reported as a leftover by `status`).
    let staged = {
        let _step = control.admit()?;
        journal.while_in(OpState::Preparing, || {
            control.at(Point::CheckedStaging)?;
            Ok(dest.stage(&token))
        })?
    };
    let staging = match staged {
        Ok(staging) => {
            hit("staging folder made");
            staging
        }
        Err(e) => {
            let code = match io_refusal(&e) {
                Stop::Refused(code, _) if code != Refusal::StagingNameTaken => Some(code),
                _ => None,
            };
            journal.log(Record::Transition {
                to: OpState::Refused,
                code,
            })?;
            return Err(Stop::Refused(code.unwrap_or(Refusal::AccessDenied), None));
        }
    };
    journal.log(Record::StagingCreated {
        id: oid(staging.identity()),
    })?;
    control.at(Point::StagingCreated)?;
    let mut runner = Runner {
        journal: Journal { ..*journal },
        plan,
        control,
        staging,
        keys: HashMap::new(),
        files: HashMap::new(),
        holds: Vec::new(),
        stream_holds: Vec::new(),
    };
    let order: Vec<usize> = (0..plan.manifest.items.len()).collect();
    for batch in order.chunks(loomward_operations::MAX_BATCH) {
        runner.batch(batch)?;
    }
    runner.directories()?;
    runner.publish()
}

impl Runner<'_, '_> {
    fn item(&self, i: usize) -> &loomward_operations::ManifestItem {
        &self.plan.manifest.items[i]
    }

    fn batch(&mut self, batch: &[usize]) -> Result<(), Stop> {
        self.control.check()?;
        self.journal.log(Record::CreateBatch {
            items: batch
                .iter()
                .map(|&i| (i as u32, self.item(i).name.clone()))
                .collect(),
        })?;
        let mut created = Vec::new();
        let mut open = Vec::new();
        let mut failure = None;
        for &i in batch {
            let item = self.item(i).clone();
            self.control.at(Point::Creating(i as u32))?;
            let step = match self.control.admit() {
                Ok(step) => step,
                Err(stop) => {
                    failure = Some(stop);
                    break;
                }
            };
            let made = path_of(&item.name, &self.keys).and_then(|(path, name)| {
                self.journal.while_copying(|| {
                    self.control.at(Point::Checked(i as u32))?;
                    match item.kind {
                        Kind::Directory => self
                            .staging
                            .create_directory(&path, name)
                            .map(|key| (key, None)),
                        Kind::File => self.staging.create_file(&path, name).and_then(|f| {
                            let key = f.identity()?;
                            Ok((key, Some(f)))
                        }),
                    }
                    .map_err(|e| io_refusal(&e))
                })
            });
            drop(step);
            match made {
                Ok((key, file)) => {
                    hit("created item");
                    created.push((i as u32, oid(&key)));
                    match file {
                        Some(f) => open.push((i, f)),
                        None => {
                            self.keys.insert(item.name.clone(), key);
                        }
                    }
                }
                Err(stop) => {
                    failure = Some(with_item(stop, i as u32));
                    break;
                }
            }
        }
        if let Some(stop) = failure {
            // Stop creating; journal the refusal, then the identities of what exists (C3 partial commit).
            self.refuse(&stop)?;
            if !created.is_empty() {
                self.journal.log(Record::Identities { items: created })?;
            }
            return Err(stop);
        }
        self.journal.log(Record::Identities { items: created })?;
        self.control.at(Point::Identities(batch[0] as u32))?;
        let mut written = Vec::new();
        for (i, file) in &open {
            match self.write(*i, file) {
                Ok(w) => written.push(w),
                Err(stop) => {
                    let stop = with_item(stop, *i as u32);
                    self.refuse(&stop)?;
                    return Err(stop);
                }
            }
            self.control.at(Point::Written(*i as u32))?;
        }
        if open.is_empty() {
            return Ok(());
        }
        // Creation-time preservation changes the identity record: journal the plan first (#506).
        self.journal.log(Record::Planned {
            items: open
                .iter()
                .map(|(i, _)| (*i as u32, self.item(*i).creation_ft))
                .collect(),
            staging: None,
        })?;
        for (i, file) in &open {
            let meta = preserved(self.item(*i));
            let key = self.control.admit().and_then(|_step| {
                self.journal.while_copying(|| {
                    self.control.at(Point::CheckedMetadata)?;
                    file.set_metadata(meta)
                        .and_then(|_| file.identity())
                        .map_err(|e| with_item(io_refusal(&e), *i as u32))
                })
            });
            match key {
                Ok(key) => {
                    hit("metadata set");
                    self.files.insert(*i, key);
                }
                Err(stop) => {
                    self.refuse(&stop)?;
                    return Err(stop);
                }
            }
        }
        // Close every write handle: the deny-write verification holds cannot coexist with them.
        drop(open);
        let mut verified = Vec::new();
        let mut redo = Vec::new();
        for w in written {
            match self.verify(&w) {
                Ok((held, mut streams)) => {
                    self.holds.push((held, w.streams.clone()));
                    self.stream_holds.append(&mut streams);
                    verified.push(w.item as u32);
                }
                Err(Stop::Refused(Refusal::VerificationMismatch, _)) => redo.push(w.item),
                Err(stop) => {
                    let stop = with_item(stop, w.item as u32);
                    self.refuse(&stop)?;
                    return Err(stop);
                }
            }
        }
        if !verified.is_empty() {
            self.journal.log(Record::Verified {
                items: verified.clone(),
            })?;
            self.control.at(Point::Verified(verified[0]))?;
        }
        for i in redo {
            self.redo(i)?;
        }
        Ok(())
    }

    /// One mismatch: redo from zero after removing the staged object by identity (removal policy on).
    /// A second mismatch, or the removal policy off, is `needs_attention`.
    fn redo(&mut self, i: usize) -> Result<(), Stop> {
        let stop = Stop::Refused(Refusal::VerificationMismatch, Some(i as u32));
        self.refuse(&stop)?;
        let replay = self.journal.replay()?;
        if replay.state != OpState::Copying {
            return Err(stop);
        }
        if !self.control.removal {
            self.journal.log(Record::Transition {
                to: OpState::NeedsAttention,
                code: Some(Refusal::VerificationMismatch),
            })?;
            return Err(stop);
        }
        let journaled = replay.items[i].id.ok_or(Stop::Attention(None))?;
        let name = self.item(i).name.clone();
        let key = self.files.remove(&i).ok_or(Stop::Attention(None))?;
        let (path, last) = path_of(&name, &self.keys)?;
        {
            // A cleanup step takes the admission lock but is not fenced (section 5, rule 4).
            let _step = self
                .control
                .admission
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            self.staging
                .cleanup_file(&path, last, &key)
                .map_err(|_| Stop::Attention(None))?;
        }
        self.journal.log(Record::ItemRemoved {
            item: i as u32,
            id: journaled,
        })?;
        self.batch(&[i])
    }

    /// Journal a runtime refusal (section 7: no further effects for that item, or the run).
    fn refuse(&self, stop: &Stop) -> Result<(), Stop> {
        match *stop {
            Stop::Refused(code, Some(item)) if code != Refusal::GrantRevoked => {
                self.journal.log(Record::ItemFailed { item, code })
            }
            Stop::Refused(code, None) if code.resumable() => self.journal.log(Record::Transition {
                to: OpState::Blocked,
                code: Some(code),
            }),
            Stop::Refused(code, _) => self.journal.log(Record::Transition {
                to: OpState::NeedsAttention,
                code: Some(code),
            }),
            Stop::Attention(code) => self.journal.log(Record::Transition {
                to: OpState::NeedsAttention,
                code,
            }),
            _ => Ok(()),
        }
    }

    fn write(&self, i: usize, file: &StagedFile) -> Result<Written, Stop> {
        self.control.check()?;
        let item = self.item(i);
        let (relative, expected) = &self.plan.sources[i];
        self.control.at(Point::OpeningSource(i as u32))?;
        let mut source = Source::open(
            &self.plan.source_root,
            &self.plan.root_identity(),
            duplicate_reader::Entry {
                relative: relative.clone(),
                expected: expected.clone(),
            },
            SOURCE_BUDGET,
        )
        .map_err(source_refusal)?;
        // The streams read now must be the ones the owner approved.
        let mut found: Vec<(String, u64)> = source
            .streams()
            .into_iter()
            .map(|(n, s)| (bare(&n).to_string(), s))
            .collect();
        found.sort();
        if found != item.streams {
            return Err(Stop::Refused(Refusal::SourceChanged, None));
        }
        self.control.at(Point::Writing(i as u32))?;
        let digest = {
            let mut reader = Cancellable::new(&mut source, self.control, &self.journal)?;
            file.write_and_flush(&mut reader, item.size)
                .map_err(|e| reader.stop(&e, |e| self.write_error(e)))?
        };
        let mut streams = Vec::new();
        for (name, size) in &item.streams {
            self.control.at(Point::CreatingStream(i as u32))?;
            let stream = {
                let _step = self.control.admit()?;
                self.journal.while_copying(|| {
                    self.control.at(Point::CheckedStream(i as u32))?;
                    file.create_stream(name).map_err(|e| io_refusal(&e))
                })?
            };
            let mut reader = source
                .stream_reader(&format!(":{name}:$DATA"))
                .map_err(|_| Stop::Refused(Refusal::SourceChanged, None))?;
            let mut reader = Cancellable::new(&mut reader, self.control, &self.journal)?;
            let d = stream
                .write_and_flush(&mut reader, *size)
                .map_err(|e| reader.stop(&e, |e| self.write_error(e)))?;
            streams.push((name.clone(), d, *size));
        }
        source.finish().map_err(source_refusal)?;
        Ok(Written {
            item: i,
            digest,
            streams,
        })
    }

    fn write_error(&self, e: &io::Error) -> Stop {
        // A failed source read inside the writer surfaces as an `Other` error naming the source.
        if (e.kind() == io::ErrorKind::Other && e.to_string().starts_with("source"))
            || e.kind() == io::ErrorKind::UnexpectedEof
        {
            Stop::Refused(Refusal::SourceChanged, None)
        } else {
            io_refusal(e)
        }
    }

    /// Reopen by the recorded ID under a deny-write hold, re-hash below the cache and compare content,
    /// size, preserved metadata and named streams with what was written.
    fn verify(&self, w: &Written) -> Result<(VerifiedFile, Vec<VerifiedFile>), Stop> {
        let item = self.item(w.item);
        let key = &self.files[&w.item];
        let (path, name) = path_of(&item.name, &self.keys)?;
        hit("verify item");
        let held = self
            .staging
            .verify(&path, name, key)
            .map_err(|e| io_refusal(&e))?;
        let mismatch = Stop::Refused(Refusal::VerificationMismatch, Some(w.item as u32));
        let streams: Vec<(String, u64)> = held
            .streams()
            .map_err(|e| io_refusal(&e))?
            .into_iter()
            .map(|(n, s)| (bare(&n).to_string(), s))
            .collect();
        let mut wanted: Vec<(String, u64)> =
            w.streams.iter().map(|(n, _, s)| (n.clone(), *s)).collect();
        wanted.sort();
        let mut streams = streams;
        streams.sort();
        if held.digest != w.digest
            || held.size != item.size
            || held.metadata().map_err(|e| io_refusal(&e))? != preserved(item)
            || streams != wanted
            || held.ea_size().map_err(|e| io_refusal(&e))? != 0
        {
            return Err(mismatch);
        }
        let mut holds = Vec::new();
        for (name, digest, size) in &w.streams {
            let s = held.verify_stream(name).map_err(|e| io_refusal(&e))?;
            if s.digest != *digest || s.size != *size {
                return Err(mismatch);
            }
            holds.push(s);
        }
        hit("verified item");
        Ok((held, holds))
    }

    /// Folder times and attributes bottom-up, after every child is verified; the staging folder last.
    fn directories(&mut self) -> Result<(), Stop> {
        self.control.check()?;
        let dirs: Vec<usize> = (0..self.plan.manifest.items.len())
            .rev()
            .filter(|&i| self.item(i).kind == Kind::Directory)
            .collect();
        let root = self.plan.manifest.root.as_ref().map(|r| r.creation_ft);
        if dirs.is_empty() && root.is_none() {
            return Ok(());
        }
        self.journal.log(Record::Planned {
            items: dirs
                .iter()
                .map(|&i| (i as u32, self.item(i).creation_ft))
                .collect(),
            staging: root,
        })?;
        for &i in &dirs {
            let item = self.item(i).clone();
            let (path, name) = path_of(&item.name, &self.keys)?;
            let key = &self.keys[&item.name];
            let step = self.control.admit()?;
            let changed = self.journal.while_copying(|| {
                self.control.at(Point::CheckedMetadata)?;
                Ok(self
                    .staging
                    .set_directory_metadata(&path, name, key, preserved(&item))
                    .and_then(|key| {
                        let now = self.staging.metadata(&path, name, &key)?;
                        Ok((key, now))
                    }))
            })?;
            drop(step);
            match changed {
                Ok((key, now)) if now == preserved(&item) => {
                    hit("directory metadata set");
                    self.keys.insert(item.name.clone(), key);
                }
                Ok(_) => {
                    let stop = Stop::Refused(Refusal::VerificationMismatch, Some(i as u32));
                    self.refuse(&Stop::Attention(Some(Refusal::VerificationMismatch)))?;
                    return Err(stop);
                }
                Err(e) => {
                    let stop = with_item(io_refusal(&e), i as u32);
                    self.refuse(&stop)?;
                    return Err(stop);
                }
            }
        }
        if let Some(root) = &self.plan.manifest.root {
            let meta = preserved(root);
            let step = self.control.admit()?;
            let ok = self.journal.while_copying(|| {
                self.control.at(Point::CheckedMetadata)?;
                Ok(self
                    .staging
                    .set_metadata(meta)
                    .and_then(|_| self.staging.own_metadata())
                    .map(|now| now == meta))
            })?;
            drop(step);
            hit("staging metadata set");
            if !matches!(ok, Ok(true)) {
                let stop = Stop::Attention(None);
                self.refuse(&stop)?;
                return Err(stop);
            }
        }
        if !dirs.is_empty() {
            self.journal.log(Record::Verified {
                items: dirs.iter().map(|&i| i as u32).collect(),
            })?;
        }
        Ok(())
    }

    /// Final tree check, publish without replace, post-publish confirmation, `committed`.
    fn publish(&mut self) -> Result<(), Stop> {
        self.control.check()?;
        self.recheck(false)?;
        self.journal.log(Record::Transition {
            to: OpState::Staged,
            code: None,
        })?;
        let final_name = self.plan.manifest.destination.folder_name.clone();
        self.journal.log(Record::PublishIntent {
            final_name: final_name.clone(),
        })?;
        self.control.at(Point::BeforePublish)?;
        self.stream_holds.clear();
        hit("rename");
        let mut tries = 0;
        loop {
            // The rename step: its fence read under the admission lock is the pre-rename check.
            let renamed = {
                let _step = self.control.admit()?;
                let (journal, control, staging) = (&self.journal, self.control, &mut self.staging);
                journal.while_in(OpState::Publishing, || {
                    control.at(Point::CheckedPublish)?;
                    let renamed = staging.publish(&final_name);
                    if renamed.is_ok() {
                        control.renamed.store(true, Ordering::SeqCst);
                    }
                    Ok(renamed)
                })?
            };
            match renamed {
                Ok(()) => break,
                // Bounded retries (section 7). The owner's cancel or a shutdown ends the wait at once
                // and leaves the run blocked with its intent kept; a fence is read by the next step.
                Err(e)
                    if transient(&e)
                        && tries + 1 < PUBLISH_TRIES
                        && self.control.pause(PUBLISH_WAIT) =>
                {
                    tries += 1;
                }
                Err(e) if transient(&e) => {
                    self.journal.log(Record::Transition {
                        to: OpState::Blocked,
                        code: Some(Refusal::PublishInUse),
                    })?;
                    return Err(Stop::Refused(Refusal::PublishInUse, None));
                }
                // The final name was taken (or anything else): staging kept, nothing replaced.
                Err(_) => {
                    let stop = Stop::Attention(None);
                    self.refuse(&stop)?;
                    return Err(stop);
                }
            }
        }
        hit("renamed");
        self.control.at(Point::AfterPublish)?;
        // Default-stream holds stayed in place through the rename (H2 positive), so that content is
        // unchanged; named streams were released for the rename and are re-hashed now; tree shape and
        // preserved metadata are rechecked (write-attributes access is not denied by a hold).
        self.recheck(true)?;
        self.journal.log(Record::Committed)?;
        self.holds.clear();
        self.stream_holds.clear();
        Ok(())
    }

    /// After the rename: every held file still carries exactly its verified named streams (the full
    /// list, so an added stream is caught) with the verified digests, and no extended attributes (a
    /// hold does not deny FILE_WRITE_EA). The re-hash handles are kept until `committed`.
    fn streams_match(&mut self) -> io::Result<bool> {
        let mut rehashed = Vec::new();
        let mut ok = true;
        for (held, streams) in &self.holds {
            let mut want: Vec<(String, u64)> =
                streams.iter().map(|(n, _, s)| (n.clone(), *s)).collect();
            want.sort();
            if !exact_streams(held, &want)? {
                ok = false;
                break;
            }
            for (name, digest, size) in streams {
                let s = held.verify_stream(name)?;
                ok &= s.digest == *digest && s.size == *size;
                rehashed.push(s);
            }
            if !ok {
                break;
            }
        }
        self.stream_holds = rehashed;
        Ok(ok)
    }

    /// Exactly the manifest's names under identity-pinned folders, nothing more.
    fn shape_matches(&self) -> io::Result<bool> {
        let want: BTreeSet<String> = self
            .plan
            .manifest
            .items
            .iter()
            .map(|i| i.name.clone())
            .collect();
        Ok(listing(&self.staging, &self.keys)? == want)
    }

    /// Preserved metadata of every entry, and no named stream or EA on any folder (#526).
    fn metadata_matches(&self) -> io::Result<bool> {
        for (i, item) in self.plan.manifest.items.iter().enumerate() {
            let key = match item.kind {
                Kind::Directory => self.keys.get(&item.name),
                Kind::File => self.files.get(&i),
            };
            let Some(key) = key else { return Ok(false) };
            let Ok((path, name)) = path_of(&item.name, &self.keys) else {
                return Ok(false);
            };
            if self.staging.metadata(&path, name, key)? != preserved(item)
                || (item.kind == Kind::Directory && !self.staging.folder_plain(&path, name, key)?)
            {
                return Ok(false);
            }
        }
        let root = self.staging.own_metadata()?;
        Ok(self
            .plan
            .manifest
            .root
            .as_ref()
            .is_none_or(|r| root == preserved(r))
            && self.staging.own_plain()?)
    }

    fn tree_matches(&mut self, renamed: bool) -> io::Result<bool> {
        Ok(self.shape_matches()?
            && self.metadata_matches()?
            && (!renamed || self.streams_match()?))
    }

    /// The final tree check before (`renamed` false) or after the rename. `changed_after_verification`
    /// only for a read that succeeded and differs, or an object gone or replaced. A sharing, lock or
    /// access failure is retried on the publish policy (section 7); still failing, the run waits
    /// `blocked` with its cause before the rename, or stays `publishing` after it. Any other read
    /// failure is an honest unknown: `needs_attention` with no code, or `publishing` after the rename.
    fn recheck(&mut self, renamed: bool) -> Result<(), Stop> {
        let mut tries = 0;
        loop {
            let read = self.tree_matches(renamed);
            let cause = match &read {
                Ok(true) => return Ok(()),
                Ok(false) => Unconfirmed::Changed,
                Err(e) => unverifiable(e),
            };
            let stop = match cause {
                Unconfirmed::Changed => Stop::Attention(Some(Refusal::ChangedAfterVerification)),
                _ if read.as_ref().is_err_and(transient)
                    && tries + 1 < PUBLISH_TRIES
                    && self.control.pause(PUBLISH_WAIT) =>
                {
                    tries += 1;
                    continue;
                }
                _ if renamed => return Err(Stop::Unconfirmed),
                Unconfirmed::Unverifiable(Some(code)) => Stop::Refused(code, None),
                Unconfirmed::Unverifiable(None) => Stop::Attention(None),
            };
            self.refuse(&stop)?;
            return Err(stop);
        }
    }
}

/// List only the identity-pinned manifest folders, including extras but never descending into them.
fn listing(
    staging: &Staging<'_>,
    keys: &HashMap<String, ObjectKey>,
) -> io::Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    for prefix in std::iter::once("").chain(keys.keys().map(String::as_str)) {
        let path = if prefix.is_empty() {
            Vec::new()
        } else {
            let (mut path, name) = path_of(prefix, keys).map_err(|_| refused("unbound folder"))?;
            path.push((name, keys[prefix].clone()));
            path
        };
        for name in staging.entries(&path)? {
            let name = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            out.insert(name);
            if out.len() > loomward_operations::MAX_ITEMS as usize {
                return Err(refused("too many staging entries"));
            }
        }
    }
    Ok(out)
}

fn bare(stream: &str) -> &str {
    stream.trim_start_matches(':').trim_end_matches(":$DATA")
}

/// Revocation or emergency stop of one operation (docs/47 section 5, steps 1 to 4): commit the fence
/// (a new epoch) unless the operation is already settled, then wait for at most the run's in-flight
/// effect step and say which side of publish it is on. Every later step reads the fence under the
/// admission lock and stops; the journal refuses any advancing record still carrying the old epoch.
pub fn fence(
    store: &Mutex<Store>,
    op: u64,
    control: &Control,
    cause: loomward_operations::FenceCause,
) -> Result<Ack, Fault> {
    let r = {
        // One store lock across the read and the append: no record slips between them.
        let mut s = store.lock().unwrap_or_else(|e| e.into_inner());
        control.fenced.store(true, Ordering::SeqCst);
        let r = s
            .replay(op)
            .map_err(|e| Fault::Journal(e.to_string()))?
            .map_err(|e| Fault::Journal(format!("{e:?}")))?;
        if r.state.is_terminal() {
            return Ok(Ack::Settled);
        }
        s.append(op, r.epoch + 1, Record::Fenced { cause })
            .map_err(|e| Fault::Journal(format!("{e:?}")))?;
        r
    };
    Ok(match control.fence(FENCE_WAIT) {
        None => Ack::PendingInFlightStep,
        Some(true) => Ack::Published,
        Some(false) if r.publish.is_none() => Ack::StoppedBeforePublish,
        // A fresh Control (a reopened service) never saw a detached worker's rename. The rename step
        // holds the journal's write lock across its check and the rename, so with the fence appended
        // it has either finished or never will: the destination says which (#672).
        Some(false) => published_on_disk(store, op, &r),
    })
}

fn published_on_disk(store: &Mutex<Store>, op: u64, r: &Replay) -> Ack {
    let plan: Option<Plan> = store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .plan(op)
        .ok()
        .flatten()
        .and_then(|t| serde_json::from_str(&t).ok());
    let (Some(plan), Some(final_name), Some(staging)) = (plan, &r.publish, r.staging) else {
        return Ack::PublishUnobserved;
    };
    let disk = PathDisk::new(&plan, r);
    let at = |name: &str| {
        disk.in_parent(name)
            .is_some_and(|o| staging.matches(&o, r.staging_planned))
    };
    if at(final_name) {
        Ack::Published
    } else if r.staging_tokens.last().is_some_and(|t| at(t)) && !disk.observation_failed.get() {
        // Only the staging folder still under its own name proves the rename never ran: a missing
        // final name alone does not (the owner may have renamed a published copy).
        Ack::StoppedBeforePublish
    } else {
        Ack::PublishUnobserved
    }
}

/// Turn where `copy` stopped into the journal's terminal or waiting state.
fn finish(
    store: &Mutex<Store>,
    op: u64,
    plan: &Plan,
    control: &Control,
    stopped: Result<(), Stop>,
) -> Result<Outcome, Fault> {
    match stopped {
        Ok(()) => {}
        Err(Stop::Crashed) => return Err(Fault::Crashed),
        // A fenced run's advancing records carry the old epoch and are refused (section 5, rule 6):
        // the fence, not the journal, stopped it. Cancel, withdrawing any publish intent.
        Err(Stop::Journal(_) | Stop::Superseded) if control.fenced() => {
            return cancel(store, op, plan, control)
        }
        Err(Stop::Journal(e)) => return Err(Fault::Journal(e)),
        Err(Stop::Cancel) => return cancel(store, op, plan, control),
        // Keep the journaled refusal or the newer control's decision; staging stays untouched.
        Err(Stop::Refused(..) | Stop::Attention(_) | Stop::Superseded | Stop::Unconfirmed) => {}
    }
    status(store, op)
}

/// Cancel (before publish) or abandon (from blocked, needs_attention or an interrupted run): journal
/// `cancelling`, then apply the Q1 policy. Default: leave the staging folder and report its name.
pub fn cancel(
    store: &Mutex<Store>,
    op: u64,
    plan: &Plan,
    control: &Control,
) -> Result<Outcome, Fault> {
    let r = replay(store, op)?;
    let journal = Journal {
        store,
        op,
        epoch: r.epoch,
    };
    let log = |record| journal.log(record).map_err(fault);
    if r.state.is_terminal() || r.state == OpState::Cancelled {
        return status(store, op);
    }
    if let Some(final_name) = &r.publish {
        // A rename may already have happened: never withdraw, never clean a published copy.
        let disk = PathDisk::new(plan, &r);
        let published = r.staging.filter(|s| {
            disk.in_parent(final_name)
                .is_some_and(|o| s.matches(&o, r.staging_planned))
        });
        if disk.observation_failed.get() {
            return status_idle(store, op);
        }
        if let Some(s) = published {
            // Published but not yet committed: abandon settles it by confirming now (a read). It is
            // committed, or recorded needs_attention with its cause; the copy is never touched.
            return if r.state == OpState::Publishing {
                confirm(store, op, plan, &r, s, final_name, true)
            } else {
                status(store, op)
            };
        }
        log(Record::PublishWithdrawn)?;
    } else if r.state != OpState::Cancelling {
        log(Record::Transition {
            to: OpState::Cancelling,
            code: None,
        })?;
    }
    clean(store, op, plan, control)
}

fn fault(stop: Stop) -> Fault {
    match stop {
        Stop::Journal(e) => Fault::Journal(e),
        Stop::Crashed => Fault::Crashed,
        _ => Fault::Journal("unexpected stop".into()),
    }
}

/// From `cancelling`: remove nothing and leave the staging (policy off), or remove Loomward's own
/// journaled staging by identity, children before parents (policy on). Any check failing:
/// `needs_attention`, the entry kept.
fn clean(store: &Mutex<Store>, op: u64, plan: &Plan, control: &Control) -> Result<Outcome, Fault> {
    let r = replay(store, op)?;
    let journal = Journal {
        store,
        op,
        epoch: r.epoch,
    };
    let log = |record| journal.log(record).map_err(fault);
    let cancelled = Record::Transition {
        to: OpState::Cancelled,
        code: None,
    };
    let Some(staging_id) = r.staging.filter(|_| !r.staging_removed && !r.staging_left) else {
        log(cancelled)?;
        return status(store, op);
    };
    if !control.removal {
        log(Record::StagingLeft { id: staging_id })?;
        log(cancelled)?;
        return status(store, op);
    }
    let attention = |log: &dyn Fn(Record) -> Result<(), Fault>| {
        log(Record::Transition {
            to: OpState::NeedsAttention,
            code: None,
        })
    };
    let token = r.staging_tokens.last().cloned().unwrap_or_default();
    let creations = |id: ObjectId, planned: Option<i64>| -> Vec<i64> {
        std::iter::once(id.creation_ft).chain(planned).collect()
    };
    let Ok(dest) = Destination::open(&plan.destination, &plan.destination_key) else {
        attention(&log)?;
        return status(store, op);
    };
    let Ok(staging) = Staging::attach(
        &dest,
        &token,
        staging_id.file_id,
        &creations(staging_id, r.staging_planned),
    ) else {
        attention(&log)?;
        return status(store, op);
    };
    // Resolve every held entry top-down (names sort parents first), then remove bottom-up.
    let mut held: Vec<_> = r.held().collect();
    held.sort_by(|a, b| a.name.cmp(&b.name));
    let mut keys: HashMap<String, ObjectKey> = HashMap::new();
    let mut resolved = Vec::new();
    let mut gone = Vec::new();
    for h in &held {
        let is_dir = plan
            .manifest
            .items
            .get(h.item as usize)
            .is_some_and(|i| i.kind == Kind::Directory);
        let key = path_of(&h.name, &keys).ok().and_then(|(path, name)| {
            staging
                .resolve(
                    &path,
                    name,
                    h.id.file_id,
                    &creations(h.id, h.planned_ft),
                    is_dir,
                )
                .ok()
        });
        // Nothing at all under the journaled name: removed before its record (a crash mid-cleanup).
        // Journal the removal; nothing is touched (C3 recovery's `gone`).
        let absent = || PathDisk::new(plan, &r).gone(staging_id, &h.name);
        match key {
            Some(key) => {
                keys.insert(h.name.clone(), key.clone());
                resolved.push((h, key, is_dir));
            }
            None if absent() => gone.push(h),
            None => {
                attention(&log)?;
                return status(store, op);
            }
        }
    }
    let remove = |effect: &mut dyn FnMut() -> bool| removal(&journal, r.state, control, effect);
    for (h, key, is_dir) in resolved.into_iter().rev() {
        let Ok((path, name)) = path_of(&h.name, &keys) else {
            attention(&log)?;
            return status(store, op);
        };
        let removed = remove(&mut || {
            if is_dir {
                staging.cleanup_directory(&path, name, &key).is_ok()
            } else {
                staging.cleanup_file(&path, name, &key).is_ok()
            }
        })?;
        match removed {
            None => return status(store, op),
            Some(false) => {
                attention(&log)?;
                return status(store, op);
            }
            Some(true) => {}
        }
        log(Record::ItemRemoved {
            item: h.item,
            id: h.id,
        })?;
    }
    for h in gone {
        log(Record::ItemRemoved {
            item: h.item,
            id: h.id,
        })?;
    }
    let mut staging = Some(staging);
    match remove(&mut || staging.take().is_some_and(|s| s.cleanup().is_ok()))? {
        None => return status(store, op),
        Some(false) => {
            attention(&log)?;
            return status(store, op);
        }
        Some(true) => {}
    }
    log(Record::StagingRemoved { id: staging_id })?;
    log(cancelled)?;
    status(store, op)
}

/// One cleanup removal under the journal's write lock, checked against the state and epoch this cleanup
/// read: a fence or decision another control journals since stops it before the removal (#672).
/// `None` when superseded, else whether the removal succeeded.
fn removal(
    journal: &Journal,
    state: OpState,
    control: &Control,
    effect: &mut dyn FnMut() -> bool,
) -> Result<Option<bool>, Fault> {
    match journal.while_in(state, || {
        control.at(Point::CheckedRemoval)?;
        Ok(effect())
    }) {
        Ok(removed) => Ok(Some(removed)),
        Err(Stop::Superseded) => Ok(None),
        Err(stop) => Err(fault(stop)),
    }
}

/// The destination as recovery's `Disk` sees it: point-in-time observations by name, which only ever
/// propose. Every removal re-checks by journaled ID before acting.
struct PathDisk {
    parent: PathBuf,
    parent_key: ObjectKey,
    staging: Option<(String, ObjectId, Option<i64>)>,
    observation_failed: std::cell::Cell<bool>,
}
impl PathDisk {
    fn new(plan: &Plan, r: &Replay) -> Self {
        let disk = PathDisk {
            parent: plan.destination.clone(),
            parent_key: plan.destination_key.clone(),
            observation_failed: std::cell::Cell::new(false),
            staging: r
                .staging
                .zip(r.staging_tokens.last().cloned())
                .map(|(id, t)| (t, id, r.staging_planned)),
        };
        // A missing/replaced parent (e.g. a detached volume) says nothing about its children.
        if !disk.parent_available() {
            disk.observation_failed.set(true);
        }
        disk
    }

    /// A confirmed absence of a staging entry: its name is not found while the staging name and the
    /// destination parent still resolve to the journaled objects. A path that now resolves elsewhere (a
    /// mounted-folder destination temporarily unmounted, a swapped folder) is unknown, not gone (#670).
    fn gone(&self, staging: ObjectId, name: &str) -> bool {
        let staged = self.staging.as_ref().is_some_and(|(token, id, planned)| {
            *id == staging
                && self
                    .in_parent(token)
                    .is_some_and(|o| id.matches(&o, *planned))
        });
        staged && self.in_staging(staging, name).is_none() && !self.observation_failed.get()
    }

    fn parent_available(&self) -> bool {
        identity::observe(&self.parent).is_ok_and(|o| o.object == self.parent_key)
    }

    fn observe(&self, path: &Path) -> Option<identity::ObservedIdentity> {
        match identity::observe(path) {
            Ok(observed) => Some(observed),
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound || !self.parent_available() {
                    self.observation_failed.set(true);
                }
                None
            }
        }
    }
}
impl Disk for PathDisk {
    fn in_parent(&self, name: &str) -> Option<ObjectId> {
        self.observe(&self.parent.join(name))
            .map(|o| oid(&o.object))
    }
    fn in_staging(&self, staging: ObjectId, name: &str) -> Option<Observed> {
        let (token, id, planned) = self.staging.as_ref()?;
        if *id != staging
            || !self
                .in_parent(token)
                .is_some_and(|o| id.matches(&o, *planned))
        {
            return None;
        }
        let path = name
            .split('/')
            .fold(self.parent.join(token), |p, c| p.join(c));
        self.observe(&path).map(|o| Observed {
            id: oid(&o.object),
            links: o.link_count,
        })
    }
}

/// Recovery at service open (docs/47 section 5): run C3's recovery reader and execute only its
/// decisions. A run never resumes by itself: `Resume` and `NeedsAttention` wait for the owner (in C5,
/// Abandon); `Reconfirm` cancels (nothing was created); `Cleanup` finishes a cancel the owner or a
/// fence already journaled; `Confirm` re-hashes the published copy by ID against the source before
/// `committed`.
pub fn recover(store: &Mutex<Store>, op: u64, control: &Control) -> Result<Outcome, Fault> {
    let plan: Option<Plan> = store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .plan(op)
        .map_err(|e| Fault::Journal(e.to_string()))?
        .and_then(|t| serde_json::from_str(&t).ok());
    let Ok(r) = replay(store, op) else {
        // An invalid journal is needs_attention by definition; nothing to execute.
        return Err(Fault::Journal("journal does not replay".into()));
    };
    if r.state.is_terminal() {
        return status(store, op);
    }
    let journal = Journal {
        store,
        op,
        epoch: r.epoch,
    };
    let log = |record| journal.log(record).map_err(fault);
    let Some(plan) = plan else {
        // Started but crashed before the plan: nothing can have been created.
        log(Record::Transition {
            to: OpState::Cancelling,
            code: None,
        })?;
        log(Record::Transition {
            to: OpState::Cancelled,
            code: None,
        })?;
        return status(store, op);
    };
    let disk = PathDisk::new(&plan, &r);
    let recovery = store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .recover(op, &disk)
        .map_err(|e| Fault::Journal(e.to_string()))?;
    if disk.observation_failed.get() {
        let mut outcome = status_idle(store, op)?;
        outcome.state = OpState::NeedsAttention;
        return Ok(outcome);
    }
    match recovery.decision {
        Decision::Reconfirm => {
            log(Record::Transition {
                to: OpState::Cancelling,
                code: None,
            })?;
            clean(store, op, &plan, control)
        }
        Decision::Cleanup(_) => {
            if r.state != OpState::Cancelling {
                return cancel(store, op, &plan, control);
            }
            clean(store, op, &plan, control)
        }
        Decision::Confirm {
            staging,
            final_name,
        } => confirm(store, op, &plan, &r, staging, &final_name, false),
        Decision::NeedsAttention(attention) => {
            // Disk's Option cannot distinguish absence from a failed observation. Persist only
            // observed damage; an unavailable destination or entry must be retried on a later open.
            if matches!(
                attention,
                loomward_operations::Attention::StagingMismatch
                    | loomward_operations::Attention::Unjournaled(_)
                    | loomward_operations::Attention::EntryMismatch(_)
            ) && !disk.observation_failed.get()
                && r.state.can_transition(OpState::NeedsAttention)
            {
                log(Record::Transition {
                    to: OpState::NeedsAttention,
                    code: None,
                })?;
            }
            let mut outcome = status(store, op)?;
            // Report the failed check now even when it must not become a durable damage finding.
            outcome.state = OpState::NeedsAttention;
            Ok(outcome)
        }
        Decision::Settled | Decision::Resume(_) => status(store, op),
    }
}

/// Why the post-publish confirmation did not pass.
enum Unconfirmed {
    /// The published copy differs from what was verified (shape, identity, metadata, streams, bytes).
    Changed,
    /// It could not be read now (sharing, access, a source changed or unreadable): not a finding about
    /// the copy. Recovery retries at the next open; abandon settles it with this cause.
    Unverifiable(Option<Refusal>),
}

fn unverifiable(e: &io::Error) -> Unconfirmed {
    match e.raw_os_error() {
        Some(32 | 33) => Unconfirmed::Unverifiable(Some(Refusal::StagingInUse)),
        Some(5) => Unconfirmed::Unverifiable(Some(Refusal::AccessDenied)),
        // Gone, or bound to another object: the published tree changed.
        Some(2 | 3) => Unconfirmed::Changed,
        _ if e.kind() == io::ErrorKind::InvalidInput => Unconfirmed::Changed,
        _ => Unconfirmed::Unverifiable(None),
    }
}

fn source_unverifiable(reason: duplicate_reader::Reason) -> Unconfirmed {
    use duplicate_reader::Reason as R;
    Unconfirmed::Unverifiable(Some(match reason {
        R::SharingViolation => Refusal::SourceInUse,
        R::AccessDenied => Refusal::AccessDenied,
        R::ChangedDuringRead | R::InvalidPath => Refusal::SourceChanged,
        _ => return Unconfirmed::Unverifiable(None),
    }))
}

/// Crash between the rename and `committed`: attach to the published folder by journaled ID, re-hash
/// every file by ID against a fresh read of the unchanged source, check shape, metadata, the full
/// stream lists and EAs, then `committed`. A real difference is `needs_attention`
/// (`changed_after_verification`, outcome published_unverified). An unverifiable read journals nothing
/// unless `settle` (owner abandon): then `needs_attention` with that cause. Never cleanup.
fn confirm(
    store: &Mutex<Store>,
    op: u64,
    plan: &Plan,
    r: &Replay,
    staging_id: ObjectId,
    final_name: &str,
    settle: bool,
) -> Result<Outcome, Fault> {
    let journal = Journal {
        store,
        op,
        epoch: r.epoch,
    };
    let creations = |id: ObjectId, planned: Option<i64>| -> Vec<i64> {
        std::iter::once(id.creation_ft).chain(planned).collect()
    };
    use Unconfirmed::Changed;
    let check = |ok: bool| if ok { Ok(()) } else { Err(Changed) };
    let confirmed = (|| -> Result<Vec<VerifiedFile>, Unconfirmed> {
        let dest = Destination::open(&plan.destination, &plan.destination_key)
            .map_err(|e| unverifiable(&e))?;
        let published = Staging::attach_published(
            &dest,
            final_name,
            staging_id.file_id,
            &creations(staging_id, r.staging_planned),
        )
        .map_err(|e| unverifiable(&e))?;
        let want: BTreeSet<String> = plan.manifest.items.iter().map(|i| i.name.clone()).collect();
        let mut keys = HashMap::new();
        let mut holds = Vec::new();
        for (i, item) in plan.manifest.items.iter().enumerate() {
            hit("confirm item");
            let journaled = &r.items[i];
            let id = journaled.id.ok_or(Changed)?;
            let (path, name) = path_of(&item.name, &keys).map_err(|_| Changed)?;
            let key = published
                .resolve(
                    &path,
                    name,
                    id.file_id,
                    &creations(id, journaled.planned_ft),
                    item.kind == Kind::Directory,
                )
                .map_err(|e| unverifiable(&e))?;
            check(
                published
                    .metadata(&path, name, &key)
                    .map_err(|e| unverifiable(&e))?
                    == preserved(item),
            )?;
            if item.kind == Kind::File {
                let held = published
                    .verify(&path, name, &key)
                    .map_err(|e| unverifiable(&e))?;
                check(exact_streams(&held, &item.streams).map_err(|e| unverifiable(&e))?)?;
                let (relative, expected) = &plan.sources[i];
                let mut source = Source::open(
                    &plan.source_root,
                    &plan.root_identity(),
                    duplicate_reader::Entry {
                        relative: relative.clone(),
                        expected: expected.clone(),
                    },
                    SOURCE_BUDGET,
                )
                .map_err(source_unverifiable)?;
                let mut digest = Sha256::new();
                io::copy(&mut source, &mut DigestWriter(&mut digest))
                    .map_err(|_| Unconfirmed::Unverifiable(Some(Refusal::SourceChanged)))?;
                check(<[u8; 32]>::from(digest.finalize()) == held.digest)?;
                for (stream, size) in &item.streams {
                    let s = held.verify_stream(stream).map_err(|e| unverifiable(&e))?;
                    let mut d = Sha256::new();
                    let mut reader = source
                        .stream_reader(&format!(":{stream}:$DATA"))
                        .map_err(|_| Unconfirmed::Unverifiable(Some(Refusal::SourceChanged)))?;
                    io::copy(&mut reader, &mut DigestWriter(&mut d))
                        .map_err(|_| Unconfirmed::Unverifiable(Some(Refusal::SourceChanged)))?;
                    check(s.size == *size && <[u8; 32]>::from(d.finalize()) == s.digest)?;
                    holds.push(s);
                }
                source.finish().map_err(source_unverifiable)?;
                holds.push(held);
            } else {
                check(
                    published
                        .folder_plain(&path, name, &key)
                        .map_err(|e| unverifiable(&e))?,
                )?;
                keys.insert(item.name.clone(), key);
            }
        }
        check(listing(&published, &keys).map_err(|e| unverifiable(&e))? == want)?;
        let root_metadata = published.own_metadata().map_err(|e| unverifiable(&e))?;
        check(
            plan.manifest
                .root
                .as_ref()
                .is_none_or(|root| root_metadata == preserved(root)),
        )?;
        check(published.own_plain().map_err(|e| unverifiable(&e))?)?;
        Ok(holds)
    })();
    let attention = |code| {
        journal
            .log(Record::Transition {
                to: OpState::NeedsAttention,
                code,
            })
            .map_err(fault)
    };
    match confirmed {
        Ok(holds) => {
            journal.log(Record::Committed).map_err(fault)?;
            drop(holds);
        }
        Err(Changed) => attention(Some(Refusal::ChangedAfterVerification))?,
        Err(Unconfirmed::Unverifiable(code)) if settle => attention(code)?,
        Err(Unconfirmed::Unverifiable(_)) => {}
    }
    status(store, op)
}

struct DigestWriter<'a>(&'a mut Sha256);
impl Write for DigestWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.update(buffer);
        Ok(buffer.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "exec_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "exec_crash_tests.rs"]
mod crash_tests;

/// The held file's named streams are exactly `want` (bare names, sorted) and it has no EAs.
fn exact_streams(held: &VerifiedFile, want: &[(String, u64)]) -> io::Result<bool> {
    let mut found: Vec<(String, u64)> = held
        .streams()?
        .into_iter()
        .map(|(n, s)| (bare(&n).to_string(), s))
        .collect();
    found.sort();
    Ok(found == want && held.ea_size()? == 0)
}
