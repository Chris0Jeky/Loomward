use loomward_learn::packet::{validate_corpus, validate_packet, validate_record};
use serde_json::{json, Value};

fn corpus() -> Value {
    serde_json::from_str(include_str!(
        "../../../fixtures/categoriser-v1/records.json"
    ))
    .unwrap()
}

fn record() -> Value {
    corpus()[0].clone()
}

fn packet() -> Value {
    record()["packet"].clone()
}

fn reject_packet(pointer: &str, replacement: Value, expected: &str) {
    let mut packet = packet();
    *packet.pointer_mut(pointer).unwrap() = replacement;
    assert_eq!(validate_packet(&packet).unwrap_err().message(), expected);
}

fn reject_record(pointer: &str, replacement: Value, expected: &str) {
    let mut record = record();
    *record.pointer_mut(pointer).unwrap() = replacement;
    assert_eq!(validate_record(&record).unwrap_err().message(), expected);
}

#[test]
fn synthetic_fixtures_validate_individually_and_as_a_corpus() {
    let corpus = corpus();
    assert_eq!(validate_corpus(&corpus).unwrap(), 12);
    for record in corpus.as_array().unwrap() {
        validate_packet(&record["packet"]).unwrap();
        validate_record(record).unwrap();
    }
}

#[test]
fn every_object_refuses_unknown_and_missing_keys() {
    let record = corpus()[6].clone(); // Includes the non-null media object.
    for (pointer, name) in [
        ("", "record"),
        ("/packet", "packet"),
        ("/packet/subject", "subject"),
        ("/packet/shape", "shape"),
        ("/packet/families/0", "family"),
        ("/packet/extensions/0", "extension"),
        ("/packet/time", "time"),
        ("/packet/duplicates", "duplicates"),
        ("/packet/media", "media"),
        ("/packet/coverage", "coverage"),
        ("/human_labels/0", "label"),
    ] {
        let mut unknown = record.clone();
        unknown
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unexpected".into(), json!(true));
        let expected = format!("Invalid keys for {name}");
        assert_eq!(validate_record(&unknown).unwrap_err().message(), expected);
        for key in record.pointer(pointer).unwrap().as_object().unwrap().keys() {
            let mut missing = record.clone();
            missing
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
            assert_eq!(
                validate_record(&missing).unwrap_err().message(),
                expected,
                "{pointer}/{key}"
            );
        }
        let mut wrong_type = record.clone();
        *wrong_type.pointer_mut(pointer).unwrap() = json!([]);
        assert_eq!(
            validate_record(&wrong_type).unwrap_err().message(),
            expected
        );
    }
    reject_record(
        "/teacher_labels",
        json!([{"axis": "meaning", "label": "archive", "extra": 0}]),
        "Invalid keys for label",
    );
}

#[test]
fn schema_constants_are_enforced() {
    reject_packet(
        "/schema",
        json!("loomward.evidence-packet.v2"),
        "Invalid packet schema",
    );
    reject_record(
        "/schema",
        json!("loomward.categoriser-record.v2"),
        "Invalid record schema",
    );
}

#[test]
fn all_closed_vocabularies_refuse_unknown_values() {
    for (pointer, expected) in [
        ("/subject/kind", "Invalid subject.kind"),
        ("/families/0/family", "Invalid family"),
        ("/markers/0", "Invalid marker"),
        ("/coverage/state", "Invalid coverage.state"),
    ] {
        reject_packet(pointer, json!("invented"), expected);
        reject_packet(pointer, json!(null), expected);
    }
    reject_record(
        "/human_labels/0/axis",
        json!("approval"),
        "Invalid label axis",
    );
    for index in 0..3 {
        reject_record(
            &format!("/human_labels/{index}/label"),
            json!("invented"),
            "Invalid label for axis",
        );
    }
    reject_record(
        "/teacher_labels",
        json!([{"axis": "meaning", "label": "active"}]),
        "Invalid label for axis",
    );
}

#[test]
fn identifiers_are_bounded_ascii_patterns() {
    for invalid in [
        "".to_owned(),
        "a".repeat(65),
        "Bad".into(),
        "-bad".into(),
        "bad/id".into(),
        "café".into(),
    ] {
        reject_packet("/packet_id", json!(invalid), "Invalid packet_id");
        reject_packet("/group_id", json!(invalid), "Invalid group_id");
        reject_record("/record_id", json!(invalid), "Invalid record_id");
    }
    let mut packet = packet();
    packet["packet_id"] = json!("a".repeat(64));
    packet["group_id"] = json!("0.group-_id");
    validate_packet(&packet).unwrap();
}

