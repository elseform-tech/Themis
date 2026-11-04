//! Memory jobs on the memory models (`memory.models`): after each turn in which the person
//! wrote, the pass reads that turn and the memory indexes and saves, updates or removes
//! memories through the store, with one memory line under the reply; when the app has been
//! idle a while, at most once per `memory.consolidate_every_hours`, a consolidation merges
//! duplicates and drops stale memories. Each job tries the models in order and moves on after a
//! provider error, a time-out, a rate limit, an answer it can't read, or the daily cap
//! (`memory.daily_budget_usd`); a job every model failed waits in a bounded queue and is tried
//! again later. Jobs run on their own tasks and never hold up the next message. The requests,
//! the answers' parsing and the plans live in `themis_core::memory`. Diagnostics carry the job
//! kind, model index, outcome, counts, sizes and tokens only, never what was said or saved.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde_json::{json, Value};
use themis_core::configuration::RuntimeConfig;
use themis_core::memory::auto as pass;
use themis_core::memory::auto::TurnText;
use themis_core::memory::jobs::{self, Failure, Pending};
use themis_core::memory::plans::{OpRefused, PlanOp, PlanRecord};
use themis_core::memory::{MemoryScope, SaveMode};

use super::AppState;
use crate::sink::EventSink;
use crate::types::MemoryChange;

/// How long the app must be idle (no run going) before waiting jobs and consolidation run.
pub(super) const IDLE_MS: u64 = 5 * 60 * 1000;
/// Projects whose memories the next consolidation tidies besides the global ones.
const MAX_RECENT_PROJECTS: usize = 16;
/// A turn whose person text is shorter than this ("ok", "thanks") holds nothing to remember.
const TRIVIAL_CHARS: usize = 8;
/// The file under app data `memory/` that records when memory was last consolidated.
const CONSOLIDATED_FILE: &str = "consolidated.json";

/// What a finished turn hands the memory pass, captured when the run started.
#[derive(Clone)]
pub(super) struct MemoryTurn {
    pub(super) project_root: PathBuf,
    /// The run's captured configuration.
    pub(super) config: RuntimeConfig,
    /// The person's words that started the turn (none for a goal turn, a hook's wake or an
    /// automation), then what they steered in during it.
    pub(super) person: Vec<String>,
    /// The assistant reply the person's message answered (empty in a new chat): what "yes, go
    /// with that" refers to.
    pub(super) answered: String,
    /// An event that started this pass on its own (goal completed or edited, a checkpoint): what
    /// happened, and its content.
    pub(super) event: Option<(&'static str, String)>,
    /// History position when the turn started: where what it ran into is read from.
    pub(super) start_seq: i64,
    /// What the turn ran into: each failed step and what the agent said next (filled when the
    /// turn ends). A turn that hit a snag and got past it is worth a pass on its own.
    pub(super) ran_into: String,
}

/// One memory job.
#[derive(Clone)]
pub(super) enum Job {
    /// The pass after a turn, with that turn's reply.
    Pass {
        thread_id: String,
        run_id: String,
        turn: MemoryTurn,
        reply: String,
    },
    /// Tidying one scope (a project's when `project_root` is set).
    Consolidate {
        config: RuntimeConfig,
        project_root: Option<PathBuf>,
    },
}

impl Job {
    fn kind(&self) -> &'static str {
        match self {
            Self::Pass { .. } => "pass",
            Self::Consolidate { .. } => "consolidate",
        }
    }

    fn config(&self) -> &RuntimeConfig {
        match self {
            Self::Pass { turn, .. } => &turn.config,
            Self::Consolidate { config, .. } => config,
        }
    }
}

/// The memory jobs' shared state.
pub(super) struct MemoryJobs {
    /// Off in headless tests unless a test turns it on; on in the app.
    pub(super) enabled: bool,
    pending: Pending<Job>,
    /// Bumped by every job and run start, so only the newest idle wait acts.
    idle_generation: u64,
    idle_ms: u64,
    /// Projects with a pass since the last consolidation.
    recent_projects: BTreeSet<PathBuf>,
    /// The configuration of the latest turn, for idle work.
    last_config: Option<RuntimeConfig>,
}

