//! Applying a job's answer: memory changes, plan record ops, tasks and decisions, and the note to a running agent.

use super::*;

impl AppState {
    /// Applies what the model planned and, after a pass, puts one memory line under the reply.
    pub(super) fn apply_memory_job(
        &self,
        sink: &dyn EventSink,
        job: &Job,
        (model_index, model): (usize, &str),
        answered: Answered,
    ) {
        let Answered {
            planned,
            mut counts,
            plan,
            tasks,
            decisions,
        } = answered;
        let project_root = match job {
            Job::Pass { turn, .. } => Some(turn.project_root.clone()),
            Job::Consolidate { project_root, .. } => project_root.clone(),
        };
        let by = job.kind();
        let mut changes = Vec::new();
        // What each saved or updated memory now says, for a run still working.
        let mut bodies: Vec<(String, String)> = Vec::new();
        for proposal in planned {
            let scope = proposal.scope();
            let Ok(store) = self.memory_store(scope, project_root.as_deref()) else {
                counts.failed += 1;
                continue;
            };
            let change = |action: &str, name: &str, description: &str| MemoryChange {
                action: action.to_owned(),
                scope: scope.as_str().to_owned(),
                name: name.to_owned(),
                description: description.to_owned(),
                model: model.to_owned(),
                model_index,
                project_root: (scope == MemoryScope::Project)
                    .then(|| {
                        project_root
                            .as_ref()
                            .map(|root| root.to_string_lossy().into_owned())
                    })
                    .flatten(),
            };
            match proposal {
                pass::Proposal::Delete { name, .. } => match store.delete(&name, None) {
                    Ok(memory) => {
                        counts.deleted += 1;
                        self.note_memory("deleted", scope, memory.kind, &store, by);
                        changes.push(change("removed", &name, &memory.description));
                    }
                    Err(_) => counts.failed += 1,
                },
                pass::Proposal::Save {
                    mode,
                    memory,
                    renamed_from,
                    ..
                } => {
                    if store.save(&memory, mode).is_err() {
                        counts.failed += 1;
                        continue;
                    }
                    // A rename removes the old memory only once the new one is saved.
                    if let Some(old) = renamed_from {
                        match store.delete(&old, None) {
                            Ok(gone) => {
                                counts.deleted += 1;
                                self.note_memory("deleted", scope, gone.kind, &store, by);
                                changes.push(change("removed", &old, &gone.description));
                            }
                            Err(_) => counts.failed += 1,
                        }
                    }
                    let verb = match mode {
                        SaveMode::Create => {
                            counts.saved += 1;
                            "saved"
                        }
                        SaveMode::Update => {
                            counts.updated += 1;
                            "updated"
                        }
                    };
                    self.note_memory(verb, scope, memory.kind, &store, by);
                    bodies.push((memory.name.clone(), memory.body.clone()));
                    changes.push(change(verb, &memory.name, &memory.description));
                }
            }
        }
        let mut plan_changed = false;
        if let (
            Job::Pass {
                thread_id, turn, ..
            },
            Some(plan),
        ) = (job, plan)
        {
            if let Some(change) = self.apply_plan_op(thread_id, turn, plan, (model_index, model)) {
                plan_changed = true;
                bodies.push((change.name.clone(), change.description.clone()));
                changes.push(change);
            }
        }
        if let Job::Pass {
            thread_id, turn, ..
        } = job
        {
            if self.apply_tasks_and_decisions(thread_id, turn, tasks, &decisions) {
                plan_changed = true;
            }
        }
        let outcome = if counts.changed() || plan_changed {
            "changed"
        } else {
            "nothing"
        };
        let mut log = json!({"kind": by, "model_index": model_index, "outcome": outcome,
            "counts": serde_json::to_value(counts).unwrap_or_default()});
        if let Job::Pass {
            thread_id, run_id, ..
        } = job
        {
            log["thread_id"] = thread_id.as_str().into();
            log["run_id"] = run_id.as_str().into();
            if changes.is_empty() {
                self.memory_working(sink, (thread_id, run_id), "done");
            }
            self.tell_running_agent(thread_id, &changes, &bodies);
            self.announce_memory(sink, (thread_id, run_id), changes);
        }
        let _ = self
            .inner
            .diagnostics
            .append("memory", "job_done", "info", log);
    }

