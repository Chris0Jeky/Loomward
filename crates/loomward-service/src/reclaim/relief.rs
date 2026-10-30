//! #1013 (docs/52 section 6.3): the relief check. The owner records a volume sample before a
//! cleanup and another after it; the view shows the free-space change between them beside the
//! plan's estimate. Metadata only: no file or process effect, and the change is never attributed
//! to the cleanup.
use super::*;
use serde_json::Value;

/// One stored sample. `None` is unknown, never zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Sample {
    /// The catalogue's volume identity plus the granted root's volume GUID where it was recorded.
    pub key: String,
    pub guid: Option<String>,
    pub at_ns: i64,
    pub capacity: Option<u64>,
    pub free: Option<u64>,
    pub bin: Option<u64>,
    pub estimate: Option<u64>,
}

/// The observed change between two samples, or why there is none. Samples of different volume
/// identities never compare (a drive letter reused by another disk), and an unreadable free count
/// on either side is unknown rather than zero.
pub(super) fn compare(
    before: &Sample,
    after: Option<&Sample>,
) -> (ReliefStatus, Option<ReliefDirection>, Option<u64>) {
    let Some(after) = after else {
        return (ReliefStatus::NotChecked, None, None);
    };
    if before.key != after.key || before.guid != after.guid {
        return (ReliefStatus::DifferentVolume, None, None);
    }
    let (Some(was), Some(now)) = (before.free, after.free) else {
        return (ReliefStatus::Unreadable, None, None);
    };
    let direction = match now.cmp(&was) {
        std::cmp::Ordering::Greater => ReliefDirection::MoreFree,
        std::cmp::Ordering::Less => ReliefDirection::LessFree,
        std::cmp::Ordering::Equal => ReliefDirection::Unchanged,
    };
    (
        ReliefStatus::Compared,
        Some(direction),
        Some(now.abs_diff(was)),
    )
}

fn wire(s: &Sample) -> Value {
    let bytes = |b: Option<u64>| b.map(Bytes);
    json!({"sampled_at": crate::ts_ns(s.at_ns), "capacity_bytes": bytes(s.capacity),
        "free_bytes": bytes(s.free), "recycle_bin_bytes": bytes(s.bin), "estimate_bytes": bytes(s.estimate)})
}

/// The relief block for a volume: absent until a before sample exists.
pub(super) fn relief(
    conn: &Connection,
    instance: &str,
    volume: i64,
) -> Result<Option<Value>, ErrorBody> {
    let read = |role: &str| -> Result<Option<Sample>, ErrorBody> {
        let number = |s: Option<String>| s.and_then(|s| s.parse().ok());
        conn.query_row(
            "SELECT volume_key,volume_guid,sampled_at_ns,capacity_bytes,free_bytes,recycle_bin_bytes,estimate_bytes
             FROM st.reclaim_sample WHERE volume_id=?1 AND role=?2 AND catalog_instance=?3",
            params![volume, role, instance],
            |r| {
                Ok(Sample {
                    key: r.get(0)?,
                    guid: r.get(1)?,
                    at_ns: r.get(2)?,
                    capacity: number(r.get(3)?),
                    free: number(r.get(4)?),
                    bin: number(r.get(5)?),
                    estimate: number(r.get(6)?),
                })
            },
        )
        .optional()
        .map_err(sql_error)
    };
    let Some(before) = read("before")? else {
        return Ok(None);
    };
    let after = read("after")?;
    let (status, direction, change) = compare(&before, after.as_ref());
    Ok(Some(
        json!({"before": wire(&before), "after": after.as_ref().map(wire),
        "status": status, "direction": direction, "change_bytes": change.map(Bytes)}),
    ))
}

