//! The background memory pass (`memory.auto`): after a turn in which the person wrote, one
//! request on the summary model reads that turn and the memory indexes and saves what is worth
//! remembering through the memory store, with the chat's usual "Remembered · …" line. It runs on
//! its own task after the turn and never holds up the next message. The request, the answer's
//! parsing and the plan live in `themis_core::memory::auto`. Diagnostics carry counts, sizes,
//! tokens and outcomes only, never what was said or saved.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::json;
use themis_core::configuration::RuntimeConfig;
use themis_core::memory::auto as pass;
use themis_core::memory::{MemoryScope, SaveMode};

use super::AppState;
use crate::sink::EventSink;

/// What a finished turn hands the memory pass, captured when the run started.
pub(super) struct MemoryTurn {
    pub(super) project_root: PathBuf,
    /// The run's captured configuration.
    pub(super) config: RuntimeConfig,
    pub(super) chat_model: String,
    /// The person's words that started the turn (none for a goal turn, a hook's wake or an
    /// automation), then what they steered in during it.
    pub(super) person: Vec<String>,
    /// The memory tool saved or updated something this turn.
    pub(super) saved: Arc<AtomicBool>,
}

impl MemoryTurn {
    /// What is left once a turn's pass has been handed its turn: nothing to pass on.
    pub(super) fn spent() -> Self {
        Self {
            project_root: PathBuf::new(),
            config: RuntimeConfig {
                memory: themis_core::configuration::MemoryConfig {
                    enabled: false,
                    auto: false,
                },
                ..RuntimeConfig::default()
            },
            chat_model: String::new(),
            person: Vec::new(),
            saved: Arc::default(),
        }
    }
}

/// Why a pass didn't run, when `memory.auto` was on.
fn skip_reason(turn: &MemoryTurn) -> Option<&'static str> {
    if !turn.config.memory.enabled {
        Some("memory_off")
    } else if turn.person.iter().all(|text| text.trim().is_empty()) {
        Some("no_person_message")
    } else if turn.saved.load(Ordering::SeqCst) {
        Some("saved_in_turn")
    } else {
        None
    }
}

impl AppState {
    /// One request with no tools on the summary model (`compaction.summary_model`, else the
    /// chat's), within `compaction.request_timeout_seconds`, recorded as summary usage. Returns
    /// the reply's text and the provider's token counts.
    pub(super) async fn summary_model_request(
        &self,
        config: &RuntimeConfig,
        chat_model: &str,
        (thread_id, run_id): (&str, &str),
        system: &str,
        user: &str,
    ) -> Result<(String, Option<(u32, u32)>), String> {
        let model = config
            .compaction
            .summary_model
            .clone()
            .unwrap_or_else(|| chat_model.to_owned());
        let provider = self
            .single_provider_config(config, &model, thread_id)
            .await?;
        let catalog = self.run_catalog(config, &[model.as_str()]).await;
        let limit = std::time::Duration::from_secs(config.compaction.request_timeout_seconds);
        let request = themis_core::providers::with_catalog_limits(
            &catalog,
            themis_core::context_metrics::with_usage_recorder(
                self.usage_recorder(thread_id, run_id),
                themis_core::context_metrics::with_usage_kind(
                    themis_core::context_metrics::UsageKind::Summary,
                    themis_core::providers::ask_once(&provider, system, user),
                ),
            ),
        );
        match tokio::time::timeout(limit, request).await {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(_)) => Err("request_failed".to_owned()),
            Err(_) => Err("timed_out".to_owned()),
        }
    }

    /// Starts the pass for a turn that just ended with `reply`, on its own task, when
    /// `memory.auto` is on.
    pub(super) fn spawn_memory_pass(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        run_id: String,
        turn: MemoryTurn,
        reply: String,
    ) {
        if !turn.config.memory.auto {
            return;
        }
        if let Some(reason) = skip_reason(&turn) {
            let _ = self.inner.diagnostics.append(
                "memory",
                "auto_pass_skipped",
                "info",
                json!({"thread_id": thread_id, "run_id": run_id, "reason": reason}),
            );
            return;
        }
        let state = self.clone();
        tokio::spawn(async move {
            state
                .memory_pass(sink.as_ref(), (&thread_id, &run_id), &turn, &reply)
                .await;
        });
    }

    async fn memory_pass(
        &self,
        sink: &dyn EventSink,
        (thread_id, run_id): (&str, &str),
        turn: &MemoryTurn,
        reply: &str,
    ) {
        let started = std::time::Instant::now();
        let index = |scope| {
            self.memory_store(scope, Some(&turn.project_root))
                .map(|store| store.index())
                .unwrap_or_default()
        };
        let (global, project) = (index(MemoryScope::Global), index(MemoryScope::Project));
        let (system, user) = pass::request(&turn.person, reply, &global, &project);
        let mut log = json!({"thread_id": thread_id, "run_id": run_id,
            "input_bytes": system.len() + user.len()});
        let answer = self
            .summary_model_request(
                &turn.config,
                &turn.chat_model,
                (thread_id, run_id),
                &system,
                &user,
            )
            .await;
        let outcome = match answer {
            Err(reason) => reason,
            Ok((text, usage)) => {
                if let Some((prompt, completion)) = usage {
                    log["prompt_tokens"] = prompt.into();
                    log["completion_tokens"] = completion.into();
                }
                match pass::parse_answer(&text, &themis_core::memory::today()) {
                    Err(error) => error.as_str().to_owned(),
                    Ok((proposals, mut counts)) => {
                        let planned = pass::plan(proposals, &global, &project, &mut counts);
                        for proposal in planned {
                            self.apply_proposal(
                                sink,
                                (thread_id, run_id),
                                turn,
                                proposal,
                                &mut counts,
                            );
                        }
                        let outcome = if counts.saved + counts.updated > 0 {
                            "saved"
                        } else {
                            "nothing"
                        };
                        log["counts"] = serde_json::to_value(counts).unwrap_or_default();
                        outcome.to_owned()
                    }
                }
            }
        };
        log["outcome"] = outcome.clone().into();
        log["elapsed_ms"] = (started.elapsed().as_millis() as u64).into();
        let level = if ["saved", "nothing"].contains(&outcome.as_str()) {
            "info"
        } else {
            "warn"
        };
        let _ = self
            .inner
            .diagnostics
            .append("memory", "auto_pass", level, log);
    }

    fn apply_proposal(
        &self,
        sink: &dyn EventSink,
        (thread_id, run_id): (&str, &str),
        turn: &MemoryTurn,
        proposal: pass::Proposal,
        counts: &mut pass::PassCounts,
    ) {
        let Ok(store) = self.memory_store(proposal.scope, Some(&turn.project_root)) else {
            counts.failed += 1;
            return;
        };
        if store.save(&proposal.memory, proposal.mode).is_err() {
            counts.failed += 1;
            return;
        }
        let verb = match proposal.mode {
            SaveMode::Create => {
                counts.saved += 1;
                "saved"
            }
            SaveMode::Update => {
                counts.updated += 1;
                "updated"
            }
        };
        self.note_memory(verb, proposal.scope, proposal.memory.kind, &store, "auto");
        self.announce_memory(
            sink,
            (thread_id, run_id),
            verb,
            proposal.scope,
            &proposal.memory,
            &turn.project_root,
        );
    }
}
