//! Making room in the context: when to compact, what to keep, and offloading completed tool arguments.

use super::*;

pub(super) async fn compact_context(
    llm: &Arc<dyn LLMProvider>,
    messages: &mut Vec<ChatMessage>,
    policy: &RunPolicy,
    events: &tokio::sync::mpsc::Sender<RunEvent>,
    stopped: &AtomicBool,
    evidence_directory: Option<&std::path::Path>,
) -> anyhow::Result<bool> {
    compact_context_with_evidence(
        llm,
        messages,
        policy,
        events,
        stopped,
        std::time::Duration::from_secs(compaction_config().timeout_seconds),
        evidence_directory.map(std::path::Path::to_owned),
    )
    .await
}

#[cfg(test)]
pub(super) async fn compact_context_with_timeout(
    llm: &Arc<dyn LLMProvider>,
    messages: &mut Vec<ChatMessage>,
    policy: &RunPolicy,
    events: &tokio::sync::mpsc::Sender<RunEvent>,
    stopped: &AtomicBool,
    timeout: std::time::Duration,
) -> anyhow::Result<bool> {
    compact_context_with_evidence(llm, messages, policy, events, stopped, timeout, None).await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn compact_context_with_evidence(
    llm: &Arc<dyn LLMProvider>,
    messages: &mut Vec<ChatMessage>,
    policy: &RunPolicy,
    events: &tokio::sync::mpsc::Sender<RunEvent>,
    stopped: &AtomicBool,
    timeout: std::time::Duration,
    evidence_directory: Option<std::path::PathBuf>,
) -> anyhow::Result<bool> {
    let started = std::time::Instant::now();
    crate::context_metrics::activity("compaction_attempts");
    crate::diagnostics::emit(
        "compaction",
        "started",
        "info",
        serde_json::json!({"estimated_input_tokens":estimated_tokens(messages),"context_token_budget":policy.context_token_budget,"deadline_ms":timeout.as_millis()}),
    );
    // The saved snapshot is the job's exact input: originals, not a separate pending copy.
    // It is saved before the summary deadline starts: the deadline bounds the summary, and a
    // slow disk flush (seconds on a busy Windows machine) must not turn a checkpoint into a
    // failure that reports the originals unsaved while the save finishes in the background.
    let mut archived = None;
    let saved = match evidence_directory {
        Some(directory) => until_stopped(save_evidence(&directory, messages), stopped, events)
            .await?
            .map(|snapshot| archived = Some(snapshot)),
        None => Ok(()),
    };
    let result = match saved {
        Ok(()) => {
            until_stopped(
                SUMMARY_DEADLINE.scope(
                    tokio::time::Instant::now() + timeout,
                    tokio::time::timeout(
                        timeout,
                        Box::pin(compact_context_inner(
                            llm, messages, policy, events, &archived,
                        )),
                    ),
                ),
                stopped,
                events,
            )
            .await?
        }
        Err(error) => Ok(Err(error)),
    };
    match result {
        Ok(Ok(installed)) => {
            crate::context_metrics::activity("compaction_completed");
            crate::diagnostics::emit(
                "compaction",
                "completed",
                "info",
                serde_json::json!({"duration_ms":started.elapsed().as_millis(),"estimated_output_tokens":estimated_tokens(messages),"fallback":false}),
            );
            // Out of the summary's deadline: a slow hook can't fail a checkpoint already made.
            if installed {
                if let Some(summary) = prior_checkpoint(messages) {
                    let summary = summary.split_once(":\n").map_or(summary, |(_, text)| text);
                    crate::plugins::hooks::post_compact(summary, events).await;
                }
            }
            Ok(installed)
        }
        failure => {
            crate::context_metrics::activity("compaction_failed");
            crate::diagnostics::emit(
                "compaction",
                "summary_failed",
                "warn",
                serde_json::json!({"duration_ms":started.elapsed().as_millis(),"deadline_exceeded":failure.is_err(),"originals_saved":archived.is_some()}),
            );
            check_stopped(stopped, events).await?;
            if let Some((_, snapshot)) = &archived {
                if let Some(progress) = suspended_progress(messages, policy, snapshot) {
                    let published = events
                        .send(RunEvent::ContextSuspended { progress })
                        .await
                        .is_ok();
                    crate::diagnostics::emit(
                        "compaction",
                        "suspended_progress",
                        "info",
                        serde_json::json!({"published":published,"context_replaced":false}),
                    );
                }
            }
            let result = match failure {
                Ok(result) => result,
                Err(_) => Err(anyhow!("Required summarization timed out")),
            };
            let preservation = if archived.is_some() {
                "Originals saved on disk."
            } else {
                "Originals were not saved on disk."
            };
            // Saved job inputs are internal; retries run within this compaction job.
            result.map_err(|error| {
                anyhow!("{error}; run suspended; working context unchanged. {preservation}")
            })
        }
    }
}

pub(super) fn suspended_progress(
    messages: &[ChatMessage],
    policy: &RunPolicy,
    snapshot: &std::path::Path,
) -> Option<String> {
    if !messages.iter().any(|message| matches!(&message.message_type, MessageType::ToolResult(results) if !results.is_empty())) {
        return None;
    }
    let task = active_request_index(messages)?;
    let snapshot = evidence_relative_path(snapshot);
    let source = serde_json::json!({
        "path":&snapshot,
        "record":messages[..=task].iter().filter(|message| message.role != ChatRole::System).count(),
        "json_pointer":"/content",
    });
    let task_text = &messages[task].content;
    let mut end = task_text.len().min(128);
    while !task_text.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = format!(
        "{}\n",
        crate::prompts::fill(
            crate::prompts::SUMMARY_SUSPENDED_PROGRESS,
            &[
                ("source", &source.to_string()),
                ("task_excerpt", &task_text[..end])
            ],
        )
    );
    let limit = (policy.context_token_budget / 6).clamp(1024, 8192);
    let ledger = ledger::render(
        messages,
        messages.len(),
        ledger::Budget {
            total: limit.saturating_sub(prefix.len()),
            actions: limit,
        },
        Some(&snapshot),
    );
    let record = format!("{prefix}{ledger}");
    (record.len() <= limit).then_some(record)
}

pub(super) async fn compact_context_inner(
    llm: &Arc<dyn LLMProvider>,
    messages: &mut Vec<ChatMessage>,
    policy: &RunPolicy,
    events: &tokio::sync::mpsc::Sender<RunEvent>,
    evidence: &Option<(String, std::path::PathBuf)>,
) -> anyhow::Result<bool> {
    let split = context_split(messages, policy);
    let older = &messages[1..split];
    if older.is_empty() {
        return Ok(false);
    }
    events.send(RunEvent::ContextCompacting).await.ok();
    let summary_tokens = checkpoint_token_allowance(messages, policy);
    let current_request = bounded_context_hint(
        active_request_index(messages).map_or("", |index| messages[index].content.as_str()),
    );
    let summary_input_budget = MODEL_REQUEST_BUDGET
        .try_with(|budget| *budget)
        .unwrap_or(policy.context_token_budget);
    let summary_input_budget = crate::providers::summary_context_limit(llm.model())
        .map_or(summary_input_budget, |cap| cap.min(summary_input_budget));
    // A window a context-length error taught for this model applies too.
    let summary_input_budget = crate::context_window::learned_limit(llm.model())
        .map_or(summary_input_budget, |limit| {
            limit.min(summary_input_budget)
        });
    let (request, coverage, sampled) = prepare_compaction(
        // Retaining recent messages verbatim must not hide their outcomes from the
        // state update, or its next-step plan can contradict already completed work.
        &messages[1..],
        &current_request,
        summary_input_budget,
        summary_tokens,
    )?;
    anyhow::ensure!(
        evidence.is_some() || !sampled,
        "sampled compaction requires saved originals; existing context preserved"
    );
    crate::diagnostics::emit(
        "compaction",
        "prepared",
        "info",
        serde_json::json!({
            "estimated_input_tokens":estimated_tokens(&request), "max_tokens":summary_tokens,
            "context_token_budget":policy.context_token_budget,
        }),
    );
    // One summary request per checkpoint. Excerpted or omitted detail stays in the saved
    // originals, which `recall` searches; no staged pass over every byte is needed.
    let mut summary = crate::providers::COMPACTION_REQUEST
        .scope((), compaction_chat(llm, &request, summary_tokens))
        .await?;
    summary.push_str("\n\n");
    summary.push_str(&coverage);
    if let Some((navigation, _)) = evidence {
        summary.push_str("\n\n");
        summary.push_str(navigation);
    }
    replace_context(
        messages,
        policy,
        split,
        summary,
        events,
        evidence.as_ref().map(|(_, path)| path.as_path()),
    )
    .await?;
    // What the checkpoint can't be trusted to keep exactly, read back as it stands now.
    if let Some(note) = CHECKPOINT_NOTE.try_with(|note| note()).ok().flatten() {
        messages.push(ChatMessage {
            role: ChatRole::User,
            message_type: MessageType::Text,
            content: note,
        });
    }
    Ok(true)
}

/// Bound the single summary request locally, keeping prior task state intact. Long fields are
/// cut to an equal head/tail ceiling; when even empty fields cannot fit, the oldest new records
/// are left to the saved originals. The flag reports any cut, which needs saved originals.
pub(super) fn prepare_compaction(
    older: &[ChatMessage],
    current_request: &str,
    budget: usize,
    summary_tokens: u32,
) -> anyhow::Result<([ChatMessage; 2], String, bool)> {
    let previous = prior_checkpoint(older)
        .map(checkpoint_task_state)
        .unwrap_or("");
    let (records, skipped) = summary_evidence::records(older)?;
    let first_request = first_request_section(older);
    let build = |limit: usize, dropped: usize| {
        let mut sampled = dropped > 0;
        let mut projected = records[dropped..].to_vec();
        for record in &mut projected {
            if let Some(text) = record["content"].as_str() {
                let (excerpt, cut) = compaction_excerpt(text, limit);
                record["content"] = excerpt.into();
                sampled |= cut;
            }
            for kind in ["ToolUse", "ToolResult"] {
                if let Some(calls) = record["message_type"]
                    .get_mut(kind)
                    .and_then(serde_json::Value::as_array_mut)
                {
                    for call in calls {
                        if let Some(text) = call["function"]["arguments"].as_str() {
                            let (excerpt, cut) = compaction_excerpt(text, limit.min(8_000));
                            call["function"]["arguments"] = excerpt.into();
                            sampled |= cut;
                        }
                    }
                }
            }
        }
        let omitted = if dropped > 0 {
            format!(" {dropped} oldest new records omitted (only in saved originals);")
        } else {
            String::new()
        };
        let completeness = if sampled || skipped > 0 {
            "coverage is incomplete; only selected excerpts were summarized."
        } else {
            "All eligible new records supplied; the resulting summary is still lossy."
        };
        let coverage = crate::prompts::fill(
            crate::prompts::SUMMARY_COVERAGE,
            &[
                ("selected", &projected.len().to_string()),
                ("omitted", &omitted),
                ("skipped", &skipped.to_string()),
                ("completeness", completeness),
            ],
        );
        let notes = projected
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let mut request = context_update_messages(&notes, previous, current_request, true);
        if let Some(section) = &first_request {
            request[1].content.push_str(&format!("\n\n{section}"));
        }
        request[1].content.push_str(&format!("\n\n{coverage}"));
        (request, coverage, sampled)
    };
    // Reserve the retry's larger output and guidance before sending any provider request.
    let reserve = summary_tokens as usize * 2 + 1024;
    let fits =
        |request: &[ChatMessage]| estimated_tokens(request).saturating_add(reserve) <= budget;
    let mut high = budget.saturating_mul(4);
    let full = build(high, 0);
    if fits(&full.0) {
        return Ok(full);
    }
    // Fewest oldest records to leave out so that the remaining ones fit with empty fields.
    let (mut keep_from, mut drop_all) = (0, records.len());
    anyhow::ensure!(
        fits(&build(0, drop_all).0),
        "prior state and summary instructions exceed the request budget"
    );
    while keep_from < drop_all {
        let middle = keep_from + (drop_all - keep_from) / 2;
        if fits(&build(0, middle).0) {
            drop_all = middle;
        } else {
            keep_from = middle + 1;
        }
    }
    let dropped = drop_all;
    let mut low = 0;
    // ponytail: equal per-field excerpt ceiling; semantic selection requires targeted recovery.
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if fits(&build(middle, dropped).0) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    Ok(build(low, dropped))
}

pub(super) fn compaction_excerpt(input: &str, limit: usize) -> (String, bool) {
    if input.len() <= limit {
        return (input.to_owned(), false);
    }
    let mut head = limit / 2;
    while !input.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = input.len().saturating_sub(limit / 2);
    while !input.is_char_boundary(tail) {
        tail += 1;
    }
    let excerpt = format!(
        "{}\n[UTF-8 bytes {head}..{tail} omitted; coverage is incomplete]\n{}",
        &input[..head],
        &input[tail..]
    );
    if excerpt.len() >= input.len() {
        (input.to_owned(), false)
    } else {
        (excerpt, true)
    }
}

/// Short system-message pointer to the chat's saved originals.
///
/// The `Recoverable evidence directory: "<json>".` prefix is parsed by clients.
pub(super) fn evidence_pointer(
    directory: &std::path::Path,
    recall_offered: bool,
) -> anyhow::Result<String> {
    let directory = serde_json::to_string(directory)?;
    let values = [
        ("directory", directory.as_str()),
        ("caution", crate::prompts::SYSTEM_EVIDENCE_CAUTION),
    ];
    let template = if recall_offered {
        crate::prompts::SYSTEM_EVIDENCE_RECALL
    } else {
        crate::prompts::SYSTEM_EVIDENCE_FILES
    };
    Ok(format!("\n{}", crate::prompts::fill(template, &values)))
}

pub(super) fn prior_checkpoint(messages: &[ChatMessage]) -> Option<&str> {
    messages
        .iter()
        .rev()
        .find(|message| {
            message.role == ChatRole::Assistant
                && matches!(&message.message_type, MessageType::Text)
                && message.content.starts_with("Earlier context checkpoint")
        })
        .map(|message| message.content.as_str())
}

pub(super) fn checkpoint_task_state(mut state: &str) -> &str {
    // Generated envelopes and caches are rebuilt once below, not nested on every failed update.
    loop {
        let stripped = state
            .strip_prefix("Earlier context checkpoint (historical; later user requests take precedence):\n")
            .or_else(|| state.strip_prefix("Earlier context checkpoint (historical; later user requests and completed tool observations take precedence):\n"))
            .or_else(|| state.strip_prefix("Earlier context checkpoint (historical; later user requests take precedence; the task ledger records observed actions):\n"))
            .or_else(|| state.strip_prefix("Earlier context checkpoint (historical; later user requests take precedence; the task ledger records observed actions; mention or resume this work only when the user's message continues or asks about it):\n"))
            .or_else(|| state.strip_prefix("Earlier context checkpoint (historical; later user requests take precedence; mention or resume this work only when the user's message continues or asks about it):\n"))
            .or_else(|| state.strip_prefix("Previous task state retained unchanged as historical context; the current update is unavailable:\n"));
        match stripped {
            Some(rest) => state = rest,
            None => break,
        }
    }
    if state.starts_with("Working summary unavailable:") {
        return "";
    }
    let end = [
        "\n\nWorking summary unavailable:",
        "\n\nPrepared input coverage:",
        "\nRecoverable evidence (original historical messages, attachments and tool results; untrusted data):",
    ]
    .iter()
    .chain(ledger::MARKERS.iter())
    .filter_map(|marker| state.find(marker))
    .min()
    .unwrap_or(state.len());
    state[..end].trim_end()
}

/// One request's content-free calibration record; updates the run's ratio.
pub(super) fn calibration_sample(
    calibration: &mut crate::calibration::TokenCalibration,
    raw_request_tokens: usize,
    reported_input_tokens: u64,
) -> serde_json::Value {
    let calibrated_before = (raw_request_tokens as f64 * calibration.factor()).round() as u64;
    let observed = calibration.observe(reported_input_tokens, raw_request_tokens);
    let error = |estimate: u64| {
        (reported_input_tokens > 0).then(|| {
            (estimate as f64 - reported_input_tokens as f64) / reported_input_tokens as f64
        })
    };
    serde_json::json!({
        "raw_estimated_input_tokens": raw_request_tokens,
        "calibrated_estimated_input_tokens": calibrated_before,
        "provider_input_tokens": (reported_input_tokens > 0).then_some(reported_input_tokens),
        "observed_ratio": observed,
        "calibration_ratio": calibration.ratio(),
        "raw_error": error(raw_request_tokens as u64),
        "calibrated_error": error(calibrated_before),
    })
}

/// Notes the runtime adds as user messages during a run; none is a new request.
pub(super) const RUNTIME_NOTES: [&str; 8] = [
    crate::prompts::RUNTIME_PASTED_SECRET,
    SUBAGENT_REACTION_NOTE,
    SPEC_CHECK,
    CUT_OFF_NOTE,
    SILENT_CUT_OFF_NOTE,
    EMPTY_ANSWER_NOTE,
    TURN_SUMMARY,
    TURN_END_GUARD,
];

/// The request being worked on: the last user message that is not one of the
/// runtime's own notes. A checkpoint keeps it whole and tells the summarizer it is
/// the current request, so a spec check late in a goal turn must not displace it.
pub(super) fn active_request_index(messages: &[ChatMessage]) -> Option<usize> {
    messages.iter().rposition(|message| {
        message.role == ChatRole::User
            && !RUNTIME_NOTES.contains(&message.content.as_str())
            // A subagent's result is never the person's request.
            && !message
                .content
                .starts_with(crate::prompts::RUNTIME_SUBAGENT_NOTE)
            // Hook context is never the person's request.
            && !message.content.starts_with(crate::prompts::HOOK_CONTEXT_NOTE)
    })
}

/// The model-facing form of a subagent result: the label, then the result fenced so text
/// inside it cannot pose as the label or close the fence early.
pub fn subagent_note(note: &str) -> String {
    let fenced = note
        .chars()
        .take(16 * 1024)
        .collect::<String>()
        .replace("<<<", "< < <")
        .replace(">>>", "> > >");
    format!(
        "{} {}\n<<<subagent-result\n{fenced}\n>>>",
        crate::prompts::RUNTIME_SUBAGENT_NOTE,
        crate::prompts::SUBAGENT_REVIEW
    )
}

pub(super) fn context_split(messages: &[ChatMessage], policy: &RunPolicy) -> usize {
    // Keep recent full messages within a third of the space left after instructions.
    let recent_budget = policy
        .context_token_budget
        .saturating_sub(estimated_tokens(&messages[..1]))
        / 3;
    let mut split = messages.len().saturating_sub(policy.recent_messages.max(4));
    split = split.max(2);
    // Measure the suffix once, then drop messages from its front.
    let mut suffix = if split < messages.len() {
        estimated_tokens(&messages[split..])
    } else {
        0
    };
    while split < messages.len() && suffix > recent_budget {
        suffix -= estimated_tokens(&messages[split..=split]);
        split += 1;
    }
    if split < messages.len() && matches!(messages[split].role, ChatRole::Tool) {
        split += 1;
    }
    split
}

pub(super) fn checkpoint_token_allowance(messages: &[ChatMessage], policy: &RunPolicy) -> u32 {
    let split = context_split(messages, policy);
    let active = active_request_index(messages);
    let fixed = estimated_tokens(&messages[..1])
        .saturating_add(estimated_tokens(&messages[split..]))
        .saturating_add(
            active
                .filter(|index| *index < split)
                .map_or(0, |index| estimated_tokens(&messages[index..=index])),
        );
    // Leave space for receipts, deterministic progress, recent work and the next tool batch.
    compaction_config().summary_max_tokens.min(
        (policy.context_token_budget / 8)
            .min(policy.context_token_budget.saturating_sub(fixed) / 3)
            .max(256)
            .min(u32::MAX as usize) as u32,
    )
}

pub(super) fn offload_completed_arguments(
    invocation: &mut ChatMessage,
    results: &[ToolCall],
    snapshot: &std::path::Path,
    record: usize,
    preserve_source_selectors: bool,
) {
    if let MessageType::ToolUse(calls) = &mut invocation.message_type {
        for (index, call) in calls.iter_mut().enumerate() {
            if call.function.arguments.len() > 2048
                && results.iter().any(|result| result.id == call.id)
                && !(preserve_source_selectors
                    && results.iter().any(|result| {
                        result.id == call.id && summary_evidence::preserve_invocation(call, result)
                    }))
            {
                call.function.arguments = serde_json::json!({
                    "arguments_offloaded":true,
                    "original_context_recovery":summary_evidence::recovery_call(call),
                    "original_snapshot":snapshot,
                    "archive_record":record,
                    "archive_pointer":format!("/message_type/ToolUse/{index}/function/arguments"),
                    "arguments_preview":take_char_prefix(&call.function.arguments,512).0,
                    "note":summary_evidence::OFFLOAD_NOTE
                })
                .to_string();
            }
        }
    }
}
