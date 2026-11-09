//! Running the model's tool calls: approvals, hooks, result allowances, repeated-result notes and failures.

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_tool_calls(
    tools: &[Box<dyn ToolT>],
    approvals: &CachingApprovals,
    calls: &[ToolCall],
    events: &tokio::sync::mpsc::Sender<RunEvent>,
    stopped: &AtomicBool,
    evidence_directory: Option<&std::path::Path>,
    narration: &str,
    batch_bytes: usize,
) -> anyhow::Result<Vec<ToolCall>> {
    // Run the calls that fit at least 1 KiB each; the rest come back unexecuted with a
    // note, so an oversized batch costs one turn instead of the whole run.
    let runnable = calls
        .len()
        .min(batch_bytes.min(48 * 1024) / (MIN_RESULT_ALLOWANCE + 4));
    let allowance = tool_result_allowance(batch_bytes, runnable);
    if runnable == 0 || allowance < MIN_RESULT_ALLOWANCE {
        let error = "tool batch exceeds the bounded delivery allowance; request fewer calls";
        events
            .send(RunEvent::Failed {
                error: error.into(),
            })
            .await
            .ok();
        anyhow::bail!(error);
    }
    let mut results = Vec::with_capacity(calls.len());
    for (index, call) in calls.iter().enumerate() {
        check_stopped(stopped, events).await?;
        let name = call.function.name.clone();
        let args = crate::tools::repair_shell_arguments(&name, &call.function.arguments);
        let summary = summarize_call(&name, &args);
        events
            .send(RunEvent::ToolCallStarted {
                tool: name.clone(),
                summary: summary.clone(),
            })
            .await
            .ok();

        let argument_allowance = tool_argument_allowance(batch_bytes);
        let arguments_rejected = serde_json::to_vec(call)?.len() > argument_allowance;
        let outcome = if index >= runnable {
            Err(crate::prompts::fill(
                crate::prompts::RUNTIME_BATCH_NOT_RUN,
                &[
                    ("calls", &calls.len().to_string()),
                    ("runnable", &runnable.to_string()),
                ],
            ))
        } else if arguments_rejected {
            Err(crate::prompts::fill(
                crate::prompts::RUNTIME_ARGUMENTS_TOO_LARGE,
                &[("argument_allowance", &argument_allowance.to_string())],
            ))
        } else {
            execute_call(tools, approvals, &name, &args, &summary, events, stopped).await
        };
        let (ok, content) = match outcome {
            Ok(content) => (true, content),
            Err(error) => (false, error),
        };
        let (mut preview, truncated) = take_char_prefix(&content, 32768);
        if truncated {
            preview.push_str("\n[Output truncated]");
        }
        let mut result = ToolCall {
            id: call.id.clone(),
            call_type: "function".to_owned(),
            function: FunctionCall {
                name: name.clone(),
                arguments: content,
            },
        };
        events
            .send(RunEvent::ToolCallFinished {
                tool: name.clone(),
                ok,
                output: preview,
            })
            .await
            .ok();
        let mut original_snapshot = None;
        if let Some(directory) = evidence_directory {
            let records = [
                ChatMessage {
                    role: ChatRole::Assistant,
                    message_type: MessageType::ToolUse(vec![call.clone()]),
                    content: narration.to_owned(),
                },
                ChatMessage {
                    role: ChatRole::Tool,
                    message_type: MessageType::ToolResult(vec![result.clone()]),
                    content: String::new(),
                },
            ];
            // A completed action may have changed files even if Stop arrived during execution.
            // Preserve its evidence before honoring Stop at the next tool/model boundary.
            let saved = tokio::time::timeout(
                std::time::Duration::from_secs(compaction_config().timeout_seconds),
                save_evidence(directory, &records),
            )
            .await
            .map_err(|_| anyhow!("saving completed tool evidence timed out"))
            .and_then(|saved| saved);
            match saved {
                Ok((_, path)) => original_snapshot = Some(path),
                Err(error) => {
                    let error = format!("Tool evidence failed: {error}");
                    events
                        .send(RunEvent::Failed {
                            error: error.clone(),
                        })
                        .await
                        .ok();
                    return Err(anyhow!(error));
                }
            }
        }
        if !arguments_rejected
            && original_snapshot.is_some()
            && serde_json::to_vec(&result)?.len() > allowance
        {
            summary_evidence::bound_delivery(call, &mut result, allowance)?;
        }
        if arguments_rejected || serde_json::to_vec(&result)?.len() > allowance {
            let Some(path) = original_snapshot else {
                let error = "tool result exceeds the delivery allowance and no evidence directory is available; completed action must not be repeated";
                events
                    .send(RunEvent::Failed {
                        error: error.into(),
                    })
                    .await
                    .ok();
                anyhow::bail!(error);
            };
            let mut receipt = serde_json::json!({
                "completed":!arguments_rejected,"tool_ok":ok,"result_offloaded":true,
                "original_result_bytes":result.function.arguments.len(),
                "original_result_fingerprint":content_fingerprint(&result.function.arguments),
                "original_snapshot":path,
                "source":{"file_path":evidence_relative_path(&path),"jsonl_record":2,"json_pointer":"/message_type/ToolResult/0/function/arguments"},
                "note":crate::prompts::TOOL_OFFLOADED_RECEIPT
            });
            if arguments_rejected {
                receipt["arguments_rejected"] = true.into();
                receipt["original_call"] = serde_json::json!({"file_path":evidence_relative_path(&path),"jsonl_record":1,"json_pointer":"/message_type/ToolUse/0/function/arguments"});
                receipt["note"] = crate::prompts::TOOL_ARGUMENTS_REJECTED_RECEIPT.into();
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&result.function.arguments)
            {
                for key in [
                    "success",
                    "ok",
                    "isError",
                    "truncated",
                    "exit_code",
                    "path",
                    "error",
                ] {
                    if let Some(value) = value.get(key).filter(|v| {
                        v.is_boolean()
                            || v.is_number()
                            || v.as_str().is_some_and(|s| s.len() <= 256)
                    }) {
                        receipt[key] = value.clone();
                    }
                }
            }
            let original = result.function.arguments;
            let mut preview_chars = 1024;
            loop {
                receipt["preview"] =
                    serde_json::Value::String(original.chars().take(preview_chars).collect());
                result.function.arguments = receipt.to_string();
                if serde_json::to_vec(&result)?.len() <= allowance {
                    break;
                }
                if preview_chars == 0 {
                    let error = format!(
                        "tool receipt exceeds the delivery allowance; original saved at {}",
                        path.display()
                    );
                    events
                        .send(RunEvent::Failed {
                            error: error.clone(),
                        })
                        .await
                        .ok();
                    return Err(anyhow!(error));
                }
                preview_chars /= 2;
            }
        }
        results.push(result);
    }
    Ok(results)
}

