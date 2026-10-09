use super::*;
use crate::types::{AutomationRepeat, AutomationSchedule};
use chrono::{Datelike, NaiveTime, TimeZone};
use chrono_tz::Tz;

impl AppState {
    pub async fn list_automations(&self) -> Vec<Automation> {
        let automations = self.inner.automations.read().await;
        let mut out: Vec<Automation> = automations.values().cloned().collect();
        out.sort_by(|left, right| left.id.cmp(&right.id));
        out
    }

    /// Creates an automation after validating the input. The project root
    /// must exist (stored canonicalized) and `next_run_at` starts at
    /// the next calendar occurrence or now + the legacy interval.
    pub async fn create_automation(&self, input: AutomationInput) -> Result<Automation, String> {
        self.check_automation_input(&input).await?;
        let root = canonical_project_dir(&input.project_root)?;
        let now = Utc::now();
        let interval_mins = if input.schedule.is_some() {
            60 // Calendar schedules do not use the legacy interval field.
        } else {
            u32::try_from(input.interval_mins).map_err(|_| {
                format!(
                    "interval_mins is out of range (got {})",
                    input.interval_mins
                )
            })?
        };
        let next_run_at = next_automation_run(input.schedule.as_ref(), interval_mins, now)?;
        let automation = Automation {
            id: uuid::Uuid::new_v4().to_string(),
            name: input.name,
            project_root: root.to_string_lossy().into_owned(),
            target_thread_id: input.target_thread_id.clone(),
            provider: input.provider,
            model: if input.target_thread_id.is_some() {
                String::new()
            } else {
                input.model
            },
            reasoning_effort: if input.target_thread_id.is_some() {
                None
            } else {
                input.reasoning_effort
            },
            skill_ids: if input.target_thread_id.is_some() {
                Vec::new()
            } else {
                dedup_ids(input.skill_ids)
            },
            interval_mins,
            schedule: input.schedule,
            task: input.task,
            enabled: input.enabled,
            last_run_at: None,
            next_run_at,
            run_count: 0,
        };
        self.inner
            .automations
            .write()
            .await
            .insert(automation.id.clone(), automation.clone());
        self.persist_automations().await;
        Ok(automation)
    }

    /// Replaces the automation `automation_id` with `input`, keeping its id,
    /// history (`last_run_at`, `run_count`), and rescheduling `next_run_at`
    /// to the next calendar occurrence or new interval.
    pub async fn update_automation(
        &self,
        automation_id: String,
        input: AutomationInput,
    ) -> Result<Automation, String> {
        self.check_automation_input(&input).await?;
        let root = canonical_project_dir(&input.project_root)?;
        let interval_mins = if input.schedule.is_some() {
            60 // Calendar schedules do not use the legacy interval field.
        } else {
            u32::try_from(input.interval_mins).map_err(|_| {
                format!(
                    "interval_mins is out of range (got {})",
                    input.interval_mins
                )
            })?
        };
        let now = Utc::now();
        let next_run_at = next_automation_run(input.schedule.as_ref(), interval_mins, now)?;
        let mut automations = self.inner.automations.write().await;
        let automation = automations
            .get_mut(&automation_id)
            .ok_or_else(|| format!("unknown automation '{automation_id}'"))?;
        automation.name = input.name;
        automation.project_root = root.to_string_lossy().into_owned();
        automation.target_thread_id = input.target_thread_id.clone();
        automation.provider = input.provider;
        automation.model = if input.target_thread_id.is_some() {
            String::new()
        } else {
            input.model
        };
        automation.reasoning_effort = if input.target_thread_id.is_some() {
            None
        } else {
            input.reasoning_effort
        };
        automation.skill_ids = if input.target_thread_id.is_some() {
            Vec::new()
        } else {
            dedup_ids(input.skill_ids)
        };
        automation.interval_mins = interval_mins;
        automation.schedule = input.schedule;
        automation.task = input.task;
        automation.enabled = input.enabled;
        automation.next_run_at = next_run_at;
        let updated = automation.clone();
        drop(automations);
        self.persist_automations().await;
        Ok(updated)
    }

    /// Deletes an automation. Threads it created stay as ordinary threads
    /// and its review items stay as history.
    pub async fn delete_automation(&self, automation_id: String) -> Result<(), String> {
        let mut automations = self.inner.automations.write().await;
        if automations.remove(&automation_id).is_none() {
            return Err(format!("unknown automation '{automation_id}'"));
        }
        drop(automations);
        self.persist_automations().await;
        Ok(())
    }

