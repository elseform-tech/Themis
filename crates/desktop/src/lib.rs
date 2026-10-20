//! Themis desktop bridge: Tauri commands, thread runs, approvals, and diffs.
//!
//! The web UI (`web/src/lib/tauri.ts`) is the caller; `web/src/lib/types.ts`
//! is the contract's source of truth, mirrored by [`types`]. Handler logic
//! lives on [`AppState`](state::AppState) behind the [`EventSink`](sink::EventSink)
//! seam so headless tests can drive the real code paths.

pub mod approvals;
pub mod cli;
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

/// The Browser tab's open streams, one per chat.
#[derive(Default)]
struct BrowserStreams(
    std::sync::Mutex<std::collections::HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
);

/// Streams one chat's browser updates to the Browser tab until it stops or
/// opens another stream for the chat.
#[tauri::command]
async fn browser_stream(
    backend: State<'_, server::Client>,
    streams: State<'_, BrowserStreams>,
    thread_id: String,
    on_event: tauri::ipc::Channel<Value>,
) -> Result<(), String> {
    let (mut stream, state) = backend.subscribe_browser(&thread_id).await?;
    let _ = on_event.send(serde_json::json!({"kind": "state", "state": state}));
    let task = tauri::async_runtime::spawn(async move {
        while let Ok(update) = server::Client::next_event(&mut stream).await {
            if on_event.send(update).is_err() {
                break;
            }
        }
    });
    if let Some(previous) = streams
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(thread_id, task)
    {
        previous.abort();
    }
    Ok(())
}

#[tauri::command]
fn browser_stream_stop(streams: State<'_, BrowserStreams>, thread_id: String) {
    if let Some(task) = streams
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&thread_id)
    {
        task.abort();
    }
}