pub(super) fn tool_argument_allowance(batch_bytes: usize) -> usize {
    batch_bytes.saturating_mul(2).min(64 * 1024)
}

/// The same action this many times in a run is worth a word: some repeats are fine (shooting).
pub(super) const REPEATED_ACTION_NOTE_AT: u32 = 5;

/// Browser actions that act on a page rather than read something.
pub(super) fn is_repeatable_action(name: &str, arguments: &str) -> bool {
    name == crate::tools::BROWSER_TOOL
        && serde_json::from_str::<serde_json::Value>(arguments).is_ok_and(|args| {
            matches!(
                args["action"].as_str().map(str::trim),
                Some("click" | "move" | "drag" | "press" | "type" | "scroll")
            )
        })
}

pub(super) const LOOP_WARNING: &str = crate::prompts::RUNTIME_LOOP_WARNING;

/// Adds the loop warning to a repeated result without dropping its content.
pub(super) fn warn_repeated_results(result: &mut ToolCall) {
    if !annotate_result(result, "loop_warning", LOOP_WARNING) {
        // A JSON array or scalar keeps its shape: wrap it rather than break it.
        let original = std::mem::take(&mut result.function.arguments);
        result.function.arguments = serde_json::json!({"result": serde_json::from_str::<serde_json::Value>(&original).unwrap_or_default(), "loop_warning": LOOP_WARNING}).to_string();
    }
}

