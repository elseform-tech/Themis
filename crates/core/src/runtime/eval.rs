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
/// The transcript's first user message, as the desktop hands it to every checkpoint request
/// (`runtime::with_first_request`). Empty when there is none.
pub fn first_user_request(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .find(|message| message.role == ChatRole::User)
        .map(|message| message.content.clone())
        .unwrap_or_default()
}

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
        content: crate::prompts::fill(crate::prompts::EVAL_ANSWER_PROBE, &[("question", question)]),
    });
    // A truncated probe answer is an eval-side failure, not a lost fact: retry once with room.
    let mut last = None;
    for max_tokens in [400, 1600] {
        let sampling = SamplingOverrides::with_max_tokens(max_tokens);
        match llm.chat_and_sampling(&asked, None, Some(&sampling)).await {
            Ok(response) => return Ok(response.text().unwrap_or_default().trim().to_owned()),
            Err(error) if error.to_string().contains("truncated") => last = Some(error),
            Err(error) => return Err(error.into()),
        }
    }
    Err(last.map_or_else(|| anyhow!("probe answer failed"), Into::into))
}

/// What one real agent turn did after a compaction.
#[derive(Debug, Clone, Serialize)]
pub struct BehaviorTurn {
    pub reply: String,
    /// Tool calls the model attempted (every call is denied; nothing executes).
    pub tool_calls: Vec<String>,
    /// Checkpoints installed during the turn.
    pub compactions: usize,
    pub error: Option<String>,
}

/// Text view of post-compaction working messages as prior conversation turns.
/// Fixture transcripts used by behavior evals are text-only; tool records are skipped.
pub fn history_from_messages(messages: &[ChatMessage]) -> Vec<ConversationTurn> {
    messages
        .iter()
        .skip(1)
        .filter(|message| matches!(message.message_type, MessageType::Text))
        .map(|message| ConversationTurn {
            role: if message.role == ChatRole::User {
                ConversationRole::User
            } else {
                ConversationRole::Assistant
            },
            text: message.content.clone(),
        })
        .collect()
}

/// Runs the real agent loop for one user message on top of `history`, with the full tool set
/// offered but every approval denied, and reports the reply, attempted tool calls and any
/// checkpoint. `short_reply` mirrors what the desktop marks for short, attachment-free messages.
pub async fn behavior_turn(
    llm: &Arc<dyn LLMProvider>,
    history: Vec<ConversationTurn>,
    message: &str,
    policy: EvalPolicy,
    short_reply: bool,
    project: &std::path::Path,
) -> anyhow::Result<BehaviorTurn> {
    use crate::tools::{boxed_tools, ApprovalHook, DenyAllHook};
    let evidence = project.join(".themis/context/behavior");
    std::fs::create_dir_all(&evidence)?;
    let approvals: Arc<dyn ApprovalHook> = Arc::new(DenyAllHook);
    let (events, mut receiver) = tokio::sync::mpsc::channel(4096);
    let collector = tokio::spawn(async move {
        let mut seen = Vec::new();
        while let Some(event) = receiver.recv().await {
            seen.push(event);
        }
        seen
    });
    #[allow(unused_mut)]
    let mut tools = boxed_tools(project, approvals.clone())?;
    // The desktop offers `recall` to every chat; mirror it when the build has the tool.
    #[cfg(eval_recall)]
    tools.push(Box::new(crate::tools::recall::RecallTool::new(
        crate::tools::recall::RecallScope {
            evidence_dir: evidence.clone(),
            attachments: Vec::new(),
            attachment_root: None,
            project_root: project.canonicalize().ok(),
        },
    )));
    let result = with_short_reply_turn(
        short_reply,
        run_task_with_evidence(
            llm.clone(),
            tools,
            message.to_owned(),
            history,
            approvals,
            RunPolicy {
                total_turns: 4,
                context_token_budget: policy.context_token_budget,
                recent_messages: policy.recent_messages,
            },
            events,
            Arc::new(AtomicBool::new(false)),
            String::new(),
            Some(evidence),
        ),
    )
    .await;
    let seen = collector.await?;
    let tool_calls = seen
        .iter()
        .filter_map(|event| match event {
            RunEvent::ToolCallStarted { tool, summary } => Some(format!("{tool}: {summary}")),
            _ => None,
        })
        .collect();
    let compactions = seen
        .iter()
        .filter(|event| matches!(event, RunEvent::ContextCheckpoint { .. }))
        .count();
    let last_text = seen.iter().rev().find_map(|event| match event {
        RunEvent::AssistantText(text) => Some(text.clone()),
        _ => None,
    });
    let (reply, error) = match result {
        Ok(text) => (text, None),
        Err(error) => (last_text.unwrap_or_default(), Some(error.to_string())),
    };
    Ok(BehaviorTurn {
        reply: reply.trim().to_owned(),
        tool_calls,
        compactions,
        error,
    })
}

