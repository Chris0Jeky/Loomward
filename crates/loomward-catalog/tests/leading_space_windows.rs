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
}
