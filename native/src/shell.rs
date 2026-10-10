//! The Tauri binding: the two `#[tauri::command]`s and the lifecycle hooks that tear streams
//! down. Generic over the runtime so the tests can drive it through Tauri's own IPC and ACL
//! with the mock runtime.

use crate::Desktop;
use loomward_protocol::{EventEnvelope, ResponseEnvelope};
use std::sync::Arc;
use tauri::ipc::{Channel, InvokeBody, Request};
use tauri::webview::PageLoadEvent;
use tauri::{Builder, Manager, Runtime, State, Webview, WindowEvent};

#[tauri::command]
async fn lw_call(
    desktop: State<'_, Arc<Desktop>>,
    request: Request<'_>,
) -> Result<ResponseEnvelope, String> {
    let desktop = desktop.inner().clone();
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("lw_call requires a raw UTF-8 JSON envelope".into());
    };
    if bytes.len() > loomward_protocol::limits::MAX_REQUEST_BYTES {
        return Err("the request exceeds 64 KiB".into());
    }
    let permit = desktop.begin_call()?;
    let bytes = bytes.clone();
    // Off the async runtime: the service is synchronous and may show a native dialog.
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        desktop.dispatch_bytes(&bytes)
    })
    .await
    .map_err(|_| "the service failed".to_string())?
}

#[tauri::command]
async fn lw_events<R: Runtime>(
    desktop: State<'_, Arc<Desktop>>,
    webview: Webview<R>,
    channel: Channel<EventEnvelope>,
    last_epoch: Option<String>,
    last_seq: Option<u64>,
) -> Result<(), String> {
    let desktop = desktop.inner().clone();
    let label = webview.label().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        desktop.open_stream(&label, last_epoch, last_seq, move |e| {
            channel.send(e).is_ok()
        })
    })
    .await
    .map_err(|_| "the service failed".to_string())?
}

/// Registers the desktop state, exactly the two commands, and stream teardown on page load
/// and window close.
pub fn configure<R: Runtime>(builder: Builder<R>, desktop: Arc<Desktop>) -> Builder<R> {
    builder
        .manage(desktop)
        .setup(|app| {
            if let Some(config) = app.config().app.windows.iter().find(|w| w.label == "main") {
                let window = tauri::WebviewWindowBuilder::from_config(app, config)?
                    .on_navigation(local_navigation)
                    .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                    .build()?;
                #[cfg(windows)]
                if let Some(dialogs) = app.try_state::<Arc<crate::dialogs::DesktopDialogs>>() {
                    dialogs.set_owner(&window)?;
                }
                let _ = window;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![lw_call, lw_events])
        .on_page_load(|webview, payload| {
            // A new document can never receive on the old one's channels.
            if payload.event() == PageLoadEvent::Started {
                let n = webview
                    .state::<Arc<Desktop>>()
                    .close_webview(webview.label());
                log_teardown(n, webview.label(), "page load");
            }
        })
        .on_window_event(|window, event| {
            // Stop at the start of a close, and again on destroy for closes that skip the request.
            if matches!(
                event,
                WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
            ) {
                let desktop = window.state::<Arc<Desktop>>();
                let mut labels: Vec<String> = window
                    .webviews()
                    .iter()
                    .map(|w| w.label().to_string())
                    .collect();
                labels.push(window.label().to_string());
                for label in labels {
                    log_teardown(desktop.close_webview(&label), &label, "window close");
                }
            }
        })
}

/// Only the embedded application origin may replace the main document.
pub fn local_navigation(url: &tauri::Url) -> bool {
    url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && matches!(
            (url.scheme(), url.host_str()),
            ("http", Some("tauri.localhost")) | ("tauri", Some("localhost"))
        )
}

/// One stderr line per teardown that stopped something: the native test log reads it.
fn log_teardown(n: usize, label: &str, why: &str) {
    if n > 0 {
        eprintln!("loomward-desktop: {why}: closed {n} event stream(s) of webview {label:?}");
    }
}
