use loomward_catalog::{
    EvidenceExtension, EvidenceFacts, EvidenceFamily, EvidenceMedia, EvidenceTime,
};
use loomward_engine::learning::packet_builder::{build, PacketIds};
use loomward_learn::packet::validate_packet;
use serde_json::{json, Value};

fn ids() -> PacketIds {
    PacketIds {
        packet_id: "packet-138".into(),
        group_id: "group-138".into(),
        observed_at: 1000,
        synthetic: true,
    }
}

fn facts(name: &str) -> EvidenceFacts {
    EvidenceFacts {
        name: name.into(),
        files: 3,
        folders: 2,
        logical_bytes: Some(60),
        allocated_bytes: Some(96),
        max_depth: 3,
        families: vec![
            EvidenceFamily {
                family: "code".into(),
                files: 2,
                bytes: Some(30),
            },
            EvidenceFamily {
                family: "executable".into(),
                files: 1,
                bytes: Some(30),
            },
        ],
        extensions: vec![
            EvidenceExtension {
                ext: "rs".into(),
                files: 2,
                bytes: Some(30),
            },
            EvidenceExtension {
                ext: "exe".into(),
                files: 1,
                bytes: Some(30),
            },
        ],
        markers: vec!["cargo_toml", "cargo_target_dir"],
        coverage: "complete".into(),
        allocation_known: true,
        online: true,
        time: EvidenceTime {
            newest_content_write: Some(300),
            oldest_content_write: Some(100),
            newest_source_write: Some(200),
            newest_generated_write: Some(300),
        },
        ..EvidenceFacts::default()
    }
}

