//! First-scan bulk build (#148): atomic publication, crash and revocation discard, cancellation,
//! the incremental second scan, and row-for-row equivalence with the incremental path.
use super::*;
use std::path::Path;

fn open(path: &Path, bulk: bool) -> crate::Catalog {
    let catalog = crate::Catalog::open(path, "synthetic").unwrap();
    catalog.writer().set_bulk_first_scan(bulk);
    catalog
}

/// Registers a fixture root on an NTFS volume; returns (grant, root directory).
fn register(w: &Writer, key: u8) -> (i64, i64) {
    let WriteReply::Root {
        grant_id, dir_id, ..
    } = w
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_key: "fixture".into(),
            display_name: format!("Fixture {key}"),
            display_path: format!("Fixture root {key}"),
            root_file_id: Some(vec![key; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))
        .unwrap()
    else {
        panic!("root receipt")
    };
    (grant_id, dir_id)
}

fn begin(w: &Writer, grant: i64) -> i64 {
    let WriteReply::Run(run) = w
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "full".into(),
            strategy: "fixture".into(),
            started_at_ns: 1,
        })
        .unwrap()
    else {
        panic!("run receipt")
    };
    run
}

fn end(w: &Writer, run: i64, state: &str) -> Result<WriteReply> {
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: state.into(),
        finished_at_ns: 2,
    })
}

fn obs(name: &str, logical: u64, id: Option<u8>) -> Observation {
    let mut o = Observation::file(name, logical, Some(logical.div_ceil(4096) * 4096));
    o.file_id = id.map(|id| vec![id; 16]);
    o.created_ft = Some(1);
    o.modified_ft = Some(i64::from(id.unwrap_or(0)));
    if let Some((_, ext)) = name.rsplit_once('.') {
        o.extension = Some(ext.into());
        o.family = "other".into();
    }
    o
}

/// The child row a parent's publication committed, live or bulk, as the service sink finds it.
fn child(path: &Path, parent: i64, name: &str) -> i64 {
    Connection::open(path.join("catalog.db"))
        .unwrap()
        .query_row(
            "SELECT id FROM dir WHERE parent_id=?1 AND name=?2 UNION ALL SELECT id FROM bulk_dir WHERE parent_id=?1 AND name=?2",
            params![parent, name],
            |r| r.get(0),
        )
        .unwrap()
}