impl MemoryJobs {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            pending: Pending::default(),
            idle_generation: 0,
            idle_ms: IDLE_MS,
            recent_projects: BTreeSet::new(),
            last_config: None,
        }
    }
}

/// Why a pass didn't run.
fn skip_reason(turn: &MemoryTurn) -> Option<&'static str> {
    let said: usize = turn.person.iter().map(|text| text.trim().len()).sum();
    if !turn.config.memory.enabled {
        Some("memory_off")
    } else if turn.event.is_some() {
        None
    } else if said == 0 && turn.ran_into.is_empty() {
        Some("no_person_message")
    } else if said > 0 && said < TRIVIAL_CHARS && turn.ran_into.is_empty() {
        Some("trivial")
    } else {
        None
    }
}

/// Whether a job's memory model thinks before answering (`memory.thinking`): with `auto`, a pass
/// that carries lessons (failed steps, a completed goal) does; others and
/// consolidation think the least.
fn memory_thinks(config: &RuntimeConfig, job: &Job) -> bool {
    use themis_core::configuration::MemoryThinking;
    match config.memory.thinking {
        MemoryThinking::Least => false,
        MemoryThinking::Model => true,
        MemoryThinking::Auto => match job {
            Job::Pass { turn, .. } => carries_lessons(turn),
            Job::Consolidate { .. } => false,
        },
    }
}

