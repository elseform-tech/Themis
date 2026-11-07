//! Replacing the context with a checkpoint: saved originals, the summary request and its validation.

use super::*;

pub(super) async fn replace_context(
    messages: &mut Vec<ChatMessage>,
    policy: &RunPolicy,
    split: usize,
    mut summary: String,
    events: &tokio::sync::mpsc::Sender<RunEvent>,
    original_snapshot: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let active_index = active_request_index(messages);
    let active_request = active_index
        .filter(|index| *index < split)
        .map(|index| messages[index].clone());
    let completed = original_snapshot.filter(|_| split == messages.len()).and_then(|path| {
        let last = messages.len().checked_sub(1)?;
        if !active_index.is_some_and(|index| index < last) || last == 0 || !matches!(messages[last - 1].message_type, MessageType::ToolUse(_)) {
            return None;
        }
        let MessageType::ToolResult(results) = &messages[last].message_type else { return None; };
        let mut invocation = messages[last - 1].clone();
        // Preserve public intent beside the observed outcome; a lossy summary can leave the next step stale.
        invocation.content = take_char_prefix(&invocation.content,512).0;
        offload_completed_arguments(&mut invocation, results, path, messages[..last].iter().filter(|m| m.role != ChatRole::System).count(), false);
        let results = results.iter().map(|result| {
            let mut receipt = serde_json::json!({"result_offloaded":true,"original_snapshot":path,"original_result_bytes":result.function.arguments.len(),"note":crate::prompts::TOOL_COMPACTED_RECEIPT});
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&result.function.arguments) {
                for field in ["success", "ok", "isError", "truncated", "exit_code", "path", "error"] {
                    if let Some(value) = value.get(field).filter(|value| value.is_boolean() || value.is_number() || value.as_str().is_some_and(|text| text.len() <= 2048)) {
                        receipt[field] = value.clone();
                    }
                }
            }
            let mut result = result.clone();
            result.function.arguments = receipt.to_string();
            result
        }).collect();
        Some([invocation, ChatMessage { role: ChatRole::Tool, message_type: MessageType::ToolResult(results), content: String::new() }])
    });
    let mut retained = messages[split..].to_vec();
    if let Some(snapshot) = original_snapshot {
        for index in 0..retained.len().saturating_sub(1) {
            let (earlier, later) = retained.split_at_mut(index + 1);
            if let MessageType::ToolResult(results) = &later[0].message_type {
                let record = messages[..split + index + 1]
                    .iter()
                    .filter(|message| message.role != ChatRole::System)
                    .count();
                offload_completed_arguments(&mut earlier[index], results, snapshot, record, true);
            }
        }
    }
    let mut compacted = vec![messages[0].clone()];
    // The checkpoint includes work completed for this request; preserve that chronology.
    compacted.extend(active_request);
    let checkpoint_index = compacted.len();
    compacted.push(ChatMessage {
        role: ChatRole::Assistant,
        message_type: MessageType::Text,
        content: format!("Earlier context checkpoint (historical; later user requests take precedence; the task ledger records observed actions; mention or resume this work only when the user's message continues or asks about it):\n{summary}"),
    });
    compacted.extend(completed.into_iter().flatten());
    compacted.extend(retained);
    if estimated_tokens(&compacted) > policy.context_token_budget {
        anyhow::bail!("checkpoint and active request exceed the context budget; original conversation preserved");
    }
    // Budget all generated caches against the actual fixed messages, including JSON escaping.
    // ponytail: halve cache allowances on overflow; originals remain available for exact recovery.
    let fixed_tokens = estimated_tokens(&compacted);
    // Leave half the available space (at least 20% overall) for the next actions, not caches.
    let continuation_reserve = policy
        .context_token_budget
        .saturating_sub(estimated_tokens(&compacted[..checkpoint_index]))
        / 2;
    let continuation_reserve = continuation_reserve.max(policy.context_token_budget / 5);
    let target = policy
        .context_token_budget
        .saturating_sub(continuation_reserve)
        .max(fixed_tokens);
    let mut cache_bytes = target.saturating_sub(fixed_tokens).saturating_mul(4) / 3;
    let checkpoint_prefix = compacted[checkpoint_index].content.clone();
    loop {
        let actions = cache_bytes
            .min((policy.context_token_budget / 12).saturating_mul(4))
            .min(8192);
        let reads = cache_bytes
            .min((policy.context_token_budget / 8).saturating_mul(4))
            .min(64_000);
        let ledger = ledger::render(
            messages,
            split,
            ledger::Budget {
                total: actions + reads,
                actions,
            },
            original_snapshot,
        );
        compacted[checkpoint_index].content = format!("{checkpoint_prefix}{ledger}");
        if estimated_tokens(&compacted) <= target || cache_bytes == 0 {
            summary.push_str(&ledger);
            break;
        }
        cache_bytes /= 2;
    }
    if estimated_tokens(&compacted) > policy.context_token_budget {
        anyhow::bail!("checkpoint and active request exceed the context budget; original conversation preserved");
    }
    let mut durable_summary = summary;
    durable_summary.push_str("\nRecent completed context:\n");
    for (index, message) in compacted.iter().enumerate().skip(1) {
        if index != checkpoint_index {
            durable_summary.push_str(&serde_json::to_string(message)?);
            durable_summary.push('\n');
        }
    }
    match events.try_send(RunEvent::ContextCheckpoint {
        summary: durable_summary,
    }) {
        Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            anyhow::bail!("checkpoint event queue is full; original context preserved");
        }
    }
    *messages = compacted;
    Ok(())
}

