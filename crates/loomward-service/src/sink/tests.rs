use super::*;
use crate::tests::{entry, open, register, temp_root};
use loomward_protocol::DatasetClass;
use std::sync::mpsc;

fn fixture(svc: &crate::Service) -> (RootId, ListingTicket, i64) {
    let (root, grant) = register(svc, 17);
    let dir = db::active_root_dir(&svc.inner.db(), root).unwrap().unwrap();
    let id = RootId::new(svc.inner.root_id(root)).unwrap();
    let sink = &svc.inner.scan;
    sink.bind(&id, grant, dir);
    sink.begin_run(&id, 1, &RunScope::FullRoot).unwrap();
    let ticket = sink
        .prepare_listing(
            &id,
            1,
            None,
            &[],
            OpenedIdentity {
                id: None,
                basis: IdBasis::None,
                volume_serial: None,
                attributes: 0x10,
                reparse_tag: None,
            },
        )
        .unwrap();
    (id, ticket, grant)
}

fn chunk(ticket: ListingTicket, n: usize) -> ScanMessage {
    ScanMessage::DirListing {
        run: 1,
        ticket,
        entries: vec![entry(&format!("synthetic-{n}.bin"), 1, false)],
    }
}

fn done(ticket: ListingTicket, outcome: ListOutcome) -> ScanMessage {
    ScanMessage::ListingDone {
        run: 1,
        ticket,
        outcome,
    }
}

fn count(conn: &rusqlite::Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM main.{table}"), [], |r| {
        r.get(0)
    })
    .unwrap()
}

fn run_state(svc: &crate::Service) -> (String, Option<i64>, Option<i64>) {
    svc.inner.db().query_row(
        "SELECT s.state,r.active_run,r.generation FROM main.scan_run s JOIN main.root r ON r.id=s.root_id",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap()
}

#[test]
fn eight_in_flight_chunks_bound_receipts_and_reserved_memory() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (id, ticket, _) = fixture(&svc);
    let sink = &svc.inner.scan;
    let conn = rusqlite::Connection::open(tmp.path().join("catalog.db")).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    std::thread::scope(|scope| {
        let (tx, rx) = mpsc::channel();
        let id = &id;
        let scan = scope.spawn(move || {
            for n in 0..9 {
                tx.send(sink.consume(id, chunk(ticket, n))).unwrap();
            }
        });
        let mut received = 0;
        for _ in 0..8 {
            if matches!(rx.recv_timeout(Duration::from_millis(200)), Ok(Ok(()))) {
                received += 1;
            } else {
                break;
            }
        }
        let ninth = rx.recv_timeout(Duration::from_millis(100));
        let reserved = svc.inner.catalog.writer().queued_bytes();
        let pending = sink.state.lock().unwrap().roots[id].pending.len();
        conn.execute_batch("ROLLBACK").unwrap();
        scan.join().unwrap();
        assert_eq!(received, 8, "chunks must queue ahead of COMMIT");
        assert!(ninth.is_err(), "the ninth chunk bypassed the receipt bound");
        // drain removes the oldest receipt while it waits: seven are retained plus one waiter.
        assert!(pending <= 8);
        assert_eq!(
            reserved,
            32 * 1024 * 1024,
            "reserve before conversion, not after it"
        );
        rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    });
    sink.consume(&id, done(ticket, ListOutcome::Complete))
        .unwrap();
    sink.finish_run(&id, 1, &RunScope::FullRoot, true).unwrap();
    drop(conn);
    assert_eq!(count(&svc.inner.db(), "file"), 9);
    assert_eq!(svc.inner.catalog.writer().queued_bytes(), 0);
    assert_eq!(run_state(&svc).0, "completed");
}

#[test]
fn committed_receipts_still_count_towards_the_in_flight_bound() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (id, ticket, _) = fixture(&svc);
    let sink = &svc.inner.scan;
    for n in 0..65 {
        sink.consume(&id, chunk(ticket, n)).unwrap();
        svc.inner
            .catalog
            .writer()
            .call(WriteCommand::Barrier)
            .unwrap();
        assert!(
            sink.state.lock().unwrap().roots[&id].pending.len() <= 8,
            "completed receipts accumulated without consuming the bound"
        );
    }
    sink.consume(&id, done(ticket, ListOutcome::Complete))
        .unwrap();
    sink.finish_run(&id, 1, &RunScope::FullRoot, true).unwrap();
    assert_eq!(count(&svc.inner.db(), "file"), 65);
}

