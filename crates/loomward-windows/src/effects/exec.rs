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
    pub source_root_attributes: u32,
    pub destination: PathBuf,
    pub destination_key: ObjectKey,
    /// The destination grant's verified root: (volume serial, file ID). The pinned destination's
    /// ancestry must contain it at preview and again at run (lab roots only, invariant 2).
    pub destination_root: (u64, [u8; 16]),
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
        if entries.len() >= loomward_operations::MAX_ITEMS as usize {
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
        source_root_attributes: root.attributes,
        destination: request.destination.to_path_buf(),
        destination_key: dest_observed.object,
        destination_root: request.destination_root,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Crash,
}

type Hook = Box<dyn Fn(Point, &Control) -> Flow + Send + Sync>;

/// One run's controls: the owner's cancel and the staging-removal policy.
pub struct Control {
    cancel: AtomicBool,
    /// docs/47 Q1 (`loomward-copy-staging-removal`, OPEN, agent-hq#461). Off by default: cancel and
    /// abandon stop and leave the staging folder for the owner (option b). On: Loomward removes its own
    /// journaled staging by identity, and a first verification mismatch is redone.
    pub removal: bool,
    hook: Option<Hook>,
}

impl Control {
    pub fn new(removal: bool) -> Self {
        Control {
            cancel: AtomicBool::new(false),
            removal,
            hook: None,
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
        }
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
    Crashed,
    Journal(String),
}

fn oid(key: &ObjectKey) -> ObjectId {
    ObjectId {
        file_id: key.file_id,
        creation_ft: key.creation_time.parse().unwrap_or(i64::MIN),
    }
}

fn io_refusal(e: &io::Error) -> Stop {
    match e.raw_os_error() {
        Some(80 | 183) => Stop::Refused(Refusal::StagingNameTaken, None),
        Some(5) => Stop::Refused(Refusal::AccessDenied, None),
        Some(32 | 33) => Stop::Refused(Refusal::StagingInUse, None),
        Some(39 | 112) => Stop::Refused(Refusal::InsufficientSpace, None),
        _ if e.kind() == io::ErrorKind::Interrupted => Stop::Cancel,
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

/// Reads the source in the writer's chunks, stopping at the next chunk once the owner cancels.
struct Cancellable<'a, R>(&'a mut R, &'a Control);
impl<R: Read> Read for Cancellable<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.1.cancel.load(Ordering::SeqCst) {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        self.0.read(buffer)
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
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .append(self.op, self.epoch, record)
            .map(drop)
            .map_err(|e| Stop::Journal(format!("{e:?}")))
    }
    fn replay(&self) -> Result<Replay, Stop> {
        replay(self.store, self.op).map_err(|e| Stop::Journal(format!("{e:?}")))
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
    let r = replay(store, op)?;
    let rows = store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .rows(op)
        .map_err(|e| Fault::Journal(e.to_string()))?;
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
        state: r.state,
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
    /// Named-stream holds: measured to block the ancestor rename (H2 negative for streams), so they
    /// are released just before publish and the streams re-hashed through the default holds after.
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
            // Revalidation failed before any effect: `preparing -> refused`.
            let code = match io_refusal(&e) {
                Stop::Refused(code, _) => Some(code),
                _ => None,
            };
            journal.log(Record::Transition {
                to: OpState::Refused,
                code,
            })
        }
        // The pinned parent must still sit under its grant's root (a swapped ancestor is refused).
        Ok(destination) if !within(&destination, plan.destination_root).unwrap_or(false) => journal
            .log(Record::Transition {
                to: OpState::Refused,
                code: None,
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
    let staging = match dest.stage(&token) {
        Ok(staging) => staging,
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
            let made = path_of(&item.name, &self.keys).and_then(|(path, name)| {
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
            });
            match made {
                Ok((key, file)) => {
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
            let key = file
                .set_metadata(meta)
                .and_then(|_| file.identity())
                .map_err(|e| with_item(io_refusal(&e), *i as u32));
            match key {
                Ok(key) => {
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
        self.staging
            .cleanup_file(&path, last, &key)
            .map_err(|_| Stop::Attention(None))?;
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
        let digest = file
            .write_and_flush(&mut Cancellable(&mut source, self.control), item.size)
            .map_err(|e| self.write_error(&e))?;
        let mut streams = Vec::new();
        for (name, size) in &item.streams {
            let stream = file.create_stream(name).map_err(|e| io_refusal(&e))?;
            let mut reader = source
                .stream_reader(&format!(":{name}:$DATA"))
                .map_err(|_| Stop::Refused(Refusal::SourceChanged, None))?;
            let d = stream
                .write_and_flush(&mut Cancellable(&mut reader, self.control), *size)
                .map_err(|e| self.write_error(&e))?;
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
            let changed = self
                .staging
                .set_directory_metadata(&path, name, key, preserved(&item))
                .and_then(|key| {
                    let now = self.staging.metadata(&path, name, &key)?;
                    Ok((key, now))
                });
            match changed {
                Ok((key, now)) if now == preserved(&item) => {
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
            let ok = self
                .staging
                .set_metadata(meta)
                .and_then(|_| self.staging.own_metadata())
                .map(|now| now == meta);
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
        let staging_name = self.journal.replay()?.staging_tokens.last().cloned();
        let staging_name = staging_name.ok_or(Stop::Attention(None))?;
        if !self.shape_matches(&staging_name) || !self.metadata_matches() {
            let stop = Stop::Attention(Some(Refusal::ChangedAfterVerification));
            self.refuse(&stop)?;
            return Err(stop);
        }
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
        let mut tries = 0;
        loop {
            match self.staging.publish(&final_name) {
                Ok(()) => break,
                Err(e) if matches!(e.raw_os_error(), Some(5 | 32)) && tries + 1 < PUBLISH_TRIES => {
                    tries += 1;
                    std::thread::sleep(PUBLISH_WAIT);
                }
                Err(e) if matches!(e.raw_os_error(), Some(5 | 32)) => {
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
        self.control.at(Point::AfterPublish)?;
        // Default-stream holds stayed in place through the rename (H2 positive), so that content is
        // unchanged; named streams were released for the rename and are re-hashed now; tree shape and
        // preserved metadata are rechecked (write-attributes access is not denied by a hold).
        if !self.shape_matches(&final_name) || !self.metadata_matches() || !self.streams_match() {
            let stop = Stop::Attention(Some(Refusal::ChangedAfterVerification));
            self.refuse(&stop)?;
            return Err(stop);
        }
        self.journal.log(Record::Committed)?;
        self.holds.clear();
        self.stream_holds.clear();
        Ok(())
    }

    /// After the rename: every held file still carries exactly its verified named streams (the full
    /// list, so an added stream is caught) with the verified digests, and no extended attributes (a
    /// hold does not deny FILE_WRITE_EA). The re-hash handles are kept until `committed`.
    fn streams_match(&mut self) -> bool {
        let mut rehashed = Vec::new();
        let ok = self.holds.iter().all(|(held, streams)| {
            let mut want: Vec<(String, u64)> =
                streams.iter().map(|(n, _, s)| (n.clone(), *s)).collect();
            want.sort();
            exact_streams(held, &want)
                && streams.iter().all(|(name, digest, size)| {
                    held.verify_stream(name).is_ok_and(|s| {
                        let same = s.digest == *digest && s.size == *size;
                        rehashed.push(s);
                        same
                    })
                })
        });
        self.stream_holds = rehashed;
        ok
    }

    /// Exactly the manifest's names under the folder, nothing more (a read; no handle kept).
    fn shape_matches(&self, folder: &str) -> bool {
        let want: BTreeSet<String> = self
            .plan
            .manifest
            .items
            .iter()
            .map(|i| i.name.clone())
            .collect();
        listing(&self.plan.destination.join(folder)).is_ok_and(|got| got == want)
    }

    fn metadata_matches(&self) -> bool {
        self.plan
            .manifest
            .items
            .iter()
            .enumerate()
            .all(|(i, item)| {
                let key = match item.kind {
                    Kind::Directory => self.keys.get(&item.name),
                    Kind::File => self.files.get(&i),
                };
                let Some(key) = key else { return false };
                path_of(&item.name, &self.keys).is_ok_and(|(path, name)| {
                    self.staging
                        .metadata(&path, name, key)
                        .is_ok_and(|m| m == preserved(item))
                })
            })
            && self
                .plan
                .manifest
                .root
                .as_ref()
                .is_none_or(|r| self.staging.own_metadata().is_ok_and(|m| m == preserved(r)))
    }
}

/// Every entry name under `folder`, staging-relative and `/`-joined. Reparse points are listed, never
/// followed.
fn listing(folder: &Path) -> io::Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    let mut pending = vec![(folder.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| refused("unrepresentable name"))?;
            let name = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let kind = entry.file_type()?;
            if kind.is_dir() && !kind.is_symlink() {
                pending.push((entry.path(), name.clone()));
            }
            out.insert(name);
        }
    }
    Ok(out)
}

fn bare(stream: &str) -> &str {
    stream.trim_start_matches(':').trim_end_matches(":$DATA")
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
        Err(Stop::Journal(e)) => return Err(Fault::Journal(e)),
        Err(Stop::Cancel) => return cancel(store, op, plan, control),
        // Already journaled by `refuse`: blocked or needs_attention, staging kept.
        Err(Stop::Refused(..) | Stop::Attention(_)) => {}
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
        if let Some(s) = r.staging.filter(|s| {
            disk.in_parent(final_name)
                .is_some_and(|o| s.matches(&o, r.staging_planned))
        }) {
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
        let absent = || {
            let path = h
                .name
                .split('/')
                .fold(plan.destination.join(&token), |p, c| p.join(c));
            matches!(std::fs::symlink_metadata(path), Err(e) if e.kind() == io::ErrorKind::NotFound)
        };
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
    for (h, key, is_dir) in resolved.into_iter().rev() {
        let removed = path_of(&h.name, &keys).ok().is_some_and(|(path, name)| {
            if is_dir {
                staging.cleanup_directory(&path, name, &key).is_ok()
            } else {
                staging.cleanup_file(&path, name, &key).is_ok()
            }
        });
        if !removed {
            attention(&log)?;
            return status(store, op);
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
    if staging.cleanup().is_err() {
        attention(&log)?;
        return status(store, op);
    }
    log(Record::StagingRemoved { id: staging_id })?;
    log(cancelled)?;
    status(store, op)
}

/// The destination as recovery's `Disk` sees it: point-in-time observations by name, which only ever
/// propose. Every removal re-checks by journaled ID before acting.
struct PathDisk {
    parent: PathBuf,
    staging: Option<(String, ObjectId, Option<i64>)>,
}
impl PathDisk {
    fn new(plan: &Plan, r: &Replay) -> Self {
        PathDisk {
            parent: plan.destination.clone(),
            staging: r
                .staging
                .zip(r.staging_tokens.last().cloned())
                .map(|(id, t)| (t, id, r.staging_planned)),
        }
    }
}
impl Disk for PathDisk {
    fn in_parent(&self, name: &str) -> Option<ObjectId> {
        identity::observe(&self.parent.join(name))
            .ok()
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
        identity::observe(&path).ok().map(|o| Observed {
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
        Decision::Settled | Decision::Resume(_) | Decision::NeedsAttention(_) => status(store, op),
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
        check(listing(&plan.destination.join(final_name)).map_err(|e| unverifiable(&e))? == want)?;
        let mut keys = HashMap::new();
        let mut holds = Vec::new();
        for (i, item) in plan.manifest.items.iter().enumerate() {
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
                check(exact_streams(&held, &item.streams))?;
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
                keys.insert(item.name.clone(), key);
            }
        }
        if let Some(root) = &plan.manifest.root {
            check(published.own_metadata().map_err(|e| unverifiable(&e))? == preserved(root))?;
        }
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

/// The held file's named streams are exactly `want` (bare names, sorted) and it has no EAs.
fn exact_streams(held: &VerifiedFile, want: &[(String, u64)]) -> bool {
    let Ok(found) = held.streams() else {
        return false;
    };
    let mut found: Vec<(String, u64)> = found
        .into_iter()
        .map(|(n, s)| (bare(&n).to_string(), s))
        .collect();
    found.sort();
    found == want && held.ea_size().is_ok_and(|n| n == 0)
}
