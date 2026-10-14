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

/// Result of answering one probe with a real agent turn that has tools.
#[derive(Debug, Clone, Serialize)]
pub struct AgentAnswer {
    /// Final assistant text (empty when the run produced none).
    pub answer: String,
    /// Names of every tool call the agent issued, in order (including `update_plan`).
    pub tools: Vec<String>,
    /// Tool calls the allow-read policy refused (including calls past the cap).
    pub denied: usize,
    /// Tool names the build offered, so a harness can tell whether `recall` existed.
    pub tools_available: Vec<String>,
    /// Provider requests issued by this turn (answer requests plus any compaction).
    pub requests: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub latency_ms: u128,
    /// Whether the run ended with a plain-text answer (not failed, stopped or incomplete).
    pub finished: bool,
    pub error: Option<String>,
}

/// Allow-read approval policy for agent-mode probes: read tools run, a short list of
/// non-writing shell programs run, everything else is refused, and at most `max_calls`
/// actions run in total. The caller must pass a fresh scratch project outside the repository;
/// this is a guard for that scratch directory, not a sandbox.
struct ReadPolicy {
    used: std::sync::atomic::AtomicUsize,
    denied: std::sync::atomic::AtomicUsize,
    max_calls: usize,
}

/// Programs that cannot write files or run other programs from their arguments. Interpreters
/// (`python3`, `awk`), `sed`, `find`, `sort -o` and `uniq OUT` are deliberately absent.
const READ_ONLY_PROGRAMS: &[&str] = &[
    "grep", "rg", "cat", "head", "tail", "wc", "ls", "jq", "nl", "cut", "tr",
];

fn read_only_shell(command: &[String]) -> bool {
    let Some(program) = command.first() else {
        return false;
    };
    let program = program.rsplit(['/', '\\']).next().unwrap_or(program);
    READ_ONLY_PROGRAMS.contains(&program)
        && !command
            .iter()
            .skip(1)
            .any(|arg| arg.starts_with("--pre") || arg.starts_with("--hostname-bin"))
}

impl crate::tools::ApprovalHook for ReadPolicy {
    fn approve(&self, action: &crate::tools::ToolAction) -> crate::tools::Approval {
        use crate::tools::{Approval, RiskLevel};
        let permitted = match action.risk {
            RiskLevel::Read => true,
            RiskLevel::Execute => action.tool == "shell" && read_only_shell(&action.command),
            _ => false,
        };
        let within_cap = permitted && self.used.fetch_add(1, Ordering::SeqCst) < self.max_calls;
        if within_cap {
            Approval::AllowOnce
        } else {
            self.denied.fetch_add(1, Ordering::SeqCst);
            Approval::Deny
        }
    }
}

