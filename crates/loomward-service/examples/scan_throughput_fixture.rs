//! Creates a registered disposable S-tier for the real service benchmark, never a personal root.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    assert_eq!(args.len(), 2, "usage: scan_throughput_fixture STATE ROOT");
    let state = std::path::Path::new(&args[0]);
    let root = std::path::Path::new(&args[1]);
    loomward_service::lab::register(
        state,
        root,
        "310",
        "S",
        &format!("sha256:{}", "00".repeat(32)),
    )?;
    for dir in 0..1000 {
        let path = root.join(format!("bucket-{dir:04}"));
        std::fs::create_dir(&path)?;
        for file in 0..100 {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path.join(format!("synthetic-{file:03}.bin")))?
                .set_len(128)?;
        }
    }
    println!("100000 synthetic files, 1000 directories, 12800000 logical bytes");
    Ok(())
}