#[test]
fn path_like_tokens_and_extensions_are_refused() {
    for token in [
        "folder/name",
        "folder\\name",
        "c:relative",
        "c:\\folder",
        "\\\\host\\share",
        "//host/share",
        "bad\0token",
    ] {
        reject_packet(
            "/subject/name_tokens/0",
            json!(token),
            "Invalid path-like name_token",
        );
        reject_packet("/extensions/0/ext", json!(token), "Invalid path-like ext");
    }
}

#[test]
fn tokens_must_already_use_full_unicode_casefold() {
    for token in ["Upper", "Straße", "ς"] {
        reject_packet(
            "/subject/name_tokens/0",
            json!(token),
            "name_tokens must already be casefolded",
        );
    }
    let mut packet = packet();
    packet["subject"]["name_tokens"] = json!(["strasse", "σ", "é".repeat(64)]);
    validate_packet(&packet).unwrap();
}

#[test]
fn token_lengths_and_extension_pattern_are_enforced() {
    for invalid in [json!(""), json!("x".repeat(65)), json!(123), Value::Null] {
        reject_packet("/subject/name_tokens/0", invalid, "Invalid name_token");
    }
    for invalid in [
        json!("RS"),
        json!(".rs"),
        json!("é"),
        json!("a".repeat(33)),
        json!(true),
    ] {
        reject_packet("/extensions/0/ext", invalid, "Invalid ext");
    }
    let mut packet = packet();
    packet["extensions"][0]["ext"] = json!("a0_+-".repeat(6));
    validate_packet(&packet).unwrap();
    packet["extensions"][0]["ext"] = json!("");
    validate_packet(&packet).unwrap();
}

#[test]
fn duplicate_family_is_refused() {
    let mut packet = packet();
    packet["families"][1]["family"] = packet["families"][0]["family"].clone();
    assert_eq!(
        validate_packet(&packet).unwrap_err().message(),
        "Duplicate family"
    );
}

#[test]
fn family_file_share_sum_is_bounded() {
    reject_packet(
        "/families/1/share_files",
        json!(0.2),
        "Family share_files sum exceeds 1.000001",
    );
}

#[test]
fn family_byte_share_sum_is_bounded() {
    reject_packet(
        "/families/1/share_bytes",
        json!(0.1),
        "Family share_bytes sum exceeds 1.000001",
    );
}

#[test]
fn family_share_rounding_tolerance_and_unknown_bytes_are_accepted() {
    let mut packet = packet();
    packet["families"] = json!([
        {"family": "code", "share_files": 0.5, "share_bytes": null},
        {"family": "other", "share_files": 0.5000005, "share_bytes": 1.0}
    ]);
    validate_packet(&packet).unwrap();
    packet["families"][0]["share_bytes"] = json!(0.0000005);
    validate_packet(&packet).unwrap();
}

#[test]
fn individual_shares_are_numbers_in_the_unit_interval() {
    for field in ["share_files", "share_bytes"] {
        for invalid in [json!(-0.1), json!(1.1), json!(true), json!("0.5")] {
            reject_packet(
                &format!("/families/0/{field}"),
                invalid,
                &format!("Invalid {field}: expected a share in [0, 1]"),
            );
        }
    }
    reject_packet(
        "/families/0/share_files",
        Value::Null,
        "Invalid share_files: expected a share in [0, 1]",
    );
}

#[test]
fn duplicate_marker_is_refused() {
    reject_packet(
        "/markers",
        json!(["cargo_toml", "cargo_toml"]),
        "Duplicate marker",
    );
}

#[test]
fn unknown_allocation_requires_null_bytes() {
    reject_packet(
        "/coverage/allocation_known",
        json!(false),
        "allocated_bytes must be null when allocation is unknown",
    );
}

#[test]
fn incomplete_coverage_does_not_turn_unknown_allocation_into_zero() {
    for state in [
        "partial",
        "stale",
        "unscanned",
        "denied",
        "excluded",
        "cancelled",
        "unknown",
    ] {
        let mut packet = packet();
        packet["coverage"]["state"] = json!(state);
        packet["shape"]["allocated_bytes"] = json!(0);
        assert_eq!(
            validate_packet(&packet).unwrap_err().message(),
            "Incomplete coverage cannot claim zero allocation for non-empty subjects"
        );
        packet["shape"]["files"] = json!(0);
        validate_packet(&packet).unwrap();
    }
    let mut packet = packet();
    packet["shape"]["allocated_bytes"] = json!(0);
    validate_packet(&packet).unwrap();
}