#[tauri::command]
async fn attachment_file(
    app: tauri::AppHandle,
    backend: State<'_, server::Client>,
    thread_id: String,
    path: String,
) -> Result<String, String> {
    let value = backend
        .call(
            "attachment_path",
            serde_json::json!({"threadId":thread_id,"path":path}),
        )
        .await?;
    let path = value.as_str().ok_or("Invalid attachment path")?.to_owned();
    #[cfg(target_os = "macos")]
    let path = if std::path::Path::new(&path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
    {
        tokio::task::spawn_blocking(move || pdf_thumbnail(&path))
            .await
            .map_err(|e| e.to_string())??
    } else {
        path
    };
    app.asset_protocol_scope()
        .allow_file(&path)
        .map_err(|e| e.to_string())?;
    Ok(path)
}

// ponytail: cache the first page per staged PDF; invalidate it if attachment editing needs live previews.
#[cfg(target_os = "macos")]
fn pdf_thumbnail(path: &str) -> Result<String, String> {
    let source = Path::new(path).canonicalize().map_err(|e| e.to_string())?;
    let parent = source.parent().ok_or("Invalid attachment path")?;
    let directory = parent.join(".preview");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    if !directory.starts_with(parent) {
        return Err("Invalid preview directory".into());
    }
    let thumbnail = directory.join(format!(
        "{}.png",
        source
            .file_name()
            .ok_or("Invalid filename")?
            .to_string_lossy()
    ));
    if !thumbnail.exists()
        && !std::process::Command::new("qlmanage")
            .args(["-t", "-s", "512", "-o"])
            .arg(&directory)
            .arg(&source)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| e.to_string())?
            .success()
    {
        return Err("PDF preview unavailable".into());
    }
    let thumbnail = thumbnail.canonicalize().map_err(|e| e.to_string())?;
    if !thumbnail.starts_with(&directory) {
        return Err("Invalid PDF preview".into());
    }
    Ok(thumbnail.to_string_lossy().into_owned())
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
                let _ = app.emit("backend-resync", ());
                while let Ok(event) = server::Client::next_event(&mut stream).await {
                    if let (Some(name), Some(payload)) = (
                        event.get("name").and_then(Value::as_str),
                        event.get("payload"),
                    ) {
                        let _ = app.emit(
                            if name == "lagged" {
                                "backend-resync"
                            } else {
                                name
                            },
                            payload.clone(),
                        );
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

/// Worker stack for the runtime that hosts the backend. A run's future nests
/// deeply (context scopes, hooks, providers); in a debug build, building a
/// request with MCP tool schemas overflowed tokio's 2 MiB default.
const WORKER_STACK_BYTES: usize = 8 * 1024 * 1024;

/// The async runtime for the app, its headless server and the CLI.
pub fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(WORKER_STACK_BYTES)
        .build()
}

/// Makes [`runtime`] Tauri's async runtime. Call once, first thing in `main`.
pub fn install_runtime() {
    let runtime = runtime().expect("could not start the async runtime");
    tauri::async_runtime::set(runtime.handle().clone());
    // Tauri keeps only the handle; the runtime lives for the whole process.
    std::mem::forget(runtime);
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

/// Turns off macOS smart quotes and smart dashes in the app's text boxes (they are on by
/// default system-wide and turn `"x"` into `“x”` and `--` into `—`, which corrupts code
/// typed into prompts). Registered as this app's defaults only: a person's own choice in
/// Edit > Substitutions still wins, and system settings are untouched.
#[cfg(target_os = "macos")]
fn turn_off_text_substitutions() {
    use objc2_foundation::{NSDictionary, NSNumber, NSString, NSUserDefaults};
    let keys = [
        "WebAutomaticQuoteSubstitutionEnabled",
        "WebAutomaticDashSubstitutionEnabled",
        "NSAutomaticQuoteSubstitutionEnabled",
        "NSAutomaticDashSubstitutionEnabled",
    ]
    .map(NSString::from_str);
    let off = NSNumber::new_bool(false);
    let off: &objc2::runtime::AnyObject = &off;
    let values = keys.each_ref().map(|_| off);
    let defaults = NSDictionary::from_slices(&keys.each_ref().map(|key| &**key), &values);
    // SAFETY: every key is an NSString and every value an NSNumber, both property-list
    // types, which is all `registerDefaults` requires.
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&defaults) };
}

/// Runs the Tauri application.
pub fn run() {
    #[cfg(target_os = "macos")]
    turn_off_text_substitutions();
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
        .on_window_event(|window, event| {
            if window.label() == "main" && matches!(event, tauri::WindowEvent::Destroyed) {
                if let Some(companion) = window.app_handle().get_webview_window("companion") {
                    let _ = companion.close();
                }
            }
        })
        .setup(|app| {
            #[cfg(target_os = "macos")]
            set_dock_icon()?;
            let data_dir = app.path().app_data_dir()?;
            let client = server::Client::new(data_dir.clone());
            ensure_server(&client, &data_dir)?;
            forward_remote_events(app.handle().clone(), client.clone());
            app.manage(client);
            app.manage(BrowserStreams::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            backend_command,
            browser_stream,
            browser_stream_stop,
            attachment_file,
            completion_haptic,
            commands::check_for_updates,
        ])
        .run(context)
        .expect("error while running tauri application");
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn pdf_thumbnail_renders_and_rejects_escaped_cache() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("document.pdf");
    std::fs::write(&source, include_bytes!("../tests/fixtures/preview.pdf")).unwrap();
    let thumbnail = pdf_thumbnail(source.to_str().unwrap()).unwrap();
    assert!(std::fs::read(&thumbnail)
        .unwrap()
        .starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(pdf_thumbnail(source.to_str().unwrap()).unwrap(), thumbnail);
    let other = tempfile::tempdir().unwrap();
    let source = other.path().join("document.pdf");
    std::fs::write(&source, include_bytes!("../tests/fixtures/preview.pdf")).unwrap();
    std::os::unix::fs::symlink(directory.path(), other.path().join(".preview")).unwrap();
    assert!(pdf_thumbnail(source.to_str().unwrap())
        .unwrap_err()
        .contains("Invalid preview directory"));
}

#[cfg(test)]
mod runtime_tests {
    /// Uses well over tokio's 2 MiB default worker stack (24 frames of 64+ KiB).
    fn deep(levels: u32) -> u64 {
        let frame = std::hint::black_box([levels as u8; 64 * 1024]);
        if levels == 0 {
            return u64::from(frame[0]);
        }
        u64::from(frame[1]) + deep(levels - 1)
    }

    #[test]
    fn backend_workers_have_room_for_deep_runs() {
        let runtime = super::runtime().unwrap();
        let sum = runtime.block_on(async { tokio::spawn(async { deep(24) }).await.unwrap() });
        assert_eq!(sum, (0..=24).sum::<u64>());
    }
}
