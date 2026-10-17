use loomward_catalog::*;
use rusqlite::Connection;

fn setup() -> (tempfile::TempDir, Catalog, i64, i64, i64) {
    let temp = tempfile::tempdir().unwrap();
    let c = Catalog::open(temp.path(), "synthetic").unwrap();
    let WriteReply::Root {
        grant_id, dir_id, ..
    } = c
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "fixture-volume".into(),
            display_name: "Synthetic volume".into(),
            display_path: "Synthetic root".into(),
            root_file_id: Some(vec![7; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 1,
        }))
        .unwrap()
    else {
        panic!()
    };
    let WriteReply::Run(run) = c
        .writer()
        .call(WriteCommand::BeginRun {
            grant_id,
            mode: "full".into(),
            strategy: "fixture".into(),
            started_at_ns: 2,
        })
        .unwrap()
    else {
        panic!()
    };
    (temp, c, grant_id, dir_id, run)
}

fn listing(run: i64, dir: i64, files: Vec<Observation>, dirs: Vec<Observation>) -> WriteCommand {
    WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: dir,
        files,
        dirs,
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    })
}

fn observation_named(name: &str) -> Observation {
    Observation::file(name, 7, Some(7))
}

fn file_count(temp: &tempfile::TempDir) -> i64 {
    Connection::open(temp.path().join("catalog.db"))
        .unwrap()
        .query_row("SELECT count(*) FROM file", [], |r| r.get(0))
        .unwrap()
}