#[test]
fn listing_done_waits_for_chunks_and_its_single_committed_publication() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (id, ticket, _) = fixture(&svc);
    let sink = &svc.inner.scan;
    let conn = rusqlite::Connection::open(tmp.path().join("catalog.db")).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    for n in 0..3 {
        sink.consume(&id, chunk(ticket, n)).unwrap();
    }
    assert_eq!(count(&conn, "file"), 0);
    std::thread::scope(|scope| {
        let (tx, rx) = mpsc::channel();
        let id = &id;
        let publication = scope.spawn(move || {
            tx.send(sink.consume(id, done(ticket, ListOutcome::Complete)))
                .unwrap();
        });
        let early = rx.recv_timeout(Duration::from_millis(100));
        conn.execute_batch("ROLLBACK").unwrap();
        publication.join().unwrap();
        assert!(
            early.is_err(),
            "ListingDone acknowledged uncommitted chunks"
        );
        rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    });
    assert_eq!(
        count(&conn, "file"),
        3,
        "a separate SQLite reader sees all chunks after the receipt"
    );
    assert_eq!(count(&conn, "stage_entry"), 0);
    assert_eq!(svc.inner.catalog.writer().timings().listing_calls, 1);
    drop(conn);
    sink.finish_run(&id, 1, &RunScope::FullRoot, true).unwrap();
    assert!(run_state(&svc).2.is_some());
}

#[test]
fn queued_chunk_failure_refuses_complete_listing_and_fails_the_run() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (id, ticket, _) = fixture(&svc);
    svc.inner
        .db()
        .execute_batch(
            "CREATE TRIGGER main.fail_chunk BEFORE INSERT ON stage_entry
        WHEN NEW.seq=1 BEGIN SELECT RAISE(ABORT,'injected chunk failure'); END;",
        )
        .unwrap();
    let sink = &svc.inner.scan;
    for n in 0..3 {
        sink.consume(&id, chunk(ticket, n)).unwrap();
    }
    assert_eq!(sink.state.lock().unwrap().roots[&id].pending.len(), 3);
    assert!(matches!(
        sink.consume(&id, done(ticket, ListOutcome::Complete)),
        Err(EngineError::Internal { .. })
    ));
    let state: String = svc
        .inner
        .db()
        .query_row(
            "SELECT listing_state FROM main.dir WHERE id=?1",
            [ticket.dir as i64],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "incomplete");
    assert_eq!(
        count(&svc.inner.db(), "file"),
        1,
        "only the accepted prefix publishes, upsert-only"
    );
    assert!(sink.finish_run(&id, 1, &RunScope::FullRoot, true).is_err());
    assert_eq!(run_state(&svc), ("failed".into(), None, None));
    assert_eq!(count(&svc.inner.db(), "stage_entry"), 0);
    sink.begin_run(&id, 2, &RunScope::FullRoot).unwrap();
}

#[test]
fn finals_and_end_run_observe_queued_failure_without_listing_done() {
    for final_first in [false, true] {
        let tmp = temp_root();
        let svc = open(tmp.path(), DatasetClass::Synthetic);
        let (id, ticket, _) = fixture(&svc);
        svc.inner
            .db()
            .execute_batch(
                "CREATE TRIGGER main.fail_chunk BEFORE INSERT ON stage_entry
            BEGIN SELECT RAISE(ABORT,'injected chunk failure'); END;",
            )
            .unwrap();
        let sink = &svc.inner.scan;
        sink.consume(&id, chunk(ticket, 0)).unwrap();
        if final_first {
            assert!(sink
                .consume(
                    &id,
                    ScanMessage::DirFinal {
                        run: 1,
                        ticket,
                        sums: loomward_engine::scan::Sums::ZERO,
                        complete: true
                    }
                )
                .is_err());
        }
        assert!(sink.finish_run(&id, 1, &RunScope::FullRoot, true).is_err());
        assert_eq!(run_state(&svc), ("failed".into(), None, None));
        assert_eq!(count(&svc.inner.db(), "file"), 0);
    }
}

