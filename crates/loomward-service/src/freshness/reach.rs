//! Journal identities only select catalogue scopes; missing ancestry never grants reach.
use loomward_windows::usn::Record;
use rusqlite::{params_from_iter, types::Value, Connection};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};

const BATCH: usize = 128;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Directory {
    Inside(u64),
    Outside,
    #[default]
    Unresolved,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Identity {
    directory: Directory,
    previous: BTreeSet<u64>,
}

#[derive(Debug, PartialEq, Eq)]
enum Reach {
    Inside {
        parent: u64,
        previous: BTreeSet<u64>,
    },
    PreviousCataloguedParent(BTreeSet<u64>),
    Outside,
    Unresolved,
}

fn canonical(id: &[u8]) -> Vec<u8> {
    let mut id = id.to_vec();
    if id.len() == 8 {
        id.resize(16, 0);
    }
    id
}

// Pure classification: neither names nor journal-supplied parents are opened.
fn classify(record: &Record, lookup: impl Fn(&[u8]) -> Identity) -> Reach {
    let parent = lookup(&canonical(&record.parent_id)).directory;
    let previous = lookup(&canonical(&record.file_id)).previous;
    match parent {
        Directory::Inside(parent) => Reach::Inside { parent, previous },
        _ if !previous.is_empty() => Reach::PreviousCataloguedParent(previous),
        Directory::Outside => Reach::Outside,
        Directory::Unresolved => Reach::Unresolved,
    }
}

/// Each values row holds a canonical identity and one exact stored-width alias. Equality keeps
/// both identity indexes usable; visibility walks upward only from matching candidates.
pub(super) fn lookup_sql(identities: &BTreeSet<Vec<u8>>, root: i64) -> (String, Vec<Value>) {
    let mut values = Vec::new();
    let mut args = vec![Value::Integer(root)];
    for id in identities {
        let aliases = std::iter::once(id.as_slice())
            .chain((id.len() == 16 && id[8..] == [0; 8]).then_some(&id[..8]));
        for alias in aliases {
            let n = args.len() + 1;
            values.push(format!("(?{n},?{})", n + 1));
            args.push(Value::Blob(id.clone()));
            args.push(Value::Blob(alias.to_vec()));
        }
    }
    let sql = format!(
        "WITH RECURSIVE wanted(canonical,file_id) AS (VALUES {}),
        candidates(canonical,id,parent_id) AS MATERIALIZED (
          SELECT w.canonical,d.id,d.parent_id FROM wanted w
          CROSS JOIN main.dir d INDEXED BY dir_identity ON d.root_id=?1 AND d.file_id=w.file_id
          WHERE d.file_id IS NOT NULL),
        previous(canonical,id) AS (
          SELECT canonical,parent_id FROM candidates WHERE parent_id IS NOT NULL
          UNION
          SELECT w.canonical,f.dir_id FROM wanted w
          CROSS JOIN main.file f INDEXED BY file_identity ON f.file_id=w.file_id
          CROSS JOIN main.dir d ON d.id=f.dir_id
          WHERE f.file_id IS NOT NULL AND d.root_id=?1),
        seeds(id) AS (SELECT id FROM candidates UNION SELECT id FROM previous),
        ancestry(seed,id,parent_id,blocked) AS (
          SELECT d.id,d.id,d.parent_id,d.listing_state IN ('absent_pending','excluded')
          FROM seeds s CROSS JOIN main.dir d ON d.id=s.id WHERE d.root_id=?1
          UNION ALL
          SELECT a.seed,d.id,d.parent_id,a.blocked OR d.listing_state IN ('absent_pending','excluded')
          FROM ancestry a JOIN main.dir d ON d.id=a.parent_id WHERE d.root_id=?1),
        visible(id) AS (SELECT seed FROM ancestry WHERE parent_id IS NULL AND NOT blocked)
        SELECT c.canonical,0,c.id,c.id IN (SELECT id FROM visible) FROM candidates c
        UNION ALL
        SELECT p.canonical,1,p.id,1 FROM previous p JOIN visible v ON v.id=p.id",
        values.join(",")
    );
    (sql, args)
}

pub(super) fn lookup(
    conn: &Connection,
    root: i64,
    identities: &BTreeSet<Vec<u8>>,
) -> rusqlite::Result<BTreeMap<Vec<u8>, Identity>> {
    let (sql, args) = lookup_sql(identities, root);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(args))?;
    let mut found: BTreeMap<Vec<u8>, Identity> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let id: Vec<u8> = row.get(0)?;
        let previous: bool = row.get(1)?;
        let dir = row.get::<_, i64>(2)? as u64;
        let visible: bool = row.get(3)?;
        let entry = found.entry(id).or_default();
        if previous {
            entry.previous.insert(dir);
        } else {
            entry.directory = if visible {
                Directory::Inside(dir)
            } else {
                Directory::Outside
            };
        }
    }
    Ok(found)
}