/// Adds a runtime note to a tool result without dropping its content: a field on a JSON
/// object, or a line after plain text. Other JSON is left untouched (returns false), so a
/// tool's JSON output stays valid JSON.
pub(super) fn annotate_result(result: &mut ToolCall, field: &str, note: &str) -> bool {
    let arguments = &mut result.function.arguments;
    match serde_json::from_str::<serde_json::Value>(arguments) {
        Ok(mut value) if value.is_object() => {
            value[field] = note.into();
            *arguments = value.to_string();
            true
        }
        Ok(_) => false,
        Err(_) => {
            arguments.push('\n');
            arguments.push_str(note);
            true
        }
    }
}

pub(super) fn remind_unchanged_result(
    result: &mut ToolCall,
    fingerprint: u64,
    allowance: usize,
) -> anyhow::Result<()> {
    let original = result.function.arguments.clone();
    let value = serde_json::from_str::<serde_json::Value>(&original).unwrap_or_default();
    let note = crate::prompts::RUNTIME_REPEAT_NOTE;
    if value.is_object() {
        let mut annotated = value.clone();
        annotated["repeated_unchanged"] = true.into();
        annotated["repeat_note"] = note.into();
        annotated["original_result_fingerprint"] = fingerprint.into();
        result.function.arguments = annotated.to_string();
        if serde_json::to_vec(&*result)?.len() <= allowance {
            return Ok(());
        }
    }
    let mut reminder = serde_json::json!({"repeated_unchanged":true,"original_result_fingerprint":fingerprint,"repeat_note":note,"preview_incomplete":true});
    for field in [
        "source",
        "original_snapshot",
        "original_call",
        "completed",
        "tool_ok",
        "result_offloaded",
        "success",
        "ok",
        "isError",
        "exit_code",
        "path",
        "found",
        "jsonl_record",
        "json_pointer",
        "decode_tool_payload",
        "evidence_kind",
        "excerpt_offset",
        "next_offset",
        "record_role",
        "find",
        "last_match_offset",
        "search_scope",
        "truncated",
        "error",
    ] {
        if let Some(field_value) = value.get(field) {
            reminder[field] = field_value.clone();
        }
    }
    let mut chars = 512;
    loop {
        reminder["previous_observation"] = take_char_prefix(&original, chars).0.into();
        result.function.arguments = reminder.to_string();
        if serde_json::to_vec(&*result)?.len() <= allowance {
            return Ok(());
        }
        if chars == 0 {
            result.function.arguments = original;
            return Ok(());
        }
        chars /= 2;
    }
}

/// Replaces a byte-identical repeat of a result that is still in context with a
/// short reference. Coordinates stay so paging continues from the same place.
pub(super) fn freshness_receipt(result: &ToolCall, earlier_id: &str, fingerprint: u64) -> ToolCall {
    let value =
        serde_json::from_str::<serde_json::Value>(&result.function.arguments).unwrap_or_default();
    let mut receipt = serde_json::json!({
        "repeated_unchanged":true,"unchanged_since_call":earlier_id,
        "original_result_fingerprint":fingerprint,
        "repeat_note":crate::prompts::fill(crate::prompts::RUNTIME_REPEAT_IN_CONTEXT, &[("earlier_id", earlier_id)]),
    });
    for field in [
        "found",
        "path",
        "success",
        "ok",
        "exit_code",
        "truncated",
        "jsonl_record",
        "json_pointer",
        "decode_tool_payload",
        "record_role",
        "evidence_kind",
        "excerpt_offset",
        "next_offset",
        "find",
        "match_count",
        "last_match_offset",
    ] {
        if let Some(field_value) = value.get(field).filter(|v| !v.is_array() && !v.is_object()) {
            receipt[field] = field_value.clone();
        }
    }
    let mut result = result.clone();
    result.function.arguments = receipt.to_string();
    result
}

pub(super) const MIN_RESULT_ALLOWANCE: usize = 1024;

pub(super) fn tool_result_allowance(batch_bytes: usize, calls: usize) -> usize {
    (batch_bytes.min(48 * 1024) / calls.max(1))
        .saturating_sub(4)
        .min(16 * 1024)
}

pub(super) fn content_fingerprint(text: &str) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hash);
    hash.finish()
}

