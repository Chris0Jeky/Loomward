use super::*;
use std::collections::BTreeSet;

fn id(n: u8) -> Identity {
    Identity {
        volume: 7,
        file_id: [n; 16],
    }
}

fn entry(name: &str, kind: Kind, n: u8, size: u64) -> SourceEntry {
    SourceEntry {
        name: name.into(),
        kind,
        identity: id(n),
        size,
        creation_ft: 100,
        last_write_ft: 200,
        change_ft: 300,
        attributes: 0x20,
        reparse_tag: None,
        ea_size: 0,
        streams: vec![],
        link_count: 1,
        explicit_aces: false,
        verified_volume: true,
    }
}

fn file(name: &str, n: u8, size: u64) -> SourceEntry {
    entry(name, Kind::File, n, size)
}

fn dir(name: &str, n: u8) -> SourceEntry {
    entry(name, Kind::Directory, n, 0)
}

fn dest() -> DestinationFacts {
    DestinationFacts {
        grant: 3,
        parent: Identity {
            volume: 9,
            file_id: [200; 16],
        },
        ancestors: vec![
            Identity {
                volume: 9,
                file_id: [200; 16],
            },
            Identity {
                volume: 9,
                file_id: [201; 16],
            },
        ],
        verified_volume: true,
        cloud_synced: false,
        unsupported_volume: false,
        protected: false,
        case_sensitive: false,
        existing_names: vec![],
        parent_path_units: 20,
        available_bytes: 100 << 30,
        volume_bytes: 200 << 30,
        cluster_bytes: 4096,
    }
}

fn ctx() -> Context {
    Context {
        elevated: false,
        grants_active: true,
        named_streams: true,
        date: "2026-10-17".into(),
    }
}

fn sample() -> Vec<SourceEntry> {
    vec![
        dir("Album", 1),
        file("Album/a.bin", 2, 10),
        dir("Album/sub", 3),
        file("Album/sub/b.bin", 4, 5000),
    ]
}

fn codes(p: &Preview) -> Vec<Code> {
    p.refusals.iter().map(|r| r.code).collect()
}

