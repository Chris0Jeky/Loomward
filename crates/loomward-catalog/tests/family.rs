//! #908: the extension-to-family table and the migration of stores that predate it.
//! Synthetic names only.

use loomward_catalog::*;

#[test]
fn every_family_maps_its_extensions_in_any_case() {
    for (family, exts) in [
        ("document", &["pdf", "DOCX", "txt", "Md"][..]),
        ("spreadsheet", &["xlsx", "XLS", "ods"]),
        ("presentation", &["pptx", "Key", "odp"]),
        (
            "image",
            &[
                "jpg", "JPEG", "png", "heic", "HEIC", "psd", "cr2", "cr3", "nef", "arw", "dng",
                "raf", "orf", "rw2", "NEF",
            ],
        ),
        ("video", &["mp4", "MKV", "mov", "avi", "webm"]),
        ("audio", &["mp3", "FLAC", "wav", "m4a"]),
        ("archive", &["zip", "7Z", "tar", "gz", "iso"]),
        ("code", &["rs", "PY", "ts", "html", "ps1"]),
        ("data", &["json", "CSV", "parquet", "sqlite", "yaml"]),
        ("model", &["safetensors", "GGUF", "onnx", "ckpt", "pt"]),
        ("executable", &["exe", "MSI", "apk"]),
        ("system", &["dll", "SYS", "lnk", "evtx"]),
        ("font", &["ttf", "OTF", "woff2"]),
    ] {
        for ext in exts {
            assert_eq!(extension_family(Some(ext)), family, "{ext}");
        }
    }
}

#[test]
fn unknown_stays_other_and_no_extension_stays_none() {
    // `bin` is ambiguous on purpose: raw dumps, firmware and model weights share it.
    for ext in [
        "bin",
        "dat",
        "zzqq",
        "",
        "caf\u{e9}",
        "averyveryverylongextension",
    ] {
        assert_eq!(extension_family(Some(ext)), "other", "{ext:?}");
    }
    assert_eq!(extension_family(None), "none");
}

#[test]
fn every_answer_is_a_catalogue_family() {
    for ext in ["pdf", "cr3", "gguf", "dll", "bin", "ttf", "rs"] {
        assert!(FAMILIES.contains(&extension_family(Some(ext))), "{ext}");
    }
}

fn file(name: &str, ext: &str, family: &str) -> Observation {
    let mut o = Observation::file(name, 10, Some(10));
    o.extension = Some(ext.into());
    o.family = family.into();
    o
}

fn register(catalog: &Catalog) -> (i64, i64) {
    let WriteReply::Root {
        grant_id, dir_id, ..
    } = catalog
        .writer()
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
            volume_key: "fixture".into(),
            display_name: "Root".into(),
            display_path: "Synthetic root".into(),
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
    (grant_id, dir_id)
}

fn scan(catalog: &Catalog, grant: i64, dir: i64, files: Vec<Observation>) -> Result<()> {
    let WriteReply::Run(run) = catalog.writer().call(WriteCommand::BeginRun {
        grant_id: grant,
        mode: "full".into(),
        strategy: "fixture".into(),
        started_at_ns: 0,
    })?
    else {
        panic!()
    };
    let listing = catalog.writer().call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: dir,
        files,
        dirs: vec![],
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }));
    catalog.writer().call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 1,
    })?;
    listing.map(|_| ())
}

#[test]
fn a_store_holding_old_other_rows_migrates_and_rescans_without_the_conflict_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (grant, dir) = {
        let catalog = Catalog::open(tmp.path(), "synthetic").unwrap();
        let (grant_id, dir_id) = register(&catalog);
        // What every pre-#908 native scan wrote: all extensions as `other`.
        scan(
            &catalog,
            grant_id,
            dir_id,
            vec![
                file("synthetic-a", "jpg", "other"),
                file("synthetic-b", "mp4", "other"),
                file("synthetic-c", "zzqq", "other"),
            ],
        )
        .unwrap();
        (grant_id, dir_id)
    };
    let catalog = Catalog::open(tmp.path(), "synthetic").unwrap();
    // The re-scan now classifies; without the migration the writer refuses the changed family.
    scan(
        &catalog,
        grant,
        dir,
        vec![
            file("synthetic-a", "jpg", extension_family(Some("jpg"))),
            file("synthetic-b", "mp4", extension_family(Some("mp4"))),
            file("synthetic-c", "zzqq", extension_family(Some("zzqq"))),
        ],
    )
    .unwrap();
    let b = catalog
        .reader()
        .unwrap()
        .breakdown(dir, "ext_family", Basis::Logical, 8, 1_000_000)
        .unwrap();
    let keys: Vec<_> = b.buckets.iter().map(|k| k.key.as_str()).collect();
    assert_eq!(keys.len(), 3, "{keys:?}");
    for key in ["image", "video", "other"] {
        assert!(keys.contains(&key), "{key} in {keys:?}");
    }
}

#[test]
fn migration_leaves_rows_it_did_not_misclassify_alone() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let catalog = Catalog::open(tmp.path(), "synthetic").unwrap();
        let (grant_id, dir_id) = register(&catalog);
        scan(
            &catalog,
            grant_id,
            dir_id,
            vec![file("synthetic-a", "qqzz", "data")],
        )
        .unwrap();
    }
    let catalog = Catalog::open(tmp.path(), "synthetic").unwrap();
    let b = catalog
        .reader()
        .unwrap()
        .breakdown(1, "ext_family", Basis::Logical, 8, 1_000_000)
        .unwrap();
    assert_eq!(b.buckets[0].key, "data");
}
