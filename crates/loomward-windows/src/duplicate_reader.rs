//! Default-stream equality evidence for admitted grants. Never effect authority or reclaimability.
use crate::{enumerate::OpenedIdentity, identity::ObservedIdentity};
#[cfg(any(windows, test))]
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::atomic::Ordering,
    time::Instant,
};
use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

#[derive(Debug, Clone)]
pub struct Entry {
    pub relative: PathBuf,
    pub expected: ObservedIdentity,
}
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// Default-stream bytes across all passes, counting both comparison inputs.
    pub bytes: u64,
    /// Shared wall-clock allowance, checked around every native call.
    /// Pending content I/O is cancelled at the deadline; metadata calls and cancellation
    /// draining still depend on the filesystem driver returning.
    pub time: Duration,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Elevated,
    Unsupported,
    InvalidPath,
    DuplicateName,
    Cancelled,
    ChangedDuringRead,
    ReparsePoint,
    Placeholder,
    Offline,
    SpecialFile,
    SharingViolation,
    AccessDenied,
    Io,
    ByteBudget,
    TimeBudget,
    EntryBudget,
    /// The root lists identity-unverified (#219): no content read, ever.
    IdentityUnverified,
}
impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Elevated => "elevated",
            Self::Unsupported => "unsupported",
            Self::InvalidPath => "invalid_path",
            Self::DuplicateName => "duplicate_name",
            Self::Cancelled => "cancelled",
            Self::ChangedDuringRead => "changed_during_read",
            Self::ReparsePoint => "reparse_point",
            Self::Placeholder => "placeholder",
            Self::Offline => "offline",
            Self::SpecialFile => "special_file",
            Self::SharingViolation => "sharing_violation",
            Self::AccessDenied => "access_denied",
            Self::Io => "io",
            Self::ByteBudget => "byte_budget",
            Self::TimeBudget => "time_budget",
            Self::EntryBudget => "entry_budget",
            Self::IdentityUnverified => "identity_unverified",
        }
    }
}
#[derive(Debug)]
pub struct Skip {
    pub entry: usize,
    pub reason: Reason,
}
#[derive(Debug, Default)]
pub struct Report {
    /// Input indices; equal default streams at observation, never permission to delete.
    pub groups: Vec<Vec<usize>>,
    /// Known native objects with multiple names (including observed single names with nlink > 1).
    pub links: Vec<Vec<usize>>,
    /// Listed names excluded from content comparison because their object is hard-linked.
    pub links_not_compared: usize,
    pub skipped: Vec<Skip>,
    pub bytes_read: u64,
    /// A budget stop leaves the remaining input unexamined, never absent.
    pub stop: Option<Reason>,
    /// Incomplete coverage, including budget hits and refused/changed entries.
    pub partial: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(windows)]
enum Event {
    Partial,
    Full,
    Read,
    Compare,
    PendingRead,
}

/// Internal reader for trusted catalogue listings; callers must use Engine::inspect_duplicates.
#[cfg(any(feature = "content-read", test))]
#[doc(hidden)]
pub fn inspect(
    root: &Path,
    expected_root: &OpenedIdentity,
    entries: &[Entry],
    budget: Budget,
    cancel: Arc<AtomicBool>,
) -> Result<Report, Reason> {
    #[cfg(windows)]
    {
        inspect_with_cancel_hook(root, expected_root, entries, budget, cancel, |_, _| {})
    }
    #[cfg(not(windows))]
    {
        let _ = (root, expected_root, entries, budget, cancel);
        Err(Reason::Unsupported)
    }
}
/// Metadata-only hydration through the same pinned, non-reparse root-relative handles.
/// Engine callers must first admit the durable grant.
#[cfg(feature = "content-read")]
#[doc(hidden)]
pub fn observe_candidate(
    root: &Path,
    expected_root: &OpenedIdentity,
    relative: &Path,
    budget: Budget,
    cancel: Arc<AtomicBool>,
) -> Result<ObservedIdentity, Reason> {
    #[cfg(windows)]
    {
        native::observe_candidate(root, expected_root, relative, budget, cancel)
    }
    #[cfg(not(windows))]
    {
        let _ = (root, expected_root, relative, budget, cancel);
        Err(Reason::Unsupported)
    }
}
#[cfg(all(windows, test))]
fn inspect_with_hook(
    root: &Path,
    expected: &OpenedIdentity,
    entries: &[Entry],
    budget: Budget,
    hook: impl FnMut(Event, usize),
) -> Result<Report, Reason> {
    inspect_with_cancel_hook(root, expected, entries, budget, Default::default(), hook)
}
#[cfg(windows)]
fn inspect_with_cancel_hook(
    root: &Path,
    expected: &OpenedIdentity,
    entries: &[Entry],
    budget: Budget,
    cancel: Arc<AtomicBool>,
    hook: impl FnMut(Event, usize),
) -> Result<Report, Reason> {
    native::non_elevated()?;
    let mut meter = Meter::new(budget);
    meter.cancel = Some(cancel);
    if let Err(reason) = meter.check() {
        return Ok(Report {
            partial: true,
            stop: Some(reason),
            ..Report::default()
        });
    }
    let mut reader = native::Reader::new(root, expected, hook)?;
    Ok(group(entries, &mut reader, &mut meter))
}

