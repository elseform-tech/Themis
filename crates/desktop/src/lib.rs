//! Themis desktop bridge: Tauri commands, thread runs, approvals, and diffs.
//!
//! The web UI (`web/src/lib/tauri.ts`) is the caller; `web/src/lib/types.ts`
//! is the contract's source of truth, mirrored by [`types`]. Handler logic
//! lives on [`AppState`](state::AppState) behind the [`EventSink`](sink::EventSink)
//! seam so headless tests can drive the real code paths.

pub mod approvals;
pub mod commands;
pub mod diff;
pub mod secrets;
pub mod settings;
pub mod sink;
pub mod state;
pub mod types;
pub mod updater;
pub mod worktree;

use std::sync::Arc;

use secrets::SessionStore;
use sink::TauriSink;
use state::AppState;
use tauri::Manager;

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
            let data_dir = app.path().app_data_dir()?;
            let state = AppState::new(
                data_dir.join("settings.json"),
                Arc::new(SessionStore::new()),
            );
            state.set_scheduler_sink(Arc::new(TauriSink::new(app.handle().clone())));
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::open_project,
            commands::create_project,
            commands::stop_thread,
            commands::rename_thread,
            commands::list_go_models,
            commands::create_thread,
            commands::get_thread,
            commands::list_threads,
            commands::merge_thread,
            commands::discard_thread,
            commands::send_message,
            commands::list_diff,
            commands::accept_file,
            commands::discard_file,
            commands::add_comment,
            commands::approve_action,
            commands::set_provider,
            commands::open_in_editor,
            commands::get_settings,
            commands::update_settings,
            commands::get_secret_status,
            commands::set_secret,
            commands::clear_secret,
            commands::list_skills,
            commands::create_skill,
            commands::update_skill,
            commands::delete_skill,
            commands::set_thread_skills,
            commands::list_automations,
            commands::create_automation,
            commands::update_automation,
            commands::delete_automation,
            commands::set_automation_enabled,
            commands::run_automation_now,
            commands::list_review_items,
            commands::dismiss_review_item,
            commands::continue_review_item,
            commands::get_diagnostics,
            commands::check_for_updates,
        ])
        .run(context)
        .expect("error while running tauri application");
}