fn wire(c: Code) -> String {
    serde_json::to_value(c)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

/// Section 7's table read from the doc: every code, and whether its "When" column has P or S.
#[test]
fn codes_and_preflight_flags_match_the_doc_table() {
    let doc = include_str!("../../../docs/47-copy-and-verify.md");
    let start = doc.find("## 7. Refusals").unwrap();
    let table = &doc[start..start + doc[start..].find("**Preflight").unwrap()];
    let mut from_doc = BTreeSet::new();
    for row in table.lines().filter(|l| l.starts_with("| `")) {
        let cols: Vec<_> = row.split('|').map(str::trim).collect();
        let pre = cols[3].split([',', ' ']).any(|t| t == "P" || t == "S");
        for code in cols[1].split('`').skip(1).step_by(2) {
            from_doc.insert((code.to_string(), pre));
        }
    }
    let ours: BTreeSet<_> = Code::ALL
        .iter()
        .map(|&c| (wire(c), c.preflight()))
        .collect();
    assert_eq!(ours, from_doc);
    assert_eq!(Code::ALL.iter().filter(|c| c.preflight()).count(), 21);
}

/// G4 at unit level: every code with a P or S entry is produced by a fixture before any handle exists.
#[test]
fn every_preflight_code_is_refused_by_a_fixture() {
    let base = || (sample(), dest(), ctx());
    let mut got = BTreeSet::new();
    let mut at_preview =
        |f: &dyn Fn(&mut Vec<SourceEntry>, &mut DestinationFacts, &mut Context)| {
            let (mut e, mut d, mut c) = base();
            f(&mut e, &mut d, &mut c);
            let p = preview(e, &d, &c);
            assert!(!p.refusals.is_empty());
            got.extend(codes(&p));
        };
    at_preview(&|_, _, c| c.elevated = true);
    at_preview(&|_, _, c| c.grants_active = false);
    at_preview(&|e, _, _| e[1].verified_volume = false);
    at_preview(&|_, d, _| d.verified_volume = false);
    at_preview(&|e, _, _| e[1].attributes |= 0x40_0000);
    at_preview(&|e, _, _| e[1].reparse_tag = Some(0x9000_301A));
    at_preview(&|e, _, _| e[2].reparse_tag = Some(0xA000_0003));
    at_preview(&|e, _, _| e[1].attributes |= 0x200);
    at_preview(&|e, _, _| e[1].ea_size = 12);
    at_preview(&|e, _, c| {
        c.named_streams = false;
        e[1].streams = vec![("Zone.Identifier".into(), 26)];
    });
    at_preview(&|e, _, _| e[1].streams = (0..65).map(|i| (format!("s{i}"), 1)).collect());
    at_preview(&|_, d, _| d.available_bytes = 1 << 30);
    at_preview(&|e, d, _| d.ancestors.push(e[2].identity));
    at_preview(&|_, d, _| d.cloud_synced = true);
    at_preview(&|_, d, _| d.unsupported_volume = true);
    at_preview(&|_, d, _| d.protected = true);
    at_preview(&|e, _, _| e.push(file("Album/A.BIN", 9, 1)));
    at_preview(&|e, _, _| e.push(file("Album/nul.txt", 9, 1)));
    at_preview(&|_, d, _| d.parent_path_units = MAX_PATH_UNITS - 10);
    at_preview(&|e, _, _| e.clear());

    // S: the consumed approval against a fresh preview.
    let p = preview(sample(), &dest(), &ctx());
    let ok = |p: &Preview| approve(p, p.manifest.digest(), 5, 1_000).unwrap();
    got.insert(ok(&p).admit(&p, 5, 1_000 + APPROVAL_TTL_MS).unwrap_err());
    let mut changed = sample();
    changed[1].last_write_ft += 1;
    got.insert(
        ok(&p)
            .admit(&preview(changed, &dest(), &ctx()), 5, 2_000)
            .unwrap_err(),
    );
    let mut other = sample();
    other.pop();
    got.insert(
        ok(&p)
            .admit(&preview(other, &dest(), &ctx()), 5, 2_000)
            .unwrap_err(),
    );

    let want: BTreeSet<_> = Code::ALL.into_iter().filter(|c| c.preflight()).collect();
    let got: BTreeSet<_> = got.into_iter().collect();
    assert_eq!(got, want);
}

#[test]
fn a_clean_selection_is_a_draft_with_totals() {
    let p = preview(sample(), &dest(), &ctx());
    assert_eq!(p.refusals, vec![]);
    // One selected folder with descendants: the new folder is its copy.
    assert_eq!(p.manifest.destination.folder_name, "Album");
    assert_eq!(p.manifest.root.as_ref().unwrap().identity, id(1));
    let names: Vec<_> = p.manifest.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["a.bin", "sub", "sub/b.bin"]);
    assert_eq!((p.files, p.folders, p.bytes), (2, 2, 5010));
    // New folder + "sub" + two files rounded to 4 KiB and 8 KiB.
    assert_eq!(p.space_needed, 4096 + 4096 + 4096 + 8192);
    assert_eq!(p.space_reserve, 2 << 30);
    assert!(!p.same_volume);
}

#[test]
fn folder_name_proposals_never_need_an_overwrite() {
    let two = vec![file("a.bin", 1, 1), file("b.bin", 2, 1)];
    let p = preview(two.clone(), &dest(), &ctx());
    assert_eq!(
        p.manifest.destination.folder_name,
        "Loomward copy 2026-10-17"
    );
    assert_eq!(p.manifest.root, None);
    assert_eq!(p.manifest.items.len(), 2);

    let mut d = dest();
    d.existing_names = vec!["album".into(), "Album (2)".into()];
    let p = preview(sample(), &d, &ctx());
    assert_eq!(p.manifest.destination.folder_name, "Album (3)");
    d.case_sensitive = true;
    let p = preview(sample(), &d, &ctx());
    assert_eq!(p.manifest.destination.folder_name, "Album");

    // A lone file or an empty folder stays an item inside a new folder.
    let p = preview(vec![dir("Empty", 1)], &dest(), &ctx());
    assert_eq!(p.manifest.items.len(), 1);
    assert_eq!(
        p.manifest.destination.folder_name,
        "Loomward copy 2026-10-17"
    );
}