// Shared global budgets apply to metadata, both hash passes and both sides of exact comparison.
#[cfg(any(windows, test))]
struct Meter {
    remaining: u64,
    read: u64,
    deadline: Instant,
    cancel: Option<Arc<AtomicBool>>,
    #[cfg(test)]
    clock: Option<std::rc::Rc<std::cell::Cell<Instant>>>,
}
#[cfg(any(windows, test))]
impl Meter {
    fn new(budget: Budget) -> Self {
        Self {
            remaining: budget.bytes,
            read: 0,
            cancel: None,
            #[cfg(test)]
            clock: None,
            deadline: Instant::now()
                .checked_add(budget.time)
                .unwrap_or_else(Instant::now),
        }
    }
    fn check(&self) -> Result<(), Reason> {
        if self
            .cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Acquire))
        {
            Err(Reason::Cancelled)
        } else if self.now() >= self.deadline {
            Err(Reason::TimeBudget)
        } else {
            Ok(())
        }
    }
    fn now(&self) -> Instant {
        #[cfg(test)]
        if let Some(clock) = &self.clock {
            return clock.get();
        }
        Instant::now()
    }
    fn reserve(&self, bytes: u64) -> Result<(), Reason> {
        self.check()?;
        if bytes > self.remaining {
            Err(Reason::ByteBudget)
        } else {
            Ok(())
        }
    }
    fn consumed(&mut self, bytes: usize) {
        self.remaining -= bytes as u64;
        self.read += bytes as u64;
    }
}
#[cfg(any(windows, test))]
trait ContentReader {
    fn validate(&mut self, index: usize, entry: &Entry, meter: &Meter) -> Result<(), Reason>;
    fn hash(
        &mut self,
        index: usize,
        entry: &Entry,
        partial: bool,
        meter: &mut Meter,
    ) -> Result<[u8; 32], Reason>;
    fn equal(
        &mut self,
        left: usize,
        right: usize,
        entries: &[Entry],
        meter: &mut Meter,
    ) -> Result<bool, Skip>;
}
#[cfg(any(windows, test))]
fn attributes(attributes: u32) -> Result<(), Reason> {
    if attributes & 0x40000 != 0 || attributes & 0x400000 != 0 {
        Err(Reason::Placeholder)
    } else if attributes & 0x1000 != 0 {
        Err(Reason::Offline)
    } else if attributes & 0x400 != 0 {
        Err(Reason::ReparsePoint)
    } else if attributes & (0x10 | 0x40) != 0 {
        Err(Reason::SpecialFile)
    } else {
        Ok(())
    }
}
#[cfg(any(windows, test))]
fn skip(report: &mut Report, index: usize, reason: Reason) {
    report.partial = true;
    if budget_reason(reason) {
        report.stop = Some(reason);
    }
    report.skipped.push(Skip {
        entry: index,
        reason,
    });
}
#[cfg(any(windows, test))]
fn budget_reason(reason: Reason) -> bool {
    matches!(
        reason,
        Reason::ByteBudget | Reason::TimeBudget | Reason::EntryBudget | Reason::Cancelled
    )
}
#[cfg(any(windows, test))]
fn group(entries: &[Entry], reader: &mut impl ContentReader, meter: &mut Meter) -> Report {
    let mut report = Report::default();
    let mut objects: BTreeMap<_, Vec<usize>> = BTreeMap::new();
    let mut names = BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let checked = meter.check().and_then(|_| {
            if index >= 200_000 {
                return Err(Reason::EntryBudget);
            }
            if !names.insert(&entry.relative) {
                return Err(Reason::DuplicateName);
            }
            attributes(entry.expected.attributes)?;
            if !matches!(
                entry.expected.object.quality,
                crate::identity::IdentityQuality::FileId128
            ) {
                return Err(Reason::Unsupported);
            }
            reader.validate(index, entry, meter)
        });
        if let Err(reason) = checked {
            skip(&mut report, index, reason);
            if budget_reason(reason) {
                break;
            }
            continue;
        }
        objects
            .entry((
                entry.expected.object.volume_serial.clone(),
                entry.expected.object.file_id,
            ))
            .or_default()
            .push(index);
    }
    let mut sizes: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for indices in objects.into_values() {
        if indices.len() > 1 || entries[indices[0]].expected.link_count > 1 {
            report.links_not_compared += indices.len();
            report.partial = true;
            report.links.push(indices);
        } else {
            let index = indices[0];
            match entries[index].expected.size.parse::<u64>() {
                Ok(0) => {}
                Ok(size) => sizes.entry(size).or_default().push(index),
                Err(_) => skip(&mut report, index, Reason::Unsupported),
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = sizes.into_values().filter(|g| g.len() > 1).collect();
    for indices in &mut groups {
        indices.sort_unstable();
    }
    for partial in [true, false] {
        let mut next = Vec::new();
        for indices in groups {
            let mut hashes: BTreeMap<[u8; 32], Vec<usize>> = BTreeMap::new();
            for index in indices {
                match reader.hash(index, &entries[index], partial, meter) {
                    Ok(hash) => hashes.entry(hash).or_default().push(index),
                    Err(reason) => skip(&mut report, index, reason),
                }
            }
            next.extend(hashes.into_values().filter(|g| g.len() > 1));
        }
        groups = next;
    }
    for indices in groups {
        // Exact comparison partitions even hash collisions; no digest establishes equality.
        let mut partitions: Vec<Vec<usize>> = Vec::new();
        let mut pending = VecDeque::from(indices);
        loop {
            while let Some(index) = pending.pop_front() {
                let mut placed = false;
                let mut position = 0;
                while position < partitions.len() {
                    let partition = &mut partitions[position];
                    match reader.equal(partition[0], index, entries, meter) {
                        Ok(true) => {
                            partition.push(index);
                            placed = true;
                            break;
                        }
                        Ok(false) => position += 1,
                        Err(failed) => {
                            skip(&mut report, failed.entry, failed.reason);
                            // Only the representative's own failure dissolves its partition; any other
                            // entry (a reader breaking the contract) drops the candidate, so the loop ends.
                            if failed.entry == index
                                || budget_reason(failed.reason)
                                || failed.entry != partitions[position][0]
                            {
                                placed = true;
                                break;
                            }
                            // A failed witness invalidates its partition's earlier comparisons.
                            let survivors = partitions.remove(position);
                            pending.push_front(index);
                            for &survivor in survivors[1..].iter().rev() {
                                pending.push_front(survivor);
                            }
                            placed = true;
                            break;
                        }
                    }
                }
                if !placed {
                    partitions.push(vec![index]);
                }
            }
            for mut partition in partitions.drain(..) {
                let representative = partition[0];
                let mut failed_representative = false;
                partition.retain(
                    |&index| match reader.validate(index, &entries[index], meter) {
                        Ok(()) => true,
                        Err(reason) => {
                            skip(&mut report, index, reason);
                            failed_representative |=
                                index == representative && !budget_reason(reason);
                            false
                        }
                    },
                );
                if failed_representative {
                    pending.extend(partition);
                } else if partition.len() > 1 {
                    report.groups.push(partition);
                }
            }
            if pending.is_empty() {
                break;
            }
        }
    }
    if let Err(reason) = meter.check() {
        report.partial = true;
        report.stop = Some(reason);
    }
    report.bytes_read = meter.read;
    report
}
#[cfg(windows)]
mod native;
#[cfg(test)]
impl Budget {
    fn test() -> Self {
        Self {
            bytes: 1_000_000,
            time: Duration::from_secs(10),
        }
    }
}
#[cfg(test)]
mod tests;
