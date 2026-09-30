//! Tauri-only update check; all application commands use the local server.

use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

use crate::types::{UpdateState, UpdateStatus};

/// Checks for app updates.
///
/// Returns `disabled` unless the updater plugin has an endpoint+pubkey pair
/// configured; update failures surface in-band as `error`, never as `Err`.
#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Result<UpdateStatus, String> {
    if let Some(disabled) = crate::updater::disabled_status(app.config().plugins.0.get("updater")) {
        return Ok(disabled);
    }
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(err) => {
            return Ok(UpdateStatus {
                state: UpdateState::Error,
                version: None,
                notes: None,
                message: Some(format!("updater unavailable: {err}")),
            });
        }
    };
    match updater.check().await {
        Ok(Some(update)) => Ok(UpdateStatus {
            state: UpdateState::Available,
            version: Some(update.version),
            notes: update.body,
            message: None,
        }),
        Ok(None) => Ok(UpdateStatus {
            state: UpdateState::UpToDate,
            version: None,
            notes: None,
            message: None,
        }),
        Err(err) => Ok(UpdateStatus {
            state: UpdateState::Error,
            version: None,
            notes: None,
            message: Some(err.to_string()),
        }),
    }
}