    /// Reads memory back into the chat's run while it still works: what changed reaches it at
    /// its next step as a runtime note, so a long run (a goal) never works from stale memory.
    /// Applies the pass's plan op to the project's plan records. A new record copies the plan
    /// reply the person answered word for word and is tied to the chat while its goal runs.
    /// Returns the change line for the chat; logs metadata only.
    fn apply_plan_op(
        &self,
        thread_id: &str,
        turn: &MemoryTurn,
        plan: Result<PlanOp, OpRefused>,
        (model_index, model): (usize, &str),
    ) -> Option<MemoryChange> {
        let today = themis_core::memory::today();
        let mut log = json!({"thread_id": thread_id});
        let result: Result<(&str, PlanRecord), &str> = (|| {
            let op = plan.map_err(|refused| match refused {
                OpRefused::Invalid => "invalid",
                OpRefused::Secret => "sensitive",
            })?;
            log["op"] = match &op {
                PlanOp::Create { .. } => "create",
                PlanOp::Change { .. } => "change",
                PlanOp::Done { .. } => "done",
                PlanOp::Delete { .. } => "delete",
            }
            .into();
            let store = self.plan_store(&turn.project_root).map_err(|_| "failed")?;
            let existing = store.read(op.name()).map_err(|_| "unreadable")?;
            match (op, existing) {
                (PlanOp::Create { name, title }, None) => {
                    if turn.answered.trim().is_empty() {
                        return Err("no_plan_reply");
                    }
                    let building = self.goal_for(thread_id).is_some_and(|goal| {
                        !matches!(goal.status, crate::types::GoalStatus::Reached)
                    });
                    let record = PlanRecord::new(
                        &name,
                        &title,
                        &turn.answered,
                        building.then_some(thread_id),
                        &today,
                    );
                    store.save(&record, true).map_err(|_| "failed")?;
                    Ok(("saved", record))
                }
                (PlanOp::Create { .. }, Some(_)) => Err("exists"),
                (PlanOp::Change { change, edits, .. }, Some(mut record)) => {
                    let (applied, missed) = record.change(&change, &edits, &today);
                    // A step the change added becomes a task too.
                    record.seed_tasks(&today);
                    log["edits_applied"] = applied.into();
                    log["edits_missed"] = missed.into();
                    store.save(&record, false).map_err(|_| "failed")?;
                    Ok(("updated", record))
                }
                (
                    PlanOp::Done {
                        results,
                        evidence,
                        lessons,
                        ..
                    },
                    Some(mut record),
                ) => {
                    // A goal's evidence, copied by Themis, wins over the model's.
                    let evidence = if record.evidence.is_empty() {
                        evidence
                    } else {
                        Vec::new()
                    };
                    record.done(&results, &evidence, &lessons, &today);
                    store.save(&record, false).map_err(|_| "failed")?;
                    Ok(("updated", record))
                }
                (PlanOp::Delete { name }, Some(_)) => {
                    let record = store.delete(&name).map_err(|_| "failed")?;
                    Ok(("removed", record))
                }
                (_, None) => Err("missing"),
            }
        })();
        log["outcome"] = match &result {
            Ok((action, _)) => (*action).into(),
            Err(why) => (*why).into(),
        };
        let _ = self.inner.diagnostics.append("memory", "plan", "info", log);
        let (action, record) = result.ok()?;
        Some(MemoryChange {
            action: action.to_owned(),
            scope: "project".to_owned(),
            name: format!("plans/{}", record.name),
            description: record.title,
            model: model.to_owned(),
            model_index,
            project_root: Some(turn.project_root.to_string_lossy().into_owned()),
        })
    }

