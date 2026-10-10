// Disposable junction builder, compiled only into the library's Windows tests.
#[cfg(windows)]
pub(crate) fn junction(path: &std::path::Path, target: &std::path::Path) {
    let output = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(path)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "synthetic junction creation failed"
    );
}
