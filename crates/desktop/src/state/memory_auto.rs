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
use themis_core::memory::jobs::{self, Failure, Pending};
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
}

impl MemoryTurn {
    /// What is left once a turn's pass has been handed its turn: nothing to pass on.
    pub(super) fn spent() -> Self {
        Self {
            project_root: PathBuf::new(),
            config: RuntimeConfig {
                memory: themis_core::configuration::MemoryConfig {
                    enabled: false,
                    ..Default::default()
                },
                ..RuntimeConfig::default()
            },
            person: Vec::new(),
        }
    }
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
    } else if said == 0 {
        Some("no_person_message")
    } else if said < TRIVIAL_CHARS {
        Some("trivial")
    } else {
        None
    }
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
    ) -> Result<(String, Option<(u32, u32)>), Failure> {
        let session = thread.map_or("themis-memory", |(thread_id, _)| thread_id);
        let provider = self
            .single_provider_config(config, model, session)
            .await
            .map_err(|_| Failure::Unavailable)?;
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
                    themis_core::providers::ask_once(&provider, system, user),
                ),
            ),
        );
        match tokio::time::timeout(limit, request).await {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(error)) if themis_core::providers::is_rate_limited(&error) => {
                Err(Failure::RateLimited)
            }
            Ok(Err(_)) => Err(Failure::ProviderError),
            Err(_) => Err(Failure::TimedOut),
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

    /// Runs `job` on the memory models in order. A job every model failed is kept for a later
    /// try (`waiting` is the earlier tries of a job taken from the queue).
    async fn run_memory_job(
        &self,
        sink: &dyn EventSink,
        job: Job,
        waiting: Option<jobs::Waiting<Job>>,
    ) {
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
                        .memory_model_request(config, &model, ids, (system, user))
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
                    Ok(Answered { planned, counts })
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
            None => self.keep_memory_job(job, waiting, &failures),
        }
    }

    /// What the model reads for `job`, and what it may change; `None` when there is nothing
    /// to do (a scope with fewer than two memories needs no tidying).
    fn memory_job_request(&self, job: &Job) -> Option<(Vec<pass::Known>, (String, String))> {
        match job {
            Job::Pass { turn, reply, .. } => {
                let index = |scope| {
                    self.memory_store(scope, Some(&turn.project_root))
                        .map(|store| store.index())
                        .unwrap_or_default()
                };
                let (global, project) = (index(MemoryScope::Global), index(MemoryScope::Project));
                let mut known = pass::known_from_index(MemoryScope::Global, &global);
                known.extend(pass::known_from_index(MemoryScope::Project, &project));
                Some((known, pass::request(&turn.person, reply, &global, &project)))
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
        } = answered;
        let project_root = match job {
            Job::Pass { turn, .. } => Some(turn.project_root.clone()),
            Job::Consolidate { project_root, .. } => project_root.clone(),
        };
        let by = job.kind();
        let mut changes = Vec::new();
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
                pass::Proposal::Save { mode, memory, .. } => {
                    if store.save(&memory, mode).is_err() {
                        counts.failed += 1;
                        continue;
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
                    changes.push(change(verb, &memory.name, &memory.description));
                }
            }
        }
        let outcome = if counts.changed() {
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
            self.announce_memory(sink, (thread_id, run_id), changes);
        }
        let _ = self
            .inner
            .diagnostics
            .append("memory", "job_done", "info", log);
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
