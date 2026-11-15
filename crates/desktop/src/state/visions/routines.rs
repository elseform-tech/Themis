//! Routines: a chief's standing instruction on a weekly schedule (days and a local `HH:MM`).
//! When one is due, the Visions tick wakes the chief with it as if the person had written it.
//! A chief adds or changes routines within its channel's `undoable` decision and the limits in
//! Settings › Visions, and each add or change posts one line in the channel; the person pauses or
//! removes any routine from the chief's profile.
//!
//! Quiet hours (`visions.quiet_hours`) hold routines and every other agent-started wake; the
//! person's own messages always go through. Fires missed while the app was closed or quiet
//! collapse to one at the next allowed moment. A routine's wake spends the channel's budget and
//! waits when it is used up, like any wake. Kept in `routines.json` under the Visions store,
//! written by the server under the Visions lock; diagnostics carry ids only.

use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration, Local, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::json;
use themis_core::configuration::QuietHours;
use themis_core::visions::Decision;

use super::store::{read_store, write_store};
use super::{AppState, ChannelEntry, WakeReason};
use crate::sink::EventSink;

const ROUTINES_FILE: &str = "routines.json";
const MAX_INSTRUCTION_CHARS: usize = 2000;

/// When a routine fires: on these weekdays (0 Sunday to 6 Saturday) at `time` (`HH:MM`, local).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub days: Vec<u8>,
    pub time: String,
}

/// A chief's standing instruction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Routine {
    /// Empty when a chief adds one; the server gives it one.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub chief: String,
    #[serde(default)]
    pub channel_id: String,
    pub schedule: Schedule,
    pub instruction: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    /// When it is next due (Unix ms), set by the server.
    #[serde(default)]
    pub next_due_ms: i64,
}

fn enabled() -> bool {
    true
}

impl Schedule {
    fn check(&self) -> Result<(), String> {
        let days_ok = !self.days.is_empty() && self.days.iter().all(|day| *day <= 6);
        if !days_ok || QuietHours::minutes(&self.time).is_none() {
            return Err(
                "A routine runs on weekdays 0 (Sunday) to 6 (Saturday) at an HH:MM time".to_owned(),
            );
        }
        Ok(())
    }

    /// The first moment after `after` this schedule is due (Unix ms).
    fn next_after(&self, after: DateTime<Local>) -> i64 {
        let minutes = QuietHours::minutes(&self.time).unwrap_or(0);
        for offset in 0..=7 {
            let date = after.date_naive() + Duration::days(offset);
            let weekday = date.weekday().num_days_from_sunday() as u8;
            if !self.days.contains(&weekday) {
                continue;
            }
            let naive = date.and_hms_opt(minutes / 60, minutes % 60, 0);
            let due = naive.and_then(|naive| Local.from_local_datetime(&naive).earliest());
            if let Some(due) = due.filter(|due| *due > after) {
                return due.timestamp_millis();
            }
        }
        i64::MAX
    }
}

impl AppState {
    fn routines_load(&self) -> Result<Vec<Routine>, String> {
        read_store(&self.inner.visions_dir, ROUTINES_FILE)
            .map_err(|error| self.visions_failed(error))
    }

    fn routines_save(&self, routines: &[Routine]) -> Result<(), String> {
        write_store(&self.inner.visions_dir, ROUTINES_FILE, &routines)
            .map_err(|error| self.visions_failed(error))
    }

    /// Now, for a chief's routine call.
    pub(crate) fn visions_clock(&self) -> DateTime<Local> {
        self.visions_now()
    }

    /// Now, for routines and quiet hours (a test may set it).
    pub(super) fn visions_now(&self) -> DateTime<Local> {
        self.visions_cache().clock.unwrap_or_else(Local::now)
    }

    #[cfg(test)]
    pub(super) fn visions_set_clock_for_test(&self, now: Option<DateTime<Local>>) {
        self.visions_cache().clock = now;
    }

    /// Whether Settings › Visions' quiet hours hold agent-started wakes at `now`.
    pub(super) fn visions_quiet_at(&self, now: DateTime<Local>) -> bool {
        let Ok(config) = self.runtime_configuration(None, None) else {
            return false;
        };
        let minute = now.time().signed_duration_since(chrono::NaiveTime::MIN);
        let minute = u32::try_from(minute.num_minutes()).unwrap_or(0);
        config
            .config
            .visions
            .quiet_hours
            .is_some_and(|quiet| quiet.contains(minute))
    }

    /// The routines of `channel_id`, in the order they were added.
    pub fn visions_routines(&self, channel_id: &str) -> Vec<Routine> {
        self.routines_load()
            .unwrap_or_default()
            .into_iter()
            .filter(|routine| routine.channel_id == channel_id)
            .collect()
    }