pub(super) async fn save_evidence(
    directory: &std::path::Path,
    messages: &[ChatMessage],
) -> anyhow::Result<(String, std::path::PathBuf)> {
    let directory = directory.to_owned();
    let messages = messages.to_vec();
    tokio::task::spawn_blocking(move || archive_context(&directory, &messages)).await?
}

pub(super) fn evidence_records(messages: &[ChatMessage]) -> anyhow::Result<Vec<String>> {
    let results = messages
        .iter()
        .filter_map(|message| match &message.message_type {
            MessageType::ToolResult(results) => Some(results),
            _ => None,
        })
        .flatten()
        .map(|result| (result.id.as_str(), result))
        .collect::<std::collections::HashMap<_, _>>();
    let recovery_ids = messages
        .iter()
        .filter_map(|message| match &message.message_type {
            MessageType::ToolUse(calls) => Some(calls),
            _ => None,
        })
        .flatten()
        .filter(|call| {
            summary_evidence::recovery_pair(call, results.get(call.id.as_str()).copied())
        })
        .map(|call| call.id.as_str())
        .collect::<HashSet<_>>();

    messages
        .iter()
        .filter(|message| message.role != ChatRole::System)
        .map(|message| {
            let mut record = serde_json::to_value(message)?;
            record["evidence_kind"] = match &message.message_type {
                MessageType::ToolResult(calls) => {
                    let count = calls
                        .iter()
                        .filter(|call| recovery_ids.contains(call.id.as_str()))
                        .count();
                    for (index, call) in calls.iter().enumerate() {
                        record["message_type"]["ToolResult"][index]["evidence_kind"] =
                            if recovery_ids.contains(call.id.as_str()) {
                                "recovery"
                            } else {
                                "original"
                            }
                            .into();
                    }
                    if count == 0 {
                        "original"
                    } else if count == calls.len() {
                        "recovery"
                    } else {
                        "mixed"
                    }
                }
                _ if message.role == ChatRole::Assistant => "navigation",
                _ => "original",
            }
            .into();
            Ok(serde_json::to_string(&record)?)
        })
        .collect()
}

