//! Themis desktop bridge: Tauri commands, thread runs, approvals, and diffs.
//!
//! The web UI (`web/src/lib/tauri.ts`) is the caller; `web/src/lib/types.ts`
//! is the contract's source of truth, mirrored by [`types`]. Handler logic
//! lives on [`AppState`](state::AppState) behind the [`EventSink`](sink::EventSink)
//! seam so headless tests can drive the real code paths.

pub mod approvals;
pub mod commands;
pub mod diff;
pub mod doctor;
pub mod secrets;
pub mod server;
pub mod settings;
pub mod sink;
pub mod state;
pub mod transcript;
pub mod types;
pub mod updater;
pub mod worktree;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use secrets::ProductionSecretStore;
use serde_json::Value;
use state::AppState;
use tauri::{Emitter, Manager, State};

#[tauri::command]
async fn backend_command(
    backend: State<'_, server::Client>,
    command: String,
    args: Value,
) -> Result<Value, String> {
    backend.call(&command, args).await
}

#[tauri::command]
fn completion_haptic(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return app
        .run_on_main_thread(|| {
            use objc2_app_kit::{
                NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
                NSHapticFeedbackPerformer,
            };
            NSHapticFeedbackManager::defaultPerformer().performFeedbackPattern_performanceTime(
                NSHapticFeedbackPattern::Generic,
                NSHapticFeedbackPerformanceTime::Now,
            );
        })
        .map_err(|error| error.to_string());
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(())
    }
}

fn forward_remote_events(app: tauri::AppHandle, client: server::Client) {
    tauri::async_runtime::spawn(async move {
        loop {
            if let Ok(mut stream) = client.subscribe().await {
                while let Ok(event) = server::Client::next_event(&mut stream).await {
                    if let (Some(name), Some(payload)) = (
                        event.get("name").and_then(Value::as_str),
                        event.get("payload"),
                    ) {
                        if name != "lagged" {
                            let _ = app.emit(name, payload.clone());
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
}

#[cfg(target_os = "macos")]
fn set_dock_icon() -> Result<(), String> {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let mtm = MainThreadMarker::new().ok_or("Dock icon must be set on the main thread")?;
    let icon = NSData::with_bytes(include_bytes!("../icons/128x128@2x.png"));
    let image = NSImage::initWithData(NSImage::alloc(), &icon)
        .ok_or("could not load the bundled Dock icon")?;
    // SAFETY: AppKit requires a valid image; `image` stays retained for the call.
    unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    Ok(())
}

/// The same executable can host the backend without starting a window.
pub fn run_headless_server(data_dir: PathBuf) -> Result<(), String> {
    tauri::async_runtime::block_on(run_headless_server_async(data_dir))
}

/// Hosts the backend on an existing async runtime (used by `themis serve`).
pub async fn run_headless_server_async(data_dir: PathBuf) -> Result<(), String> {
    std::fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join("server.lock"))
        .map_err(|error| error.to_string())?;
    lock.try_lock()
        .map_err(|error| format!("another local server owns this data directory: {error}"))?;
    let state = AppState::new(
        data_dir.join("settings.json"),
        Arc::new(ProductionSecretStore::new()),
    );
    server::Server::bind(state, &data_dir).await?.run().await
}

fn ensure_server(client: &server::Client, data_dir: &Path) -> Result<(), String> {
    let ping = || {
        tauri::async_runtime::block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_millis(500),
                client.call("ping", Value::Null),
            )
            .await
        })
        .is_ok_and(|result| result.is_ok())
    };
    if ping() {
        return Ok(());
    }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut child = std::process::Command::new(executable)
        .arg("--themis-server")
        .arg(data_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("could not start local server: {error}"))?;
    for _ in 0..50 {
        if ping() {
            return Ok(());
        }
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Err(format!("local server exited during startup: {status}"));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err("local server did not become ready within five seconds".to_owned())
}

/// Runs the Tauri application.
pub fn run() {
    let context = tauri::generate_context!();
    // The updater plugin panics at startup without a deserializable
    // `plugins.updater` section, so only register it when the bundled config
    // carries one. `check_for_updates` short-circuits to `Disabled` otherwise.
    let updater_usable = crate::updater::plugin_usable(context.config().plugins.0.get("updater"));
    let builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init());
    let builder = if updater_usable {
        builder.plugin(tauri_plugin_updater::Builder::new().build())
    } else {
        builder
    };
    builder
        .setup(|app| {
            #[cfg(target_os = "macos")]
            set_dock_icon()?;
            let data_dir = app.path().app_data_dir()?;
            let client = server::Client::new(data_dir.clone());
            ensure_server(&client, &data_dir)?;
            forward_remote_events(app.handle().clone(), client.clone());
            app.manage(client);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            backend_command,
            completion_haptic,
            commands::check_for_updates,
        ])
        .run(context)
        .expect("error while running tauri application");
}