#[test]
fn tokens_are_ordered_deduplicated_casefolded_bounded_word_runs_never_raw_names() {
    for (name, expected) in [
        (
            "Old Camera-Tool_2023",
            json!(["old", "camera", "tool", "2023"]),
        ),
        ("../../etc/passwd", json!(["etc", "passwd"])),
        ("Camera CAMERA tool_camera", json!(["camera", "tool"])),
        ("Straße STRASSE", json!(["strasse"])),
    ] {
        let packet = build(&facts(name), ids()).unwrap();
        assert_eq!(packet["subject"]["name_tokens"], expected);
        assert_eq!(packet["subject"]["depth"], 0);
        assert!(packet["subject"].get("name").is_none());
    }
    let name = (0..20)
        .map(|i| format!("word{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let packet = build(&facts(&name), ids()).unwrap();
    assert_eq!(
        packet["subject"]["name_tokens"].as_array().unwrap().len(),
        16
    );
    assert_eq!(packet["subject"]["name_tokens"][15], "word15");
    let name = format!("{} ok", "x".repeat(65));
    assert_eq!(
        build(&facts(&name), ids()).unwrap()["subject"]["name_tokens"],
        json!(["ok"])
    );
}

#[test]
fn rust_photo_partial_and_shielded_facts_validate_and_unknowns_stay_unknown() {
    let rust = facts("project");
    let mut photo = facts("shoot");
    photo.families = vec![EvidenceFamily {
        family: "image".into(),
        files: 3,
        bytes: Some(60),
    }];
    photo.extensions.clear();
    photo.markers = vec!["raw_xmp_pairs", "dcim_layout"];
    photo.media = Some(EvidenceMedia {
        raw_files: 2,
        sidecar_files: 1,
        raw_with_sidecar: 1,
        ..EvidenceMedia::default()
    });
    let mut partial = facts("partial");
    partial.coverage = "partial".into();
    partial.partial = true;
    partial.allocation_known = false;
    partial.logical_bytes = None;
    partial.allocated_bytes = None;
    for family in &mut partial.families {
        family.bytes = None;
    }
    for ext in &mut partial.extensions {
        ext.bytes = None;
    }
    let shielded = EvidenceFacts {
        name: "safe".into(),
        files: 1,
        folders: 0,
        logical_bytes: Some(5),
        allocated_bytes: Some(8),
        coverage: "complete".into(),
        allocation_known: true,
        online: true,
        ..EvidenceFacts::default()
    };
    for input in [&rust, &photo, &partial, &shielded] {
        let packet = build(input, ids()).unwrap();
        validate_packet(&packet).unwrap();
        assert_eq!(packet["time"]["last_access"], "not_evidence");
        for key in ["git_last_commit", "git_dirty"] {
            assert!(packet["time"][key].is_null());
        }
        assert_eq!(
            packet["duplicates"],
            json!({"duplicate_bytes":null,"groups":null})
        );
        assert_eq!(build(input, ids()).unwrap(), packet);
    }
    let packet = build(&partial, ids()).unwrap();
    assert!(packet["shape"]["logical_bytes"].is_null());
    assert!(packet["families"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["share_bytes"].is_null()));
}

#[test]
fn shares_sum_to_at_most_one_and_zero_files_do_not_divide_by_zero() {
    let mut input = facts("rounding");
    input.files = 9;
    input.logical_bytes = Some(9);
    input.allocated_bytes = Some(9);
    input.extensions.clear();
    input.families = loomward_catalog::FAMILIES
        .into_iter()
        .take(9)
        .map(|name| EvidenceFamily {
            family: name.into(),
            files: 1,
            bytes: Some(1),
        })
        .collect();
    let packet = build(&input, ids()).unwrap();
    for share in ["share_files", "share_bytes"] {
        assert!(
            packet["families"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| f[share].as_f64().unwrap())
                .sum::<f64>()
                <= 1.0
        );
    }
    input.files = 0;
    input.logical_bytes = Some(0);
    input.allocated_bytes = Some(0);
    for family in &mut input.families {
        family.files = 0;
        family.bytes = Some(0);
    }
    let packet = build(&input, ids()).unwrap();
    assert!(packet["families"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["share_files"] == 0.0 && f["share_bytes"] == 0.0));
}

#[test]
fn unobserved_coverage_builds_valid_packets_with_nulls() {
    let mut expected = build(&facts("denied"), ids()).unwrap();
    for state in ["denied", "unscanned", "excluded", "cancelled", "unknown"] {
        let mut input = facts("denied");
        input.coverage = state.into();
        for key in ["files", "folders", "logical_bytes", "allocated_bytes"] {
            expected["shape"][key] = Value::Null;
        }
        expected["shape"]["max_depth"] = Value::Null;
        expected["coverage"]["state"] = json!(state);
        expected["coverage"]["allocation_known"] = json!(false);
        expected["coverage"]["link_counts_observed"] = json!(false);
        expected["families"] = json!([]);
        expected["extensions"] = json!([]);
        expected["markers"] = json!([]);
        expected["media"] = Value::Null;
        for key in [
            "newest_content_write",
            "oldest_content_write",
            "newest_source_write",
            "newest_generated_write",
        ] {
            expected["time"][key] = Value::Null;
        }
        validate_packet(&expected).unwrap();
        assert_eq!(build(&input, ids()).unwrap(), expected);
    }
}

#[test]
fn validator_failures_are_returned_without_a_fallback_packet() {
    let mut invalid = ids();
    invalid.packet_id = "../../private".into();
    assert!(build(&facts("valid"), invalid)
        .unwrap_err()
        .contains("packet_id"));
    let mut input = facts("bad_marker");
    input.markers.push("not_in_schema");
    assert!(build(&input, ids()).is_err());
    input = facts("too_many_files");
    input.files = u64::MAX;
    assert!(build(&input, ids()).is_err());
}

#[test]
fn catalogue_snapshot_to_validated_packet_exercises_the_real_sqlite_boundary() {
    use loomward_catalog::{Catalog, RootObservation, WriteCommand, WriteReply};
    use rusqlite::{params, Connection};
    let temp = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(temp.path(), "synthetic").unwrap();
    let WriteReply::Root {
        root_id, dir_id, ..
    } = catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "packet-lab".into(),
            display_name: "Lab".into(),
            display_path: "Synthetic packet lab".into(),
            root_file_id: Some(vec![0; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))
        .unwrap()
    else {
        panic!()
    };
    let db = Connection::open(temp.path().join("catalog.db")).unwrap();
    db.execute(
        "UPDATE dir SET listing_state='complete',sub_complete=1 WHERE id=?1",
        [dir_id],
    )
    .unwrap();
    db.execute("INSERT INTO dir(root_id,parent_id,name,id_quality,id_basis,depth,attrs,flags,listing_state,born_run,seen_run,sub_complete) VALUES(?1,?2,'target','path_observation','none',1,16,0,'complete',1,1,1)",params![root_id,dir_id]).unwrap();
    let target = db.last_insert_rowid();
    db.execute(
        "INSERT INTO ext(ext,family) VALUES('cr3','image'),('xmp','data')",
        [],
    )
    .unwrap();
    for (parent, name, ext, size) in [
        (dir_id, "Cargo.toml", None, 10),
        (target, "generated", None, 20),
        (dir_id, "shot.CR3", Some("cr3"), 30),
        (dir_id, "shot.xmp", Some("xmp"), 5),
    ] {
        db.execute("INSERT INTO file(dir_id,name,ext_id,logical,allocated,attrs,flags,born_run,seen_run) VALUES(?1,?2,(SELECT id FROM ext WHERE ext=?3),?4,?4,32,0,1,1)",params![parent,name,ext,size]).unwrap();
    }
    let mut reader = catalog.reader().unwrap();
    let input = reader.evidence_facts(dir_id).unwrap();
    let packet = build(&input, ids()).unwrap();
    validate_packet(&packet).unwrap();
    assert_eq!(packet["shape"]["files"], 4);
    assert_eq!(packet["shape"]["logical_bytes"], 65);
    assert_eq!(packet["media"]["raw_with_sidecar"], 1);
    assert!(packet["markers"]
        .as_array()
        .unwrap()
        .contains(&json!("cargo_target_dir")));
    db.execute("UPDATE file SET allocated=NULL WHERE name='shot.CR3'", [])
        .unwrap();
    let packet = build(&reader.evidence_facts(dir_id).unwrap(), ids()).unwrap();
    assert!(packet["shape"]["allocated_bytes"].is_null());
    assert!(packet["families"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["share_bytes"].is_null()));
    db.execute(
        "UPDATE dir SET listing_state='incomplete',sub_complete=0 WHERE id=?1",
        [dir_id],
    )
    .unwrap();
    let packet = build(&reader.evidence_facts(dir_id).unwrap(), ids()).unwrap();
    assert_eq!(packet["coverage"]["state"], "partial");
    assert!(packet["shape"]["logical_bytes"].is_null());
    db.execute(
        "UPDATE dir SET listing_state='complete',sub_complete=1 WHERE id=?1",
        [dir_id],
    )
    .unwrap();
    db.execute("UPDATE dir SET attrs=1040 WHERE id=?1", [target])
        .unwrap();
    let packet = build(&reader.evidence_facts(dir_id).unwrap(), ids()).unwrap();
    assert_eq!(packet["shape"]["files"], 3);
    assert!(!packet["markers"]
        .as_array()
        .unwrap()
        .contains(&json!("cargo_target_dir")));
}
