//! One subagent run from first request to its ending and result.

use super::*;
use crate::state::{attachments, subagent_tools};

impl SubagentKit {
    pub(super) async fn run_child(&self, agent: &Arc<Subagent>) -> Ending {
        let approvals = self.approvals(agent);
        let hooks = self.hook_runtime(agent, Arc::clone(&approvals));
        // Hook outcomes of SubagentStart/End are not part of its steps.
        let (hook_events, mut hook_outcomes) = tokio::sync::mpsc::channel::<RunEvent>(64);
        tokio::spawn(async move { while hook_outcomes.recv().await.is_some() {} });
        if let Err(error) = hooks
            .emit(
                "SubagentStart",
                json!({"name": agent.name, "brief": agent.task.lock().unwrap_or_else(|e| e.into_inner()).clone(), "model": agent.model}),
                &hook_events,
            )
            .await
        {
            return Ending::failed(format!("A SubagentStart hook refused: {error}"));
        }
        let candidates = self.candidates(&agent.model);
        let mut ending = Ending::failed("No model could start".into());
        let mut reason = String::new();
        for (index, model) in candidates.iter().enumerate() {
            let last = index + 1 == candidates.len();
            let llm = match self.llm_for(model).await {
                Ok(llm) => llm,
                Err(error) => {
                    reason = error;
                    ending = Ending::failed(reason.clone());
                    continue;
                }
            };
            if index > 0 {
                agent.ran_on(model, &reason);
                (self.reporter())(agent);
                let _ = self.state.inner.diagnostics.append_with_level(
                    "subagents",
                    "fallback",
                    "warn",
                    json!({"thread_id": self.thread_id, "subagent_id": agent.id,
                        "from": agent.model, "to": model}),
                    Some(&self.config.logging.level),
                );
            }
            let (outcome, incomplete) = self
                .attempt(agent, model, llm, Arc::clone(&approvals), &hooks)
                .await;
            // Its model failed before it did anything (a quota, a provider error): try the next.
            let untouched = agent.tool_calls.load(Ordering::SeqCst) == 0;
            if let Err(error) = &outcome {
                if untouched && !last && !self.stopping(agent) {
                    reason = format!("{model} failed: {}", first_line_of(&error.to_string()));
                    continue;
                }
            }
            let status = if self.stopping(agent) {
                "stopped"
            } else {
                match (&outcome, incomplete) {
                    (Ok(_), true) => "incomplete",
                    (Ok(_), false) => "finished",
                    (Err(_), _) => "failed",
                }
            };
            ending = Ending {
                status: status.into(),
                result: match outcome {
                    Ok(result) => result,
                    Err(error) => error.to_string(),
                },
            };
            break;
        }
        let _ = hooks
            .emit(
                "SubagentEnd",
                json!({"status": ending.status, "ok": ending.status == "finished"}),
                &hook_events,
            )
            .await;
        ending
    }