/// Answers `question` with a real agent turn (default toolset under an allow-read policy)
/// that starts from the post-compaction `messages`. `project` is the tools' root and
/// `evidence` must be a directory inside it (the app's `.themis/context/<thread>` layout).
/// At most `max_calls` tool actions may run; the turn is also bounded in model turns.
#[allow(clippy::too_many_arguments)]
pub async fn answer_probe_agent(
    llm: &Arc<dyn LLMProvider>,
    messages: &[ChatMessage],
    question: &str,
    project: &std::path::Path,
    evidence: &std::path::Path,
    policy: EvalPolicy,
    max_calls: usize,
) -> anyhow::Result<AgentAnswer> {
    // Mirror the durable checkpoint the app stores: summary, then every other non-system
    // message in order (the active request may precede the checkpoint message).
    let checkpoint = messages
        .iter()
        .position(|message| message.content.starts_with("Earlier context checkpoint"))
        .ok_or_else(|| anyhow!("post-compaction context has no checkpoint to continue from"))?;
    let (_, summary) = messages[checkpoint]
        .content
        .split_once(":\n")
        .ok_or_else(|| anyhow!("malformed checkpoint message"))?;
    let mut saved = format!("{summary}\nRecent completed context:\n");
    for (index, message) in messages.iter().enumerate().skip(1) {
        if index != checkpoint {
            saved.push_str(&serde_json::to_string(message)?);
            saved.push('\n');
        }
    }
    let history = vec![ConversationTurn {
        role: ConversationRole::Checkpoint,
        text: saved,
    }];
    let hook = Arc::new(ReadPolicy {
        used: Default::default(),
        denied: Default::default(),
        max_calls,
    });
    let tools = crate::tools::boxed_tools_for_chat(project, hook.clone(), false, None, None)?;
    let tools_available = tools.iter().map(|tool| tool.name().to_owned()).collect();
    let (events, mut sink) = tokio::sync::mpsc::channel(1024);
    let collector = tokio::spawn(async move {
        let (mut tools, mut finished, mut error, mut metrics) =
            (Vec::new(), None, None, serde_json::Value::Null);
        while let Some(event) = sink.recv().await {
            match event {
                RunEvent::ToolCallStarted { tool, .. } => tools.push(tool),
                RunEvent::ContextUsage { metrics: latest } => metrics = latest,
                RunEvent::Finished { result } => finished = Some(result),
                RunEvent::Incomplete { result } => error = Some(format!("incomplete: {result}")),
                RunEvent::Failed { error: failure } => error = Some(failure),
                _ => {}
            }
        }
        (tools, finished, error, metrics)
    });
    let task = format!("Memory check. Earlier material in this conversation was compacted; the saved originals are under {} (project-relative). Answer the question below. The checkpoint is only a lossy summary: when it does not state the exact detail, search and read the saved originals with your tools (at most {max_calls} tool calls) instead of guessing. Reply with the answer in one short sentence, or exactly UNKNOWN if the original material never says. For a quote question copy the wording exactly. After the answer add one line per supporting source in the form `Citation: <file path> record <number>` using the snapshot file and its record (line) number where you read the fact. Do not cite a checkpoint.\n\nQuestion: {question}",
        evidence
            .strip_prefix(project)
            .unwrap_or(evidence)
            .display()
    );
    let started = std::time::Instant::now();
    let result = run_task_with_evidence(
        llm.clone(),
        tools,
        task,
        history,
        hook.clone(),
        RunPolicy {
            total_turns: max_calls + 4,
            context_token_budget: policy.context_token_budget,
            recent_messages: policy.recent_messages,
        },
        events,
        Arc::new(AtomicBool::new(false)),
        String::new(),
        Some(evidence.to_owned()),
    )
    .await;
    let latency_ms = started.elapsed().as_millis();
    let (tools, finished, event_error, metrics) = collector.await?;
    let (answer, error) = match (result, finished) {
        (Ok(text), _) => (text, None),
        (Err(error), Some(text)) => (text, Some(error.to_string())),
        (Err(error), None) => (
            String::new(),
            Some(event_error.unwrap_or_else(|| error.to_string())),
        ),
    };
    Ok(AgentAnswer {
        finished: error.is_none() && !answer.trim().is_empty(),
        answer: answer.trim().to_owned(),
        tools,
        denied: hook.denied.load(Ordering::SeqCst),
        tools_available,
        requests: metrics["total_requests"].as_u64().unwrap_or(0),
        input_tokens: metrics["provider_input_tokens"].as_u64(),
        output_tokens: metrics["provider_output_tokens"].as_u64(),
        latency_ms,
        error,
    })
}

#[cfg(test)]
mod agent_policy_tests {
    use super::*;
    use crate::tools::{Approval, ApprovalHook, RiskLevel, ToolAction};

    fn action(tool: &str, risk: RiskLevel, command: &[&str]) -> ToolAction {
        ToolAction {
            tool: tool.into(),
            summary: String::new(),
            risk,
            command: command.iter().map(|part| (*part).to_owned()).collect(),
        }
    }

    fn policy(max_calls: usize) -> ReadPolicy {
        ReadPolicy {
            used: Default::default(),
            denied: Default::default(),
            max_calls,
        }
    }

    #[test]
    fn only_non_writing_programs_run_and_calls_are_capped() {
        let hook = policy(2);
        let shell = |command: &[&str]| hook.approve(&action("shell", RiskLevel::Execute, command));
        for refused in [
            &["python3", "-c", "open('x','w')"][..],
            &["awk", "BEGIN{system(\"id\")}"],
            &["sed", "-i", "s/a/b/", "f"],
            &["find", ".", "-delete"],
            &["rg", "--pre", "sh", "x"],
            &["rm", "-rf", "."],
            &[],
        ] {
            assert_eq!(shell(refused), Approval::Deny, "{refused:?}");
        }
        assert_eq!(shell(&["grep", "-rn", "x", "."]), Approval::AllowOnce);
        assert_eq!(
            hook.approve(&action("read_file", RiskLevel::Read, &[])),
            Approval::AllowOnce
        );
        // The third permitted action exceeds the cap of two.
        assert_eq!(shell(&["cat", "f"]), Approval::Deny);
        assert_eq!(
            hook.approve(&action("write_file", RiskLevel::Write, &[])),
            Approval::Deny
        );
        assert_eq!(hook.denied.load(Ordering::SeqCst), 9);
    }
}
