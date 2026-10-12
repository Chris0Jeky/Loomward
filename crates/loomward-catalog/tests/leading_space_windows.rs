//! Native evidence for the leading-space review on #428. Disposable synthetic paths only.
#[cfg(windows)]
#[test]
fn ordinary_win32_open_preserves_a_leading_space_component() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let leading = root.join(" leading");
    let ordinary = root.join("leading");
    std::fs::create_dir(&leading).unwrap();
    std::fs::create_dir(&ordinary).unwrap();
    std::fs::write(leading.join("identity.txt"), b"leading-space").unwrap();
    std::fs::write(ordinary.join("identity.txt"), b"ordinary").unwrap();
    let text = leading.to_str().unwrap();
    let plain = text.strip_prefix(r"\\?\").unwrap();
    assert_eq!(
        std::fs::read(std::path::Path::new(plain).join("identity.txt")).unwrap(),
        b"leading-space"
    );
    // Rust may internally normalise an OS path. Exercise SQLite's VFS independently too.
    for (directory, marker) in [(&leading, "leading-space"), (&ordinary, "ordinary")] {
        let db = rusqlite::Connection::open(directory.join("identity.db")).unwrap();
        db.execute_batch("CREATE TABLE identity(marker TEXT NOT NULL)")
            .unwrap();
        db.execute("INSERT INTO identity VALUES(?1)", [marker])
            .unwrap();
    }
    let db = rusqlite::Connection::open(std::path::Path::new(plain).join("identity.db")).unwrap();
    let marker: String = db
        .query_row("SELECT marker FROM identity", [], |row| row.get(0))
        .unwrap();
    assert_eq!(marker, "leading-space");
    // A safe ordinary name must retain the WAL-lock workaround, not fall back to verbatim UNC.
    assert_eq!(loomward_catalog::sqlite_dir(&leading), std::path::Path::new(plain));
}
