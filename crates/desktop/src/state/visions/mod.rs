//! The Visions store: Vision profiles, channels, which chat belongs to which Vision, and each
//! channel's log, under app data `visions/`.
//!
//! `visions.json`, `channels.json` and `owners.json` are written whole with the plugin
//! store's pattern (a fresh 0600 temporary file, fsync, rename), refuse symlinks and are read
//! only up to 32 MiB. A file that cannot be read is reported and left as it is: lists show
//! nothing from it and saves to it fail until the person fixes or removes it. Profiles and
//! channels carry a revision, and a save names the revision it was opened at.
//!
//! Each channel's log is `channels/<channel id>.jsonl`, one `{seq, at_ms, entry}` per line,
//! appended and paged from the end (see `channel_log`).

mod budget;
mod channel_log;
mod control;
mod drive;
mod envelope;
mod listing;
mod marks;
mod owned_index;
mod runs;
mod store;
mod wakes;

#[cfg(test)]
mod channel_log_tests;
#[cfg(test)]
mod drive_tests;
#[cfg(test)]
mod listing_tests;
#[cfg(test)]
mod marks_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod wire_tests;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;
use themis_core::configuration::ApprovalMode;
use themis_core::visions::Autonomy;

use super::AppState;
pub(crate) use budget::Over as BudgetOver;
pub use channel_log::{ChannelLine, ChannelPage};

pub use runs::vision_decisions;
pub(super) use runs::{boxed_run, confine, run_scope, run_tools, vision_approvals, VisionRunSpec};
use store::StoreError;
pub use wakes::WakeReason;

/// Directory name under the app data directory.
pub(super) const VISIONS_DIR: &str = "visions";
const VISIONS_FILE: &str = "visions.json";
const CHANNELS_FILE: &str = "channels.json";
const OWNERS_FILE: &str = "owners.json";
/// Channel logs, `channels/<channel id>.jsonl`, under [`VISIONS_DIR`].
const CHANNEL_LOGS_DIR: &str = "channels";
/// A Vision's soul (who it is and how it works) is at most this many bytes.
pub(crate) const SOUL_MAX_BYTES: usize = 4 * 1024;

/// A Vision's profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vision {
    /// Empty when saving a new Vision; the store gives it one.
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub role: String,
    pub shape: String,
    pub color: String,
    /// The model its runs use; the configured default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Overrides `visions.autonomy` for this Vision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<Autonomy>,
    /// Overrides `visions.custom` rows for this Vision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<BTreeMap<String, bool>>,
    /// Overrides the daily budget for this Vision, in US dollars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_budget_usd: Option<f64>,
    /// Who it is and how it works, at most [`SOUL_MAX_BYTES`].
    #[serde(default)]
    pub soul: String,
    #[serde(default)]
    pub paused: bool,
    /// Set by the store on every save.
    #[serde(default)]
    pub revision: String,
}

/// A channel: where the person and a project's Visions talk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    /// Empty when saving a new channel; the store gives it one.
    #[serde(default)]
    pub id: String,
    /// `[a-z0-9-]{1,40}`, unique in its project.
    pub name: String,
    #[serde(default)]
    pub topic: String,
    pub project_root: PathBuf,
    /// Set by the person and copied onto every chat a Vision starts here.
    pub approval_mode: ApprovalMode,
    /// Vision ids.
    #[serde(default)]
    pub members: Vec<String>,
    /// Set by the store on every save.
    #[serde(default)]
    pub revision: String,
}

/// What a chat owned by a Vision is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnedKind {
    /// The Vision's own hidden chat for a channel.
    VisionChat,
    /// A chat the Vision started to do work.
    Work,
}

/// Whether the person holds an owned chat back from its Vision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hold {
    #[default]
    None,
    StoppedByPerson,
    TakenOver,
}

/// Which Vision and channel a chat belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub thread_id: String,
    pub vision_id: String,
    pub channel_id: String,
    pub kind: OwnedKind,
    #[serde(default)]
    pub hold: Hold,
}

/// Why a notice was posted in a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    Stopped,
    Paused,
    Resumed,
    BudgetReached,
    ChatFailed,
    HandedBack,
    /// A Vision's run was refused: no sandbox here can hold it in its envelope.
    SandboxUnavailable,
    /// Today's spend cannot be checked (`reason` says why), so the Vision starts nothing.
    BudgetUnknown,
    /// Some of today's requests had no price (`unpriced_requests`); they count as $0.
    Unpriced,
}