/// The one MCP tool a garbled MCP name most likely meant: models copy the
/// opaque `mcp_<id>_<tool>` names imperfectly (a prefix added, a digit lost,
/// another tool's id). Matches on the server's own tool name after the id.
pub(super) fn mcp_tool_meant<'a>(name: &str, available: &[&'a str]) -> Option<&'a str> {
    fn tool_part(name: &str) -> Option<&str> {
        let rest = &name[name.find("mcp_")? + 4..];
        let tool = rest.trim_start_matches(|c: char| c.is_ascii_hexdigit());
        (tool.len() < rest.len()).then_some(tool.strip_prefix('_')?)
    }
    let wanted = tool_part(name).filter(|tool| !tool.is_empty())?;
    let mut matches = available
        .iter()
        .filter(|candidate| candidate.starts_with("mcp_") && tool_part(candidate) == Some(wanted));
    let meant = matches.next()?;
    matches.next().is_none().then_some(*meant)
}

/// What the model is told when a plugin's hook denied a tool call: the plugin's words, fenced and
/// labelled so they can't pose as anything else.
pub(super) fn hook_refusal(reason: &str) -> String {
    let fenced = reason.replace("<<<", "< < <").replace(">>>", "> > >");
    format!(
        "{}\n<<<hook-reason\n{fenced}\n>>>",
        crate::prompts::HOOK_BLOCK_REASON_NOTE
    )
}

/// What an approval box says a call does: a file tool's plain words ("write `notes.txt`") as
/// the file tools' own approvals say them, otherwise the run's summary of the call.
fn approval_summary(name: &str, args: &serde_json::Value, summary: &str) -> String {
    let plain = crate::tools::summarize_fs_call(name, args);
    if plain.starts_with("call `") {
        summary.to_owned()
    } else {
        plain
    }
}