#[test]
fn cancellation_with_chunks_in_flight_keeps_the_listing_and_run_partial() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (id, ticket, _) = fixture(&svc);
    let sink = &svc.inner.scan;
    let conn = rusqlite::Connection::open(tmp.path().join("catalog.db")).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    for n in 0..3 {
        sink.consume(&id, chunk(ticket, n)).unwrap();
    }
    assert_eq!(sink.state.lock().unwrap().roots[&id].pending.len(), 3);
    conn.execute_batch("ROLLBACK").unwrap();
    drop(conn);
    sink.consume(
        &id,
        done(ticket, ListOutcome::Incomplete(IncompleteReason::Cancelled)),
    )
    .unwrap();
    sink.finish_run(&id, 1, &RunScope::FullRoot, false).unwrap();
    assert_eq!(run_state(&svc), ("cancelled".into(), None, None));
    assert_eq!(count(&svc.inner.db(), "file"), 3);
    assert_eq!(count(&svc.inner.db(), "stage_entry"), 0);
    assert_eq!(svc.inner.catalog.writer().queued_bytes(), 0);
    let state: String = svc
        .inner
        .db()
        .query_row(
            "SELECT listing_state FROM main.dir WHERE id=?1",
            [ticket.dir as i64],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "incomplete");
}

#[test]
fn durable_revocation_fences_chunks_in_flight_and_releases_reservations() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (id, ticket, grant) = fixture(&svc);
    let sink = &svc.inner.scan;
    let conn = rusqlite::Connection::open(tmp.path().join("catalog.db")).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    for n in 0..3 {
        sink.consume(&id, chunk(ticket, n)).unwrap();
    }
    assert_eq!(sink.state.lock().unwrap().roots[&id].pending.len(), 3);
    conn.execute_batch("ROLLBACK").unwrap();
    drop(conn);
    svc.inner.revoke(grant).unwrap();
    assert!(sink.state.lock().unwrap().roots[&id].cancel.is_cancelled());
    let staged_at_revoke = count(&svc.inner.db(), "stage_entry");
    for message in [chunk(ticket, 3), done(ticket, ListOutcome::Complete)] {
        assert!(matches!(
            sink.consume(&id, message),
            Err(EngineError::PermissionDenied { .. })
        ));
    }
    assert!(sink.finish_run(&id, 1, &RunScope::FullRoot, true).is_err());
    assert_eq!(run_state(&svc), ("cancelled".into(), None, None));
    assert_eq!(count(&svc.inner.db(), "file"), 0);
    assert_eq!(count(&svc.inner.db(), "stage_entry"), staged_at_revoke);
    assert_eq!(svc.inner.catalog.writer().queued_bytes(), 0);
    assert!(sink.state.lock().unwrap().roots[&id].pending.is_empty());
}

#[test]
fn converted_engine_chunk_fits_the_preconstruction_reservation() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (id, ticket, _) = fixture(&svc);
    let entries = (0..256)
        .map(|n| {
            let mut e = entry("synthetic.bin", 1, false);
            e.name = vec![0xd800; 250];
            e.name.extend(format!("{n:03}.bin").encode_utf16());
            e.file_id = Some(FileIdObs::Id128([1; 16]));
            e.identity_eligible = true;
            e
        })
        .collect();
    let sink = &svc.inner.scan;
    sink.consume(
        &id,
        ScanMessage::DirListing {
            run: 1,
            ticket,
            entries,
        },
    )
    .unwrap();
    assert_eq!(sink.state.lock().unwrap().roots[&id].pending.len(), 1);
    sink.consume(&id, done(ticket, ListOutcome::Complete))
        .unwrap();
    sink.finish_run(&id, 1, &RunScope::FullRoot, true).unwrap();
    assert_eq!(count(&svc.inner.db(), "file"), 256);
}

#[test]
fn staging_returns_before_the_writer_can_commit() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (id, ticket, _) = fixture(&svc);
    let conn = rusqlite::Connection::open(tmp.path().join("catalog.db")).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    std::thread::scope(|scope| {
        let (tx, rx) = mpsc::channel();
        let sink = &svc.inner.scan;
        let scan = scope.spawn(move || {
            tx.send(sink.consume(&id, chunk(ticket, 0))).unwrap();
        });
        let early = rx.recv_timeout(Duration::from_millis(200));
        conn.execute_batch("ROLLBACK").unwrap();
        scan.join().unwrap();
        assert!(
            early.is_ok(),
            "staging waited for COMMIT instead of queuing its receipt"
        );
        early.unwrap().unwrap();
    });
}
