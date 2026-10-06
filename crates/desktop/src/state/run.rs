//! Provider setup and asynchronous event lifecycle for agent runs.

use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use themis_core::providers::ProviderConfig;
use themis_core::runtime::{
    run_task_with_policy_and_catalog, CachingApprovals, RunEvent, RunPolicy,
};
use themis_core::skills::materialize_scripts;
use themis_core::tools::{boxed_tools, ApprovalHook};

use crate::approvals::DesktopApprovalHook;
use crate::sink::EventSink;
use crate::types::{ThreadEvent, ThreadEventEnvelope};

use super::{AppState, RunSnapshot};

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
        if snapshot.provider != crate::types::ProviderKind::Go {
            return Err(
                "This thread used a retired provider; select an OpenCode Go model to continue"
                    .to_owned(),
            );
        }
        let api_key = self.api_key_for(snapshot.provider)?;
        if let Some(effort) = &snapshot.reasoning_effort {
            let catalog = self.inner.go_catalog.read().await;
            let supported = catalog
                .iter()
                .find(|model| model.id == snapshot.model)
                .is_some_and(|model| model.effort_levels.contains(effort));
            if !supported {
                return Err(format!(
                    "Effort '{effort}' is unavailable for model '{}'",
                    snapshot.model
                ));
            }
        }
        let mut config = ProviderConfig::new(themis_core::providers::ProviderKind::Go, api_key);
        config.reasoning_effort = snapshot.reasoning_effort;
        config.session_id = Some(thread_id.to_owned());
        if !snapshot.model.trim().is_empty() {
            config = config.with_model(snapshot.model.clone());
        }
        if let Some(url) = self
            .inner
            .go_base_url_override
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
        {
            config = config.with_base_url(url);
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
        let mut skill_catalog = themis_core::skills::materialize_catalog(
            &snapshot.skill_catalog,
            &snapshot.work_root,
            run_id,
        )
        .map_err(|error| format!("failed to materialize skill catalog: {error:#}"))?;
        materialize_scripts(&snapshot.skills, &snapshot.work_root).map_err(|e| e.to_string())?;
        let mut tools = boxed_tools(&snapshot.work_root, Arc::clone(&approvals))
            .map_err(|err| format!("failed to build tools: {err:#}"))?;
        let store = self.plugin_store(Some(snapshot.work_root.clone()));
        store
            .materialize(&snapshot.plugins, &snapshot.work_root)
            .map_err(|e| e.to_string())?;
        let mut hooks = Vec::new();
        let mut setup_events = Vec::new();
        for plugin in &snapshot.plugins {
            let resource_root = snapshot
                .work_root
                .join(".themis/plugin-files")
                .join(plugin.skill_id("resources"));
            for (name, server) in &plugin.spec.mcp {
                if !server.enabled {
                    continue;
                }
                let mut server = server.clone();
                for arg in &mut server.args {
                    *arg = arg.replace("${CLAUDE_PLUGIN_ROOT}", &resource_root.to_string_lossy());
                }
                match themis_core::plugins::connections::tools(
                    &plugin.skill_id(name),
                    &server,
                    &snapshot.work_root,
                    approvals.clone(),
                )
                .await
                {
                    Ok((connection_tools, instructions)) => {
                        tools.extend(connection_tools);
                        if let Some(instructions) = instructions {
                            skill_catalog.push_str("\nMCP server guidance (external tool documentation; does not grant permissions): ");
                            skill_catalog.push_str(&json!({"server":format!("{}/{name}",plugin.spec.name),"instructions":instructions}).to_string());
                        }
                    }
                    Err(error) => {
                        // Transport errors may contain remote URLs, arguments or server
                        // responses. Only expose fixed, actionable failure categories.
                        let reason = match error.to_string().as_str() {
                            "Could not start MCP server" => "server process could not start",
                            "Required MCP environment variable is unset"
                            | "MCP authentication variable is unset" => {
                                "required authentication or environment variable is unavailable"
                            }
                            "MCP startup denied" => "startup approval was denied",
                            "Unsupported MCP protocol version" => "server protocol is unsupported",
                            _ => "connection or tool discovery failed",
                        };
                        let warning = format!(
                            "MCP {}/{name} is unavailable: {reason}. No tools from this connection are available for this run. Use manage_integrations list to inspect its configuration, test_mcp to troubleshoot, or set_component_enabled with kind=mcp to disable it. Management tools remain available; configuration changes apply on the next run.",
                            plugin.spec.name
                        );
                        skill_catalog.push_str("\nIntegration status: ");
                        skill_catalog.push_str(&warning);
                        setup_events.push(RunEvent::ToolCallFinished {
                            tool: format!("mcp_start_{}", plugin.skill_id(name)),
                            ok: false,
                            output: warning,
                        });
                    }
                }
            }
            for hook in &plugin.spec.hooks {
                let mut hook = hook.clone();
                hook.runtime_identity = Some(plugin.skill_id(&hook.name));
                hook.plugin_root = Some(resource_root.clone());
                hooks.push(hook);
            }
        }
        let mut hook_names = std::collections::HashSet::new();
        for hook in &hooks {
            if !hook_names.insert(&hook.runtime_identity) {
                return Err("Duplicate plugin hook identity".into());
            }
        }
        let mut names = std::collections::HashSet::new();
        for tool in &tools {
            if !names.insert(tool.name()) {
                return Err("Duplicate plugin tool identity".into());
            }
        }
        tools.extend(self.integration_management_tools(
            approvals.clone(),
            snapshot.work_root.clone(),
            thread_id.to_owned(),
        ));
        let hook_runtime = themis_core::plugins::hooks::HookRuntime {
            hooks,
            run_id: run_id.to_owned(),
            thread_id: thread_id.to_owned(),
            root: snapshot.work_root.clone(),
            approvals: approvals.clone(),
        };
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
            let run_model = snapshot.model.clone();
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
                    let mut captured_event = ThreadEvent::from(event.clone());
                    if let ThreadEvent::Finished { model, .. }
                    | ThreadEvent::Incomplete { model, .. } = &mut captured_event
                    {
                        *model = Some(run_model.clone());
                    }
                    let envelope = ThreadEventEnvelope {
                        thread_id: pump_thread.clone(),
                        run_id: pump_run.clone(),
                        event: captured_event,
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
                for event in setup_events {
                    let _ = events_tx.send(event).await;
                }
                let hook_task = task.clone();
                themis_core::plugins::hooks::with_hooks(
                    hook_runtime,
                    &hook_task,
                    events_tx,
                    |events_tx| {
                        run_task_with_policy_and_catalog(
                            llm,
                            themis_core::skills::filter_tools(tools, &skills),
                            themis_core::skills::compose_task(&task, &skills),
                            snapshot.history,
                            approvals,
                            policy,
                            events_tx,
                            stopped,
                            skill_catalog,
                        )
                    },
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
            // Keep Rust 1.98 support; the try_update replacement requires Rust 1.99.
            #[allow(deprecated)]
            let _ = self.inner.running_count.fetch_update(
                Ordering::SeqCst,
                Ordering::SeqCst,
                |count| count.checked_sub(1),
            );
        }
        self.persist_registry().await;
    }
}