    /// Flips an automation's `enabled` flag. Enabling reschedules
    /// `next_run_at` to its next scheduled occurrence.
    pub async fn set_automation_enabled(
        &self,
        automation_id: String,
        enabled: bool,
    ) -> Result<Automation, String> {
        let now = Utc::now();
        let mut automations = self.inner.automations.write().await;
        let automation = automations
            .get_mut(&automation_id)
            .ok_or_else(|| format!("unknown automation '{automation_id}'"))?;
        if enabled {
            automation.next_run_at =
                next_automation_run(automation.schedule.as_ref(), automation.interval_mins, now)?;
        }
        automation.enabled = enabled;
        let updated = automation.clone();
        drop(automations);
        self.persist_automations().await;
        Ok(updated)
    }

    /// Validates automation input: non-empty name and task, an existing
    /// project dir, a usable Go model, existing skill ids, and
    /// `interval_mins >= 1`.
    async fn check_automation_input(&self, input: &AutomationInput) -> Result<(), String> {
        if input.name.trim().is_empty() {
            return Err("automation name must not be empty".to_owned());
        }
        if input.task.trim().is_empty() {
            return Err("automation task must not be empty".to_owned());
        }
        if input.schedule.is_none() && input.interval_mins < 1 {
            return Err(format!(
                "interval_mins must be at least 1 (got {})",
                input.interval_mins
            ));
        }
        if let Some(schedule) = &input.schedule {
            next_calendar_run(schedule, Utc::now())?;
        }
        let root = canonical_project_dir(&input.project_root)?;
        let legacy: Vec<_> = self
            .inner
            .skills
            .read()
            .await
            .values()
            .map(Skill::core_skill)
            .collect();
        self.plugin_store(Some(root))
            .resolve_prompt(&input.task, &legacy)
            .map_err(|e| e.to_string())?;
        if input.provider != ProviderKind::Go {
            return Err("Only OpenCode Go is supported".to_owned());
        }
        if let Some(thread_id) = &input.target_thread_id {
            let threads = self.inner.threads.read().await;
            let thread = threads
                .get(thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            if thread.project_root != canonical_project_dir(&input.project_root)? {
                return Err("Selected thread is in a different project".to_owned());
            }
        } else {
            self.check_skill_ids(&input.skill_ids).await?;
        }
        Ok(())
    }

    /// Test hook: forcibly reschedules an automation (lets tick tests make
    /// one due without waiting out its interval).
    pub async fn set_automation_next_run_for_test(
        &self,
        automation_id: &str,
        next_run_at: String,
    ) -> Result<Automation, String> {
        let mut automations = self.inner.automations.write().await;
        let automation = automations
            .get_mut(automation_id)
            .ok_or_else(|| format!("unknown automation '{automation_id}'"))?;
        automation.next_run_at = next_run_at;
        let updated = automation.clone();
        drop(automations);
        self.persist_automations().await;
        Ok(updated)
    }

    /// Manually triggers one automation run. Unlike the scheduler tick, this
    /// works even when the automation is disabled or the global
    /// `automations_enabled` kill-switch is off — but the concurrency gate
    /// still applies.
    pub async fn run_automation_now(
        &self,
        sink: Arc<dyn EventSink>,
        automation_id: String,
    ) -> Result<RunAutomationNow, String> {
        let automation = {
            let automations = self.inner.automations.read().await;
            automations
                .get(&automation_id)
                .cloned()
                .ok_or_else(|| format!("unknown automation '{automation_id}'"))?
        };
        if !Path::new(&automation.project_root).is_dir() {
            return Err(format!(
                "automation '{}' cannot run: project_root '{}' no longer exists",
                automation.id, automation.project_root
            ));
        }
        let (thread_id, run_id) = self.fire_automation(&sink, &automation).await?;
        Ok(RunAutomationNow {
            automation_id: automation.id,
            thread_id,
            run_id,
        })
    }

    /// Runs one scheduler pass: fires every due automation (enabled, with
    /// `next_run_at <= now`). A saturated concurrency gate or a vanished
    /// project dir skips the automation WITHOUT advancing its schedule, so
    /// it is retried on the next tick. No-op while the global
    /// `automations_enabled` kill-switch is off.
    ///
    /// Firing runs in the selected thread or creates a new one. There is
    /// deliberately no path to [`AppState::merge_thread`] — output waits for
    /// human review.
    pub async fn tick_automations_once(&self, sink: &Arc<dyn EventSink>) {
        if !self.inner.settings.get().await.automations_enabled {
            return;
        }
        let now = Utc::now();
        let due: Vec<Automation> = {
            let automations = self.inner.automations.read().await;
            let mut due: Vec<Automation> = automations
                .values()
                .filter(|automation| {
                    automation.enabled && next_run_due(&automation.next_run_at, now)
                })
                .cloned()
                .collect();
            due.sort_by(|left, right| left.id.cmp(&right.id));
            due
        };
        for automation in due {
            if !Path::new(&automation.project_root).is_dir() {
                eprintln!(
                    "themis: automation '{}' skipped: project_root '{}' no longer exists",
                    automation.id, automation.project_root
                );
                continue;
            }
            if let Err(err) = self.fire_automation(sink, &automation).await {
                eprintln!("themis: automation '{}' did not fire: {err}", automation.id);
            }
        }
    }

    /// Fires one automation in its selected thread or a new project thread.
    async fn fire_automation(
        &self,
        sink: &Arc<dyn EventSink>,
        automation: &Automation,
    ) -> Result<(String, String), String> {
        // Gate pre-check (the send path re-checks atomically under the state
        // lock; this avoids minting a stray empty thread in the common
        // saturated case).
        let limit = usize::try_from(self.inner.settings.get().await.concurrency_limit)
            .unwrap_or(usize::MAX);
        if self.inner.running_count.load(Ordering::SeqCst) >= limit {
            return Err(format!(
                "concurrency limit reached: automation '{}' stays due for the next tick",
                automation.id
            ));
        }
        let (thread_id, model, effort) = if let Some(thread_id) = &automation.target_thread_id {
            let threads = self.inner.threads.read().await;
            let thread = threads.get(thread_id).ok_or_else(|| {
                format!("automation target thread '{thread_id}' no longer exists")
            })?;
            if thread.project_root != canonical_project_dir(&automation.project_root)? {
                return Err("Automation target thread moved to a different project".to_owned());
            }
            (
                thread_id.clone(),
                thread.model.clone(),
                thread.reasoning_effort.clone(),
            )
        } else {
            (
                String::new(),
                automation.model.clone(),
                automation.reasoning_effort.clone(),
            )
        };
        if let Some(effort) = &effort {
            let models = self.list_go_models().await?;
            if !models
                .iter()
                .any(|item| item.id == model && item.effort_levels.contains(effort))
            {
                return Err(format!(
                    "Effort '{effort}' is unavailable for model '{}'",
                    model
                ));
            }
        }
        let now = Utc::now();
        let title = format!(
            "Automation {} — {}",
            automation.name,
            now.to_rfc3339_opts(SecondsFormat::Secs, true)
        );
        let thread_id = if automation.target_thread_id.is_some() {
            thread_id
        } else {
            let info = self
                .create_thread(
                    automation.project_root.clone(),
                    automation.provider,
                    Some(model),
                )
                .await?;
            {
                let mut threads = self.inner.threads.write().await;
                let record = threads
                    .get_mut(&info.id)
                    .ok_or_else(|| format!("automation thread '{}' vanished", info.id))?;
                record.title = title.clone();
                record.titled = true;
                record.reasoning_effort = automation.reasoning_effort.clone();
                record.skill_ids = automation.skill_ids.clone();
            }
            self.persist_registry().await;
            info.id
        };
        let run_id = uuid::Uuid::new_v4().to_string();
        // Register BEFORE spawning: the pump attributes terminal events via
        // this map, and registering after the spawn could miss a fast failure.
        self.inner
            .automation_runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                run_id.clone(),
                AutomationRunContext {
                    automation_id: automation.id.clone(),
                    thread_id: thread_id.clone(),
                    title,
                },
            );
        match self
            .send_message_with_options(
                Arc::clone(sink),
                thread_id.clone(),
                format!(
                    "{}{}",
                    automation.task,
                    automation
                        .skill_ids
                        .iter()
                        .map(|id| format!(" [[skill:{id}]]"))
                        .collect::<String>()
                ),
                run_id.clone(),
                effort,
                Vec::new(),
            )
            .await
        {
            Ok(_) => Ok((thread_id, run_id)),
            Err(err) => {
                self.inner
                    .automation_runs
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&run_id);
                if err.contains(GATE_SATURATED_MARKER) || err.contains("is busy") {
                    // Lost a gate race with another run: skip WITHOUT
                    // advancing; the automation stays due for the next tick.
                    return Err(err);
                }
                // No run exists, so no review item is owed — but advance the
                // schedule so a persistently broken automation (missing key,
                // bad model) doesn't error-loop every tick.
                self.record_automation_spawn_failure(&automation.id).await;
                Err(err)
            }
        }
    }

    /// Advances an automation's schedule after a spawn failure (no run, so no
    /// review item and no `last_run_at` run record beyond the timestamp).
    async fn record_automation_spawn_failure(&self, automation_id: &str) {
        let now = Utc::now();
        {
            let mut automations = self.inner.automations.write().await;
            if let Some(automation) = automations.get_mut(automation_id) {
                automation.last_run_at = Some(now.to_rfc3339_opts(SecondsFormat::Secs, true));
                advance_automation_schedule(automation, now);
                automation.run_count += 1;
            }
        }
        self.persist_automations().await;
    }

    /// Attributes a terminal run event to its automation (a no-op for
    /// ordinary runs). Called from the run-event pump while the thread is
    /// already idle but before the terminal event is emitted.
    pub(super) async fn complete_automation_run(
        &self,
        sink: &Arc<dyn EventSink>,
        run_id: &str,
        event: &RunEvent,
    ) {
        let summary = match event {
            RunEvent::Finished { result } => result.clone(),
            RunEvent::Incomplete { result } => result.clone(),
            RunEvent::Failed { error } => format!("failed: {error}"),
            _ => return,
        };
        self.finish_automation_run(sink, run_id, summary).await;
    }

    /// Records one finished automation run: creates the pending review item,
    /// emits `review-item-added`, and advances the automation's schedule
    /// (`last_run_at`/`next_run_at`/`run_count`) on success and failure
    /// alike. The take-once registry makes double counting impossible; an
    /// automation deleted mid-run keeps its review item as history with no
    /// schedule to advance.
    pub(super) async fn finish_automation_run(
        &self,
        sink: &Arc<dyn EventSink>,
        run_id: &str,
        summary: String,
    ) {
        let context = self
            .inner
            .automation_runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(run_id);
        let Some(context) = context else {
            return;
        };
        let now = Utc::now();
        let item = ReviewItem {
            id: uuid::Uuid::new_v4().to_string(),
            automation_id: context.automation_id.clone(),
            thread_id: context.thread_id,
            created_at: now.to_rfc3339_opts(SecondsFormat::Secs, true),
            title: context.title,
            summary,
            status: ReviewStatus::Pending,
        };
        {
            let mut reviews = self.inner.reviews.write().await;
            reviews.insert(item.id.clone(), item.clone());
        }
        self.persist_reviews().await;
        {
            let mut automations = self.inner.automations.write().await;
            if let Some(automation) = automations.get_mut(&context.automation_id) {
                automation.last_run_at = Some(now.to_rfc3339_opts(SecondsFormat::Secs, true));
                advance_automation_schedule(automation, now);
                automation.run_count += 1;
            }
        }
        self.persist_automations().await;
        sink.emit_review_item(&item);
    }
}