#[test]
fn reversed_content_times_are_refused() {
    reject_packet(
        "/time/oldest_content_write",
        json!(1800000000),
        "oldest_content_write exceeds newest_content_write",
    );
}

#[test]
fn reversed_capture_times_are_refused() {
    let mut packet = corpus()[6]["packet"].clone();
    packet["media"]["capture_start"] = json!(1700000000);
    assert_eq!(
        validate_packet(&packet).unwrap_err().message(),
        "capture_start exceeds capture_end"
    );
}

#[test]
fn raw_pairs_cannot_exceed_either_file_count() {
    for key in ["raw_files", "sidecar_files"] {
        let mut packet = corpus()[6]["packet"].clone();
        packet["media"][key] = json!(119);
        assert_eq!(
            validate_packet(&packet).unwrap_err().message(),
            "raw_with_sidecar exceeds raw_files or sidecar_files"
        );
    }
}

#[test]
fn last_access_is_never_evidence() {
    reject_packet(
        "/time/last_access",
        json!(1700000000),
        "Invalid last_access",
    );
    reject_packet(
        "/time/last_access",
        json!("observed"),
        "Invalid last_access",
    );
}

#[test]
fn count_and_byte_fields_are_bounded_integers() {
    for (pointer, name) in [
        ("/shape/files", "files"),
        ("/shape/folders", "folders"),
        ("/shape/logical_bytes", "logical_bytes"),
        ("/shape/allocated_bytes", "allocated_bytes"),
        ("/extensions/0/files", "extension files"),
        ("/extensions/0/bytes", "extension bytes"),
        ("/duplicates/duplicate_bytes", "duplicate_bytes"),
        ("/duplicates/groups", "groups"),
        ("/media/raw_files", "raw_files"),
        ("/media/sidecar_files", "sidecar_files"),
        ("/media/raw_with_sidecar", "raw_with_sidecar"),
        ("/media/camera_models", "camera_models"),
    ] {
        for invalid in [
            json!(-1),
            json!(9007199254740992u64),
            json!(1.5),
            json!(true),
            json!("1"),
        ] {
            let mut packet = corpus()[6]["packet"].clone();
            *packet.pointer_mut(pointer).unwrap() = invalid;
            assert_eq!(
                validate_packet(&packet).unwrap_err().message(),
                format!("{name} must be an integer in [0, 9007199254740991]"),
                "{pointer}"
            );
        }
    }
    let mut packet = packet();
    packet["shape"]["files"] = json!(9007199254740991u64);
    packet["shape"]["logical_bytes"] = json!(9007199254740991u64);
    validate_packet(&packet).unwrap();
}

#[test]
fn depths_are_bounded_integers() {
    for (pointer, name) in [
        ("/subject/depth", "depth"),
        ("/shape/max_depth", "max_depth"),
    ] {
        for invalid in [
            json!(-1),
            json!(4097),
            json!(false),
            json!(1.5),
            Value::Null,
        ] {
            reject_packet(
                pointer,
                invalid,
                &format!("{name} must be an integer in [0, 4096]"),
            );
        }
        let mut packet = packet();
        *packet.pointer_mut(pointer).unwrap() = json!(4096);
        validate_packet(&packet).unwrap();
    }
}

#[test]
fn unix_fields_are_bounded_integers() {
    for pointer in [
        "/observed_at",
        "/time/newest_content_write",
        "/time/oldest_content_write",
        "/time/newest_source_write",
        "/time/newest_generated_write",
        "/time/git_last_commit",
        "/media/capture_start",
        "/media/capture_end",
    ] {
        let name = pointer.rsplit('/').next().unwrap();
        for invalid in [json!(-1), json!(32503680001u64), json!(false), json!(1.5)] {
            let mut packet = corpus()[6]["packet"].clone();
            *packet.pointer_mut(pointer).unwrap() = invalid;
            assert_eq!(
                validate_packet(&packet).unwrap_err().message(),
                format!("{name} must be an integer in [0, 32503680000]")
            );
        }
    }
    let mut packet = packet();
    packet["observed_at"] = json!(32503680000u64);
    validate_packet(&packet).unwrap();
}