    /// `chief`, leading `channel_id`, adds a routine or changes one of its own, within its
    /// channel's `undoable` decision and the limits; posts one line in the channel.
    pub(crate) fn visions_save_routine(
        &self,
        channel_id: &str,
        chief: &str,
        mut routine: Routine,
        now: DateTime<Local>,
    ) -> Result<Routine, String> {
        let channel = self
            .channel_get(channel_id)
            .filter(|channel| channel.is_chief(chief))
            .ok_or_else(|| "Only the channel's chief of staff keeps its routines".to_owned())?;
        let config = self
            .runtime_configuration(Some(&channel.project_root), None)?
            .config
            .visions;
        if !super::vision_decisions().contains(&Decision::Undoable) {
            return Err(themis_core::prompts::AGENT_CLI_ROUTINE_ABOVE_LEVEL.to_owned());
        }
        routine.schedule.check()?;
        let instruction = routine.instruction.trim().to_owned();
        if instruction.is_empty() || instruction.chars().count() > MAX_INSTRUCTION_CHARS {
            return Err(format!(
                "A routine's instruction is some text of at most {MAX_INSTRUCTION_CHARS} characters"
            ));
        }
        routine.instruction = instruction;
        routine.chief = chief.to_owned();
        routine.channel_id = channel_id.to_owned();
        routine.next_due_ms = routine.schedule.next_after(now);
        let changed = !routine.id.is_empty();
        {
            let _guard = self.visions_cache();
            let mut routines = self.routines_load()?;
            if changed {
                // Routines belong to the channel: its chief now changes any of them.
                let at = routines
                    .iter()
                    .position(|old| old.id == routine.id && old.channel_id == channel_id)
                    .ok_or("That routine is not this channel's")?;
                routine.enabled = routines[at].enabled;
                routines[at] = routine.clone();
            } else {
                let mine = routines
                    .iter()
                    .filter(|old| old.channel_id == channel_id)
                    .count();
                if mine >= config.routines_per_chief as usize {
                    return Err(format!(
                        "A chief keeps at most {} routines; remove one first",
                        config.routines_per_chief
                    ));
                }
                routine.id = uuid::Uuid::new_v4().to_string();
                routines.push(routine.clone());
            }
            self.routines_save(&routines)?;
        }
        let entry = ChannelEntry::RoutineSet {
            vision_id: chief.to_owned(),
            routine_id: routine.id.clone(),
            schedule: routine.schedule.clone(),
            instruction: routine.instruction.clone(),
            changed,
        };
        let _ = self.channel_post(channel_id, chief, entry);
        self.visions_note(
            "routine_saved",
            json!({"routine_id": routine.id, "channel_id": channel_id, "changed": changed}),
        );
        Ok(routine)
    }

    /// Removes a routine: the person's (any), or a chief's (its own).
    pub fn visions_delete_routine(&self, routine_id: &str) -> Result<(), String> {
        self.routines_edit(routine_id, None, |_| None)
    }

    /// A chief removes one of its channel's routines.
    pub(crate) fn visions_chief_deletes_routine(
        &self,
        channel_id: &str,
        routine_id: &str,
    ) -> Result<(), String> {
        self.routines_edit(routine_id, Some(channel_id), |_| None)
    }

    /// The person turns a routine on or off (on again: next due from now).
    pub fn visions_set_routine_enabled(&self, routine_id: &str, on: bool) -> Result<(), String> {
        let now = self.visions_now();
        self.routines_edit(routine_id, None, |mut routine| {
            routine.enabled = on;
            if on {
                routine.next_due_ms = routine.schedule.next_after(now);
            }
            Some(routine)
        })
    }

    /// Applies `edit` to one routine (`None` removes it), when it is `channel`'s (if given).
    fn routines_edit(
        &self,
        routine_id: &str,
        channel: Option<&str>,
        edit: impl FnOnce(Routine) -> Option<Routine>,
    ) -> Result<(), String> {
        let _guard = self.visions_cache();
        let mut routines = self.routines_load()?;
        let at = routines
            .iter()
            .position(|r| {
                r.id == routine_id && channel.is_none_or(|channel| r.channel_id == channel)
            })
            .ok_or("No such routine")?;
        if let Some(edited) = edit(routines.remove(at)) {
            routines.insert(at, edited);
        }
        self.routines_save(&routines)
    }

    /// Fires the routines due at `now` (once each, however many fires were missed), outside
    /// quiet hours; when quiet hours have just ended, what waited on them may run.
    pub(crate) fn visions_routines_tick(&self, sink: &Arc<dyn EventSink>, now: DateTime<Local>) {
        let quiet = self.visions_quiet_at(now);
        let was_quiet = std::mem::replace(&mut self.visions_cache().was_quiet, quiet);
        if quiet {
            return;
        }
        if was_quiet {
            self.visions_cache().awake_in_quiet.clear();
            self.visions_wake_waiting();
        }
        let now_ms = now.timestamp_millis();
        // Read before the lock is taken (reading them takes it).
        let channels: std::collections::HashSet<String> = self
            .channels_all()
            .into_iter()
            .map(|channel| channel.id)
            .collect();
        let due: Vec<Routine> = {
            let _guard = self.visions_cache();
            let Ok(mut routines) = self.routines_load() else {
                return;
            };
            // A deleted channel's routines go with it.
            let before = routines.len();
            routines.retain(|routine| channels.contains(&routine.channel_id));
            let pruned = routines.len() != before;
            let mut due = Vec::new();
            for routine in routines.iter_mut() {
                if routine.enabled && routine.next_due_ms <= now_ms {
                    due.push(routine.clone());
                    routine.next_due_ms = routine.schedule.next_after(now);
                }
            }
            if (due.is_empty() && !pruned) || self.routines_save(&routines).is_err() {
                return;
            }
            due
        };
        for routine in due {
            let Some(chief) = self
                .channel_get(&routine.channel_id)
                .map(|channel| channel.chief)
                .filter(|chief| !chief.is_empty())
            else {
                continue;
            };
            // As if the person had written it: the channel may wake `max_wakes` times again.
            self.visions_reset_wakes(&routine.channel_id);
            let reason = WakeReason::Routine(routine.id.clone(), routine.instruction.clone());
            self.visions_wake(sink, &routine.channel_id, &chief, reason);
        }
    }
}