/// Rows of `root` a separate reader can see: (directories, files).
fn visible(path: &Path, root_dir: i64) -> (i64, i64) {
    let db = Connection::open(path.join("catalog.db")).unwrap();
    db.query_row(
        "SELECT (SELECT count(*) FROM dir WHERE root_id=(SELECT root_id FROM dir WHERE id=?1)),
                (SELECT count(*) FROM file f JOIN dir d ON d.id=f.dir_id WHERE d.root_id=(SELECT root_id FROM dir WHERE id=?1))",
        [root_dir],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

fn bulk_rows(path: &Path) -> i64 {
    Connection::open(path.join("catalog.db"))
        .unwrap()
        .query_row(
            "SELECT (SELECT count(*) FROM bulk_dir)+(SELECT count(*) FROM bulk_file)",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

/// One first scan covering staged and direct listings, excluded, identity-less and lossy names,
/// a hard link, an incomplete listing, a stale publication and its relist (old-row matching), a
/// final, and the run's end. `between` runs after every command; replies are returned.
fn scan(
    w: &Writer,
    path: &Path,
    grant: i64,
    root_dir: i64,
    state: &str,
    between: &mut dyn FnMut(),
) -> (i64, Vec<String>) {
    let run = begin(w, grant);
    let mut replies = Vec::new();
    let mut call = |command: WriteCommand| {
        let reply = w.call(command);
        replies.push(format!("{reply:?}"));
        between();
        reply
    };
    let mut lossy = obs("lossy", 4, Some(4));
    lossy.name_utf16 = Some(vec![0x00, 0xd8]);
    lossy.flags = 1 << 9;
    let mut skipped = obs("placeholder.txt", 50, Some(6));
    skipped.flags = 1 << 3;
    let mut excluded_dir = obs("junction", 0, Some(0x13));
    excluded_dir.flags = 1 << 4;
    call(WriteCommand::StageChunk {
        run_id: run,
        dir_id: root_dir,
        seq: 0,
        files: vec![
            obs("a.txt", 10, Some(1)),
            obs("b.bin", 20_000, Some(2)),
            obs("c", 30, None),
            lossy,
            skipped,
        ],
        dirs: vec![
            obs("sub1", 0, Some(0x11)),
            obs("sub2", 0, Some(0x12)),
            excluded_dir,
        ],
    })
    .unwrap();
    call(WriteCommand::StageChunk {
        run_id: run,
        dir_id: root_dir,
        seq: 1,
        files: vec![obs("e.txt", 5, Some(5))],
        dirs: vec![],
    })
    .unwrap();
    call(WriteCommand::ListingDone {
        run_id: run,
        dir_id: root_dir,
        outcome: ListingOutcome::Complete,
        skipped: 2,
        errors: 0,
    })
    .unwrap();
    let sub1 = child(path, root_dir, "sub1");
    let sub2 = child(path, root_dir, "sub2");
    let listed = call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: sub1,
        files: vec![obs("b-link.bin", 20_000, Some(2)), obs("g.txt", 7, Some(7))],
        dirs: vec![obs("inner", 0, Some(0x21))],
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }));
    let inner = child(path, sub1, "inner");
    call(WriteCommand::StageChunk {
        run_id: run,
        dir_id: inner,
        seq: 0,
        files: vec![obs("deep.dat", 3, Some(8))],
        dirs: vec![],
    })
    .unwrap();
    call(WriteCommand::ListingDone {
        run_id: run,
        dir_id: inner,
        outcome: ListingOutcome::Incomplete("partial".into()),
        skipped: 0,
        errors: 1,
    })
    .unwrap();
    call(WriteCommand::StageChunk {
        run_id: run,
        dir_id: sub2,
        seq: 0,
        files: vec![obs("x.txt", 1, Some(9)), obs("y.txt", 2, Some(10))],
        dirs: vec![],
    })
    .unwrap();
    call(WriteCommand::ListingDone {
        run_id: run,
        dir_id: sub2,
        outcome: ListingOutcome::Incomplete("stale".into()),
        skipped: 0,
        errors: 0,
    })
    .unwrap();
    call(WriteCommand::StageChunk {
        run_id: run,
        dir_id: sub2,
        seq: 0,
        files: vec![obs("x-renamed.txt", 1, Some(9)), obs("z.txt", 3, Some(11))],
        dirs: vec![],
    })
    .unwrap();
    call(WriteCommand::ListingDone {
        run_id: run,
        dir_id: sub2,
        outcome: ListingOutcome::Complete,
        skipped: 0,
        errors: 0,
    })
    .unwrap();
    if let Ok(WriteReply::Listing { revision }) = listed {
        let _ = call(WriteCommand::DirFinal {
            run_id: run,
            dir_id: sub1,
            input_revision: revision,
            totals: Totals {
                files: 2,
                logical: 20_007,
                allocated: 2 * 4096 + 4096,
                complete: false,
                ..Totals::default()
            },
        });
    }
    let _ = call(WriteCommand::EndRun {
        run_id: run,
        state: state.into(),
        finished_at_ns: 2,
    });
    (run, replies)
}

/// Every catalogue row (bulk sequences aside) plus the reader API's view of each root, with the
/// per-catalogue random instance replaced.
fn dump(path: &Path, catalog: &crate::Catalog, roots: &[(i64, i64)]) -> Vec<String> {
    let db = Connection::open(path.join("catalog.db")).unwrap();
    let instance: String = db
        .query_row("SELECT value FROM meta WHERE key='instance_id'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mut out = Vec::new();
    for sql in [
        "SELECT * FROM dir ORDER BY id",
        "SELECT * FROM file ORDER BY id",
        "SELECT * FROM ext ORDER BY id",
        "SELECT * FROM multilink ORDER BY volume_id,file_id",
        "SELECT * FROM root ORDER BY id",
        "SELECT * FROM scan_run ORDER BY id",
        "SELECT * FROM revision",
        "SELECT * FROM volume ORDER BY id",
        "SELECT * FROM stage_entry",
        "SELECT * FROM meta WHERE key!='instance_id' ORDER BY key",
        "SELECT name,seq FROM sqlite_sequence WHERE name NOT LIKE 'bulk_%' ORDER BY name",
        "SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT LIKE '%bulk_%' ORDER BY name",
    ] {
        let mut s = db.prepare(sql).unwrap();
        let columns = s.column_count();
        let rows = s
            .query_map([], |r| {
                (0..columns)
                    .map(|i| r.get::<_, rusqlite::types::Value>(i))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        out.push(format!("{sql}: {rows:?}"));
    }
    let mut reader = catalog.reader().unwrap();
    let dirs: Vec<i64> = db
        .prepare("SELECT id FROM dir ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let files: Vec<i64> = db
        .prepare("SELECT id FROM file ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for &(grant, root_dir) in roots {
        for basis in [Basis::Logical, Basis::Allocated] {
            out.push(format!(
                "{:?}",
                reader.slice(&SliceRequest {
                    anchor: NodeKey::Dir(root_dir),
                    depth: 8,
                    max_nodes: 500,
                    min_share: 0.0,
                    basis,
                    include_files: true,
                })
            ));
            out.push(format!(
                "{:?}",
                reader.breakdown(root_dir, "extension", basis, 10, 1_000_000)
            ));
        }
        for text in ["x", ".", "lossy"] {
            out.push(format!(
                "{:?}",
                reader.search(&SearchRequest {
                    root_id: Some(grant),
                    text: text.into(),
                    extension: None,
                    min_bytes: None,
                    kind: "any".into(),
                    limit: 100,
                    cursor: None,
                    work_budget: 1_000_000,
                })
            ));
        }
        out.push(format!("{:?}", reader.duplicate_candidates(grant, 1, 200)));
    }
    for &dir in &dirs {
        for sort in [Sort::SizeDesc, Sort::NameAsc, Sort::ModifiedDesc] {
            out.push(format!(
                "{:?}",
                reader.children(&ChildrenRequest {
                    dir_id: dir,
                    sort,
                    basis: Basis::Logical,
                    limit: 100,
                    cursor: None,
                })
            ));
        }
        out.push(format!("{:?}", reader.inspect(NodeKey::Dir(dir))));
        out.push(format!("{:?}", reader.path(NodeKey::Dir(dir))));
    }
    for &file in &files {
        out.push(format!("{:?}", reader.inspect(NodeKey::File(file))));
    }
    out.into_iter()
        .map(|line| line.replace(&instance, "<instance>"))
        .collect()
}

/// Root A's first scan with a neighbour publishing between A's commands: none; root B's ordinary
/// refresh, larger than A by A's swap, so the swap takes the ordered-insert branch; or root C's
/// own first scan, still building when A swaps, so A's swap must leave C's build alone.
fn equivalent_catalogue(bulk: bool, state: &str, neighbour: &str) -> (Vec<String>, Vec<String>) {
    let temp = tempfile::tempdir().unwrap();
    let catalog = open(temp.path(), bulk);
    let w = catalog.writer();
    let (grant_b, root_b) = register(w, 2);
    end(w, begin(w, grant_b), "completed").unwrap();
    let run_b = begin(w, grant_b);
    let (grant_c, root_c) = register(w, 3);
    let run_c = begin(w, grant_c);
    let (grant_a, root_a) = register(w, 1);
    let mut next = 0u8;
    let mut built = 0;
    let mut between = || {
        built = built.max(bulk_rows(temp.path()));
        let (run_id, dir_id, prefix) = match neighbour {
            "live" => (run_b, root_b, 100),
            "bulk" => (run_c, root_c, 60),
            _ => return,
        };
        w.call(WriteCommand::DirListing(DirListing {
            run_id,
            dir_id,
            files: (0..=next)
                .map(|i| obs(&format!("n{i}.log"), 100 + u64::from(i), Some(prefix + i)))
                .collect(),
            dirs: vec![obs(&format!("ndir{next}"), 0, Some(250 - next))],
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        }))
        .unwrap();
        next += 1;
    };
    let (_, replies) = scan(w, temp.path(), grant_a, root_a, state, &mut between);
    assert!(
        next == 0 || next > 11,
        "the neighbour outgrows A's eleven files"
    );
    end(w, run_b, "completed").unwrap();
    end(w, run_c, "completed").unwrap();
    // The bulk variant really built off to the side; the incremental one never did.
    assert_eq!(built > 0, bulk);
    assert_eq!(bulk_rows(temp.path()), 0);
    (
        replies,
        dump(
            temp.path(),
            &catalog,
            &[(grant_a, root_a), (grant_b, root_b), (grant_c, root_c)],
        ),
    )
}

#[test]
fn bulk_first_scan_matches_the_incremental_path_row_for_row_and_query_for_query() {
    for state in ["completed", "cancelled"] {
        for neighbour in ["none", "live", "bulk"] {
            let (normal_replies, normal) = equivalent_catalogue(false, state, neighbour);
            let (bulk_replies, bulk) = equivalent_catalogue(true, state, neighbour);
            assert_eq!(normal_replies, bulk_replies, "{state} {neighbour}");
            assert_eq!(normal.len(), bulk.len());
            for (n, b) in normal.iter().zip(&bulk) {
                assert_eq!(n, b, "{state} {neighbour}");
            }
            // The scenario really published: files under A, a hard link and an extension.
            assert!(normal[1].contains("b-link.bin") && normal[3].contains("Blob"));
        }
    }
}

#[test]
fn readers_see_the_empty_root_until_end_run_commits_the_whole_build() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = open(temp.path(), true);
    let w = catalog.writer();
    let (grant, root_dir) = register(w, 1);
    let mut checks = 0;
    let mut between = || {
        if bulk_rows(temp.path()) > 1 && checks < 11 {
            // Mid-build: committed bulk rows exist, readers still see only the root directory.
            assert_eq!(visible(temp.path(), root_dir), (1, 0));
            let mut reader = catalog.reader().unwrap();
            let page = reader
                .children(&ChildrenRequest {
                    dir_id: root_dir,
                    sort: Sort::NameAsc,
                    basis: Basis::Logical,
                    limit: 100,
                    cursor: None,
                })
                .unwrap();
            assert!(page.items.is_empty(), "{page:?}");
            checks += 1;
        }
    };
    let (run, replies) = scan(w, temp.path(), grant, root_dir, "never", &mut between);
    // "never" is refused, so the build is still unpublished; a snapshot taken now keeps that view.
    assert!(replies.last().unwrap().contains("end state"));
    assert!(checks >= 10, "{checks}");
    assert_eq!(visible(temp.path(), root_dir), (1, 0));
    let snapshot = Connection::open(temp.path().join("catalog.db")).unwrap();
    snapshot
        .execute_batch("BEGIN; SELECT count(*) FROM file")
        .unwrap();
    end(w, run, "completed").unwrap();
    let count = |db: &Connection| {
        db.query_row("SELECT count(*) FROM file", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    assert_eq!(count(&snapshot), 0, "an open snapshot keeps the empty root");
    snapshot.execute_batch("ROLLBACK").unwrap();
    assert_eq!(visible(temp.path(), root_dir), (5, 11));
    assert_eq!(count(&snapshot), 11);
    assert_eq!(bulk_rows(temp.path()), 0);
    // Indexes were dropped and rebuilt inside the EndRun transaction: all four are back.
    let indexes: i64 = snapshot
        .query_row("SELECT count(*) FROM sqlite_schema WHERE type='index' AND tbl_name='file' AND name IN ('file_by_logical','file_by_allocated','file_identity','file_by_ext')", [], |r| r.get(0))
        .unwrap();
    assert_eq!(indexes, 4);
}

/// Live state after a reopen that found an unpublished build: root directory only, run failed,
/// root partial, bulk tables empty, every file index intact.
fn assert_discarded(path: &Path, root_dir: i64) {
    drop(crate::db::open_pair(path, "synthetic").unwrap());
    assert_eq!(visible(path, root_dir), (1, 0));
    assert_eq!(bulk_rows(path), 0);
    let db = Connection::open(path.join("catalog.db")).unwrap();
    let state = |sql: &str| db.query_row(sql, [], |r| r.get::<_, String>(0)).unwrap();
    assert_eq!(
        state("SELECT state FROM scan_run ORDER BY id DESC LIMIT 1"),
        "failed"
    );
    assert_eq!(
        state("SELECT state FROM root WHERE active_run IS NULL ORDER BY id LIMIT 1"),
        "partial"
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='index' AND tbl_name='file'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        4
    );
}

#[test]
fn a_crash_between_build_and_swap_leaves_nothing_visible_and_reopen_discards_the_build() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = open(temp.path(), true);
    let (grant, root_dir) = register(catalog.writer(), 1);
    // Every receipt follows COMMIT, so dropping the writer here is a kill after the last commit.
    let (_, replies) = scan(
        catalog.writer(),
        temp.path(),
        grant,
        root_dir,
        "never",
        &mut || {},
    );
    assert!(replies.last().unwrap().contains("end state"));
    assert!(bulk_rows(temp.path()) > 0);
    drop(catalog);
    assert_eq!(visible(temp.path(), root_dir), (1, 0));
    assert_discarded(temp.path(), root_dir);
}

#[test]
fn a_failed_swap_commit_rolls_back_the_whole_publication() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = open(temp.path(), true);
    let (grant, root_dir) = register(catalog.writer(), 1);
    // The EndRun savepoint succeeds; its COMMIT then fails a deferred foreign key.
    Connection::open(temp.path().join("catalog.db")).unwrap().execute_batch("CREATE TABLE fault_parent(id INTEGER PRIMARY KEY);
        CREATE TABLE fault_child(id INTEGER REFERENCES fault_parent(id) DEFERRABLE INITIALLY DEFERRED);
        CREATE TRIGGER fail_swap AFTER UPDATE OF state ON scan_run WHEN NEW.state='completed' BEGIN INSERT INTO fault_child VALUES(1); END;").unwrap();
    let (_, replies) = scan(
        catalog.writer(),
        temp.path(),
        grant,
        root_dir,
        "completed",
        &mut || {},
    );
    assert!(replies.last().unwrap().contains("Err"), "{replies:?}");
    assert_eq!(visible(temp.path(), root_dir), (1, 0));
    drop(catalog);
    assert_discarded(temp.path(), root_dir);
}

#[test]
fn revocation_mid_build_discards_the_build() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = open(temp.path(), true);
    let w = catalog.writer();
    let (grant, root_dir) = register(w, 1);
    let (run, _) = scan(w, temp.path(), grant, root_dir, "never", &mut || {});
    assert!(bulk_rows(temp.path()) > 0);
    w.call(WriteCommand::RevokeGrant {
        grant_id: grant,
        revoked_at_ns: 3,
    })
    .unwrap();
    assert_eq!(bulk_rows(temp.path()), 0);
    assert_eq!(visible(temp.path(), root_dir), (1, 0));
    assert!(matches!(
        end(w, run, "completed"),
        Err(Error::Invalid("inactive run or grant"))
    ));
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT state FROM scan_run WHERE id=?1", [run], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "cancelled"
    );
    assert_eq!(visible(temp.path(), root_dir), (1, 0));
}

#[test]
fn a_failed_run_discards_the_build_and_the_next_scan_is_incremental() {
    for first in ["failed", "completed"] {
        let temp = tempfile::tempdir().unwrap();
        let catalog = open(temp.path(), true);
        let w = catalog.writer();
        let (grant, root_dir) = register(w, 1);
        scan(w, temp.path(), grant, root_dir, first, &mut || {});
        assert_eq!(bulk_rows(temp.path()), 0);
        let db = Connection::open(temp.path().join("catalog.db")).unwrap();
        let root_state = || {
            db.query_row("SELECT state FROM root WHERE id=1", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap()
        };
        if first == "failed" {
            assert_eq!(visible(temp.path(), root_dir), (1, 0));
            assert_eq!(root_state(), "partial");
        } else {
            assert_eq!(visible(temp.path(), root_dir), (5, 11));
            assert_eq!(root_state(), "partial");
        }
        // The second scan of the root publishes each listing as it commits, never in bulk.
        let run = begin(w, grant);
        w.call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id: root_dir,
            files: vec![obs("only.txt", 9, Some(1))],
            dirs: vec![],
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        }))
        .unwrap();
        assert_eq!(bulk_rows(temp.path()), 0);
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM file WHERE dir_id=?1 AND name='only.txt'",
                [root_dir],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        end(w, run, "completed").unwrap();
    }
}