/// Select the next local calendar occurrence strictly after `after`.
/// A DST gap moves to the first valid minute; a fold uses the earlier instant
/// once, so a completed occurrence is never repeated within that local day.
fn next_calendar_run(
    schedule: &AutomationSchedule,
    after: DateTime<Utc>,
) -> Result<String, String> {
    let zone: Tz = schedule
        .timezone
        .parse()
        .map_err(|_| "Choose a valid IANA time zone.".to_owned())?;
    let time = NaiveTime::parse_from_str(&schedule.time, "%H:%M")
        .ok()
        .filter(|time| time.format("%H:%M").to_string() == schedule.time)
        .ok_or_else(|| "Choose a time in HH:MM format.".to_owned())?;
    if schedule.repeat == AutomationRepeat::Weekly && schedule.weekday > 6 {
        return Err("Choose a weekday from Monday (0) through Sunday (6).".to_owned());
    }
    let first_date = after.with_timezone(&zone).date_naive();
    for offset in 0..=8 {
        let date = first_date + chrono::Duration::days(offset);
        let day = date.weekday().num_days_from_monday();
        let matches = match schedule.repeat {
            AutomationRepeat::Daily => true,
            AutomationRepeat::Weekdays => day < 5,
            AutomationRepeat::Weekly => day == u32::from(schedule.weekday),
        };
        if !matches {
            continue;
        }
        let mut local = date.and_time(time);
        // Zone transitions can skip a whole date. Bound the search to this
        // calendar day; in that case select the next matching date instead.
        while local.date() == date {
            if let Some(candidate) = zone.from_local_datetime(&local).earliest() {
                if candidate.with_timezone(&Utc) > after {
                    return Ok(candidate
                        .with_timezone(&Utc)
                        .to_rfc3339_opts(SecondsFormat::Secs, true));
                }
                break;
            }
            local += chrono::Duration::minutes(1);
        }
    }
    Err("No upcoming occurrence in this time zone; choose another schedule.".to_owned())
}