#[cfg(test)]
mod file_tool_confinement_tests {
    use crate::tools::{boxed_tools_for_chat, AllowAllHook};
    use autoagents::core::tool::ToolT;
    use serde_json::{json, Value};
    use std::sync::Arc;

    async fn call(tools: &[Box<dyn ToolT>], name: &str, args: Value) -> String {
        let tool = tools.iter().find(|tool| tool.name() == name).unwrap();
        match tool.execute(args).await {
            Ok(value) => value.to_string(),
            Err(error) => error.to_string(),
        }
    }

    /// The eval agent's file tools (the same ones the app uses) must not reveal anything
    /// outside the scratch project, whether by absolute path, `..` or a symlink.
    #[tokio::test]
    async fn read_list_and_search_cannot_leave_the_project() {
        let scratch = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "ANSWER-KEY-7731").unwrap();
        let project = scratch.path().join("project");
        std::fs::create_dir_all(project.join(".themis/context/t")).unwrap();
        std::fs::write(project.join(".themis/context/t/s.jsonl"), "inside-fact").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), project.join("link")).unwrap();
        let tools =
            boxed_tools_for_chat(&project, Arc::new(AllowAllHook), false, None, None).unwrap();
        let secret = outside
            .path()
            .join("secret.txt")
            .to_string_lossy()
            .into_owned();
        let secret_dir = outside.path().to_string_lossy().into_owned();
        let mut attempts = vec![
            ("read_file", json!({"file_path": secret})),
            ("read_file", json!({"file_path": "../../secret.txt"})),
            (
                "read_file",
                json!({"file_path": format!("../{}/secret.txt", outside.path().file_name().unwrap().to_string_lossy())}),
            ),
            ("list_dir", json!({"directory_path": secret_dir})),
            ("list_dir", json!({"directory_path": ".."})),
            (
                "search_file",
                json!({"directory": secret_dir, "pattern": "ANSWER"}),
            ),
            (
                "search_file",
                json!({"directory": "..", "pattern": "ANSWER"}),
            ),
        ];
        #[cfg(unix)]
        attempts.extend([
            ("read_file", json!({"file_path": "link/secret.txt"})),
            ("list_dir", json!({"directory_path": "link"})),
            (
                "search_file",
                json!({"directory": "link", "pattern": "ANSWER"}),
            ),
            (
                "search_file",
                json!({"directory": ".", "pattern": "ANSWER"}),
            ),
        ]);
        for (name, args) in attempts {
            let output = call(&tools, name, args.clone()).await;
            assert!(
                !output.contains("ANSWER-KEY-7731") && !output.contains("secret.txt\""),
                "{name} {args} leaked: {output}"
            );
        }
        let inside = call(
            &tools,
            "read_file",
            json!({"file_path": ".themis/context/t/s.jsonl"}),
        )
        .await;
        assert!(inside.contains("inside-fact"), "{inside}");
    }
}
