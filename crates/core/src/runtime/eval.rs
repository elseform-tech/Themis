//! Repeatable compaction-quality evals (cargo feature `eval`; never enabled in releases).
//!
//! Replays a fixture transcript, forces one compaction per chunk through the same
//! internals a run uses, and answers probe questions from the post-compaction
//! working messages alone. Grading lives in the `evals/compaction` harness.

use super::*;

/// Context policy for an eval run.
#[derive(Debug, Clone, Copy)]
pub struct EvalPolicy {
    pub context_token_budget: usize,
    pub recent_messages: usize,
}

/// Metrics for one forced compaction.
#[derive(Debug, Clone, Serialize)]
pub struct ChunkReport {
    pub chunk: usize,
    /// Transcript records replayed so far (excluding the system message).
    pub records_before: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub duration_ms: u128,
    pub installed: bool,
    /// Provider requests issued by this compaction (sections, merges, retries).
    pub requests: u64,
    pub error: Option<String>,
}

/// Post-compaction working context plus per-chunk metrics.
#[derive(Debug, Clone)]
pub struct EvalOutcome {
    pub messages: Vec<ChatMessage>,
    pub chunks: Vec<ChunkReport>,
    /// False when any compaction failed or installed nothing: the probes would then
    /// be answered from uncompacted history, so the trial must not count as retention.
    pub valid: bool,
    /// 1-based transcript records still present word for word in `messages`
    /// (the verbatim tail and the active request); every other fact survives only
    /// through the checkpoint.
    pub verbatim_records: Vec<usize>,
}

#[derive(Deserialize)]
struct FixtureCall {
    id: String,
    name: String,
    #[serde(default)]
    arguments: serde_json::Value,
    #[serde(default)]
    output: String,
}

#[derive(Deserialize)]
struct FixtureRecord {
    role: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Vec<FixtureCall>,
    #[serde(default)]
    results: Vec<FixtureCall>,
}

fn fixture_call(call: FixtureCall, result: bool) -> ToolCall {
    let arguments = if result {
        call.output
    } else {
        call.arguments.to_string()
    };
    ToolCall {
        id: call.id,
        call_type: "function".into(),
        function: FunctionCall {
            name: call.name,
            arguments,
        },
    }
}

/// Parses `transcript.jsonl`: `user`/`assistant` text, `assistant` with
/// `tool_calls` (`id`, `name`, `arguments`), and `tool` with `results`
/// (`id`, `name`, `output`). The real system message is prepended.
pub fn parse_transcript(jsonl: &str) -> anyhow::Result<Vec<ChatMessage>> {
    let mut messages = vec![initial_messages(Vec::new(), String::new()).remove(0)];
    for (number, line) in jsonl
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let record: FixtureRecord = serde_json::from_str(line)
            .map_err(|error| anyhow!("transcript line {}: {error}", number + 1))?;
        let message = match record.role.as_str() {
            "user" | "assistant" if record.tool_calls.is_empty() => ChatMessage {
                role: if record.role == "user" {
                    ChatRole::User
                } else {
                    ChatRole::Assistant
                },
                message_type: MessageType::Text,
                content: record.content,
            },
            "assistant" => ChatMessage {
                role: ChatRole::Assistant,
                message_type: MessageType::ToolUse(
                    record
                        .tool_calls
                        .into_iter()
                        .map(|call| fixture_call(call, false))
                        .collect(),
                ),
                content: record.content,
            },
            "tool" => ChatMessage {
                role: ChatRole::Tool,
                message_type: MessageType::ToolResult(
                    record
                        .results
                        .into_iter()
                        .map(|call| fixture_call(call, true))
                        .collect(),
                ),
                content: String::new(),
            },
            other => anyhow::bail!("transcript line {}: unknown role {other:?}", number + 1),
        };
        messages.push(message);
    }
    Ok(messages)
}

/// Number of transcript records (excluding the system message).
pub fn record_count(messages: &[ChatMessage]) -> usize {
    messages.len().saturating_sub(1)
}

/// Replays `transcript` and forces a compaction after each record count in
/// `chunk_ends` (ascending). Records after the last boundary stay verbatim.
/// A failed compaction leaves the working context unchanged and is reported.
pub async fn compact_chunks(
    llm: &Arc<dyn LLMProvider>,
    transcript: &[ChatMessage],
    chunk_ends: &[usize],
    policy: EvalPolicy,
    evidence_directory: &std::path::Path,
) -> anyhow::Result<EvalOutcome> {
    let policy = RunPolicy {
        total_turns: 1,
        context_token_budget: policy.context_token_budget,
        recent_messages: policy.recent_messages,
    };
    let (events, mut sink) = tokio::sync::mpsc::channel(1024);
    let drain = tokio::spawn(async move { while sink.recv().await.is_some() {} });
    let stopped = AtomicBool::new(false);
    let mut working = vec![transcript[0].clone()];
    let mut taken = 1;
    let mut chunks = Vec::new();
    let result = crate::context_metrics::collect(async {
        for (index, end) in chunk_ends.iter().enumerate() {
            let end = (end + 1).min(transcript.len());
            anyhow::ensure!(end > taken, "chunk {index} ends before its start");
            working.extend_from_slice(&transcript[taken..end]);
            taken = end;
            let requests = || {
                crate::context_metrics::snapshot()
                    .and_then(|snapshot| snapshot["total_requests"].as_u64())
                    .unwrap_or(0)
            };
            let (before, tokens_before, started) = (
                requests(),
                estimated_tokens(&working),
                std::time::Instant::now(),
            );
            let outcome = compact_context_with_evidence(
                llm,
                &mut working,
                &policy,
                &events,
                &stopped,
                std::time::Duration::from_secs(compaction_config().timeout_seconds),
                Some(evidence_directory.to_owned()),
            )
            .await;
            chunks.push(ChunkReport {
                chunk: index,
                records_before: end - 1,
                tokens_before,
                tokens_after: estimated_tokens(&working),
                duration_ms: started.elapsed().as_millis(),
                installed: matches!(outcome, Ok(true)),
                requests: requests().saturating_sub(before),
                error: outcome.err().map(|error| error.to_string()),
            });
        }
        Ok(())
    })
    .await;
    drop(events);
    drain.abort();
    result?;
    working.extend_from_slice(&transcript[taken..]);
    let present: std::collections::HashSet<String> = working
        .iter()
        .filter_map(|message| serde_json::to_string(message).ok())
        .collect();
    let verbatim_records = (1..transcript.len())
        .filter(|&record| {
            serde_json::to_string(&transcript[record]).is_ok_and(|text| present.contains(&text))
        })
        .collect();
    Ok(EvalOutcome {
        messages: working,
        valid: !chunks.is_empty() && chunks.iter().all(|chunk| chunk.installed),
        chunks,
        verbatim_records,
    })
}

/// Answers `question` from `messages` alone (no tools, no archive access).
pub async fn answer_probe(
    llm: &Arc<dyn LLMProvider>,
    messages: &[ChatMessage],
    question: &str,
) -> anyhow::Result<String> {
    let mut asked = messages.to_vec();
    asked.push(ChatMessage {
        role: ChatRole::User,
        message_type: MessageType::Text,
        content: format!("Memory check. Answer from the conversation context above only; you have no tools. Reply with the answer in one short sentence, or exactly UNKNOWN if the context does not say. Do not guess.\n\nQuestion: {question}"),
    });
    let sampling = SamplingOverrides::with_max_tokens(400);
    let response = llm.chat_and_sampling(&asked, None, Some(&sampling)).await?;
    Ok(response.text().unwrap_or_default().trim().to_owned())
}