pub(super) fn archive_context(
    directory: &std::path::Path,
    messages: &[ChatMessage],
) -> anyhow::Result<(String, std::path::PathBuf)> {
    use std::io::Write;
    let version = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let path = directory.join(format!("snapshot-{version}.jsonl"));
    let temporary = path.with_extension("partial");
    let mut file = std::io::BufWriter::new(
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?,
    );
    let saved = (|| {
        for record in evidence_records(messages)? {
            file.write_all(record.as_bytes())?;
            file.write_all(b"\n")?;
        }
        file.flush()?;
        file.get_ref().sync_all()?;
        Ok::<_, anyhow::Error>(())
    })();
    drop(file);
    if let Err(error) = saved {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    // Publish complete evidence atomically without replacing an existing snapshot.
    let published = std::fs::hard_link(&temporary, &path);
    let _ = std::fs::remove_file(&temporary);
    published?;
    let count = std::fs::read_dir(directory)?.try_fold(0usize, |count, entry| {
        let entry = entry?;
        let snapshot = entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "jsonl")
            && entry.file_type()?.is_file();
        Ok::<_, std::io::Error>(count + usize::from(snapshot))
    })?;
    let text = crate::prompts::fill(
        crate::prompts::SUMMARY_ARCHIVE_CONTEXT,
        &[
            ("directory", &serde_json::to_string(directory)?),
            ("count", &count.to_string()),
            ("latest_snapshot", &serde_json::to_string(&path)?),
        ],
    );
    Ok((text, path))
}

// ponytail: bounded head/tail relevance hint; the active request remains intact for continuation.
pub(super) fn bounded_context_hint(input: &str) -> String {
    if input.len() <= 8_192 {
        input.to_owned()
    } else {
        let mut head = 4_096;
        while !input.is_char_boundary(head) {
            head -= 1;
        }
        let mut tail = input.len() - 4_096;
        while !input.is_char_boundary(tail) {
            tail += 1;
        }
        let omitted = crate::prompts::fill(
            crate::prompts::SUMMARY_RELEVANCE_HINT_OMITTED,
            &[("omitted_bytes", &(tail - head).to_string())],
        );
        format!("{}\n{omitted}\n{}", &input[..head], &input[tail..])
    }
}

// Retry only the failed model request; the caller owns the shared deadline and Stop boundary.
pub(super) fn configured_summary_prompt(default: &str) -> String {
    let mut text = default.trim_end().to_owned();
    if let Some(prompt) = compaction_config().prompt {
        text.push_str("\n\n");
        text.push_str(crate::prompts::SUMMARY_CONFIGURED_GUIDANCE);
        text.push('\n');
        text.push_str(&prompt);
    }
    text
}

pub(super) async fn summary_request(
    llm: &Arc<dyn LLMProvider>,
    messages: &[ChatMessage],
    sampling: &SamplingOverrides,
) -> Result<Box<dyn autoagents::llm::chat::ChatResponse>, autoagents::llm::error::LLMError> {
    let configured = std::time::Duration::from_secs(compaction_config().request_timeout_seconds);
    let remaining = SUMMARY_DEADLINE
        .try_with(|deadline| deadline.saturating_duration_since(tokio::time::Instant::now()))
        .unwrap_or(configured);
    let slice = SUMMARY_STAGE_DEADLINE
        .try_with(|deadline| deadline.saturating_duration_since(tokio::time::Instant::now()))
        .unwrap_or(configured);
    tokio::time::timeout(
        configured.min(remaining).min(slice),
        llm.chat_and_sampling(messages, None, Some(sampling)),
    )
    .await
    .map_err(|_| autoagents::llm::error::LLMError::Generic("Summarizer request timed out".into()))?
}

pub(super) async fn summary_attempt(
    llm: &Arc<dyn LLMProvider>,
    messages: &[ChatMessage],
    max_tokens: u32,
    purpose: summary_job::Purpose,
) -> Result<String, autoagents::llm::error::LLMError> {
    use autoagents::llm::error::LLMError;
    let answer = summary_request(
        llm,
        messages,
        &SamplingOverrides::with_max_tokens(max_tokens),
    )
    .await?;
    let text = answer.text().unwrap_or_default();
    if text.trim().is_empty() {
        return Err(LLMError::Generic("Summarizer returned empty text".into()));
    }
    purpose
        .validate(&text)
        .map_err(|_| LLMError::Generic("Summarizer returned malformed checkpoint".into()))?;
    Ok(text)
}