/// (listing_state, listing_outcome) for one directory row.
fn outcome_of(temp: &tempfile::TempDir, dir: i64) -> (String, String) {
    Connection::open(temp.path().join("catalog.db"))
        .unwrap()
        .query_row(
            "SELECT listing_state, listing_outcome FROM dir WHERE id=?1",
            [dir],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

#[test]
fn empty_name_is_refused_as_invalid_observation() {
    let (temp, c, _, root, run) = setup();
    let result = c.writer().call(listing(
        run,
        root,
        vec![Observation::file("", 7, Some(7))],
        vec![],
    ));
    let refused = matches!(&result, Err(Error::Invalid("invalid observation")));
    assert!(refused, "empty name must be refused, got: {result:?}");
    assert_eq!(file_count(&temp), 0);
}

#[test]
fn path_separator_in_name_is_refused_as_invalid_observation() {
    for name in ["a/b", "a\\b"] {
        let (temp, c, _, root, run) = setup();
        let result = c
            .writer()
            .call(listing(run, root, vec![observation_named(name)], vec![]));
        let refused = matches!(&result, Err(Error::Invalid("invalid observation")));
        assert!(refused, "name {name:?} must be refused, got: {result:?}");
        assert_eq!(file_count(&temp), 0);
    }
}

#[test]
fn nul_in_name_is_refused_as_invalid_observation() {
    let (temp, c, _, root, run) = setup();
    let result = c
        .writer()
        .call(listing(run, root, vec![observation_named("a\0b")], vec![]));
    let refused = matches!(&result, Err(Error::Invalid("invalid observation")));
    assert!(refused, "NUL name must be refused, got: {result:?}");
    assert_eq!(file_count(&temp), 0);
}

#[test]
fn file_id_of_9_bytes_is_refused_as_invalid_observation() {
    let (temp, c, _, root, run) = setup();
    let mut bad = observation_named("bad-id");
    bad.file_id = Some(vec![9u8; 9]);
    let result = c.writer().call(listing(run, root, vec![bad], vec![]));
    let refused = matches!(&result, Err(Error::Invalid("invalid observation")));
    assert!(refused, "9-byte file_id must be refused, got: {result:?}");
    assert_eq!(file_count(&temp), 0);
}

#[test]
fn file_id_of_8_bytes_is_accepted() {
    let (temp, c, _, root, run) = setup();
    let mut ok = observation_named("short-id");
    ok.file_id = Some(vec![3u8; 8]);
    c.writer()
        .call(listing(run, root, vec![ok], vec![]))
        .unwrap();
    assert_eq!(
        Connection::open(temp.path().join("catalog.db"))
            .unwrap()
            .query_row("SELECT name FROM file", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "short-id"
    );
}

#[test]
fn allocated_size_above_i64_max_is_refused_as_invalid_observation() {
    let (temp, c, _, root, run) = setup();
    let result = c.writer().call(listing(
        run,
        root,
        vec![Observation::file(
            "huge-alloc",
            7,
            Some((i64::MAX as u64) + 1),
        )],
        vec![],
    ));
    let refused = matches!(&result, Err(Error::Invalid("invalid observation")));
    assert!(
        refused,
        "allocated size above i64::MAX must be refused, got: {result:?}"
    );
    assert_eq!(file_count(&temp), 0);
}

#[test]
fn logical_size_at_i64_max_is_accepted() {
    let (temp, c, _, root, run) = setup();
    c.writer()
        .call(listing(
            run,
            root,
            vec![Observation::file("max-logical", i64::MAX as u64, None)],
            vec![],
        ))
        .unwrap();
    assert_eq!(
        Connection::open(temp.path().join("catalog.db"))
            .unwrap()
            .query_row("SELECT logical FROM file", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        i64::MAX
    );
}

#[test]
fn unknown_family_is_refused_as_invalid_observation() {
    let (temp, c, _, root, run) = setup();
    let mut bad = observation_named("weird");
    bad.family = "invented".into();
    let result = c.writer().call(listing(run, root, vec![bad], vec![]));
    let refused = matches!(&result, Err(Error::Invalid("invalid observation")));
    assert!(refused, "unknown family must be refused, got: {result:?}");
    assert_eq!(file_count(&temp), 0);
}

#[test]
fn flag_bit_11_is_refused_as_invalid_observation() {
    let (temp, c, _, root, run) = setup();
    let mut bad = observation_named("flagged");
    bad.flags = 1 << 11;
    let result = c.writer().call(listing(run, root, vec![bad], vec![]));
    let refused = matches!(&result, Err(Error::Invalid("invalid observation")));
    assert!(refused, "flag bit 11 must be refused, got: {result:?}");
    assert_eq!(file_count(&temp), 0);
}

#[test]
fn all_eleven_known_flag_bits_are_accepted_but_hardlink_suspected_is_derived_not_stored() {
    // HARDLINK_SUSPECTED (bit 2) is derived from the multilink table at finalisation, so an
    // observation cannot assert it; the writer strips it on insert. The other ten bits persist.
    let (temp, c, _, root, run) = setup();
    let mut ok = observation_named("all-flags");
    ok.flags = (1u32 << 11) - 1;
    c.writer()
        .call(listing(run, root, vec![ok], vec![]))
        .unwrap();
    assert_eq!(
        Connection::open(temp.path().join("catalog.db"))
            .unwrap()
            .query_row("SELECT flags FROM file", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        (2047 & !HARDLINK_SUSPECTED) as i64
    );
}

#[test]
fn minimal_valid_observation_is_accepted_with_exact_values() {
    let (temp, c, _, root, run) = setup();
    c.writer()
        .call(listing(
            run,
            root,
            vec![Observation::file("ok", 7, Some(7))],
            vec![],
        ))
        .unwrap();
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    let row: (String, i64) = db
        .query_row("SELECT name, logical FROM file", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(row, ("ok".to_string(), 7));
    assert_eq!(
        db.query_row("SELECT listing_state FROM dir WHERE id=?1", [root], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "complete"
    );
}

#[test]
fn complete_outcome_persists_complete_coverage() {
    let (temp, c, _, root, run) = setup();
    c.writer()
        .call(listing(run, root, vec![observation_named("seed")], vec![]))
        .unwrap();
    c.writer()
        .call(WriteCommand::ListingDone {
            run_id: run,
            dir_id: root,
            outcome: ListingOutcome::Incomplete("partial".into()),
            skipped: 0,
            errors: 0,
        })
        .unwrap();
    assert_eq!(
        outcome_of(&temp, root),
        ("incomplete".to_string(), "partial".to_string())
    );
    c.writer()
        .call(WriteCommand::ListingDone {
            run_id: run,
            dir_id: root,
            outcome: ListingOutcome::Complete,
            skipped: 0,
            errors: 0,
        })
        .unwrap();
    assert_eq!(
        outcome_of(&temp, root),
        ("complete".to_string(), "complete".to_string())
    );
}

#[test]
fn every_non_complete_state_round_trips_as_its_own_coverage_string() {
    for reason in [
        "partial",
        "denied",
        "excluded",
        "unscanned",
        "stale",
        "cancelled",
    ] {
        let (temp, c, _, root, run) = setup();
        c.writer()
            .call(listing(run, root, vec![observation_named("seed")], vec![]))
            .unwrap();
        c.writer()
            .call(WriteCommand::ListingDone {
                run_id: run,
                dir_id: root,
                outcome: ListingOutcome::Incomplete(reason.into()),
                skipped: 0,
                errors: 0,
            })
            .unwrap();
        assert_eq!(
            outcome_of(&temp, root),
            ("incomplete".to_string(), reason.to_string()),
            "reason {reason:?} must round-trip as its own coverage string"
        );
    }
}

#[test]
fn incomplete_claiming_complete_is_refused() {
    let (temp, c, _, root, run) = setup();
    c.writer()
        .call(listing(run, root, vec![observation_named("seed")], vec![]))
        .unwrap();
    c.writer()
        .call(WriteCommand::ListingDone {
            run_id: run,
            dir_id: root,
            outcome: ListingOutcome::Complete,
            skipped: 0,
            errors: 0,
        })
        .unwrap();
    let before = outcome_of(&temp, root);
    assert_eq!(before, ("complete".to_string(), "complete".to_string()));
    let result = c.writer().call(WriteCommand::ListingDone {
        run_id: run,
        dir_id: root,
        outcome: ListingOutcome::Incomplete("complete".into()),
        skipped: 0,
        errors: 0,
    });
    let refused = matches!(&result, Err(Error::Invalid("incomplete listing reason")));
    assert!(
        refused,
        "Incomplete(\"complete\") must be refused, got: {result:?}"
    );
    assert_eq!(outcome_of(&temp, root), before);
}

#[test]
fn incomplete_with_unknown_reason_is_refused() {
    for reason in ["invented", ""] {
        let (temp, c, _, root, run) = setup();
        c.writer()
            .call(listing(run, root, vec![observation_named("seed")], vec![]))
            .unwrap();
        c.writer()
            .call(WriteCommand::ListingDone {
                run_id: run,
                dir_id: root,
                outcome: ListingOutcome::Complete,
                skipped: 0,
                errors: 0,
            })
            .unwrap();
        let result = c.writer().call(WriteCommand::ListingDone {
            run_id: run,
            dir_id: root,
            outcome: ListingOutcome::Incomplete(reason.into()),
            skipped: 0,
            errors: 0,
        });
        let refused = matches!(&result, Err(Error::Invalid("incomplete listing reason")));
        assert!(
            refused,
            "unknown reason {reason:?} must be refused, got: {result:?}"
        );
        assert_eq!(
            outcome_of(&temp, root),
            ("complete".to_string(), "complete".to_string())
        );
    }
}