    /// Applies the pass's task ops and decisions: a task naming a plan record that exists goes
    /// to that record, any other to the project's tasks file; a decision goes to its record.
    /// They stay out of the chat's memory line (the agent reads them next run); `logs/` gets
    /// counts only. Returns whether anything changed.
    fn apply_tasks_and_decisions(
        &self,
        thread_id: &str,
        turn: &MemoryTurn,
        (ops, refused): (Vec<themis_core::memory::tasks::TaskOp>, usize),
        decisions: &[pass::Decision],
    ) -> bool {
        if ops.is_empty() && decisions.is_empty() && refused == 0 {
            return false;
        }
        let today = themis_core::memory::today();
        let Ok(store) = self.plan_store(&turn.project_root) else {
            return false;
        };
        let mut records: std::collections::BTreeMap<String, PlanRecord> = Default::default();
        let mut loose = store.tasks().read();
        let (mut applied, mut missed, mut decided) = (0, refused, 0);
        let mut loose_changed = false;
        for op in &ops {
            // An update whose task is in no list it names is looked up by title in the
            // records, so a missing or wrong `work` can't lose a status change.
            let work = match op {
                themis_core::memory::tasks::TaskOp::Update { title, .. }
                    if !holds(&loose, title)
                        && op.work().is_none_or(|work| {
                            store
                                .read(work)
                                .ok()
                                .flatten()
                                .is_none_or(|record| !holds(&record.tasks, title))
                        }) =>
                {
                    store.list().ok().and_then(|all| {
                        all.into_iter()
                            .find(|record| holds(&record.tasks, title))
                            .map(|record| record.name)
                    })
                }
                _ => op.work().map(str::to_owned),
            };
            let record = work.as_deref().and_then(|work| {
                if !records.contains_key(work) {
                    let found = store.read(work).ok().flatten()?;
                    records.insert(work.to_owned(), found);
                }
                records.get_mut(work)
            });
            let changed = match record {
                Some(record) => record.apply_task(op, &today),
                None => {
                    let changed = themis_core::memory::tasks::apply(&mut loose, op, &today);
                    loose_changed |= changed;
                    changed
                }
            };
            if changed {
                applied += 1;
            } else {
                missed += 1;
            }
        }
        for decision in decisions {
            if !records.contains_key(&decision.work) {
                match store.read(&decision.work).ok().flatten() {
                    Some(found) => {
                        records.insert(decision.work.clone(), found);
                    }
                    None => {
                        missed += 1;
                        continue;
                    }
                }
            }
            if let Some(record) = records.get_mut(&decision.work) {
                record.decide(&decision.what, &decision.why, &today);
                decided += 1;
            }
        }
        let mut failed = 0;
        for record in records.values() {
            if store.save(record, false).is_err() {
                failed += 1;
            }
        }
        if loose_changed && store.tasks().write(&loose).is_err() {
            failed += 1;
        }
        let _ = self.inner.diagnostics.append(
            "memory",
            "tasks",
            "info",
            json!({"thread_id": thread_id, "applied": applied, "missed": missed, "decisions": decided, "failed": failed}),
        );
        applied + decided > 0 && failed == 0
    }

    fn tell_running_agent(
        &self,
        thread_id: &str,
        changes: &[MemoryChange],
        bodies: &[(String, String)],
    ) {
        if changes.is_empty() {
            return;
        }
        let notes = self
            .inner
            .live_notes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
            .cloned();
        let Some(notes) = notes else { return };
        let lines: Vec<String> = changes
            .iter()
            .map(|change| {
                let body = bodies
                    .iter()
                    .find(|(name, _)| *name == change.name)
                    .map(|(_, body)| format!("\n  {}", body.trim().replace('\n', "\n  ")))
                    .unwrap_or_default();
                format!(
                    "- {} {} ({}): {}{body}",
                    change.action, change.name, change.scope, change.description
                )
            })
            .collect();
        notes.deliver(format!(
            "{}{}",
            themis_core::runtime::MEMORY_NOTE_MARKER,
            themis_core::prompts::fill(
                themis_core::prompts::MEMORY_CHANGED_NOTE,
                &[("changes", &lines.join("\n"))],
            )
        ));
    }
}

/// What one model's answer to a job gave: the changes it planned, with their counts.
pub(super) struct Answered {
    pub(super) planned: Vec<pass::Proposal>,
    pub(super) counts: pass::PassCounts,
    /// The plan record op the pass proposed, if any.
    pub(super) plan: Option<Result<PlanOp, OpRefused>>,
    /// Task ops the pass proposed, and how many it had refused.
    pub(super) tasks: (Vec<themis_core::memory::tasks::TaskOp>, usize),
    /// Decisions the pass proposed for plan records.
    pub(super) decisions: Vec<pass::Decision>,
}

/// Whether `tasks` lists a task titled `title` (case aside).
fn holds(tasks: &[themis_core::memory::tasks::Task], title: &str) -> bool {
    tasks
        .iter()
        .any(|task| task.title.trim().eq_ignore_ascii_case(title.trim()))
}
