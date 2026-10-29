//! Disposable registered lab for the real Reclaim browser journey; never an owner inventory.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    assert_eq!(args.len(), 2, "usage: reclaim_selection_fixture STATE ROOT");
    let state = std::path::Path::new(&args[0]);
    let root = std::path::Path::new(&args[1]);
    loomward_service::lab::register(
        state,
        root,
        "reclaim-selection-1",
        "tiny",
        &format!("sha256:{}", "00".repeat(32)),
    )?;
    std::fs::write(root.join("Cargo.toml"), b"synthetic manifest")?;
    std::fs::create_dir(root.join("target"))?;
    std::fs::create_dir_all(root.join(".cargo/registry/cache"))?;
    std::fs::write(root.join("target/shared.bin"), vec![1u8; 8192])?;
    std::fs::write(root.join("target/own.bin"), vec![2u8; 12288])?;
    std::fs::hard_link(
        root.join("target/shared.bin"),
        root.join(".cargo/registry/cache/shared.bin"),
    )?;
    println!("reclaim-selection-1: two candidates, three names, two objects, 20480 unique allocated bytes expected on NTFS");
    Ok(())
}