impl Inner {
    /// Records one sample from the volume's current metadata and answers the relief block.
    pub(crate) fn reclaim_sample(&self, req: &RequestEnvelope) -> Result<Out, ErrorBody> {
        let r: ReclaimSampleRequest = req.decode_payload()?;
        if r.role == SampleRole::After && r.estimate_bytes.is_some() {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "an estimate belongs with the before sample",
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let input = Input::open(self, deadline, None)?;
        let volume_row = self.volume_row(r.volume_id.as_str())?;
        let key = db::volumes(&input.conn, None)
            .map_err(sql_error)?
            .into_iter()
            .find(|v| v.id == volume_row)
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown volume"))?
            .key;
        let observed = input
            .volumes(deadline, true, false)?
            .into_iter()
            .find(|v| v.volume.volume_id.as_str() == format!("vo_{volume_row}"))
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown volume"))?
            .volume;
        let roots = active_roots(&input.conn, volume_row)?;
        // Offline also covers a foreign disk on the root's letter (#910): it is not this volume.
        if !observed.online {
            return Err(with_reason(
                fail(ErrorCode::DeviceOffline, "the volume is offline"),
                "volume_offline",
            ));
        }
        let guid = input
            .conn
            .query_row(
                "SELECT volume_guid FROM st.root_grant WHERE id=?1",
                [roots[0].grant],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(sql_error)?
            .flatten();
        // The bin is per volume, so it is read only through a volume-root grant; otherwise unknown.
        let bin = roots.iter().find_map(|root| {
            let path = Path::new(&root.display_path);
            (path.is_absolute() && path.parent().is_none())
                .then(|| loomward_windows::recycle_bin::query(path))
                .flatten()
        });
        let sample = Sample {
            key,
            guid,
            at_ns: crate::now_ns(),
            capacity: observed.capacity_bytes.map(Bytes::get),
            free: observed.free_bytes.map(Bytes::get),
            bin: bin.map(|b| b.bytes),
            estimate: r.estimate_bytes.map(Bytes::get),
        };
        drop(input);
        let instance = self.instance.as_str();
        let _m = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = self.db();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let role = if r.role == SampleRole::Before {
            "before"
        } else {
            "after"
        };
        if r.role == SampleRole::After
            && tx
                .query_row(
                    "SELECT 1 FROM st.reclaim_sample WHERE volume_id=?1 AND role='before' AND catalog_instance=?2",
                    params![volume_row, instance],
                    |_| Ok(()),
                )
                .optional()
                .map_err(sql_error)?
                .is_none()
        {
            return Err(with_reason(
                fail(ErrorCode::InvalidRequest, "record a before sample first"),
                "no_before_sample",
            ));
        }
        if r.role == SampleRole::Before {
            // A new before sample starts a new check: the old after sample no longer belongs to it.
            tx.execute(
                "DELETE FROM st.reclaim_sample WHERE volume_id=?1",
                [volume_row],
            )
            .map_err(sql_error)?;
        }
        let text = |b: Option<u64>| b.map(|b| b.to_string());
        tx.execute(
            "INSERT OR REPLACE INTO st.reclaim_sample(volume_id,role,catalog_instance,volume_key,volume_guid,sampled_at_ns,capacity_bytes,free_bytes,recycle_bin_bytes,estimate_bytes)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![volume_row, role, instance, sample.key, sample.guid, sample.at_ns,
                text(sample.capacity), text(sample.free), text(sample.bin), text(sample.estimate)],
        )
        .map_err(sql_error)?;
        let result = relief(&tx, instance, volume_row)?
            .ok_or_else(|| fail(ErrorCode::InternalError, "sample not stored"))?;
        tx.commit().map_err(sql_error)?;
        drop(conn);
        let dto: ReclaimRelief = serde_json::from_value(result)
            .map_err(|_| fail(ErrorCode::InternalError, "encode relief check"))?;
        Ok(Out {
            result: serde_json::to_value(dto)
                .map_err(|_| fail(ErrorCode::InternalError, "encode relief check"))?,
            catalog_rev: None,
            state_rev: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(key: &str, guid: Option<&str>, free: Option<u64>) -> Sample {
        Sample {
            key: key.into(),
            guid: guid.map(Into::into),
            at_ns: 0,
            capacity: Some(1000),
            free,
            bin: None,
            estimate: None,
        }
    }

    #[test]
    fn more_free_less_free_and_unchanged_are_reported_as_such() {
        let before = sample("vsn_1", Some("g"), Some(400));
        for (now, direction, change) in [
            (650, ReliefDirection::MoreFree, 250),
            (100, ReliefDirection::LessFree, 300),
            (400, ReliefDirection::Unchanged, 0),
        ] {
            let after = sample("vsn_1", Some("g"), Some(now));
            assert_eq!(
                compare(&before, Some(&after)),
                (ReliefStatus::Compared, Some(direction), Some(change))
            );
        }
    }

    #[test]
    fn a_different_guid_or_key_never_compares() {
        let before = sample("vsn_1", Some("g1"), Some(400));
        for other in [
            sample("vsn_1", Some("g2"), Some(900)),
            sample("vsn_1", None, Some(900)),
            sample("vsn_2", Some("g1"), Some(900)),
        ] {
            assert_eq!(
                compare(&before, Some(&other)),
                (ReliefStatus::DifferentVolume, None, None)
            );
        }
        let unrecorded = sample("vsn_1", None, Some(400));
        assert_eq!(
            compare(&unrecorded, Some(&sample("vsn_1", None, Some(401)))).0,
            ReliefStatus::Compared,
            "an unrecorded GUID on both sides still binds on the catalogue's volume key"
        );
    }

    #[test]
    fn unreadable_free_space_is_unknown_not_zero() {
        let known = sample("vsn_1", Some("g"), Some(400));
        let unknown = sample("vsn_1", Some("g"), None);
        assert_eq!(
            compare(&known, Some(&unknown)),
            (ReliefStatus::Unreadable, None, None)
        );
        assert_eq!(
            compare(&unknown, Some(&known)),
            (ReliefStatus::Unreadable, None, None)
        );
    }

    #[test]
    fn a_before_sample_alone_is_not_checked() {
        assert_eq!(
            compare(&sample("vsn_1", Some("g"), Some(400)), None),
            (ReliefStatus::NotChecked, None, None)
        );
    }

    #[test]
    fn the_largest_difference_does_not_overflow() {
        let before = sample("vsn_1", None, Some(0));
        let after = sample("vsn_1", None, Some(u64::MAX));
        assert_eq!(
            compare(&before, Some(&after)).2,
            Some(u64::MAX),
            "an unsigned difference, not a signed subtraction"
        );
    }
}
