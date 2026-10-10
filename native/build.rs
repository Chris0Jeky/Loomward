// Declares the two app commands to the ACL, so each needs an explicit permission and the
// capability file (capabilities/default.json) is the whole list of what the webview may invoke.
//
// The common-controls v6 manifest (Tauri's own) is linked into every target, test binaries
// included: without it a test exe fails to load with STATUS_ENTRYPOINT_NOT_FOUND
// (TaskDialogIndirect lives only in comctl32 v6).
fn main() {
    let manifest =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
    let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(windows)
            .app_manifest(tauri_build::AppManifest::new().commands(&["lw_call", "lw_events"])),
    )
    .expect("tauri-build failed");
}