#[test]
fn names_that_windows_cannot_create_are_unrepresentable() {
    for bad in [
        "CON",
        "con.txt",
        "Nul ",
        "COM1",
        "lpt9.log",
        "COM\u{b9}",
        "a.",
        "a ",
        "a:b",
        "",
    ] {
        assert!(name_unrepresentable(bad), "{bad:?}");
    }
    assert!(name_unrepresentable(&"x".repeat(256)));
    for good in [
        "CONSOLE",
        "COM10",
        "a.b",
        "x".repeat(255).as_str(),
        ".hidden",
    ] {
        assert!(!name_unrepresentable(good), "{good:?}");
    }
}

#[test]
fn digest_is_canonical_and_pinned() {
    let p = preview(sample(), &dest(), &ctx());
    let mut shuffled = sample();
    shuffled.reverse();
    assert_eq!(
        preview(shuffled, &dest(), &ctx()).manifest.digest(),
        p.manifest.digest()
    );
    // Pinned so an encoding change is a visible contract change (approvals in flight would all fail).
    let hex: String = p
        .manifest
        .digest()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(hex, PINNED);
}

const PINNED: &str = "5ce0591058a0177bddc74b27b2c9112cb19f70fbf8cc335841231638aec983c8";

#[test]
fn digest_covers_membership_destination_bytes_and_identity() {
    let digest =
        |e: Vec<SourceEntry>, d: &DestinationFacts| preview(e, d, &ctx()).manifest.digest();
    let base = digest(sample(), &dest());
    let mut variants: Vec<[u8; 32]> = Vec::new();
    let mut with = |f: &dyn Fn(&mut Vec<SourceEntry>, &mut DestinationFacts)| {
        let (mut e, mut d) = (sample(), dest());
        f(&mut e, &mut d);
        let v = digest(e, &d);
        assert_ne!(v, base);
        assert!(!variants.contains(&v));
        variants.push(v);
    };
    with(&|e, _| {
        e.pop();
    });
    with(&|e, _| e.push(file("Album/c.bin", 9, 0)));
    with(&|e, _| e[1].name = "Album/A.bin".into());
    with(&|_, d| d.grant = 4);
    with(&|_, d| d.parent.file_id[0] = 1);
    with(&|_, d| d.existing_names = vec!["Album".into()]);
    with(&|e, _| e[1].size += 1);
    with(&|e, _| e[1].identity.file_id[3] = 0);
    with(&|e, _| e[1].last_write_ft += 1);
    with(&|e, _| e[1].attributes |= 0x1);
    with(&|e, _| e[1].streams = vec![("s".into(), 1)]);
    with(&|e, _| e[0].creation_ft += 1);
    // Exact names, never normalised: NFC and NFD spellings are different members.
    let nfc = digest(vec![file("caf\u{e9}", 1, 1), file("z", 2, 1)], &dest());
    let nfd = digest(vec![file("cafe\u{301}", 1, 1), file("z", 2, 1)], &dest());
    assert_ne!(nfc, nfd);
}

