//! The one-time move of a Phase 1 store (channels of peer Visions, `# administration`) to teams: `# administration`
//! becomes `# administration` with the admin as its chief, every other channel's first member
//! becomes its chief, the per-Vision autonomy and budget overrides go (the channel's settings
//! replace them), and old heartbeat notes are deleted. Logs and owned chats stay as they are.
//!
//! It runs on the store's first load and is idempotent: each step only changes what is still in
//! the old shape, so a run cut short is finished by the next one. The `conversion` marker is
//! written last, after `# administration` was told (see `finish`), so the note is posted once.

use std::path::Path;

use serde_json::Value;

use super::admin::{ADMINISTRATION, ADMIN_VISION_ID, OLD_HQ};
use super::store::{read_store, write_store, StoreError};
use super::{ChannelEntry, NoticeKind, VisionsCache, CHANNELS_FILE, VISIONS_FILE};

/// Written last, holding [`VERSION`], once the store is in the teams shape.
pub(super) const MARKER_FILE: &str = "conversion";
const VERSION: &str = "2";
/// Phase 1's heartbeat notes, if any were written.
const HEARTBEATS_DIR: &str = "heartbeats";
/// The Vision fields teams replace with the channel's.
const DROPPED_VISION_FIELDS: &[&str] = &["autonomy", "custom", "daily_budget_usd"];

/// Whether the store is already in the teams shape.
pub(super) fn done(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join(MARKER_FILE)).is_ok_and(|text| text.trim() == VERSION)
}

/// Moves what is still in the old shape; returns the `# administration` channel ids to tell.
/// A store with no files yet needs nothing and returns none.
pub(super) fn convert(dir: &Path) -> Result<Vec<String>, StoreError> {
    let mut channels: Vec<Value> = read_store(dir, CHANNELS_FILE)?;
    let mut changed = false;
    let mut administrations = Vec::new();
    for channel in &mut channels {
        changed |= convert_channel(channel);
        if channel["name"] == ADMINISTRATION {
            if let Some(id) = channel["id"].as_str() {
                administrations.push(id.to_owned());
            }
        }
    }
    if changed {
        write_store(dir, CHANNELS_FILE, &channels)?;
    }
    let chiefs: Vec<String> = channels
        .iter()
        .filter_map(|channel| channel["chief"].as_str().map(str::to_owned))
        .collect();
    let mut visions: Vec<Value> = read_store(dir, VISIONS_FILE)?;
    let mut dropped = false;
    for vision in &mut visions {
        dropped |= move_soul(dir, vision, &chiefs)?;
        if let Some(fields) = vision.as_object_mut() {
            for field in DROPPED_VISION_FIELDS {
                dropped |= fields.remove(*field).is_some();
            }
        }
    }
    if dropped {
        write_store(dir, VISIONS_FILE, &visions)?;
    }
    let heartbeats = dir.join(HEARTBEATS_DIR);
    if heartbeats.is_dir() {
        std::fs::remove_dir_all(&heartbeats)
            .map_err(|error| StoreError::write_failed(MARKER_FILE, &error))?;
    }
    Ok(administrations)
}

/// A chief's or Atlas's soul text moves to its `soul.md` (kept when one is already there); a
/// worker's is dropped. Returns whether the record changed.
fn move_soul(dir: &Path, vision: &mut Value, chiefs: &[String]) -> Result<bool, StoreError> {
    let Some(fields) = vision.as_object_mut() else {
        return Ok(false);
    };
    let Some(soul) = fields.remove("soul") else {
        return Ok(false);
    };
    let id = fields.get("id").and_then(Value::as_str).unwrap_or_default();
    let text = soul.as_str().unwrap_or_default();
    let keeps = id == ADMIN_VISION_ID || chiefs.iter().any(|chief| chief == id);
    if keeps && !text.trim().is_empty() && matches!(super::souls::read_soul(dir, id), Ok(None)) {
        super::souls::write_soul(dir, id, text, "")
            .map_err(|message| StoreError::refused(VISIONS_FILE, &message))?;
    }
    Ok(true)
}

/// One channel in the teams shape: `# administration` renamed with the admin as chief, else a chief chosen
/// from the members when it has none. Returns whether it changed.
fn convert_channel(channel: &mut Value) -> bool {
    let Some(fields) = channel.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    if fields.get("name").and_then(Value::as_str) == Some(OLD_HQ) {
        fields.insert("name".into(), Value::from(ADMINISTRATION));
        changed = true;
    }
    let has_chief = fields
        .get("chief")
        .and_then(Value::as_str)
        .is_some_and(|chief| !chief.is_empty());
    if !has_chief {
        let first = fields
            .get("members")
            .and_then(Value::as_array)
            .and_then(|members| members.first())
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let chief = if fields.get("name").and_then(Value::as_str) == Some(ADMINISTRATION) {
            ADMIN_VISION_ID.to_owned()
        } else {
            first
        };
        if !chief.is_empty() {
            fields.insert("chief".into(), Value::from(chief));
            changed = true;
        }
    }
    changed
}

/// Only the channel step, as if the app quit before the rest (tests).
#[cfg(test)]
pub(super) fn convert_channels_only_for_test(dir: &Path) {
    let mut channels: Vec<Value> = read_store(dir, CHANNELS_FILE).unwrap();
    for channel in &mut channels {
        convert_channel(channel);
    }
    write_store(dir, CHANNELS_FILE, &channels).unwrap();
}

impl VisionsCache {
    /// Converts the store on its first load in this process (see the module docs); the
    /// `# administration` channels still to tell are kept for `finish`.
    pub(super) fn ensure_converted(&mut self, dir: &Path) -> Result<(), StoreError> {
        if self.conversion_checked {
            return Ok(());
        }
        if !done(dir) {
            // A file that cannot be read is reported by the ordinary load; the conversion waits
            // until it is fixed.
            match convert(dir) {
                Ok(pending) => self.conversion_pending = Some(pending),
                Err(_) => return Ok(()),
            }
        }
        self.conversion_checked = true;
        Ok(())
    }

    /// Tells each converted `# administration` once (a log that already holds the note is left
    /// alone), then writes the marker. Returns the channels newly told.
    pub(super) fn finish_conversion(&mut self, dir: &Path) -> Result<Vec<String>, StoreError> {
        let Some(pending) = self.conversion_pending.take() else {
            return Ok(Vec::new());
        };
        let mut told = Vec::new();
        for channel_id in pending {
            if !self.log_has_teams_note(dir, &channel_id)? {
                let note = ChannelEntry::Notice {
                    kind: NoticeKind::TeamsArrived,
                    vision_id: Some(ADMIN_VISION_ID.to_owned()),
                    thread_id: None,
                    unpriced_requests: None,
                    reason: None,
                };
                self.append_entry(dir, &channel_id, note)?;
                told.push(channel_id);
            }
        }
        std::fs::write(dir.join(MARKER_FILE), VERSION)
            .map_err(|error| StoreError::write_failed(MARKER_FILE, &error))?;
        Ok(told)
    }

    fn log_has_teams_note(&mut self, dir: &Path, channel_id: &str) -> Result<bool, StoreError> {
        let path = super::store::channel_log_path(dir, channel_id)?;
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(text.contains("\"teams_arrived\"")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(StoreError::unreadable(
                CHANNELS_FILE,
                &error.kind().to_string(),
            )),
        }
    }
}