/// Whether a provider error is worth retrying: the library's transient errors, plus HTTP 499,
/// which OpenCode Go returns when its upstream endpoint is briefly unavailable.
pub(super) fn transient(error: &autoagents::llm::error::LLMError) -> bool {
    error.is_retryable() || error.http_status_code() == Some(499)
}

pub(super) fn summary_retry_reason(
    error: &autoagents::llm::error::LLMError,
) -> Option<&'static str> {
    use autoagents::llm::error::LLMError;
    match error {
        LLMError::Generic(message) => match message.as_str() {
            "Provider response was truncated" => Some("output_truncated"),
            "Provider response failed or was incomplete" => Some("response_incomplete"),
            // The providers' own word for an empty reply, and the summarizer's.
            "Summarizer returned empty text" | "Provider returned no text or tools" => {
                Some("empty_output")
            }
            "Summarizer returned malformed checkpoint" => Some("malformed_output"),
            "Summarizer request timed out" => Some("request_timeout"),
            _ => transient(error).then_some("transient_provider_error"),
        },
        error if transient(error) => Some("transient_provider_error"),
        _ => None,
    }
}

pub(super) async fn compaction_chat(
    llm: &Arc<dyn LLMProvider>,
    messages: &[ChatMessage],
    max_tokens: u32,
) -> anyhow::Result<String> {
    summary_chat(llm, messages, max_tokens, summary_job::Purpose::Context).await
}

pub(super) async fn summary_chat(
    llm: &Arc<dyn LLMProvider>,
    messages: &[ChatMessage],
    max_tokens: u32,
    purpose: summary_job::Purpose,
) -> anyhow::Result<String> {
    use autoagents::llm::error::LLMError;
    anyhow::ensure!(!messages.is_empty(), "Summary requires source messages");
    let mut bounded = messages.to_vec();
    let words = (max_tokens / 3).clamp(80, 1200).to_string();
    bounded[0].content.push('\n');
    bounded[0].content.push_str(&crate::prompts::fill(
        crate::prompts::SUMMARY_OUTPUT_BUDGET,
        &[("words", &words), ("guidance", purpose.output_guidance())],
    ));
    let config = compaction_config();
    let deadline = SUMMARY_DEADLINE
        .try_with(|deadline| *deadline)
        .unwrap_or_else(|_| {
            tokio::time::Instant::now() + std::time::Duration::from_secs(config.timeout_seconds)
        });
    let deadline = SUMMARY_STAGE_DEADLINE
        .try_with(|stage| deadline.min(*stage))
        .unwrap_or(deadline);
    // All attempts and backoff share one deadline. Retries never restart the stage plan.
    let result = tokio::time::timeout_at(deadline, async {
        for attempt in 0..=config.retry_limit {
            if tokio::time::Instant::now() >= deadline {
                return Err(LLMError::Generic("Required summarization timed out".into()));
            }
            // Input preparation reserves 2x output, so later retries must not grow it again.
            let allowance = max_tokens.saturating_mul(if attempt == 0 { 1 } else { 2 });
            let result = crate::context_metrics::with_attempt(
                attempt as usize, summary_attempt(llm, &bounded, allowance, purpose),
            ).await;
            match result {
                Ok(text) => return Ok(text),
                Err(error) => {
                    let reason = summary_retry_reason(&error);
                    if attempt == config.retry_limit || reason.is_none() {
                        return Err(error);
                    }
                    let delay = request_retry_delay_with_rate_limit_floor(&error, attempt as u32, config.rate_limit_backoff_seconds);
                    crate::diagnostics::emit("compaction", "request_retry", "warn", serde_json::json!({
                        "retry":attempt+1,"reason":reason,"status":error.http_status_code(),
                        "backoff_ms":delay.as_millis(),"max_tokens":max_tokens.saturating_mul(2),
                        "remaining_ms":deadline.saturating_duration_since(tokio::time::Instant::now()).as_millis(),
                    }));
                    // Retry-After is a floor: an expiring deadline cancels the wait, not the floor.
                    tokio::time::sleep(delay).await;
                }
            }
        }
        unreachable!("every attempt returns success, failure, or advances to the next retry")
    }).await.unwrap_or_else(|_| Err(LLMError::Generic("Required summarization timed out".into())));
    result.inspect_err(|error| {
        // Provider bodies and malformed summary text are never diagnostic payloads.
        crate::diagnostics::emit("compaction", "request_failed", "warn", serde_json::json!({
            "status":error.http_status_code(),"transport":matches!(error, LLMError::HttpError(_)),
            "reason":summary_retry_reason(error).unwrap_or("permanent_or_deadline_failure"),
        }));
    }).map_err(Into::into)
}