fn next_automation_run(
    schedule: Option<&AutomationSchedule>,
    interval_mins: u32,
    after: DateTime<Utc>,
) -> Result<String, String> {
    match schedule {
        Some(schedule) => next_calendar_run(schedule, after),
        None => Ok(rfc3339_plus_minutes(after, i64::from(interval_mins))),
    }
}

fn advance_automation_schedule(automation: &mut Automation, after: DateTime<Utc>) {
    match next_automation_run(
        automation.schedule.as_ref(),
        automation.interval_mins,
        after,
    ) {
        Ok(next) => automation.next_run_at = next,
        Err(error) => {
            automation.enabled = false;
            eprintln!("automation {} disabled: {error}", automation.id);
        }
    }
}

#[cfg(test)]
mod schedule_tests {
    use super::*;

    fn schedule(repeat: AutomationRepeat, time: &str) -> AutomationSchedule {
        AutomationSchedule {
            repeat,
            time: time.into(),
            timezone: "Europe/Berlin".into(),
            weekday: 0,
        }
    }
    fn next(schedule: &AutomationSchedule, after: &str) -> String {
        next_calendar_run(
            schedule,
            DateTime::parse_from_rfc3339(after)
                .unwrap()
                .with_timezone(&Utc),
        )
        .unwrap()
    }