/// One entry in a channel's log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChannelEntry {
    Person {
        text: String,
    },
    Vision {
        vision_id: String,
        text: String,
    },
    ChatStarted {
        vision_id: String,
        thread_id: String,
        title: String,
    },
    /// A Vision answered a chat's question itself; `decision` is the autonomy row id and
    /// `reason` the Vision's why (all the person needs to change the answer).
    Answered {
        vision_id: String,
        thread_id: String,
        question_id: String,
        decision: String,
        question: String,
        answer: String,
        reason: String,
    },
    /// A Vision passed a chat's question on to the person, who answers it in the channel.
    PassedOn {
        vision_id: String,
        thread_id: String,
        question_id: String,
        decision: String,
        question: String,
    },
    Notice {
        kind: NoticeKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vision_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        /// With `budget_reached`: how many of the day's counted requests had no price (they
        /// count as $0).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unpriced_requests: Option<u64>,
        /// With `budget_unknown`: why spend cannot be checked (`usage_unavailable`,
        /// `usage_unreadable`, `owners_unreadable`, `catalog_not_loaded`, `config_unreadable`,
        /// `vision_gone`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

/// The Visions cache behind its lock, and the index of owned chats (`owned_index`) apart from
/// it, so the check after every model request takes no Visions lock for a chat no Vision owns.
#[derive(Default)]
pub(crate) struct VisionsShared {
    cache: std::sync::Mutex<VisionsCache>,
    owned: owned_index::OwnedIndex,
    /// The minute budget re-check was started (see `listing`).
    recheck_loop: std::sync::atomic::AtomicBool,
}

/// The store files as loaded, each on first use, plus the newest seq of each channel log
/// appended to. A file that fails to load stays unloaded, so the next use reads it again.
#[derive(Default)]
pub(crate) struct VisionsCache {
    visions: Option<BTreeMap<String, Vision>>,
    channels: Option<BTreeMap<String, Channel>>,
    owners: Option<HashMap<String, Owner>>,
    last_seq: HashMap<String, u64>,
    /// Each loaded file's size and modified time when read or last written, to notice edits
    /// made on disk since.
    stamps: HashMap<&'static str, store::Stamp>,
    /// Each Vision's wakes per channel (in memory only).
    wakes: wakes::WakeSlots,
    /// Today's spend and budget notices (see `budget`).
    budget: budget::Ledger,
    /// What the app was last told of each Vision (see `listing`).
    shown: listing::Shown,
    /// Snapshots plain chat's listings took of who runs which chat (for the scale test).
    marks_built: usize,
}

impl AppState {
    fn visions_cache(&self) -> std::sync::MutexGuard<'_, VisionsCache> {
        self.inner
            .visions
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Notes a store failure in diagnostics (file name and a fixed reason, never contents)
    /// and returns its message.
    fn visions_failed(&self, error: StoreError) -> String {
        let _ = self.inner.diagnostics.append(
            "visions",
            error.event(),
            "warn",
            json!({"file": error.file(), "reason": error.reason()}),
        );
        error.message
    }

    /// Every Vision, by name. Empty while `visions.json` cannot be read.
    pub fn visions_list(&self) -> Vec<Vision> {
        let mut cache = self.visions_cache();
        match cache.visions(&self.inner.visions_dir) {
            Ok(visions) => store::sorted_visions(visions),
            Err(error) => {
                drop(cache);
                self.visions_failed(error);
                Vec::new()
            }
        }
    }

    /// Creates a Vision (empty `id`, no `expected`) or saves one opened at `expected`.
    pub fn vision_save(&self, spec: Vision, expected: Option<&str>) -> Result<Vision, String> {
        let result = self
            .visions_cache()
            .save_vision(&self.inner.visions_dir, spec, expected);
        result.map_err(|error| self.visions_failed(error))
    }

    /// The channels of the project at `project_root`, by name. Empty while `channels.json`
    /// cannot be read.
    pub fn channel_list(&self, project_root: &Path) -> Vec<Channel> {
        let mut cache = self.visions_cache();
        match cache.channels(&self.inner.visions_dir) {
            Ok(channels) => store::project_channels(channels, project_root),
            Err(error) => {
                drop(cache);
                self.visions_failed(error);
                Vec::new()
            }
        }
    }

    /// Creates a channel (empty `id`, no `expected`) or saves one opened at `expected`.
    pub fn channel_save(&self, spec: Channel, expected: Option<&str>) -> Result<Channel, String> {
        let result = self
            .visions_cache()
            .save_channel(&self.inner.visions_dir, spec, expected);
        result.map_err(|error| self.visions_failed(error))
    }

    /// The Vision and channel `thread_id` belongs to, if any. None while `owners.json` cannot
    /// be read.
    pub fn owner_of(&self, thread_id: &str) -> Option<Owner> {
        let mut cache = self.visions_cache();
        match cache.owners(&self.inner.visions_dir) {
            Ok(owners) => owners.get(thread_id).cloned(),
            Err(error) => {
                drop(cache);
                self.visions_failed(error);
                None
            }
        }
    }

    /// Records (or replaces) the owner of `owner.thread_id`.
    pub fn owner_set(&self, owner: Owner) -> Result<(), String> {
        let result = self
            .visions_cache()
            .set_owner(&self.inner.visions_dir, owner);
        self.visions_rebuild_owned();
        result.map_err(|error| self.visions_failed(error))
    }

    /// One Vision, when it exists (and `visions.json` can be read).
    pub fn vision_get(&self, vision_id: &str) -> Option<Vision> {
        self.visions_list()
            .into_iter()
            .find(|vision| vision.id == vision_id)
    }

    /// One channel, when it exists (and `channels.json` can be read).
    pub fn channel_get(&self, channel_id: &str) -> Option<Channel> {
        self.channels_all()
            .into_iter()
            .find(|channel| channel.id == channel_id)
    }

    /// Every channel of every project. Empty while `channels.json` cannot be read.
    pub fn channels_all(&self) -> Vec<Channel> {
        let mut cache = self.visions_cache();
        match cache.channels(&self.inner.visions_dir) {
            Ok(channels) => channels.values().cloned().collect(),
            Err(error) => {
                drop(cache);
                self.visions_failed(error);
                Vec::new()
            }
        }
    }

    /// The owner records `keep` selects. Empty while `owners.json` cannot be read.
    pub fn owners_where(&self, keep: impl Fn(&Owner) -> bool) -> Vec<Owner> {
        let mut cache = self.visions_cache();
        match cache.owners(&self.inner.visions_dir) {
            Ok(owners) => {
                let mut list: Vec<Owner> = owners.values().filter(|o| keep(o)).cloned().collect();
                list.sort_by(|a, b| a.thread_id.cmp(&b.thread_id));
                list
            }
            Err(error) => {
                drop(cache);
                self.visions_failed(error);
                Vec::new()
            }
        }
    }

    /// Sets the hold `hold` picks on `thread_id`'s owner, in one step; None when the chat has
    /// no owner.
    pub(crate) fn owner_hold(
        &self,
        thread_id: &str,
        hold: impl FnOnce(&Owner) -> Hold,
    ) -> Result<Option<Owner>, String> {
        let result = self
            .visions_cache()
            .set_hold(&self.inner.visions_dir, thread_id, hold);
        result.map_err(|error| self.visions_failed(error))
    }

    /// Removes a Vision from the store and from its channels' members.
    pub(crate) fn vision_remove(&self, vision_id: &str) -> Result<(), String> {
        let result = self
            .visions_cache()
            .delete_vision(&self.inner.visions_dir, vision_id);
        result.map_err(|error| self.visions_failed(error))
    }

    /// Removes a channel from the store; its log stays.
    pub(crate) fn channel_remove(&self, channel_id: &str) -> Result<(), String> {
        let result = self
            .visions_cache()
            .delete_channel(&self.inner.visions_dir, channel_id);
        result.map_err(|error| self.visions_failed(error))
    }

    /// Forgets the owner of `thread_id`; nothing happens when it has none.
    pub fn owner_release(&self, thread_id: &str) -> Result<(), String> {
        let result = self
            .visions_cache()
            .release_owner(&self.inner.visions_dir, thread_id);
        self.visions_rebuild_owned();
        result.map_err(|error| self.visions_failed(error))
    }

    /// Appends `entry` to the channel's log and returns its seq.
    pub fn channel_append(&self, channel_id: &str, entry: ChannelEntry) -> Result<u64, String> {
        let result = self
            .visions_cache()
            .append_entry(&self.inner.visions_dir, channel_id, entry);
        result.map_err(|error| self.visions_failed(error))
    }

    /// [`Self::channel_append`] for the server's own writes (a reply, a notice), which no
    /// command announces: a recorded entry announces `visions-changed` with its channel and
    /// Vision on the scheduler's sink.
    pub(super) fn channel_post(
        &self,
        channel_id: &str,
        vision_id: &str,
        entry: ChannelEntry,
    ) -> Result<u64, String> {
        let seq = self.channel_append(channel_id, entry)?;
        if let Some(sink) = self.scheduler_sink() {
            sink.emit_visions_changed(&crate::types::VisionsChange {
                channel_id: Some(channel_id.to_owned()),
                vision_id: Some(vision_id.to_owned()),
            });
        }
        Ok(seq)
    }

    /// The newest entries of the channel's log older than `before_seq` (all when absent),
    /// oldest first, within `max_bytes` of log lines (but at least one entry, even one longer
    /// than that). `next_before` continues the walk back; None means the start was reached.
    pub fn channel_page(
        &self,
        channel_id: &str,
        before_seq: Option<u64>,
        max_bytes: usize,
    ) -> Result<ChannelPage, String> {
        let path = store::channel_log_path(&self.inner.visions_dir, channel_id)
            .map_err(|error| self.visions_failed(error))?;
        channel_log::read_page(&path, before_seq, max_bytes)
            .map_err(|error| self.visions_failed(error))
    }
}