#[test]
fn boolean_fields_refuse_non_booleans() {
    for (pointer, name) in [
        ("/synthetic", "packet synthetic"),
        ("/coverage/allocation_known", "allocation_known"),
        ("/coverage/link_counts_observed", "link_counts_observed"),
        ("/coverage/online", "online"),
        ("/time/git_dirty", "git_dirty"),
    ] {
        reject_packet(pointer, json!(1), &format!("Invalid {name}"));
    }
    reject_record("/synthetic", json!("true"), "Invalid record synthetic");
}

#[test]
fn nullable_fields_accept_unknown_and_reject_wrong_types() {
    let mut packet = corpus()[6]["packet"].clone();
    for pointer in [
        "/shape/logical_bytes",
        "/shape/allocated_bytes",
        "/families/0/share_bytes",
        "/extensions/0/bytes",
        "/time/newest_content_write",
        "/time/oldest_content_write",
        "/time/newest_source_write",
        "/time/newest_generated_write",
        "/time/git_last_commit",
        "/time/git_dirty",
        "/duplicates/duplicate_bytes",
        "/duplicates/groups",
        "/media/capture_start",
        "/media/capture_end",
        "/media/camera_models",
    ] {
        *packet.pointer_mut(pointer).unwrap() = Value::Null;
    }
    packet["coverage"]["allocation_known"] = json!(false);
    validate_packet(&packet).unwrap();
    packet["media"] = Value::Null;
    validate_packet(&packet).unwrap();
    reject_packet(
        "/shape/files",
        Value::Null,
        "files must be an integer in [0, 9007199254740991]",
    );
    reject_packet(
        "/observed_at",
        Value::Null,
        "observed_at must be an integer in [0, 32503680000]",
    );
    reject_packet("/coverage/online", Value::Null, "Invalid online");
}

#[test]
fn array_caps_and_types_are_enforced() {
    for (pointer, name, cap) in [
        ("/subject/name_tokens", "name_tokens", 16),
        ("/families", "families", 15),
        ("/extensions", "extensions", 16),
        ("/markers", "markers", 32),
    ] {
        let first = packet().pointer(pointer).unwrap()[0].clone();
        let expected = format!("Invalid {name}: expected at most {cap} items");
        reject_packet(pointer, json!(vec![first; cap + 1]), &expected);
        reject_packet(pointer, Value::Null, &expected);
    }
    for (pointer, name, cap) in [
        ("/human_labels", "human_labels", 3),
        ("/teacher_labels", "teacher_labels", 3),
        ("/summary_facts", "summary_facts", 12),
    ] {
        let expected = format!("Invalid {name}: expected at most {cap} items");
        reject_record(pointer, json!(vec![Value::Null; cap + 1]), &expected);
        reject_record(pointer, json!({}), &expected);
    }
}

#[test]
fn non_synthetic_corpus_is_refused_even_when_flags_match() {
    let mut corpus = corpus();
    corpus[0]["synthetic"] = json!(false);
    corpus[0]["packet"]["synthetic"] = json!(false);
    validate_record(&corpus[0]).unwrap();
    assert_eq!(
        validate_corpus(&corpus).unwrap_err().message(),
        "Only explicitly synthetic corpora are accepted"
    );
}

#[test]
fn duplicate_record_ids_are_refused() {
    let mut corpus = corpus();
    corpus[1]["record_id"] = corpus[0]["record_id"].clone();
    assert_eq!(
        validate_corpus(&corpus).unwrap_err().message(),
        "Duplicate record_id"
    );
}

#[test]
fn duplicate_packet_ids_are_refused() {
    let mut corpus = corpus();
    corpus[1]["packet"]["packet_id"] = corpus[0]["packet"]["packet_id"].clone();
    assert_eq!(
        validate_corpus(&corpus).unwrap_err().message(),
        "Duplicate packet_id"
    );
}

#[test]
fn corpus_size_is_bounded_and_nonempty() {
    for invalid in [json!([]), json!({}), json!(vec![Value::Null; 10_001])] {
        assert_eq!(
            validate_corpus(&invalid).unwrap_err().message(),
            "Corpus needs 1..10000 records"
        );
    }
    assert_eq!(validate_corpus(&json!([record()])).unwrap(), 1);
}