/// The caller takes/releases the DB mutex for each lookup, never across native journal IO.
pub(super) fn targets(
    records: &[Record],
    deadline: Instant,
    mut lookup_batch: impl FnMut(&BTreeSet<Vec<u8>>) -> rusqlite::Result<BTreeMap<Vec<u8>, Identity>>,
) -> Option<BTreeSet<u64>> {
    let mut targets = BTreeSet::new();
    let mut new_dirs = BTreeMap::new();
    for batch in records.chunks(BATCH) {
        if Instant::now() >= deadline {
            return None;
        }
        let ids = batch
            .iter()
            .flat_map(|r| [canonical(&r.parent_id), canonical(&r.file_id)])
            .collect();
        let found = lookup_batch(&ids).ok()?;
        if Instant::now() >= deadline {
            return None;
        }
        for record in batch {
            let reach = classify(record, |id| {
                let mut identity = found.get(id).cloned().unwrap_or_default();
                if let Some(&parent) = new_dirs.get(id) {
                    identity.directory = Directory::Inside(parent);
                }
                identity
            });
            match reach {
                Reach::Inside { parent, previous } => {
                    targets.insert(parent);
                    targets.extend(previous);
                    if record.is_directory {
                        new_dirs.insert(canonical(&record.file_id), parent);
                    }
                }
                Reach::PreviousCataloguedParent(previous) => targets.extend(previous),
                Reach::Outside | Reach::Unresolved => {}
            }
            if new_dirs.len() > 100_000 || targets.len() > 1024 {
                return None;
            }
        }
    }
    (Instant::now() < deadline).then_some(targets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn record(file: u64, parent: u64, directory: bool) -> Record {
        Record {
            file_id: file.to_le_bytes().to_vec(),
            parent_id: parent.to_le_bytes().to_vec(),
            reason: 0x2000,
            is_directory: directory,
        }
    }

    #[test]
    fn move_in_out_rename_hardlink_and_unknown_parent_classification() {
        let lookup = |id: &[u8]| match u64::from_le_bytes(id[..8].try_into().unwrap()) {
            1 => Identity {
                directory: Directory::Inside(11),
                ..Default::default()
            },
            2 => Identity {
                directory: Directory::Inside(22),
                ..Default::default()
            },
            3 => Identity {
                directory: Directory::Outside,
                ..Default::default()
            },
            4 => Identity {
                previous: BTreeSet::from([22]),
                ..Default::default()
            },
            _ => Identity::default(),
        };
        assert_eq!(
            classify(&record(99, 1, false), lookup),
            Reach::Inside {
                parent: 11,
                previous: BTreeSet::new()
            }
        ); // move in
        assert_eq!(
            classify(&record(4, 3, false), lookup),
            Reach::PreviousCataloguedParent(BTreeSet::from([22]))
        ); // move out
        assert_eq!(
            classify(&record(4, 1, false), lookup),
            Reach::Inside {
                parent: 11,
                previous: BTreeSet::from([22])
            }
        ); // rename within
        let mut hardlink = record(4, 3, false);
        hardlink.reason = 0x10000;
        assert_eq!(
            classify(&hardlink, lookup),
            Reach::PreviousCataloguedParent(BTreeSet::from([22]))
        );
        assert_eq!(classify(&record(99, 3, false), lookup), Reach::Outside);
        assert_eq!(classify(&record(99, 999, false), lookup), Reach::Unresolved);
    }

    #[test]
    fn new_directory_chains_survive_batch_boundaries_but_unknown_chains_are_skipped() {
        let mut records = vec![record(2, 1, true)];
        records.extend((0..127).map(|i| record(1000 + i, 999, false)));
        records.extend([
            record(3, 2, true),
            record(4, 3, false),
            record(6, 5, true),
            record(7, 6, false),
        ]);
        let result = targets(&records, Instant::now() + Duration::from_secs(1), |_| {
            Ok(BTreeMap::from([(
                canonical(&1u64.to_le_bytes()),
                Identity {
                    directory: Directory::Inside(11),
                    ..Default::default()
                },
            )]))
        });
        assert_eq!(result, Some(BTreeSet::from([11])));
    }

    #[test]
    fn expired_batch_deadlines_and_lookup_failures_are_not_empty_success() {
        let records = [record(2, 1, false)];
        assert_eq!(
            targets(&records, Instant::now(), |_| panic!(
                "expired before lookup"
            )),
            None
        );
        assert_eq!(
            targets(&records, Instant::now() + Duration::from_secs(1), |_| Err(
                rusqlite::Error::InvalidQuery
            )),
            None
        );
        let deadline = Instant::now() + Duration::from_millis(5);
        assert_eq!(
            targets(&records, deadline, |_| {
                std::thread::sleep(Duration::from_millis(10));
                Ok(BTreeMap::new())
            }),
            None
        );
    }

    #[test]
    fn indexed_lookup_checks_ancestry_widths_and_all_in_root_hardlink_parents() {
        let conn = super::super::tests::catalogue(10);
        // Row 3 remains under excluded row 2; row 4 is absent. Neither is a visible parent.
        conn.execute_batch(
            "UPDATE dir SET listing_state='excluded' WHERE id=2;
            UPDATE dir SET parent_id=2 WHERE id=3;
            UPDATE dir SET listing_state='absent_pending' WHERE id=4;
            UPDATE dir SET file_id=substr(file_id,1,8) WHERE id=5;
            INSERT INTO root(id,grant_id,state) VALUES(2,2,'complete');
            UPDATE dir SET root_id=2,parent_id=NULL WHERE id=10;",
        )
        .unwrap();
        for parent in [2, 3, 4, 5, 6, 10] {
            conn.execute("INSERT INTO file(dir_id,name,file_id,logical,attrs,born_run,seen_run) VALUES(?1,'synthetic-link',?2,1,0,1,1)", rusqlite::params![parent, 99u64.to_le_bytes().to_vec()]).unwrap();
        }
        let ids = [1u64, 2, 3, 4, 5, 99, 100]
            .map(|id| canonical(&id.to_le_bytes()))
            .into_iter()
            .collect();
        let found = lookup(&conn, 1, &ids).unwrap();
        let get = |id: u64| {
            found
                .get(&canonical(&id.to_le_bytes()))
                .cloned()
                .unwrap_or_default()
        };
        assert_eq!(get(1).directory, Directory::Inside(1));
        for id in [2, 3, 4] {
            assert_eq!(get(id).directory, Directory::Outside);
        }
        assert_eq!(get(5).directory, Directory::Inside(5));
        assert_eq!(get(99).previous, BTreeSet::from([5, 6]));
        assert_eq!(get(100).directory, Directory::Unresolved);
        let mut wide = canonical(&5u64.to_le_bytes());
        wide[8] = 1;
        assert!(
            lookup(&conn, 1, &BTreeSet::from([wide]))
                .unwrap()
                .is_empty(),
            "never truncate a V3 ID"
        );
    }
}
