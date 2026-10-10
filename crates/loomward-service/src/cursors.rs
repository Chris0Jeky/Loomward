//! Opaque cursors (semantics section 5). The wire carries a random handle; the state stays in the
//! service, so a cursor is bound to this session by construction and cannot be forged or edited.
//! Each entry binds the command, the complete filter set, sort and basis (the canonical request
//! without `cursor` and `limit`), the catalogue's own keyset cursor (which carries the catalogue
//! instance and the subtree or catalogue revision), and, for state-backed pages, `state_rev`
//! (errata #117 partial 15).

use loomward_catalog::{ChildrenCursor, SearchCursor};
use loomward_protocol::Command;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

/// The catalogue keyset position behind a handle.
#[derive(Debug, Clone, PartialEq)]
pub enum Position {
    Children(ChildrenCursor),
    Search(SearchCursor),
}

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    command: Command,
    binding: String,
    position: Position,
    state_rev: Option<i64>,
}

/// Why a handle did not resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// Unknown handle: expired, evicted, from another session, or never issued. Restart paging.
    Expired,
    /// Issued for another command or another filter set: the request is wrong.
    Mismatch,
    /// The bound `state_rev` moved (`state_rev_changed`). Restart paging.
    StateChanged,
}

pub struct Cursors {
    capacity: usize,
    inner: Mutex<(HashMap<String, Entry>, VecDeque<String>)>,
}

impl Cursors {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            inner: Mutex::new(Default::default()),
        }
    }

    // ponytail: FIFO eviction of a bounded map; an evicted cursor restarts paging (stale_generation).
    pub fn issue(
        &self,
        command: Command,
        binding: String,
        position: Position,
        state_rev: Option<i64>,
    ) -> String {
        let mut raw = [0u8; 16];
        getrandom::fill(&mut raw).expect("OS random source");
        let handle = format!(
            "cu{}",
            raw.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (map, order) = &mut *guard;
        while map.len() >= self.capacity {
            match order.pop_front() {
                Some(old) => {
                    map.remove(&old);
                }
                None => break,
            }
        }
        map.insert(
            handle.clone(),
            Entry {
                command,
                binding,
                position,
                state_rev,
            },
        );
        order.push_back(handle.clone());
        handle
    }

    /// The position a handle resumes, if it was issued for exactly this command and binding and
    /// its state revision (when bound) still holds.
    pub fn resume(
        &self,
        handle: &str,
        command: Command,
        binding: &str,
        state_rev: Option<i64>,
    ) -> Result<Position, Resume> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let entry = guard.0.get(handle).ok_or(Resume::Expired)?;
        if entry.command != command || entry.binding != binding {
            return Err(Resume::Mismatch);
        }
        if entry.state_rev.is_some() && entry.state_rev != state_rev {
            return Err(Resume::StateChanged);
        }
        Ok(entry.position.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loomward_catalog::{Basis, CursorValue, Sort};

    fn position() -> Position {
        Position::Children(ChildrenCursor {
            dir_id: 1,
            subtree_rev: 3,
            catalog_instance: "i".into(),
            sort: Sort::SizeDesc,
            basis: Basis::Logical,
            value: CursorValue::Number(5),
            kind: 1,
            row_id: 9,
        })
    }

    #[test]
    fn handles_bind_command_filters_and_state_rev() {
        let cursors = Cursors::new(8);
        let h = cursors.issue(Command::TreeChildren, "b".into(), position(), None);
        assert!(loomward_protocol::Cursor::new(&h).is_ok());
        assert_eq!(
            cursors.resume(&h, Command::TreeChildren, "b", None),
            Ok(position())
        );
        assert_eq!(
            cursors.resume(&h, Command::SearchQuery, "b", None),
            Err(Resume::Mismatch)
        );
        assert_eq!(
            cursors.resume(&h, Command::TreeChildren, "other filters", None),
            Err(Resume::Mismatch)
        );
        assert_eq!(
            cursors.resume("cu00", Command::TreeChildren, "b", None),
            Err(Resume::Expired)
        );
        // partial #15: a state-backed page goes stale when state_rev moves, even if the
        // catalogue revision did not.
        let s = cursors.issue(Command::CollectionsMembers, "c".into(), position(), Some(4));
        assert!(cursors
            .resume(&s, Command::CollectionsMembers, "c", Some(4))
            .is_ok());
        assert_eq!(
            cursors.resume(&s, Command::CollectionsMembers, "c", Some(5)),
            Err(Resume::StateChanged)
        );
    }

    #[test]
    fn eviction_is_bounded_and_oldest_first() {
        let cursors = Cursors::new(2);
        let a = cursors.issue(Command::TreeChildren, "b".into(), position(), None);
        let b = cursors.issue(Command::TreeChildren, "b".into(), position(), None);
        let c = cursors.issue(Command::TreeChildren, "b".into(), position(), None);
        assert_eq!(
            cursors.resume(&a, Command::TreeChildren, "b", None),
            Err(Resume::Expired)
        );
        assert!(cursors.resume(&b, Command::TreeChildren, "b", None).is_ok());
        assert!(cursors.resume(&c, Command::TreeChildren, "b", None).is_ok());
    }
}