    #[test]
    fn daily_uses_local_clock_and_is_strictly_future() {
        let daily = schedule(AutomationRepeat::Daily, "09:00");
        assert_eq!(next(&daily, "2026-10-06T06:59:00Z"), "2026-10-06T07:00:00Z");
        assert_eq!(next(&daily, "2026-10-06T07:00:00Z"), "2026-10-07T07:00:00Z");
    }
    #[test]
    fn weekdays_and_weekly_skip_non_matching_days() {
        assert_eq!(
            next(
                &schedule(AutomationRepeat::Weekdays, "09:00"),
                "2026-10-09T08:00:00Z"
            ),
            "2026-10-12T07:00:00Z"
        );
        let mut weekly = schedule(AutomationRepeat::Weekly, "09:00");
        weekly.weekday = 4;
        assert_eq!(
            next(&weekly, "2026-10-09T08:00:00Z"),
            "2026-10-16T07:00:00Z"
        );
    }
    #[test]
    fn daylight_saving_changes_preserve_local_hour() {
        let daily = schedule(AutomationRepeat::Daily, "09:00");
        assert_eq!(next(&daily, "2026-03-28T08:00:00Z"), "2026-03-29T07:00:00Z");
        assert_eq!(next(&daily, "2026-10-24T07:00:00Z"), "2026-10-25T08:00:00Z");
    }
    #[test]
    fn daylight_saving_gap_moves_forward_and_fold_runs_once() {
        let daily = schedule(AutomationRepeat::Daily, "02:30");
        assert_eq!(next(&daily, "2026-03-28T02:00:00Z"), "2026-03-29T01:00:00Z");
        assert_eq!(next(&daily, "2026-10-24T02:00:00Z"), "2026-10-25T00:30:00Z");
        assert_eq!(next(&daily, "2026-10-25T00:31:00Z"), "2026-10-26T01:30:00Z");
    }
    #[test]
    fn invalid_calendar_settings_are_actionable_errors() {
        let mut daily = schedule(AutomationRepeat::Daily, "24:00");
        assert!(next_calendar_run(&daily, Utc::now())
            .unwrap_err()
            .contains("HH:MM"));
        daily.time = "09:00".into();
        daily.timezone = "not-a-zone".into();
        assert!(next_calendar_run(&daily, Utc::now())
            .unwrap_err()
            .contains("time zone"));
        daily.timezone = "Europe/Berlin".into();
        daily.repeat = AutomationRepeat::Weekly;
        daily.weekday = 7;
        assert!(next_calendar_run(&daily, Utc::now())
            .unwrap_err()
            .contains("weekday"));
    }
}
