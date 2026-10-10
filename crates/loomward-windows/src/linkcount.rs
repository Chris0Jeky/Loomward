//! Budgeted link-count pass for placement relief.
//!
//! Owner: lane L12 (LW-108, LW-030 wiring); consumed by `loomward-engine::placement`.
//!
//! Scope: metadata-only opens (no content read), placeholders skipped, 200k files per group;
//! reports link counts keyed by identity so hard-linked objects are counted once.
//!
//! #117 errata items for this module: partial #4 (verified relief requires known allocation and
//! identity-matched link observations). Observations are not race-free effect authority.

use crate::identity::{ObjectKey, ObservedIdentity};
use std::{path::PathBuf, time::Instant};

/// Hard cap per candidate group, including skipped or failed names.
pub const MAX_FILES_PER_GROUP: usize = 200_000;

/// Offline or recall-on-open/data-access attributes indicate possible hydration.
pub fn is_placeholder(attributes: u32) -> bool {
    attributes & (0x1000 | 0x40000 | 0x400000) != 0
}

/// One trusted catalogue name to observe, never a path supplied by a service caller.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Exact granted path binding, supplied by the catalogue adapter.
    pub path: PathBuf,
    /// Identity from the listing; absent identity cannot establish relief.
    pub expected: Option<ObjectKey>,
    /// Listing attributes used to skip placeholders before any open.
    pub attributes: u32,
}

/// A name's observation or its explicit unknown reason.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// Handle identity matches the listing; default-stream sizes and link count observed.
    Matched(Box<ObservedIdentity>),
    /// Placeholder, reparse entry, missing/changed identity or metadata error.
    Unknown,
}

/// Why the bounded pass stopped before observing all names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// 200,000 names were considered.
    FileBudget,
    /// The caller's wall-clock budget expired.
    Deadline,
}

/// Outcomes correspond to the input prefix; a stopped suffix is unknown, never zero relief.
#[derive(Debug)]
pub struct Report {
    /// Observed or explicitly skipped prefix.
    pub outcomes: Vec<Outcome>,
    /// No stop means all input entries were considered, not that all were matched.
    pub stop: Option<Stop>,
}

/// Metadata-only pass. Placeholders are skipped before opens and checked again on the handle.
/// The deadline is checked between OS calls; a blocked metadata open cannot be preempted here.
pub fn observe(entries: &[Entry], deadline: Instant) -> Report {
    let mut report = Report {
        outcomes: Vec::with_capacity(entries.len().min(MAX_FILES_PER_GROUP)),
        stop: None,
    };
    for entry in entries {
        if Instant::now() >= deadline {
            report.stop = Some(Stop::Deadline);
            break;
        }
        if report.outcomes.len() == MAX_FILES_PER_GROUP {
            report.stop = Some(Stop::FileBudget);
            break;
        }
        let outcome = if is_placeholder(entry.attributes)
            || entry.attributes & (0x400 | 0x10) != 0
            || entry.expected.is_none()
        {
            Outcome::Unknown
        } else {
            one(entry).unwrap_or(Outcome::Unknown)
        };
        report.outcomes.push(outcome);
    }
    if report.stop.is_none() && Instant::now() >= deadline {
        report.stop = Some(Stop::Deadline);
    }
    report
}

#[cfg(not(windows))]
fn one(_: &Entry) -> std::io::Result<Outcome> {
    Ok(Outcome::Unknown)
}

#[cfg(windows)]
fn one(entry: &Entry) -> std::io::Result<Outcome> {
    use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES};
    // A fresh pre-open check complements the listed flags; the handle is checked again below.
    let path = crate::win::wide(&entry.path)?;
    let attributes = unsafe { GetFileAttributesW(path.as_ptr()) };
    if attributes == INVALID_FILE_ATTRIBUTES
        || is_placeholder(attributes)
        || attributes & (0x400 | 0x10) != 0
    {
        return Ok(Outcome::Unknown);
    }
    // identity::observe uses FILE_READ_ATTRIBUTES, share all and OPEN_REPARSE_POINT|BACKUP_SEMANTICS.
    let observed = crate::identity::observe(&entry.path)?;
    if is_placeholder(observed.attributes)
        || observed.attributes & (0x400 | 0x10) != 0
        || observed.object.reparse_tag != 0
        || observed.link_count == 0
        || entry.expected.as_ref() != Some(&observed.object)
    {
        Ok(Outcome::Unknown)
    } else {
        Ok(Outcome::Matched(Box::new(observed)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn linkcount_skips_placeholders_and_stops_at_deadline_and_file_cap() {
        let entry = Entry {
            path: PathBuf::from("unused"),
            expected: None,
            attributes: 0x40000,
        };
        let report = observe(
            std::slice::from_ref(&entry),
            Instant::now() + Duration::from_secs(10),
        );
        assert!(matches!(report.outcomes.as_slice(), [Outcome::Unknown]));
        assert_eq!(report.stop, None);
        let report = observe(std::slice::from_ref(&entry), Instant::now());
        assert_eq!(report.stop, Some(Stop::Deadline));
        assert!(report.outcomes.is_empty());
        let report = observe(
            &vec![entry; MAX_FILES_PER_GROUP + 1],
            Instant::now() + Duration::from_secs(10),
        );
        assert_eq!(report.outcomes.len(), MAX_FILES_PER_GROUP);
        assert_eq!(report.stop, Some(Stop::FileBudget));
    }

    #[cfg(windows)]
    #[test]
    fn linkcount_disposable_hardlinks_match_identity_and_refuse_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        std::fs::write(&a, [7u8; 100]).unwrap();
        std::fs::hard_link(&a, &b).unwrap();
        let expected = crate::identity::observe(&a).unwrap().object;
        let entries = vec![
            Entry {
                path: a,
                expected: Some(expected.clone()),
                attributes: 0,
            },
            Entry {
                path: b.clone(),
                expected: Some(expected.clone()),
                attributes: 0,
            },
        ];
        let report = observe(&entries, Instant::now() + Duration::from_secs(10));
        assert_eq!(report.stop, None);
        assert_eq!(report.outcomes.len(), 2);
        for outcome in report.outcomes {
            let Outcome::Matched(o) = outcome else {
                panic!("hardlink metadata unknown")
            };
            assert_eq!(o.object, expected);
            assert_eq!(o.link_count, 2);
            assert_eq!(o.size, "100");
        }
        for attributes in [0x1000, 0x40000, 0x400000] {
            let mut placeholder = entries[0].clone();
            placeholder.attributes = attributes;
            let report = observe(&[placeholder], Instant::now() + Duration::from_secs(10));
            assert!(matches!(report.outcomes.as_slice(), [Outcome::Unknown]));
        }
        std::fs::remove_file(&b).unwrap();
        std::fs::write(&b, [8u8; 100]).unwrap();
        let report = observe(&entries[1..], Instant::now() + Duration::from_secs(10));
        assert!(matches!(report.outcomes.as_slice(), [Outcome::Unknown]));
    }
}