/// Approval-checks and executes one tool call.
///
/// Returns the result text for the model on success, or the error text (denial,
/// unknown tool, bad arguments, execution failure) to feed back on failure.
/// Emits [`RunEvent::ApprovalDecided`] whenever the approval hook is consulted.
pub(super) async fn execute_call(
    tools: &[Box<dyn ToolT>],
    approvals: &CachingApprovals,
    name: &str,
    args: &str,
    summary: &str,
    events: &tokio::sync::mpsc::Sender<RunEvent>,
    stopped: &AtomicBool,
) -> Result<String, String> {
    let Some(tool) = tools.iter().find(|tool| tool.name() == name) else {
        let available: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
        let hint = mcp_tool_meant(name, &available)
            .map(|meant| format!(" Did you mean '{meant}'? Copy the name exactly."))
            .unwrap_or_default();
        return Err(format!(
            "unknown tool '{name}'.{hint} (available: {})",
            available.join(", ")
        ));
    };
    let approval_args: serde_json::Value = serde_json::from_str(args).unwrap_or_default();
    let self_approving = crate::tools::tool_approves_itself(name);
    let sensitive = if self_approving {
        None
    } else {
        approvals.sensitive_access(name, &approval_args)
    };
    let (decision, review_note) = if self_approving {
        (Approval::AllowOnce, None)
    } else {
        let action = ToolAction {
            display: crate::tools::command_display_for_call(name, &approval_args),
            tool: crate::tools::automation_approval_identity(name, &approval_args),
            summary: approval_summary(name, &approval_args, summary),
            risk: if name == "shell" {
                // Destructive variants (`git push --force`, `rm -rf`, ...) carry the destructive
                // risk class; commands that only read inside the project are reads.
                crate::tools::shell_risk(&crate::tools::shell_argv(&approval_args))
            } else {
                crate::tools::automation_risk(name, &approval_args)
                    .unwrap_or_else(|| risk_for(name))
            },
            // Rules and grants on a command see it here: the tool's own check is then skipped.
            command: match name {
                "shell" => crate::tools::shell_argv(&approval_args),
                _ => Vec::new(),
            },
        };
        // A plugin may say no before the person is asked, never yes.
        let hook_denied = if approvals.would_ask(&action) {
            crate::plugins::hooks::permission_request(name, &approval_args, events).await
        } else {
            None
        };
        if let Some(reason) = hook_denied {
            crate::plugins::hooks::permission_denied(name, &approval_args, &reason, events).await;
            return Err(format!(
                "approval denied for '{name}': {}",
                hook_refusal(&reason)
            ));
        }
        let decision = match &sensitive {
            Some(access) => approvals.approve_sensitive(&action, access),
            None => approvals.approve(&action),
        };
        events
            .send(RunEvent::ApprovalDecided {
                tool: name.to_owned(),
                decision: ApprovalDecision::from(decision),
            })
            .await
            .ok();
        // Auto mode's reviewer says why, and what to do instead.
        let review_note = (decision == Approval::DenyReviewed)
            .then(|| crate::tools::take_review_note(&action))
            .flatten();
        (decision, review_note)
    };
    if stopped.load(Ordering::SeqCst) {
        return Err("Stopped by you".to_owned());
    }
    if decision.is_denied() {
        // Say which files: a shell command's summary may not make it plain.
        let files = sensitive.as_ref().map_or_else(String::new, |access| {
            format!(
                " {}",
                crate::prompts::fill(
                    crate::prompts::TOOL_SENSITIVE_DENIED,
                    &[(
                        "files",
                        &crate::tools::sensitive::name_list(&access.names())
                    )]
                )
            )
        });
        let note = review_note.unwrap_or_else(|| decision.refusal_note().to_owned());
        let reason = format!("{summary}{files} {note}");
        crate::plugins::hooks::permission_denied(name, &approval_args, &reason, events).await;
        return Err(format!("{} '{name}': {reason}", decision.refusal_prefix()));
    }
    let parsed: serde_json::Value = match serde_json::from_str(args) {
        Ok(parsed) => parsed,
        Err(err) => return Err(format!("tool '{name}' got invalid arguments: {err}")),
    };
    crate::plugins::hooks::skill_read_hooks(name, &parsed, None, events)
        .await
        .map_err(|e| e.to_string())?;
    crate::plugins::hooks::emit_current(
        "BeforeTool",
        serde_json::json!({"event":"BeforeTool","tool":name,"arguments":parsed}),
        events,
    )
    .await
    .map_err(|e| e.to_string())?;
    // Dispatch already consulted the hook above: arm the single-use permit so
    // the tool's own execution-time check passes without prompting twice.
    // (Direct `execute` calls outside `run_task` carry no permit and stay
    // fully gated.)
    let result = if self_approving {
        tool.execute(parsed.clone()).await
    } else {
        crate::tools::with_dispatch_permit(tool.execute(parsed.clone())).await
    };
    let outcome = match result {
        Ok(mut value) => {
            // Staging or committing a secret-looking file: the agent hears about it in
            // every mode, so the habit doesn't rest on it having looked first.
            let committing = sensitive
                .as_ref()
                .map(crate::tools::sensitive::SensitiveAccess::committing)
                .unwrap_or_default();
            if !committing.is_empty() && value.is_object() {
                value["secret_note"] = crate::prompts::fill(
                    crate::prompts::TOOL_SENSITIVE_COMMIT,
                    &[("files", &crate::tools::sensitive::name_list(&committing))],
                )
                .into();
            }
            Ok(serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_owned()))
        }
        Err(err) => Err(format!("tool '{name}' failed: {}", tool_failure(&err))),
    };
    crate::plugins::hooks::emit_current("AfterTool", serde_json::json!({"event":"AfterTool","tool":name,"arguments":parsed,"ok":outcome.is_ok(),"output":outcome.as_ref().ok(),"error":outcome.as_ref().err()}), events).await.map_err(|e|e.to_string())?;
    crate::plugins::hooks::skill_read_hooks(name, &parsed, Some(outcome.is_ok()), events)
        .await
        .map_err(|e| e.to_string())?;
    outcome
}

/// A tool's own error message: the tool library prints every error as "Runtime Error
/// <message>", which reached the model and the person as noise ("failed: Runtime Error ...").
pub(super) fn tool_failure(error: &autoagents::core::tool::ToolCallError) -> String {
    match error {
        autoagents::core::tool::ToolCallError::RuntimeError(message) => message.to_string(),
        other => other.to_string(),
    }
}