    /// One run of the subagent on `model`. Returns its outcome and whether it ended incomplete.
    pub(super) async fn attempt(
        &self,
        agent: &Arc<Subagent>,
        model: &str,
        llm: Arc<dyn LLMProvider>,
        approvals: Arc<dyn ApprovalHook>,
        hooks: &themis_core::plugins::hooks::HookRuntime,
    ) -> (anyhow::Result<String>, bool) {
        let (mcp_tools, mcp_missing) = self.mcp_tools_for(agent, &approvals).await;
        let mcp_labels: Vec<String> = mcp_tools
            .iter()
            .map(|tool| {
                let label = self.mcp.labels.get(tool.name());
                label.map_or_else(|| tool.name().to_owned(), Clone::clone)
            })
            .collect();
        let mut tools = match self.tools(agent, Arc::clone(&approvals), mcp_tools) {
            Ok(tools) => tools,
            Err(error) => return (Err(anyhow::anyhow!(error)), false),
        };
        // Reading only, so a profile's narrower list doesn't take it away.
        if let Some(tool) = themis_core::skills::skill_tool(&self.skill_list) {
            tools.push(Box::new(subagent_tools::StatusField {
                inner: tool,
                agent: Arc::clone(agent),
                report: self.reporter(),
            }) as Box<dyn ToolT>);
        }
        let nested = self.nested_kit(agent, &tools);
        if let Some(kit) = &nested {
            let report = self.reporter();
            tools.extend(subagent_tools::tools(kit).into_iter().map(|tool| {
                Box::new(subagent_tools::StatusField {
                    inner: tool,
                    agent: Arc::clone(agent),
                    report: Arc::clone(&report),
                }) as Box<dyn ToolT>
            }));
        }
        let notes = nested
            .as_ref()
            .map_or_else(RunNotes::new, |kit| Arc::clone(&kit.notes));
        let (events_tx, events_rx) = tokio::sync::mpsc::channel::<RunEvent>(1024);
        let pump = tokio::spawn(pump_events(
            events_rx,
            self.transcript_path(agent),
            Arc::clone(agent),
            self.reporter(),
            Arc::clone(&self.mcp.labels),
        ));
        // Stopping the chat stops its subagents.
        let done = Arc::new(AtomicBool::new(false));
        tokio::spawn({
            let parent = Arc::clone(&self.stopped);
            let child = Arc::clone(&agent.stop);
            let done = Arc::clone(&done);
            async move {
                while !done.load(Ordering::SeqCst) {
                    if parent.load(Ordering::SeqCst) {
                        child.store(true, Ordering::SeqCst);
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        });
        let capacity = self
            .catalog
            .iter()
            .find(|entry| entry.id == model)
            .and_then(|entry| entry.context_tokens);
        let access = if agent.write_paths.is_empty() {
            themis_core::prompts::SUBAGENT_READ_ONLY_NOTE.to_owned()
        } else {
            themis_core::prompts::fill(
                themis_core::prompts::SUBAGENT_WRITE_PATHS_NOTE,
                &[("paths", &agent.write_paths.join(", "))],
            )
        };
        // Before the brief, and with its access stated after it, so that has the last word. In
        // the system message it outlasts a checkpoint, a fallback and being picked up again.
        let role = agent.profile.as_ref().map(|named| {
            themis_core::prompts::fill(
                themis_core::prompts::SUBAGENT_PROFILE,
                &[
                    ("name", &named.profile.name),
                    ("plugin", &named.plugin),
                    ("instructions", &named.profile.instructions),
                ],
            )
        });
        let mcp_note = (!mcp_labels.is_empty()).then(|| {
            themis_core::prompts::fill(
                themis_core::prompts::SUBAGENT_MCP_NOTE,
                &[("tools", &mcp_labels.join(", "))],
            )
        });
        let web_note = (!agent.web.tools.is_empty()).then(|| {
            themis_core::prompts::fill(
                themis_core::prompts::SUBAGENT_WEB_NOTE,
                &[("tools", &agent.web.tools.join(", "))],
            )
        });
        let shell_limit = self.shell_limit(agent);
        let shell_note = (!shell_limit.is_empty()).then(|| {
            themis_core::prompts::fill(
                themis_core::prompts::SUBAGENT_SHELL_SCOPE_NOTE,
                &[("scopes", &shell_limit.join(", "))],
            )
        });
        let nested_note = nested
            .as_ref()
            .map(|_| themis_core::prompts::SUBAGENT_NESTED_NOTE);
        let ask_note = names(agent, "ask_user").then_some(themis_core::prompts::SUBAGENT_ASK_NOTE);
        let background_note = names(agent, "background_task")
            .then_some(themis_core::prompts::SUBAGENT_BACKGROUND_NOTE);
        let names_note = agent.profile.as_ref().map(|_| {
            let available: Vec<String> = claude_names(&tools);
            themis_core::prompts::fill(
                themis_core::prompts::SUBAGENT_CLAUDE_NAMES,
                &[("names", &available.join(", "))],
            )
        });
        let left_out: Vec<&str> = agent
            .mcp
            .left_out
            .iter()
            .chain(&mcp_missing)
            .chain(&agent.web.left_out)
            .map(String::as_str)
            .collect();
        let left_out_note = (!left_out.is_empty()).then(|| {
            themis_core::prompts::fill(
                themis_core::prompts::SUBAGENT_MCP_LEFT_OUT,
                &[("tools", &left_out.join(", "))],
            )
        });
        let head = std::iter::once(themis_core::prompts::SUBAGENT_SYSTEM)
            .chain([themis_core::prompts::SUBAGENT_SECRETS_NOTE])
            .chain(role.as_deref())
            .chain(names_note.as_deref())
            .chain([access.as_str()])
            .chain(shell_note.as_deref())
            .chain(background_note)
            .chain(ask_note)
            .chain(nested_note)
            .chain(mcp_note.as_deref())
            .chain(web_note.as_deref())
            .chain(left_out_note.as_deref())
            .chain((!self.project_guidance.is_empty()).then_some(self.project_guidance.trim()))
            .chain((!self.skills.is_empty()).then_some(themis_core::prompts::SUBAGENT_SKILLS_NOTE))
            .chain((!self.skills.is_empty()).then_some(self.skills.trim()))
            .chain([themis_core::prompts::SUBAGENT_STATUS_FIELD])
            .collect::<Vec<_>>()
            .join("\n\n");
        let workspace = themis_core::prompts::fill(
            themis_core::prompts::DESKTOP_RUN_WORKSPACE,
            &[(
                "workspace",
                &json!({"project_root": self.work_root}).to_string(),
            )],
        );
        let policy = RunPolicy {
            total_turns: self.config.subagents.max_turns,
            ..self.policy
        };
        // Without it, one call or result too big for the context would end the subagent.
        let evidence_directory = attachments::project_storage_directory(
            &self.work_root,
            &["context", &self.thread_id, "subagents", &agent.id],
        )
        .ok();
        let task = agent.task.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let run = themis_core::runtime::run_task_with_evidence(
            llm,
            tools,
            task,
            agent
                .earlier
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            approvals,
            policy,
            events_tx,
            Arc::clone(&agent.stop),
            format!("\n{workspace}\n"),
            evidence_directory,
        );
        let outcome = themis_core::providers::with_catalog_limits(
            &self.catalog,
            themis_core::context_metrics::with_usage_recorder(
                self.state.usage_recorder(&self.thread_id, &agent.run_id),
                themis_core::diagnostics::ACTIVE.scope(
                    self.diagnostic_context(agent),
                    themis_core::runtime::with_model_context_capacity(
                        (!self.endpoint_override).then_some(capacity),
                        themis_core::context_metrics::with_run_policy(
                            &self.config,
                            themis_core::runtime::with_configuration(
                                self.config.compaction.clone(),
                                themis_core::plugins::hooks::ACTIVE.scope(
                                    hooks.clone(),
                                    themis_core::runtime::with_steer_inbox(
                                        Arc::clone(&agent.inbox),
                                        themis_core::runtime::with_run_notes(
                                            notes,
                                            // Boxed: nested subagents stack these futures, and
                                            // unboxed they overflow a runtime worker's stack.
                                            Box::pin(themis_core::runtime::with_system_head(
                                                head,
                                                Box::pin(run),
                                            )),
                                        ),
                                    ),
                                ),
                            ),
                        ),
                    ),
                ),
            ),
        )
        .await;
        done.store(true, Ordering::SeqCst);
        // Nothing owns what it started once it is over.
        if let Some(kit) = &nested {
            let left: Vec<Arc<Subagent>> = kit
                .started
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter(|child| !child.ended())
                .cloned()
                .collect();
            for child in left {
                kit.stop(&child);
            }
        }
        let incomplete = pump.await.unwrap_or(false);
        (outcome, incomplete)
    }

    /// Stops the background programs it left running: nothing owns them after it.
    pub(super) async fn stop_its_programs(&self, agent: &Subagent) {
        let registry = self.state.background_registry();
        let (thread, run) = (self.thread_id.clone(), agent.run_id.clone());
        let _ = tokio::task::spawn_blocking(move || {
            for view in registry.list(&thread, 0) {
                if view.record.run_id.as_deref() == Some(run.as_str())
                    && view.record.state == themis_core::tools::background::TaskState::Running
                {
                    let _ = registry.stop(
                        &thread,
                        &view.record.id,
                        themis_core::tools::background::STOP_WAIT,
                    );
                }
            }
        })
        .await;
    }

    pub(super) async fn end(&self, agent: &Subagent, ending: Ending) {
        self.stop_its_programs(agent).await;
        let result = bounded(&ending.result);
        let note = themis_core::prompts::fill(
            themis_core::prompts::SUBAGENT_RESULT_NOTE,
            &[
                ("name", &agent.name),
                ("id", &agent.id),
                ("status", &ending.status),
                ("result", &result),
            ],
        );
        let seconds = agent
            .started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
            .as_secs();
        let tool_calls = agent.tool_calls.load(Ordering::SeqCst);
        self.publish(ThreadEvent::SubagentEnded {
            id: agent.id.clone(),
            status: ending.status.clone(),
            result: result.clone(),
            tool_calls,
            seconds,
            model: agent.progress().ran_on,
        });
        // Marked ended only now: a message picking it up again then follows this ending in the chat.
        {
            let mut progress = agent.progress.lock().unwrap_or_else(|e| e.into_inner());
            progress.ended = Some((ending.status.clone(), result.clone()));
            progress.note = note.clone();
        }
        let _ = self.state.inner.diagnostics.append_with_level(
            "subagents",
            "ended",
            "info",
            json!({"thread_id": self.thread_id, "run_id": self.run_id, "subagent_id": agent.id,
                "status": ending.status, "seconds": seconds, "tool_calls": tool_calls}),
            Some(&self.config.logging.level),
        );
        self.notes.deliver(note);
    }

    pub(super) fn transcript_path(&self, agent: &Subagent) -> Option<PathBuf> {
        self.state
            .subagent_transcript_path(&self.thread_id, &agent.id)
    }
}

pub(super) struct Ending {
    status: String,
    result: String,
}

impl Ending {
    pub(super) fn stopped(result: String) -> Self {
        Self {
            status: "stopped".into(),
            result,
        }
    }
    pub(super) fn failed(result: String) -> Self {
        Self {
            status: "failed".into(),
            result,
        }
    }
}

/// The first line of an error, for a fallback reason.
pub(super) fn first_line_of(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect()
}

/// At most [`RESULT_LIMIT`] bytes, cut on a character boundary.
pub(super) fn bounded(result: &str) -> String {
    if result.len() <= RESULT_LIMIT {
        return result.to_owned();
    }
    let mut end = RESULT_LIMIT;
    while !result.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[cut at {} KiB]", &result[..end], RESULT_LIMIT / 1024)
}

/// Saves the child's events to its own transcript, counts tool calls and keeps its status
/// line. Returns whether the run ended incomplete.
pub(super) async fn pump_events(
    mut events: tokio::sync::mpsc::Receiver<RunEvent>,
    path: Option<PathBuf>,
    agent: Arc<Subagent>,
    report: Reporter,
    labels: Arc<HashMap<String, String>>,
) -> bool {
    let mut file = path.and_then(|path| {
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    });
    let mut text = String::new();
    let mut incomplete = false;
    let save = |event: ThreadEvent, file: &mut Option<std::fs::File>| {
        use std::io::Write;
        let Some(handle) = file.as_mut() else { return };
        if handle.metadata().map(|m| m.len()).unwrap_or(0) >= TRANSCRIPT_LIMIT {
            return;
        }
        if let Ok(line) = serde_json::to_string(&ThreadEventEnvelope {
            thread_id: String::new(),
            run_id: agent.run_id.clone(),
            event,
        }) {
            let _ = writeln!(handle, "{line}");
        }
    };
    while let Some(event) = events.recv().await {
        match &event {
            RunEvent::AssistantText(delta) => {
                text.push_str(delta);
                continue;
            }
            RunEvent::ToolCallStarted { .. } => {
                agent.tool_calls.fetch_add(1, Ordering::SeqCst);
                report(&agent);
            }
            RunEvent::Incomplete { .. } => incomplete = true,
            RunEvent::Started { .. } | RunEvent::ContextUsage { .. } => continue,
            _ => {}
        }
        if !text.is_empty() {
            save(
                ThreadEvent::AssistantText {
                    text: std::mem::take(&mut text),
                },
                &mut file,
            );
        }
        let mut saved = ThreadEvent::from(event);
        if let ThreadEvent::ToolStarted { tool, label, .. } = &mut saved {
            *label = labels.get(tool.as_str()).cloned();
        }
        save(saved, &mut file);
    }
    if !text.is_empty() {
        save(ThreadEvent::AssistantText { text }, &mut file);
    }
    incomplete
}
