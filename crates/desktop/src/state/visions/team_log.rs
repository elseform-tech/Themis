//! Team logs: what passes between a chief and one of its workers, one log per worker per channel
//! (`teams/<channel id>/<worker id>.jsonl` under the Visions store), in the same bounded JSONL
//! as channel logs (see `channel_log`). A chief tasks, redirects, pauses and resumes a worker; a
//! worker reports and pushes back. Only a channel's workers have one; its chief writes and reads
//! it through its grant, the worker its own.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::channel_log::{append_line, read_page, LogPage};
use super::store::StoreError;
use super::{AppState, VisionsCache, CHANNELS_FILE};

/// Team logs, `teams/<channel id>/<worker id>.jsonl`, under the Visions store.
const TEAMS_DIR: &str = "teams";

/// One entry of a team log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TeamEntry {
    /// The chief's task.
    Task {
        from_chief: String,
        text: String,
    },
    /// The chief changed the task's course.
    Redirect {
        text: String,
    },
    /// The worker's report back.
    Report {
        text: String,
    },
    /// The worker's objection, with a safer way.
    Pushback {
        text: String,
        safer: String,
    },
    Paused,
    Resumed,
}

impl TeamEntry {
    /// Its kind's name in diagnostics (never its text).
    fn label(&self) -> &'static str {
        match self {
            TeamEntry::Task { .. } => "task",
            TeamEntry::Redirect { .. } => "redirect",
            TeamEntry::Report { .. } => "report",
            TeamEntry::Pushback { .. } => "pushback",
            TeamEntry::Paused => "paused",
            TeamEntry::Resumed => "resumed",
        }
    }
}

/// A page of a team log.
pub type TeamPage = LogPage<TeamEntry>;

/// The log of `worker_id` in `channel_id`, for ids the store could have given.
fn team_log_path(dir: &Path, channel_id: &str, worker_id: &str) -> Result<PathBuf, StoreError> {
    let safe = crate::state::chat_grants::safe_chat_id;
    if !safe(channel_id) || !safe(worker_id) {
        return Err(StoreError::refused(CHANNELS_FILE, "Unknown team member"));
    }
    Ok(dir
        .join(TEAMS_DIR)
        .join(channel_id)
        .join(format!("{worker_id}.jsonl")))
}

impl VisionsCache {
    /// The log of a worker of a saved channel (a member, not its chief).
    fn team_log(
        &mut self,
        dir: &Path,
        channel_id: &str,
        worker_id: &str,
    ) -> Result<PathBuf, StoreError> {
        let path = team_log_path(dir, channel_id, worker_id)?;
        let works_there = self.channels(dir)?.get(channel_id).is_some_and(|channel| {
            channel.members.iter().any(|member| member == worker_id) && !channel.is_chief(worker_id)
        });
        if !works_there {
            return Err(StoreError::refused(CHANNELS_FILE, "Unknown team member"));
        }
        Ok(path)
    }

    fn append_team(
        &mut self,
        dir: &Path,
        channel_id: &str,
        worker_id: &str,
        entry: TeamEntry,
    ) -> Result<u64, StoreError> {
        let path = self.team_log(dir, channel_id, worker_id)?;
        // Ids hold no `/`, so this key is never a channel's.
        let key = format!("{channel_id}/{worker_id}");
        let last = self.last_seq.get(&key).copied();
        match append_line(&path, last, entry) {
            Ok(seq) => {
                self.last_seq.insert(key, seq);
                Ok(seq)
            }
            Err(error) => {
                self.last_seq.remove(&key);
                Err(error)
            }
        }
    }
}

impl AppState {
    /// Appends `entry` to the team log of `worker_id` in `channel_id` and returns its seq.
    pub(crate) fn team_append(
        &self,
        channel_id: &str,
        worker_id: &str,
        entry: TeamEntry,
    ) -> Result<u64, String> {
        let kind = entry.label();
        let result =
            self.visions_cache()
                .append_team(&self.inner.visions_dir, channel_id, worker_id, entry);
        let seq = result.map_err(|error| self.visions_failed(error))?;
        self.visions_note(
            "team_entry",
            serde_json::json!({"channel_id": channel_id, "worker_id": worker_id, "kind": kind, "seq": seq}),
        );
        Ok(seq)
    }

    /// A page of a worker's team log, like [`AppState::channel_page`].
    pub(crate) fn team_page(
        &self,
        channel_id: &str,
        worker_id: &str,
        before_seq: Option<u64>,
        max_bytes: usize,
    ) -> Result<TeamPage, String> {
        let path = self
            .visions_cache()
            .team_log(&self.inner.visions_dir, channel_id, worker_id);
        let path = path.map_err(|error| self.visions_failed(error))?;
        read_page(&path, before_seq, max_bytes).map_err(|error| self.visions_failed(error))
    }
}

/// A worker as its chief's `list_team` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TeamMember {
    pub id: String,
    pub name: String,
    pub role: String,
    pub paused: bool,
}

impl AppState {
    /// The workers of `channel_id` (its members but its chief), in its member order.
    pub(crate) fn team_members(&self, channel_id: &str) -> Vec<TeamMember> {
        let Some(channel) = self.channel_get(channel_id) else {
            return Vec::new();
        };
        channel
            .members
            .iter()
            .filter(|member| !channel.is_chief(member))
            .filter_map(|member| self.vision_get(member))
            .map(|vision| TeamMember {
                paused: vision.paused || channel.paused_workers.contains(&vision.id),
                id: vision.id,
                name: vision.name,
                role: vision.role,
            })
            .collect()
    }

    /// The chief pauses (or resumes) a worker in its channel only; pausing stops the worker's
    /// running chats there. The person's own pause of the Vision is left as it is.
    pub(crate) fn visions_team_pause(
        &self,
        channel_id: &str,
        worker_id: &str,
        paused: bool,
    ) -> Result<(), String> {
        let saved =
            self.visions_cache()
                .team_pause(&self.inner.visions_dir, channel_id, worker_id, paused);
        saved.map_err(|error| self.visions_failed(error))?;
        if paused {
            self.visions_stop_owned(|owner| {
                owner.channel_id == channel_id
                    && owner.vision_id == worker_id
                    && owner.hold != super::Hold::TakenOver
            });
        }
        Ok(())
    }

    /// Whether `vision_id` is paused in `channel_id`: by the person, or by its chief there.
    pub(crate) fn paused_in(&self, channel_id: &str, vision_id: &str) -> bool {
        let person = self
            .vision_get(vision_id)
            .is_none_or(|vision| vision.paused);
        person
            || self.channel_get(channel_id).is_some_and(|channel| {
                channel
                    .paused_workers
                    .iter()
                    .any(|worker| worker == vision_id)
            })
    }

    /// Whether `vision_id` leads `channel_id` now.
    pub(crate) fn leads(&self, channel_id: &str, vision_id: &str) -> bool {
        self.channel_get(channel_id)
            .is_some_and(|channel| channel.is_chief(vision_id))
    }

    /// Whether `vision_id` works in `channel_id` now (a member, not its chief).
    pub(crate) fn works_in(&self, channel_id: &str, vision_id: &str) -> bool {
        self.channel_get(channel_id).is_some_and(|channel| {
            channel.members.iter().any(|member| member == vision_id) && !channel.is_chief(vision_id)
        })
    }
}
