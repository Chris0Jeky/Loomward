use super::*;

struct StateWrite {
    conn: Connection,
    before: &'static str,
    result: Option<rusqlite::Result<usize>>,
}

thread_local! {
    // A trace is connection-local; thread-local storage keeps parallel tests independent.
    static STATE_WRITE: std::cell::RefCell<Option<StateWrite>> = const { std::cell::RefCell::new(None) };
}

const STATE_UPDATE: &str = "UPDATE revision SET state_rev=state_rev+1 WHERE id=1";

fn write_state_before_upgrade(event: rusqlite::trace::TraceEvent<'_>) {
    if let rusqlite::trace::TraceEvent::Stmt(_, sql) = event {
        STATE_WRITE.with_borrow_mut(|probe| {
            let probe = probe.as_mut().unwrap();
            if probe.result.is_none() && sql.starts_with(probe.before) {
                // The preceding SELECT has completed. A separate WAL writer can invalidate
                // that snapshot under DEFERRED, but must be refused under IMMEDIATE.
                probe.result = Some(probe.conn.execute(STATE_UPDATE, []));
            }
        });
    }
}

fn with_state_write<T>(
    conn: &mut Connection,
    dir: &std::path::Path,
    before: &'static str,
    operation: impl FnOnce(&mut Connection) -> Result<T>,
) -> T {
    let other = Connection::open(dir.join("state.db")).unwrap();
    other.busy_timeout(Duration::ZERO).unwrap();
    STATE_WRITE.with_borrow_mut(|probe| {
        assert!(probe.is_none());
        *probe = Some(StateWrite {
            conn: other,
            before,
            result: None,
        });
    });
    conn.trace_v2(
        rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
        Some(write_state_before_upgrade),
    );
    let result = operation(conn);
    conn.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
    let probe = STATE_WRITE.with_borrow_mut(|probe| probe.take().unwrap());
    let attempt = probe.result.expect("concurrent state write was attempted");
    let value = result.expect("writer must survive the concurrent state write");
    let error = attempt.expect_err("state write must wait until the writer commits");
    assert_eq!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy)
    );
    // The lock is released by COMMIT, so the contender can finish without a retry loop.
    assert_eq!(probe.conn.execute(STATE_UPDATE, []).unwrap(), 1);
    value
}

#[test]
fn retirement_survives_a_concurrent_state_write_after_its_token_read() {
    let temp = tempfile::tempdir().unwrap();
    let mut conn = crate::db::open_pair(temp.path(), "synthetic").unwrap();
    conn.execute("INSERT INTO st.object_ref(volume_key,file_id,creation_ft,kind,state,last_resolved_ns,catalog_instance,row_id,born_run) VALUES('fixture',zeroblob(16),1,'file','resolved',1,'fixture-instance',1,1)", []).unwrap();
    with_state_write(
        &mut conn,
        temp.path(),
        "INSERT OR REPLACE INTO st.pending_reference",
        |conn| retire_selected(conn, "SELECT id FROM st.object_ref", []),
    );
    let pending: (i64, String, i64) = conn.query_row(
        "SELECT object_ref_id,catalog_instance,length(publication_token) FROM st.pending_reference",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    assert_eq!(pending, (1, "fixture-instance".into(), 32));
}

fn root_observation() -> RootObservation {
    RootObservation {
        volume_guid: Some("fixture-guid".into()),
        volume_key: "fixture".into(),
        display_name: "Synthetic root".into(),
        display_path: "Synthetic root".into(),
        root_file_id: Some(vec![1; 16]),
        filesystem: Some("NTFS".into()),
        origin: "fixture".into(),
        granted_via: "fixture".into(),
        observed_at_ns: 1,
    }
}

#[test]
fn lab_registration_survives_a_concurrent_state_write_after_its_provenance_read() {
    let temp = tempfile::tempdir().unwrap();
    let mut conn = crate::db::open_pair(temp.path(), "synthetic").unwrap();
    conn.execute("INSERT INTO st.lab_root(volume_key,root_file_id,manifest_digest,seed,tier,registered_at_ns,volume_guid) VALUES('fixture',?1,'synthetic-digest','seed','small',0,'fixture-guid')", [vec![1u8; 16]]).unwrap();
    let mut observation = root_observation();
    observation.origin = "lab_generated".into();
    with_state_write(
        &mut conn,
        temp.path(),
        "INSERT INTO st.root_grant",
        |conn| {
            prepare_state(
                conn,
                &WriteCommand::RegisterRoot(observation),
                &HashSet::new(),
                &HashMap::new(),
            )
        },
    );
    assert_eq!(
        conn.query_row("SELECT lab_root_id FROM st.root_grant", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn root_refresh_survives_a_concurrent_state_write_after_its_grant_read() {
    let temp = tempfile::tempdir().unwrap();
    let mut conn = crate::db::open_pair(temp.path(), "synthetic").unwrap();
    let mut initial = root_observation();
    initial.filesystem = None;
    prepare_state(
        &mut conn,
        &WriteCommand::RegisterRoot(initial),
        &HashSet::new(),
        &HashMap::new(),
    )
    .unwrap();
    with_state_write(&mut conn, temp.path(), "UPDATE st.root_grant", |conn| {
        prepare_state(
            conn,
            &WriteCommand::RefreshRoot {
                grant_id: 1,
                observation: root_observation(),
            },
            &HashSet::new(),
            &HashMap::new(),
        )
    });
    assert_eq!(
        conn.query_row("SELECT filesystem FROM st.root_grant", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "NTFS"
    );
}

#[test]
fn identity_change_survives_a_concurrent_state_write_after_its_grant_read() {
    let temp = tempfile::tempdir().unwrap();
    let mut conn = crate::db::open_pair(temp.path(), "synthetic").unwrap();
    prepare_state(
        &mut conn,
        &WriteCommand::RegisterRoot(root_observation()),
        &HashSet::new(),
        &HashMap::new(),
    )
    .unwrap();
    with_state_write(&mut conn, temp.path(), "UPDATE st.root_grant", |conn| {
        prepare_state(
            conn,
            &WriteCommand::GrantIdentityChanged { grant_id: 1 },
            &HashSet::new(),
            &HashMap::new(),
        )
    });
    assert_eq!(
        conn.query_row("SELECT state FROM st.root_grant", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "identity_changed"
    );
}