#[test]
fn summary_pointers_must_resolve_inside_the_packet() {
    for pointer in [
        "/shape/missing",
        "/markers/99",
        "/packet/shape",
        "/record_id",
        "/media/raw_files",
    ] {
        reject_record(
            "/summary_facts",
            json!([pointer]),
            "summary_facts pointer does not resolve inside packet",
        );
    }
    let mut record = record();
    record["summary_facts"] = json!(["/markers/0", "/duplicates/groups", "/media", "/shape/files"]);
    validate_record(&record).unwrap(); // Existing null fields are valid evidence references.
}

#[test]
fn summary_pointer_syntax_and_length_are_enforced() {
    for pointer in [
        "",
        "/",
        "shape/files",
        "/Shape/files",
        "/shape/~0",
        "/shape/files-",
        &format!("/{}", "a".repeat(96)),
    ] {
        reject_record(
            "/summary_facts",
            json!([pointer]),
            "Invalid summary_facts pointer",
        );
    }
}

#[test]
fn each_provenance_has_at_most_one_label_per_axis() {
    for provenance in ["human_labels", "teacher_labels"] {
        reject_record(
            &format!("/{provenance}"),
            json!([
                {"axis": "meaning", "label": "source_project"},
                {"axis": "meaning", "label": "generated_build"}
            ]),
            &format!("Duplicate label axis in {provenance}"),
        );
    }
}

#[test]
fn record_and_packet_synthetic_flags_must_match() {
    reject_record(
        "/synthetic",
        json!(false),
        "Record and packet synthetic flags must match",
    );
    reject_record(
        "/packet/synthetic",
        json!(false),
        "Record and packet synthetic flags must match",
    );
}

#[test]
fn every_schema_enum_value_is_accepted() {
    let packet_schema: Value =
        serde_json::from_str(include_str!("../../../schemas/evidence-packet-v1.json")).unwrap();
    for (schema_pointer, packet_pointer) in [
        ("/properties/subject/properties/kind/enum", "/subject/kind"),
        (
            "/properties/families/items/properties/family/enum",
            "/families/0/family",
        ),
        ("/properties/markers/items/enum", "/markers/0"),
        (
            "/properties/coverage/properties/state/enum",
            "/coverage/state",
        ),
    ] {
        for value in packet_schema
            .pointer(schema_pointer)
            .unwrap()
            .as_array()
            .unwrap()
        {
            let mut packet = packet();
            packet["families"] = json!([{"family": "code", "share_files": 1, "share_bytes": 1}]);
            packet["markers"] = json!(["cargo_toml"]);
            *packet.pointer_mut(packet_pointer).unwrap() = value.clone();
            validate_packet(&packet).unwrap();
        }
    }
    let record_schema: Value =
        serde_json::from_str(include_str!("../../../schemas/categoriser-record-v1.json")).unwrap();
    for variant in record_schema["$defs"]["labels"]["items"]["oneOf"]
        .as_array()
        .unwrap()
    {
        let axis = &variant["properties"]["axis"]["const"];
        for label in variant["properties"]["label"]["enum"].as_array().unwrap() {
            let mut record = record();
            for provenance in ["human_labels", "teacher_labels"] {
                record[provenance] = json!([{"axis": axis, "label": label}]);
            }
            validate_record(&record).unwrap();
        }
    }
}

#[test]
fn numeric_endpoints_and_equal_or_unknown_times_are_accepted() {
    let mut packet = corpus()[6]["packet"].clone();
    for value in [0u64, 9007199254740991] {
        for pointer in [
            "/shape/files",
            "/shape/folders",
            "/shape/logical_bytes",
            "/shape/allocated_bytes",
            "/extensions/0/files",
            "/extensions/0/bytes",
            "/duplicates/duplicate_bytes",
            "/duplicates/groups",
            "/media/raw_files",
            "/media/sidecar_files",
            "/media/raw_with_sidecar",
            "/media/camera_models",
        ] {
            *packet.pointer_mut(pointer).unwrap() = json!(value);
        }
        validate_packet(&packet).unwrap();
    }
    for value in [0u64, 32503680000] {
        for pointer in [
            "/observed_at",
            "/time/newest_content_write",
            "/time/oldest_content_write",
            "/time/newest_source_write",
            "/time/newest_generated_write",
            "/time/git_last_commit",
            "/media/capture_start",
            "/media/capture_end",
        ] {
            *packet.pointer_mut(pointer).unwrap() = json!(value);
        }
        validate_packet(&packet).unwrap();
    }
    packet["time"]["oldest_content_write"] = Value::Null;
    packet["media"]["capture_end"] = Value::Null;
    validate_packet(&packet).unwrap();
}