/// A turn whose pass may keep lessons: it ran into failed steps, or a goal completed.
fn carries_lessons(turn: &MemoryTurn) -> bool {
    !turn.ran_into.trim().is_empty()
        || turn
            .event
            .as_ref()
            .is_some_and(|(what, _)| *what == "a goal was completed")
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The start of tomorrow in this computer's time zone, when the daily cap resets.
fn tomorrow_ms() -> i64 {
    let tomorrow = chrono::Local::now().date_naive() + chrono::Duration::days(1);
    crate::usage::local_start(&chrono::Local, tomorrow).timestamp_millis()
}

/// What one model's answer to a job gave: the changes it planned, with their counts.
struct Answered {
    planned: Vec<pass::Proposal>,
    counts: pass::PassCounts,
    /// The plan record op the pass proposed, if any.
    plan: Option<Result<PlanOp, OpRefused>>,
}

impl AppState {
    fn memory_jobs(&self) -> std::sync::MutexGuard<'_, MemoryJobs> {
        self.inner
            .memory_jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Turns memory jobs on or off, with the idle wait and first retry wait (headless tests;
    /// the app always runs them with the defaults).
    pub fn set_memory_jobs_for_test(&self, enabled: bool, idle_ms: u64, first_retry_ms: i64) {
        let mut jobs = self.memory_jobs();
        jobs.enabled = enabled;
        jobs.idle_ms = idle_ms;
        jobs.pending = Pending::with_first_retry(first_retry_ms);
    }

    /// Settings › Memory's status: `null`, or how many jobs wait for the memory models, why
    /// the last try failed and when the next try is.
    pub(crate) fn memory_waiting(&self) -> Value {
        let jobs = self.memory_jobs();
        if jobs.pending.is_empty() {
            return Value::Null;
        }
        let reasons: Vec<&str> = jobs
            .pending
            .last_reasons()
            .iter()
            .map(|reason| reason.as_str())
            .collect();
        json!({"jobs": jobs.pending.len(), "reasons": reasons, "next_at": jobs.pending.next_at()})
    }

    /// A run started: idle work waits for the app to be quiet again.
    pub(super) fn note_memory_activity(&self) {
        self.memory_jobs().idle_generation += 1;
    }

    /// Estimated dollars the memory models spent today (list prices; unpriced models count 0).
    async fn memory_spent_today(&self) -> f64 {
        let prices: HashMap<(String, String), themis_core::providers::ModelPrice> = self
            .inner
            .go_catalog
            .read()
            .await
            .iter()
            .filter_map(|model| {
                let provider = model
                    .provider
                    .clone()
                    .unwrap_or_else(|| themis_core::providers::GO_PROVIDER_ID.to_owned());
                Some(((provider, model.id.clone()), model.price.clone()?))
            })
            .collect();
        let today = chrono::Local::now().date_naive();
        let since = crate::usage::local_start(&chrono::Local, today).timestamp_millis();
        let usage = Arc::clone(&self.inner.usage);
        let rows = tokio::task::spawn_blocking(move || usage.rows_since(since))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        rows.iter()
            .filter(|row| row.kind == themis_core::context_metrics::UsageKind::Memory)
            .filter_map(|row| {
                let price = prices.get(&(row.provider.clone(), row.model.clone()))?;
                Some(price.request_cost(
                    row.input_tokens.unwrap_or(0),
                    row.cached_input_tokens.unwrap_or(0),
                    row.output_tokens.unwrap_or(0),
                ))
            })
            .sum()
    }

    /// Whether the catalog lists `model` as free: the daily cap never holds it back.
    async fn memory_model_free(&self, config: &RuntimeConfig, model: &str) -> bool {
        self.run_catalog(config, &[model])
            .await
            .iter()
            .find(|entry| entry.id == model)
            .and_then(|entry| entry.price.as_ref())
            .is_some_and(themis_core::providers::ModelPrice::is_free)
    }

    /// One request with no tools on `model`, within `compaction.request_timeout_seconds`,
    /// recorded as memory usage (for `thread`'s chat when there is one).
    async fn memory_model_request(
        &self,
        config: &RuntimeConfig,
        model: &str,
        thread: Option<(&str, &str)>,
        (system, user): (&str, &str),
        think: bool,
    ) -> Result<(String, Option<(u32, u32)>), Failure> {
        self.memory_model_request_why(config, model, thread, (system, user), think)
            .await
            .map_err(|(failure, _)| failure)
    }

    /// [`Self::memory_model_request`], with the provider's own words on a failure (for the
    /// person's dry run only; never logged).
    async fn memory_model_request_why(
        &self,
        config: &RuntimeConfig,
        model: &str,
        thread: Option<(&str, &str)>,
        (system, user): (&str, &str),
        think: bool,
    ) -> Result<(String, Option<(u32, u32)>), (Failure, String)> {
        let session = thread.map_or("themis-memory", |(thread_id, _)| thread_id);
        let provider = self
            .single_provider_config(config, model, session)
            .await
            .map_err(|error| (Failure::Unavailable, error))?;
        // The catalog says how far each model's thinking goes down: without it the least
        // thinking is a guess some models refuse (a fresh app has none until something loads it).
        if !Self::is_configured_model(config, model) {
            let _ = self.ensure_go_catalog().await;
        }
        let catalog = self.run_catalog(config, &[model]).await;
        let limit = std::time::Duration::from_secs(config.compaction.request_timeout_seconds);
        let recorder: themis_core::context_metrics::UsageRecorder = {
            let state = self.clone();
            let thread =
                thread.map(|(thread_id, run_id)| (thread_id.to_owned(), run_id.to_owned()));
            Arc::new(move |record| {
                state
                    .inner
                    .usage
                    .record(crate::usage::UsageRow::from_record(
                        record,
                        now_ms(),
                        thread.as_ref().map(|(thread_id, _)| thread_id.as_str()),
                        thread.as_ref().map(|(_, run_id)| run_id.as_str()),
                    ));
            })
        };
        let request = themis_core::providers::with_catalog_limits(
            &catalog,
            themis_core::context_metrics::with_usage_recorder(
                recorder,
                themis_core::context_metrics::with_usage_kind(
                    themis_core::context_metrics::UsageKind::Memory,
                    // A short extraction thinks the least each model allows; one that carries
                    // lessons may think (see `memory_thinks`).
                    async {
                        let ask = themis_core::providers::ask_once(&provider, system, user);
                        if think {
                            ask.await
                        } else {
                            themis_core::providers::with_least_thinking(ask).await
                        }
                    },
                ),
            ),
        );
        match tokio::time::timeout(limit, request).await {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(error)) if themis_core::providers::is_rate_limited(&error) => {
                Err((Failure::RateLimited, error.to_string()))
            }
            Ok(Err(error)) => Err((Failure::ProviderError, format!("{error:#}"))),
            Err(_) => Err((Failure::TimedOut, "no answer in time".to_owned())),
        }
    }

    /// Starts the pass for a turn that just ended with `reply`, on its own task.
    pub(super) fn spawn_memory_pass(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        run_id: String,
        turn: MemoryTurn,
        reply: String,
    ) {
        if !self.memory_jobs().enabled {
            return;
        }
        if let Some(reason) = skip_reason(&turn) {
            let _ = self.inner.diagnostics.append(
                "memory",
                "pass_skipped",
                "info",
                json!({"thread_id": thread_id, "run_id": run_id, "reason": reason}),
            );
            return;
        }
        {
            let mut jobs = self.memory_jobs();
            jobs.last_config = Some(turn.config.clone());
            if jobs.recent_projects.len() < MAX_RECENT_PROJECTS {
                jobs.recent_projects.insert(turn.project_root.clone());
            }
        }
        let state = self.clone();
        tokio::spawn(async move {
            let job = Job::Pass {
                thread_id,
                run_id,
                turn,
                reply,
            };
            state.run_memory_job(sink.as_ref(), job, None).await;
            state.schedule_memory_idle(sink);
        });
    }

    /// Starts a pass for an event that matters on its own (no person words needed): a goal
    /// completed or edited, a checkpoint. It reads the event's content with the chat's current
    /// configuration.
    pub(super) async fn spawn_memory_event(
        &self,
        sink: Arc<dyn EventSink>,
        (thread_id, run_id): (&str, &str),
        what: &'static str,
        (content, answered): (String, String),
    ) {
        let Some(root) = self.thread_project_root(thread_id).await else {
            return;
        };
        let root = PathBuf::from(root);
        let Ok(resolved) = self.runtime_configuration(Some(&root), None) else {
            return;
        };
        let _ = self.inner.diagnostics.append(
            "memory",
            "event",
            "info",
            json!({"thread_id": thread_id, "event": what}),
        );
        self.spawn_memory_pass(
            sink,
            thread_id.to_owned(),
            run_id.to_owned(),
            MemoryTurn {
                project_root: root,
                config: resolved.config,
                person: Vec::new(),
                answered,
                event: Some((what, content)),
                start_seq: 0,
                ran_into: String::new(),
            },
            String::new(),
        );
    }

    /// Runs `job` on the memory models in order. A job every model failed is kept for a later
    /// try (`waiting` is the earlier tries of a job taken from the queue).
    async fn run_memory_job(
        &self,
        sink: &dyn EventSink,
        job: Job,
        waiting: Option<jobs::Waiting<Job>>,
    ) {
        // After the job before it, so its request reads the memory that job left.
        let _one_at_a_time = self.inner.memory_job_lock.lock().await;
        let config = job.config().clone();
        let models = config.memory.models.clone();
        let budget = config.memory.daily_budget_usd;
        let kind = job.kind();
        let request = self.memory_job_request(&job);
        let input_bytes = request
            .as_ref()
            .map_or(0, |(_, (system, user))| system.len() + user.len());
        let Some((known, (system, user))) = request else {
            return;
        };
        let thread = match &job {
            Job::Pass {
                thread_id, run_id, ..
            } => Some((thread_id.clone(), run_id.clone())),
            Job::Consolidate { .. } => None,
        };
        let consolidating = match &job {
            Job::Consolidate { project_root, .. } => Some(if project_root.is_some() {
                MemoryScope::Project
            } else {
                MemoryScope::Global
            }),
            Job::Pass { .. } => None,
        };
        if let Some((thread_id, run_id)) = &thread {
            self.memory_working(sink, (thread_id, run_id), "checking");
        }
        let think = memory_thinks(&config, &job);
        let (done, failures) = jobs::on_models(&models, |index, model| {
            let (system, user, known, config, thread) =
                (&system, &user, &known, &config, thread.clone());
            async move {
                let started = std::time::Instant::now();
                let mut log =
                    json!({"kind": kind, "model_index": index, "input_bytes": input_bytes});
                if let Some((thread_id, run_id)) = &thread {
                    log["thread_id"] = thread_id.as_str().into();
                    log["run_id"] = run_id.as_str().into();
                }
                let result = async {
                    if !self.memory_model_free(config, &model).await
                        && self.memory_spent_today().await >= budget
                    {
                        return Err(Failure::OverBudget);
                    }
                    let ids = thread.as_ref().map(|(t, r)| (t.as_str(), r.as_str()));
                    let (text, usage) = self
                        .memory_model_request(config, &model, ids, (system, user), think)
                        .await?;
                    if let Some((prompt, completion)) = usage {
                        log["prompt_tokens"] = prompt.into();
                        log["completion_tokens"] = completion.into();
                    }
                    let limit = if consolidating.is_some() {
                        pass::MAX_CONSOLIDATE_CHANGES
                    } else {
                        pass::MAX_CHANGES
                    };
                    let (proposals, mut counts) =
                        pass::parse_answer(&text, &themis_core::memory::today(), limit)
                            .map_err(|_| Failure::Unreadable)?;
                    let proposals = match consolidating {
                        Some(scope) => pass::only_scope(proposals, scope, &mut counts),
                        None => proposals,
                    };
                    let planned = pass::plan(proposals, known, &mut counts);
                    let plan = consolidating
                        .is_none()
                        .then(|| pass::parse_plan_answer(&text))
                        .flatten();
                    Ok(Answered {
                        planned,
                        counts,
                        plan,
                    })
                }
                .await;
                log["outcome"] = result
                    .as_ref()
                    .map_or_else(|failure| failure.as_str(), |_| "answered")
                    .into();
                log["elapsed_ms"] = (started.elapsed().as_millis() as u64).into();
                let level = if result.is_ok() { "info" } else { "warn" };
                let _ = self.inner.diagnostics.append("memory", "job", level, log);
                result
            }
        })
        .await;
        match done {
            Some((index, answered)) => {
                let model = models[index].clone();
                self.apply_memory_job(sink, &job, (index, &model), answered);
            }
            None => {
                if let Some((thread_id, run_id)) = &thread {
                    self.memory_working(sink, (thread_id, run_id), "waiting");
                }
                self.keep_memory_job(job, waiting, &failures);
            }
        }
    }

    /// What the model reads for `job`, and what it may change; `None` when there is nothing
    /// to do (a scope with fewer than two memories needs no tidying).
    fn memory_job_request(&self, job: &Job) -> Option<(Vec<pass::Known>, (String, String))> {
        match job {
            Job::Pass {
                turn,
                reply,
                thread_id,
                ..
            } => {
                let index = |scope| {
                    self.memory_store(scope, Some(&turn.project_root))
                        .map(|store| store.index())
                        .unwrap_or_default()
                };
                let (global, project) = (index(MemoryScope::Global), index(MemoryScope::Project));
                let mut known = pass::known_from_index(MemoryScope::Global, &global);
                known.extend(pass::known_from_index(MemoryScope::Project, &project));
                // The memories the message (and the reply it answers) may touch, in full, so an
                // update keeps what still holds.
                let about = format!(
                    "{}\n{}\n{reply}\n{}",
                    turn.person.join("\n"),
                    turn.answered,
                    turn.event
                        .as_ref()
                        .map_or("", |(_, content)| content.as_str())
                );
                let relevant: Vec<(MemoryScope, themis_core::memory::Memory)> =
                    themis_core::memory::relevant_names(&about, &global, &project)
                        .into_iter()
                        .filter_map(|(scope, name)| {
                            let store = self.memory_store(scope, Some(&turn.project_root)).ok()?;
                            Some((scope, store.read(&name).ok()??))
                        })
                        .collect();
                // What the model was shown in full is known in full, so an update that keeps a
                // memory's description but changes its text is an update, not a duplicate.
                for (scope, memory) in &relevant {
                    if let Some(entry) = known
                        .iter_mut()
                        .find(|entry| entry.scope == *scope && entry.name == memory.name)
                    {
                        entry.body = Some(memory.body.clone());
                    }
                }
                // The plan records' index and the record this chat is about, in full.
                let (plans_index, record) =
                    self.plan_record_for(&turn.project_root, thread_id, &about);
                let record = record
                    .map(|record| themis_core::memory::plans::serialize(&record))
                    .unwrap_or_default();
                Some((
                    known,
                    pass::request(
                        &TurnText {
                            person: &turn.person,
                            answered: &turn.answered,
                            reply,
                            ran_into: &turn.ran_into,
                        },
                        &global,
                        &project,
                        &relevant,
                        (&plans_index, &record),
                        turn.event
                            .as_ref()
                            .map(|(what, content)| (*what, content.as_str())),
                    ),
                ))
            }
            Job::Consolidate { project_root, .. } => {
                let scope = if project_root.is_some() {
                    MemoryScope::Project
                } else {
                    MemoryScope::Global
                };
                let store = self.memory_store(scope, project_root.as_deref()).ok()?;
                let listing = store.list().ok()?;
                if listing.memories.len() < 2 {
                    return None;
                }
                Some((
                    pass::known_from_listing(scope, &listing.memories),
                    pass::consolidation_request(scope, &listing.memories),
                ))
            }
        }
    }

    /// Applies what the model planned and, after a pass, puts one memory line under the reply.
    fn apply_memory_job(
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

    /// A dry run of one memory pass, for evals and diagnosis: what the memory model would
    /// propose for this turn (`person`, `answered`, `reply`, `ran_into`, an optional
    /// `event` of `{what, content}`), with `thinking` (`least` or `model`) and `model` (the first
    /// memory model by default) chosen for this call. It reads the project's memory and plan
    /// indexes but writes nothing, and logs only the job's metadata.
    pub async fn memory_try_pass(
        &self,
        project_root: &str,
        input: &Value,
    ) -> Result<Value, String> {
        let root = PathBuf::from(project_root);
        let mut config = self
            .runtime_configuration(Some(&root), None)
            .map_err(|error| error.to_string())?
            .config;
        config.memory.thinking = match input["thinking"].as_str() {
            None | Some("auto") => themis_core::configuration::MemoryThinking::Auto,
            Some("least") => themis_core::configuration::MemoryThinking::Least,
            Some("model") => themis_core::configuration::MemoryThinking::Model,
            Some(_) => return Err("thinking is auto, least or model".to_owned()),
        };
        let model = input["model"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| config.memory.models.first().cloned())
            .ok_or("no memory model")?;
        let text = |key: &str| input[key].as_str().unwrap_or_default().to_owned();
        let person: Vec<String> = input["person"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let event = input["event"]["what"].as_str().map(|what| {
            (
                what.to_owned(),
                input["event"]["content"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        });
        let index = |scope| {
            self.memory_store(scope, Some(&root))
                .map(|store| store.index())
                .unwrap_or_default()
        };
        let (global, project) = (index(MemoryScope::Global), index(MemoryScope::Project));
        let (plans_index, record) = self.plan_record_for(&root, "", &text("plan_about"));
        let record = record
            .map(|record| themis_core::memory::plans::serialize(&record))
            .unwrap_or_default();
        let (answered, reply, ran_into) = (text("answered"), text("reply"), text("ran_into"));
        let (system, user) = pass::request(
            &TurnText {
                person: &person,
                answered: &answered,
                reply: &reply,
                ran_into: &ran_into,
            },
            &global,
            &project,
            &[],
            (&plans_index, &record),
            event
                .as_ref()
                .map(|(what, content)| (what.as_str(), content.as_str())),
        );
        let turn = MemoryTurn {
            project_root: root.clone(),
            config: config.clone(),
            person: person.clone(),
            answered: text("answered"),
            event: None,
            start_seq: 0,
            ran_into: text("ran_into"),
        };
        let think = match config.memory.thinking {
            themis_core::configuration::MemoryThinking::Auto => {
                carries_lessons(&turn)
                    || event
                        .as_ref()
                        .is_some_and(|(what, _)| what == "a goal was completed")
            }
            themis_core::configuration::MemoryThinking::Least => false,
            themis_core::configuration::MemoryThinking::Model => true,
        };
        let started = std::time::Instant::now();
        let answer = self
            .memory_model_request_why(&config, &model, None, (&system, &user), think)
            .await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let _ = self.inner.diagnostics.append(
            "memory",
            "try_pass",
            "info",
            json!({"model": model, "thinking": input["thinking"], "thought": think, "elapsed_ms": elapsed_ms, "ok": answer.is_ok()}),
        );
        let (answer, usage) = answer.map_err(|(failure, why)| {
            format!(
                "{}: {}",
                failure.as_str(),
                why.chars().take(400).collect::<String>()
            )
        })?;
        let parsed = pass::parse_answer(&answer, &themis_core::memory::today(), pass::MAX_CHANGES)
            .map(|(proposals, _)| {
                proposals
                    .iter()
                    .map(|p| format!("{p:?}"))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let plan = pass::parse_plan_answer(&answer).map(|op| format!("{op:?}"));
        Ok(json!({
            "model": model,
            "thought": think,
            "elapsed_ms": elapsed_ms,
            "usage": usage.map(|(prompt, completion)| json!({"prompt": prompt, "completion": completion})),
            "answer": answer,
            "memories": parsed,
            "plan": plan,
        }))
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

    /// Keeps a job every model failed for a later try, bounded; one with no tries left (or
    /// pushed out of a full queue) is dropped and logged.
    fn keep_memory_job(
        &self,
        job: Job,
        waiting: Option<jobs::Waiting<Job>>,
        failures: &[(usize, Failure)],
    ) {
        let kind = job.kind();
        let mut waiting = waiting.unwrap_or_else(|| jobs::first_failure(job.clone(), failures));
        waiting.job = job;
        waiting.reasons = failures.iter().map(|(_, failure)| *failure).collect();
        let capped = !failures.is_empty()
            && failures
                .iter()
                .all(|(_, failure)| *failure == Failure::OverBudget);
        let (dropped, pending) = {
            let mut jobs = self.memory_jobs();
            let dropped = jobs
                .pending
                .keep(waiting, now_ms(), capped.then(tomorrow_ms));
            (dropped, jobs.pending.len())
        };
        let reasons: Vec<&str> = failures
            .iter()
            .map(|(_, failure)| failure.as_str())
            .collect();
        let _ = self.inner.diagnostics.append(
            "memory",
            "job_waiting",
            "warn",
            json!({"kind": kind, "reasons": reasons, "pending": pending}),
        );
        if let Some(dropped) = dropped {
            let _ = self.inner.diagnostics.append(
                "memory",
                "job_dropped",
                "warn",
                json!({"kind": dropped.kind(), "pending": pending}),
            );
        }
    }

    /// After a job: once the app has been idle for the idle wait (no newer job or run), retries
    /// the jobs whose wait is over and consolidates when it is due.
    pub(super) fn schedule_memory_idle(&self, sink: Arc<dyn EventSink>) {
        let (generation, wait) = {
            let mut jobs = self.memory_jobs();
            jobs.idle_generation += 1;
            let mut wait = jobs.idle_ms;
            if let Some(next_at) = jobs.pending.next_at() {
                wait = wait.max(u64::try_from(next_at - now_ms()).unwrap_or(0));
            }
            (jobs.idle_generation, wait)
        };
        let state = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
            let quiet = state.memory_jobs().idle_generation == generation
                && state.inner.running_count.load(Ordering::SeqCst) == 0;
            if !quiet {
                return;
            }
            let due = state.memory_jobs().pending.take_due(now_ms());
            for waiting in due {
                let job = waiting.job.clone();
                state
                    .run_memory_job(sink.as_ref(), job, Some(waiting))
                    .await;
            }
            state.consolidate_memory_when_due(sink.as_ref()).await;
            let more = !state.memory_jobs().pending.is_empty();
            if more {
                state.schedule_memory_idle(sink);
            }
        });
    }

    fn consolidated_path(&self) -> PathBuf {
        self.app_data_dir()
            .join(super::memory::MEMORY_DIR)
            .join(CONSOLIDATED_FILE)
    }

    /// Consolidates the global memories and those of the projects with a pass since, when the
    /// last consolidation is older than `memory.consolidate_every_hours`.
    async fn consolidate_memory_when_due(&self, sink: &dyn EventSink) {
        let (config, projects) = {
            let mut jobs = self.memory_jobs();
            let Some(config) = jobs.last_config.clone() else {
                return;
            };
            (config, std::mem::take(&mut jobs.recent_projects))
        };
        if !config.memory.enabled {
            return;
        }
        let path = self.consolidated_path();
        let last: Option<i64> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| value["at_ms"].as_i64());
        let every = i64::from(config.memory.consolidate_every_hours) * 60 * 60 * 1000;
        let now = now_ms();
        if last.is_some_and(|last| now - last < every) {
            // Not yet: the projects wait for the next one.
            self.memory_jobs().recent_projects.extend(projects);
            return;
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, json!({"at_ms": now}).to_string());
        let _ = self.inner.diagnostics.append(
            "memory",
            "consolidation",
            "info",
            json!({"projects": projects.len()}),
        );
        let scopes = std::iter::once(None).chain(projects.into_iter().map(Some));
        for project_root in scopes {
            let job = Job::Consolidate {
                config: config.clone(),
                project_root,
            };
            self.run_memory_job(sink, job, None).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(ran_into: &str, event: Option<&'static str>) -> MemoryTurn {
        MemoryTurn {
            project_root: PathBuf::from("/p"),
            config: RuntimeConfig::default(),
            person: vec!["hello there, friend".to_owned()],
            answered: String::new(),
            event: event.map(|what| (what, String::new())),
            start_seq: 0,
            ran_into: ran_into.to_owned(),
        }
    }

    /// `auto` thinks only for a pass that carries lessons; the other settings are fixed.
    #[test]
    fn memory_thinks_when_a_pass_carries_lessons() {
        use themis_core::configuration::MemoryThinking;
        let pass = |turn: MemoryTurn| Job::Pass {
            thread_id: "t".into(),
            run_id: "r".into(),
            turn,
            reply: String::new(),
        };
        let mut config = RuntimeConfig::default();
        assert_eq!(config.memory.thinking, MemoryThinking::Auto);
        assert!(!memory_thinks(&config, &pass(turn("", None))));
        assert!(memory_thinks(
            &config,
            &pass(turn("- shell failed: x", None))
        ));
        assert!(memory_thinks(
            &config,
            &pass(turn("", Some("a goal was completed")))
        ));
        assert!(!memory_thinks(
            &config,
            &pass(turn("", Some("a goal was created")))
        ));
        let consolidate = Job::Consolidate {
            config: config.clone(),
            project_root: None,
        };
        assert!(!memory_thinks(&config, &consolidate));
        config.memory.thinking = MemoryThinking::Least;
        assert!(!memory_thinks(
            &config,
            &pass(turn("- shell failed: x", None))
        ));
        config.memory.thinking = MemoryThinking::Model;
        assert!(memory_thinks(&config, &pass(turn("", None))));
        // A turn that hit a snag is worth a pass even when the person said nothing.
        let mut quiet = turn("- shell failed: x", None);
        quiet.person.clear();
        assert_eq!(skip_reason(&quiet), None);
        quiet.ran_into.clear();
        assert_eq!(skip_reason(&quiet), Some("no_person_message"));
    }
}
