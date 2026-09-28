//! Provider setup and asynchronous event lifecycle for agent runs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use themis_core::providers::ProviderConfig;
use themis_core::runtime::{run_task_with_policy, CachingApprovals, RunEvent, RunPolicy};
use themis_core::skills::materialize_scripts;
use themis_core::tools::{boxed_tools, ApprovalHook};

use crate::approvals::DesktopApprovalHook;
use crate::sink::EventSink;
use crate::types::{ThreadEvent, ThreadEventEnvelope};

use super::{check_go_model, AppState, RunSnapshot, CUSTOM_BASE_URL_ENV_VAR};

/// Channel capacity for the run-event pump (runs emit a handful of events).
const EVENT_CHANNEL_CAPACITY: usize = 1024;

impl AppState {
    /// Resolves the provider, builds tools, and spawns the run task.
    pub(super) async fn spawn_run(
        &self,
        sink: &Arc<dyn EventSink>,
        thread_id: &str,
        run_id: &str,
        snapshot: RunSnapshot,
        task: String,
        policy: RunPolicy,
    ) -> Result<(), String> {
        let core_kind = snapshot.provider.core_kind();
        let api_key = self.api_key_for(snapshot.provider)?;
        if core_kind == themis_core::providers::ProviderKind::Go {
            check_go_model(&snapshot.model)?;
        }
        let mut config = ProviderConfig::new(core_kind, api_key);
        config.reasoning_effort = snapshot.reasoning_effort;
        config.session_id = Some(thread_id.to_owned());
        if !snapshot.model.trim().is_empty() {
            config = config.with_model(snapshot.model.clone());
        }
        if core_kind == themis_core::providers::ProviderKind::Custom {
            match self.custom_base_url() {
                Some(url) => config = config.with_base_url(url),
                None => {
                    return Err(format!(
                        "provider 'custom' needs a base URL: set the \
                         {CUSTOM_BASE_URL_ENV_VAR} environment variable"
                    ));
                }
            }
        }
        let llm = themis_core::providers::resolve(&config)
            .await
            .map_err(|err| {
                format!(
                    "failed to resolve provider '{}': {err:#}",
                    snapshot.provider.as_str()
                )
            })?;
        let hook = Arc::new(
            DesktopApprovalHook::new(
                thread_id.to_owned(),
                snapshot.is_git,
                Arc::clone(sink),
                Arc::clone(&self.inner.pending),
            )
            .with_timeout(std::time::Duration::from_secs(u64::from(
                snapshot.approval_timeout_seconds,
            )))
            .with_read_approval(snapshot.confirm_reads),
        );
        let approvals: Arc<dyn ApprovalHook> = CachingApprovals::wrap(hook);
        // Materialize skill scripts into the run workroot BEFORE building
        // tools: a failure aborts the spawn (the caller resets `running`),
        // so a run never starts half-skilled.
        if let Err(err) = materialize_scripts(&snapshot.skills, &snapshot.work_root) {
            return Err(format!("failed to materialize skill scripts: {err:#}"));
        }
        let tools = boxed_tools(&snapshot.work_root, Arc::clone(&approvals))
            .map_err(|err| format!("failed to build tools: {err:#}"))?;
        let skills = snapshot.skills;
        let stopped = self
            .inner
            .stop_flags
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
            .cloned()
            .ok_or_else(|| "Run is no longer active".to_owned())?;

        let state = self.clone();
        let thread_id = thread_id.to_owned();
        let run_id = run_id.to_owned();
        let sink = Arc::clone(sink);
        tauri::async_runtime::spawn(async move {
            let (events_tx, mut events_rx) =
                tokio::sync::mpsc::channel::<RunEvent>(EVENT_CHANNEL_CAPACITY);
            let pump_sink = Arc::clone(&sink);
            let pump_thread = thread_id.clone();
            let pump_run = run_id.clone();
            let pump_state = state.clone();
            let pump_thread_id = thread_id.clone();
            let terminated = Arc::new(AtomicBool::new(false));
            let pump_terminated = Arc::clone(&terminated);
            let pump = tokio::spawn(async move {
                let mut pending_text = String::new();
                while let Some(event) = events_rx.recv().await {
                    if let RunEvent::AssistantText(delta) = &event {
                        pending_text.push_str(delta);
                        pump_sink.emit_thread_event(&ThreadEventEnvelope {
                            thread_id: pump_thread.clone(),
                            run_id: pump_run.clone(),
                            event: ThreadEvent::AssistantText {
                                text: delta.clone(),
                            },
                        });
                        if pending_text.len() < 4096 {
                            continue;
                        }
                    }
                    if !pending_text.is_empty() {
                        let envelope = ThreadEventEnvelope {
                            thread_id: pump_thread.clone(),
                            run_id: pump_run.clone(),
                            event: ThreadEvent::AssistantText {
                                text: std::mem::take(&mut pending_text),
                            },
                        };
                        if let Err(error) = pump_state.inner.transcript.append_event(&envelope) {
                            pump_state.record_error("save_thread_event", error);
                        }
                    }
                    if matches!(event, RunEvent::AssistantText(_)) {
                        continue;
                    }
                    // A terminal event means the run is over: mark the thread
                    // idle BEFORE emitting, so observers (merge/discard,
                    // provider switch) never see `Finished` on a busy thread.
                    let terminal = matches!(
                        event,
                        themis_core::runtime::RunEvent::Finished { .. }
                            | themis_core::runtime::RunEvent::Incomplete { .. }
                            | themis_core::runtime::RunEvent::Failed { .. }
                    );
                    let envelope = ThreadEventEnvelope {
                        thread_id: pump_thread.clone(),
                        run_id: pump_run.clone(),
                        event: ThreadEvent::from(event.clone()),
                    };
                    if let Err(error) = pump_state.inner.transcript.append_event(&envelope) {
                        pump_state.record_error("save_thread_event", error);
                    }
                    if terminal {
                        pump_state.finish_run(&pump_thread_id).await;
                        pump_terminated.store(true, Ordering::SeqCst);
                        // Automation attribution runs while the thread is
                        // idle but BEFORE the terminal event goes out: the
                        // idle-before-terminal invariant is preserved, and by
                        // the time observers see the terminal event the review
                        // item (if any) already exists.
                        pump_state
                            .complete_automation_run(&pump_sink, &pump_run, &event)
                            .await;
                    }
                    pump_sink.emit_thread_event(&envelope);
                }
                if !pending_text.is_empty() {
                    let envelope = ThreadEventEnvelope {
                        thread_id: pump_thread.clone(),
                        run_id: pump_run.clone(),
                        event: ThreadEvent::AssistantText { text: pending_text },
                    };
                    if let Err(error) = pump_state.inner.transcript.append_event(&envelope) {
                        pump_state.record_error("save_thread_event", error);
                    }
                }
            });
            // A panicking run must neither stick the thread busy nor leave the
            // UI waiting: join the run, then synthesize the terminal event.
            let run_outcome = tokio::spawn(async move {
                run_task_with_policy(
                    llm,
                    themis_core::skills::filter_tools(tools, &skills),
                    themis_core::skills::compose_task(&task, &skills),
                    snapshot.history,
                    approvals,
                    policy,
                    events_tx,
                    stopped,
                )
                .await
            })
            .await;
            let _pump = pump.await;
            if !terminated.load(Ordering::SeqCst) {
                state.finish_run(&thread_id).await;
            }
            // Synthesize the terminal event only if the run died without
            // emitting one (a panic after a terminal send must not double it).
            if let Err(join_error) = run_outcome {
                if terminated.load(Ordering::SeqCst) {
                    return;
                }
                let error = format!("agent run panicked: {join_error}");
                // The pump never saw a terminal event, so attribute here; the
                // take-once registry makes a double count impossible.
                state
                    .finish_automation_run(&sink, &run_id, format!("failed: {error}"))
                    .await;
                let envelope = ThreadEventEnvelope {
                    thread_id: thread_id.clone(),
                    run_id: run_id.clone(),
                    event: ThreadEvent::Failed { error },
                };
                if let Err(error) = state.inner.transcript.append_event(&envelope) {
                    state.record_error("save_thread_event", error);
                }
                sink.emit_thread_event(&envelope);
            }
        });
        Ok(())
    }
}

impl AppState {
    /// Marks a run idle, releases its concurrency permit, and persists.
    pub(super) async fn finish_run(&self, thread_id: &str) {
        self.inner
            .stop_flags
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(thread_id);
        // Both the terminal-event pump and run tail can finish a run; release
        // the permit only for the path that still owns the running state.
        let was_running = self
            .inner
            .threads
            .write()
            .await
            .get_mut(thread_id)
            .map(|record| std::mem::replace(&mut record.running, false))
            .unwrap_or(false);
        if was_running {
            let _ = self.inner.running_count.fetch_update(
                Ordering::SeqCst,
                Ordering::SeqCst,
                |count| count.checked_sub(1),
            );
        }
        self.persist_registry().await;
    }
}
