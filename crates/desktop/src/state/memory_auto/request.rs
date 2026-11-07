//! The memory models' requests: one request on a model, what a job reads, and the dry run of one pass.

use super::*;

impl AppState {
    /// One request with no tools on `model`, within `compaction.request_timeout_seconds`,
    /// recorded as memory usage (for `thread`'s chat when there is one).
    pub(super) async fn memory_model_request(
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
    pub(in crate::state) async fn memory_model_request_why(
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

    /// What the model reads for `job`, and what it may change; `None` when there is nothing
    /// to do (a scope with fewer than two memories needs no tidying).
    pub(super) fn memory_job_request(
        &self,
        job: &Job,
    ) -> Option<(Vec<pass::Known>, (String, String))> {
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
                let tasks = self
                    .plan_store(&turn.project_root)
                    .map(|store| themis_core::memory::tasks::render_list(&store.tasks().read()))
                    .unwrap_or_default();
                let earlier_answer = self.earlier_answer(thread_id, &turn.person);
                Some((
                    known,
                    pass::request(
                        &TurnText {
                            person: &turn.person,
                            answered: &turn.answered,
                            reply,
                            ran_into: &turn.ran_into,
                            earlier_answer: &earlier_answer,
                        },
                        &global,
                        &project,
                        &relevant,
                        (&plans_index, &record, &tasks),
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

    /// The chat's latest answered question, as `Question: …\nAnswer: …`, unless this turn
    /// already carries it (then the pass reads it as the person's words). Empty when none.
    fn earlier_answer(&self, thread_id: &str, person: &[String]) -> String {
        let Some(question) = self
            .questions_for(thread_id)
            .into_iter()
            .filter(|question| question.answer.is_some())
            .max_by_key(|question| question.asked_at)
        else {
            return String::new();
        };
        let answer = question.answer.unwrap_or_default();
        if person.iter().any(|said| said.contains(&answer)) {
            return String::new();
        }
        format!("Question: {}\nAnswer: {answer}", question.question)
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
        // An eval may hand in a record and a task list of its own (a long session's state).
        let record = match input["plan_record"].as_str() {
            Some(given) => given.to_owned(),
            None => record
                .map(|record| themis_core::memory::plans::serialize(&record))
                .unwrap_or_default(),
        };
        let plans_index = match input["plans_index"].as_str() {
            Some(given) => given.to_owned(),
            None => plans_index,
        };
        let tasks = match input["tasks"].as_str() {
            Some(given) => given.to_owned(),
            None => self
                .plan_store(&root)
                .map(|store| themis_core::memory::tasks::render_list(&store.tasks().read()))
                .unwrap_or_default(),
        };
        let (answered, reply, ran_into) = (text("answered"), text("reply"), text("ran_into"));
        let (system, user) = pass::request(
            &TurnText {
                person: &person,
                answered: &answered,
                reply: &reply,
                ran_into: &ran_into,
                earlier_answer: &text("earlier_answer"),
            },
            &global,
            &project,
            &[],
            (&plans_index, &record, &tasks),
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
        let (task_ops, tasks_refused) = pass::parse_tasks_answer(&answer);
        let tasks: Vec<String> = task_ops.iter().map(|op| format!("{op:?}")).collect();
        let decisions: Vec<String> = pass::parse_decisions_answer(&answer)
            .iter()
            .map(|decision| format!("{decision:?}"))
            .collect();
        Ok(json!({
            "tasks": tasks,
            "tasks_refused": tasks_refused,
            "decisions": decisions,
            "model": model,
            "thought": think,
            "elapsed_ms": elapsed_ms,
            "usage": usage.map(|(prompt, completion)| json!({"prompt": prompt, "completion": completion})),
            "answer": answer,
            "memories": parsed,
            "plan": plan,
        }))
    }
}
