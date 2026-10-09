// Packaging scaffold only. Shows the synthetic preview; no daemon/LLM/OS IPC bridge.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("could not start the observation-only Loomward preview shell");
}