#[test]
fn approval_binds_the_exact_digest_and_is_consumed_by_start() {
    let p = preview(sample(), &dest(), &ctx());
    let d = p.manifest.digest();
    assert_eq!(approve(&p, [0; 32], 1, 0), Err(Code::DigestMismatch));
    let refused = preview(
        sample(),
        &dest(),
        &Context {
            elevated: true,
            ..ctx()
        },
    );
    assert_eq!(
        approve(&refused, refused.manifest.digest(), 1, 0),
        Err(Code::Elevated)
    );

    let a = approve(&p, d, 1, 1_000).unwrap();
    assert_eq!((a.digest(), a.destination_grant()), (d, 3));
    assert_eq!(a.expires_at_ms(), 1_000 + APPROVAL_TTL_MS);
    // `admit` takes the approval by value: a second start needs a second approval.
    assert_eq!(
        a.admit(&p, 1, 1_000 + APPROVAL_TTL_MS - 1),
        Ok(Admitted {
            digest: d,
            items: 3
        })
    );

    let fresh = |f: &dyn Fn(&mut Vec<SourceEntry>, &mut DestinationFacts)| {
        let (mut e, mut dd) = (sample(), dest());
        f(&mut e, &mut dd);
        approve(&p, d, 1, 0)
            .unwrap()
            .admit(&preview(e, &dd, &ctx()), 1, 1)
    };
    // Changed membership or destination: another manifest.
    assert_eq!(
        fresh(&|e, _| e.push(file("Album/new", 9, 1))),
        Err(Code::DigestMismatch)
    );
    assert_eq!(fresh(&|_, d| d.grant = 8), Err(Code::DigestMismatch));
    assert_eq!(
        fresh(&|_, d| d.parent.file_id[0] = 0),
        Err(Code::DigestMismatch)
    );
    assert_eq!(
        fresh(&|_, d| d.existing_names = vec!["Album".into()]),
        Err(Code::DigestMismatch)
    );
    // Same shape, other bytes or identity: the source changed.
    assert_eq!(fresh(&|e, _| e[3].size = 5001), Err(Code::SourceChanged));
    assert_eq!(
        fresh(&|e, _| e[3].identity.file_id[0] = 9),
        Err(Code::SourceChanged)
    );
    // A start-time refusal wins over the digest, and a moved epoch over everything.
    assert_eq!(
        fresh(&|_, d| d.available_bytes = 0),
        Err(Code::InsufficientSpace)
    );
    assert_eq!(
        approve(&p, d, 1, 0).unwrap().admit(&p, 2, 1),
        Err(Code::GrantRevoked)
    );
    assert_eq!(
        fresh(&|_, _| {}),
        Ok(Admitted {
            digest: d,
            items: 3
        })
    );
}

/// The wire vocabulary (`contracts/v3`) is these types' serde names: states and codes cannot drift.
#[test]
fn contract_enums_equal_the_state_machine_and_refusal_codes() {
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../contracts/v3/view-service.schema.json"
    ))
    .unwrap();
    let names = |def: &str| -> Vec<String> {
        schema["$defs"][def]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    };
    let codes: Vec<_> = Code::ALL.into_iter().map(wire).collect();
    assert_eq!(names("CopyRefusalCode"), codes);
    let states: Vec<_> = crate::state::OpState::ALL
        .into_iter()
        .map(|s| s.name().to_string())
        .collect();
    assert_eq!(names("CopyState"), states);
}

#[test]
fn stream_enumeration_order_never_changes_the_digest() {
    let with = |streams: Vec<(&str, u64)>| {
        let mut e = sample();
        e[1].streams = streams.into_iter().map(|(n, s)| (n.into(), s)).collect();
        preview(e, &dest(), &ctx()).manifest.digest()
    };
    assert_eq!(
        with(vec![("Zone.Identifier", 26), ("b", 1)]),
        with(vec![("b", 1), ("Zone.Identifier", 26)])
    );
}

#[test]
fn case_collisions_are_refused_conservatively_at_preview() {
    let collides = |a: &str, b: &str, sensitive: bool| {
        let d = DestinationFacts {
            case_sensitive: sensitive,
            ..dest()
        };
        codes(&preview(vec![file(a, 1, 1), file(b, 2, 1)], &d, &ctx()))
            .contains(&Code::CaseCollision)
    };
    assert!(collides("Report.TXT", "report.txt", false));
    assert!(!collides("Report.TXT", "report.txt", true));
    // U+1FB3 uppercases to two characters, but U+1FBC lowercases to U+1FB3: the lowercase key catches it.
    assert!(collides("\u{1fb3}", "\u{1fbc}", false));
    assert!(!collides("a", "b", false));
    // The proposed folder name avoids both keys too.
    let d = DestinationFacts {
        existing_names: vec!["\u{1fb3}".into()],
        ..dest()
    };
    let p = preview(
        vec![dir("\u{1fbc}", 1), file("\u{1fbc}/x", 2, 1)],
        &d,
        &ctx(),
    );
    assert_eq!(p.manifest.destination.folder_name, "\u{1fbc} (2)");
}