pub(super) fn validate_checkpoint(text: &str) -> anyhow::Result<()> {
    // Observed actions, reads and the plan live in the runtime task ledger; the
    // model keeps only what the runtime cannot record. Older five-section
    // checkpoints contain these headings in order and remain valid.
    // Checkpoints written with a "Parked work:" section use four; older ones use three.
    let has_parked_heading = text.lines().any(|line| {
        line.trim_start()
            .to_ascii_lowercase()
            .starts_with("parked work:")
    });
    let headings: &[&str] = if has_parked_heading {
        &[
            "goal/constraints:",
            "unresolved work:",
            "parked work:",
            "next action:",
        ]
    } else {
        &["goal/constraints:", "unresolved work:", "next action:"]
    };
    validate_summary_sections(text, headings)
}

pub(super) fn validate_summary_sections(text: &str, headings: &[&str]) -> anyhow::Result<()> {
    let normalized = text.to_ascii_lowercase();
    let mut positions = Vec::new();
    let mut after = 0;
    for heading in headings {
        let position = normalized[after..]
            .find(heading)
            .map(|offset| after + offset)
            .ok_or_else(|| anyhow!("Required summary is missing or reorders section {heading}"))?;
        positions.push((position, position + heading.len()));
        after = position + heading.len();
    }
    for (index, (_, start)) in positions.iter().enumerate() {
        let end = positions.get(index + 1).map_or(text.len(), |p| p.0);
        anyhow::ensure!(
            normalized[*start..end].chars().any(char::is_alphanumeric),
            "Required summary has an empty section {}",
            headings[index]
        );
    }
    Ok(())
}

pub(super) fn context_update_messages(
    notes: &str,
    previous: &str,
    current_request: &str,
    originals: bool,
) -> [ChatMessage; 2] {
    let previous = checkpoint_task_state(previous);
    let label = if originals {
        "NEW MESSAGE RECORDS WITH PROVENANCE"
    } else {
        "ORDERED SECTION NOTES TO RECONCILE"
    };
    [
        ChatMessage {
            role: ChatRole::System,
            message_type: MessageType::Text,
            content: configured_summary_prompt(crate::prompts::COMPACTION_CHECKPOINT),
        },
        ChatMessage {
            role: ChatRole::User,
            message_type: MessageType::Text,
            content: crate::prompts::fill(
                crate::prompts::SUMMARY_CHECKPOINT_REQUEST,
                &[
                    ("previous", previous),
                    ("label", label),
                    ("notes", notes),
                    ("current_request", current_request),
                ],
            ),
        },
    ]
}

/// Decode only trusted runtime-written checkpoints. Malformed legacy data stays intact.
pub fn checkpoint_parts(saved: &str) -> (&str, Vec<ChatMessage>) {
    let Some((summary, lines)) = saved.rsplit_once("\nRecent completed context:\n") else {
        return (saved, Vec::new());
    };
    let recent = lines
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str::<ChatMessage>)
        .collect::<Result<Vec<_>, _>>();
    match recent {
        Ok(recent)
            if recent
                .iter()
                .all(|message| message.role != ChatRole::System) =>
        {
            (summary, recent)
        }
        _ => (saved, Vec::new()),
    }
}
