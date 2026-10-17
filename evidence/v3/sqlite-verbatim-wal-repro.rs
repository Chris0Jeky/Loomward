//! Standalone #398 upstream probe: std + rusqlite 0.38.0 with `bundled`.
//! Run separate processes with `plain` and `verbatim`, and a new synthetic directory each time.
//! The directory is created exclusively; an existing path is refused. No Loomward code is linked.
use rusqlite::Connection;
use std::{
    path::PathBuf,
    sync::{Arc, Barrier},
    time::Duration,
};

fn main() {
    assert!(cfg!(windows), "this probe targets SQLite's Windows VFS");
    // A failed worker must terminate the probe rather than strand its peers at the barrier.
    let panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        panic(info);
        std::process::exit(2);
    }));
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(45));
        eprintln!("probe exceeded 45 seconds");
        std::process::exit(124);
    });
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("plain or verbatim");
    assert!(matches!(mode.as_str(), "plain" | "verbatim"));
    let dir = PathBuf::from(args.next().expect("new synthetic directory"));
    assert!(args.next().is_none(), "unexpected argument");
    std::fs::create_dir(&dir).expect("refuse an existing directory");
    let canonical = dir.canonicalize().unwrap();
    let text = canonical.to_str().unwrap();
    let ordinary = text.strip_prefix(r"\\?\").unwrap();
    assert!(ordinary.as_bytes()[0].is_ascii_alphabetic() && &ordinary[1..3] == r":\");
    let path = if mode == "plain" {
        PathBuf::from(ordinary)
    } else {
        canonical
    }
    .join("probe.db");
    let state = Connection::open(&path).unwrap();
    state.busy_timeout(Duration::ZERO).unwrap();
    let (version, source): (String, String) = state
        .query_row("SELECT sqlite_version(),sqlite_source_id()", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    println!("mode={mode} sqlite={version} source_id={source}");
    let options: Vec<String> = state
        .prepare("PRAGMA compile_options")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    println!("compile_options={}", options.join(","));
    state
        .execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE probe(x INTEGER)")
        .unwrap();
    let barrier = Arc::new(Barrier::new(9));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let mut conn = Connection::open(&path).unwrap();
            conn.busy_timeout(Duration::ZERO).unwrap();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                for _ in 0..100 {
                    barrier.wait();
                    let tx = conn.transaction().unwrap();
                    let _: i64 = tx
                        .query_row("SELECT count(*) FROM probe", [], |r| r.get(0))
                        .unwrap();
                    barrier.wait();
                    tx.commit().unwrap();
                    barrier.wait();
                }
            })
        })
        .collect();
    for round in 0..100 {
        state
            .execute("INSERT INTO probe VALUES(?1)", [round])
            .unwrap();
        barrier.wait();
        barrier.wait();
        barrier.wait();
        let (busy, frames, done): (i64, i64, i64) = state
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        if busy != 0 {
            println!("retained_lock round={round} busy={busy} frames={frames} checkpointed={done}");
            std::process::exit(3);
        }
    }
    for worker in workers {
        worker.join().unwrap();
    }
    println!("PASS: 100 checkpoints after eight simultaneous reader commits");
}
