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

/// One step of a [`task_run`]: a tool call (with whether it succeeded) or a checkpoint.
#[derive(Debug, Clone, Serialize)]
pub struct TaskStep {
    /// `"call"` or `"checkpoint"`.
    pub kind: &'static str,
    /// Tool name for a call.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tool: String,
    /// One-line call summary (tool arguments); empty for a checkpoint.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Whether the call succeeded; `None` for a checkpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
}

/// A whole agent run of one task, compacting mid-turn whenever the budget requires.
#[derive(Debug, Clone, Serialize)]
pub struct TaskRun {
    pub steps: Vec<TaskStep>,
    pub result: String,
    pub error: Option<String>,
}

/// Lets file tools edit the throwaway project; refuses `shell` and `git`, so no
/// model-written command runs on the machine that hosts the eval.
struct FileToolsOnly;

impl crate::tools::ApprovalHook for FileToolsOnly {
    fn approve(&self, action: &crate::tools::ToolAction) -> crate::tools::Approval {
        if matches!(action.tool.as_str(), "shell" | "git") {
            crate::tools::Approval::Deny
        } else {
            crate::tools::Approval::AllowOnce
        }
    }
}

/// Runs `task` from an empty conversation in `project`, as a chat turn would, and
/// records every tool call and in-run checkpoint in order.
pub async fn task_run(
    llm: &Arc<dyn LLMProvider>,
    task: &str,
    project: &std::path::Path,
    policy: EvalPolicy,
    max_turns: usize,
) -> anyhow::Result<TaskRun> {
    use crate::tools::{boxed_tools, ApprovalHook};
    let evidence = project.join(".themis/context/task");
    std::fs::create_dir_all(&evidence)?;
    let approvals: Arc<dyn ApprovalHook> = Arc::new(FileToolsOnly);
    let (events, mut receiver) = tokio::sync::mpsc::channel(4096);
    let collector = tokio::spawn(async move {
        let mut steps: Vec<TaskStep> = Vec::new();
        while let Some(event) = receiver.recv().await {
            match event {
                RunEvent::ToolCallStarted { tool, summary } => steps.push(TaskStep {
                    kind: "call",
                    tool,
                    summary,
                    ok: None,
                }),
                RunEvent::ToolCallFinished { ok, .. } => {
                    if let Some(step) = steps.iter_mut().rev().find(|s| s.kind == "call") {
                        step.ok.get_or_insert(ok);
                    }
                }
                RunEvent::ContextCheckpoint { .. } => steps.push(TaskStep {
                    kind: "checkpoint",
                    tool: String::new(),
                    summary: String::new(),
                    ok: None,
                }),
                _ => {}
            }
        }
        steps
    });
    let tools = boxed_tools(project, approvals.clone())?;
    let result = run_task_with_evidence(
        llm.clone(),
        tools,
        task.to_owned(),
        Vec::new(),
        approvals,
        RunPolicy {
            total_turns: max_turns,
            context_token_budget: policy.context_token_budget,
            recent_messages: policy.recent_messages,
        },
        events,
        Arc::new(AtomicBool::new(false)),
        String::new(),
        Some(evidence),
    )
    .await;
    let steps = collector.await?;
    let (result, error) = match result {
        Ok(text) => (text, None),
        Err(error) => (String::new(), Some(error.to_string())),
    };
    Ok(TaskRun {
        steps,
        result,
        error,
    })
}

/// What one tool call returned to the agent during a probe (bounded), so a harness can
/// check that a correct answer was grounded in material the agent actually read.
#[derive(Debug, Clone, Serialize)]
pub struct ToolOutput {
    pub tool: String,
    pub ok: bool,
    pub output: String,
}

/// Bytes of each tool output kept in a probe report.
const TOOL_OUTPUT_KEEP: usize = 16_384;

/// Result of answering one probe with a real agent turn that has tools.
#[derive(Debug, Clone, Serialize)]
pub struct AgentAnswer {
    /// Final assistant text (empty when the run produced none).
    pub answer: String,
    /// Names of every tool call the agent issued, in order (including `notes`).
    pub tools: Vec<String>,
    /// What each finished tool call returned, in order (each bounded to 16 KiB).
    pub tool_outputs: Vec<ToolOutput>,
    /// Tool calls the allow-read policy refused (including calls past the cap).
    pub denied: usize,
    /// Shell arguments the policy refused because they leave the scratch project (bounded,
    /// truncated), so a harness can flag attempts to read outside it.
    pub refused_paths: Vec<String>,
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
    /// Canonical scratch project root; every path-like shell argument must stay inside it.
    project: std::path::PathBuf,
    refused_paths: std::sync::Mutex<Vec<String>>,
    used: std::sync::atomic::AtomicUsize,
    denied: std::sync::atomic::AtomicUsize,
    max_calls: usize,
}

/// Programs that cannot write files or run other programs from their arguments. Interpreters
/// (`python3`, `awk`), `sed`, `find`, `sort -o` and `uniq OUT` are deliberately absent.
const READ_ONLY_PROGRAMS: &[&str] = &[
    "grep", "rg", "cat", "head", "tail", "wc", "ls", "jq", "nl", "cut", "tr",
];

fn read_only_program(command: &[String]) -> bool {
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

/// The part of a flag that could be a path: `--file=/x` -> `/x`, `-f/x` -> `/x`, `-rn` -> ``.
fn flag_value(flag: &str) -> &str {
    match flag.split_once('=') {
        Some((_, value)) => value,
        None => flag
            .trim_start_matches('-')
            .trim_start_matches(|c: char| c.is_ascii_alphanumeric() || c == '-'),
    }
}

/// Resolves `candidate` against the project root and refuses anything outside it: `~`, `..`
/// components, absolute paths elsewhere, and existing paths whose canonical form (symlinks
/// followed) leaves the project.
fn confined(project: &std::path::Path, candidate: &str) -> bool {
    use std::path::Component;
    if candidate.starts_with('~') {
        return false;
    }
    let path = std::path::Path::new(candidate);
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return false;
    }
    let full = if path.is_absolute() {
        path.to_owned()
    } else {
        project.join(path)
    };
    // Canonicalize the longest existing ancestor so a symlinked directory cannot hide an escape.
    let mut existing = full.as_path();
    while !existing.exists() {
        match existing.parent() {
            Some(parent) => existing = parent,
            None => return false,
        }
    }
    existing
        .canonicalize()
        .is_ok_and(|real| real.starts_with(project))
}

/// First argument of a read-only shell command that is not confined to the project, if any.
fn escaping_argument<'a>(project: &std::path::Path, command: &'a [String]) -> Option<&'a str> {
    command.iter().skip(1).find_map(|arg| {
        let candidate = if arg.starts_with('-') {
            let value = flag_value(arg);
            let pathlike = value.contains('/') || value.starts_with('~') || value.starts_with("..");
            if !pathlike {
                return None;
            }
            value
        } else {
            arg.as_str()
        };
        (!confined(project, candidate)).then_some(arg.as_str())
    })
}

impl crate::tools::ApprovalHook for ReadPolicy {
    fn approve(&self, action: &crate::tools::ToolAction) -> crate::tools::Approval {
        use crate::tools::{Approval, RiskLevel};
        let permitted = match action.risk {
            RiskLevel::Read => true,
            RiskLevel::Execute => {
                action.tool == "shell"
                    && read_only_program(&action.command)
                    && match escaping_argument(&self.project, &action.command) {
                        None => true,
                        Some(argument) => {
                            if let Ok(mut refused) = self.refused_paths.lock() {
                                if refused.len() < 8 {
                                    refused.push(argument.chars().take(120).collect());
                                }
                            }
                            false
                        }
                    }
            }
            // `git` is deliberately refused: `--output=` writes anywhere and `--no-index` reads anywhere.
            _ => false,
        };
        let permitted = permitted && action.tool != "git";
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
#[allow(clippy::too_many_arguments, unexpected_cfgs)]
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
        project: project.canonicalize()?,
        refused_paths: Default::default(),
        used: Default::default(),
        denied: Default::default(),
        max_calls,
    });
    #[allow(unused_mut)]
    let mut tools = crate::tools::boxed_tools_for_chat(project, hook.clone(), false, None, None)?;
    // Builds that register the `recall` tool for chats (cfg set by the Python harness when
    // `tools/recall.rs` exists in the tree) give the eval agent the same read-only tool, scoped
    // to the same evidence directory as the file tools.
    #[cfg(eval_recall)]
    tools.push(Box::new(crate::tools::recall::RecallTool::new(
        crate::tools::recall::RecallScope {
            evidence_dir: evidence.to_owned(),
            attachments: Vec::new(),
            attachment_root: None,
            project_root: project.canonicalize().ok(),
        },
    )));
    let tools_available = tools.iter().map(|tool| tool.name().to_owned()).collect();
    let (events, mut sink) = tokio::sync::mpsc::channel(1024);
    let collector = tokio::spawn(async move {
        let (mut tools, mut outputs, mut finished, mut error, mut metrics) =
            (Vec::new(), Vec::new(), None, None, serde_json::Value::Null);
        while let Some(event) = sink.recv().await {
            match event {
                RunEvent::ToolCallStarted { tool, .. } => tools.push(tool),
                RunEvent::ToolCallFinished { tool, ok, output } => outputs.push(ToolOutput {
                    tool,
                    ok,
                    output: super::take_char_prefix(&output, TOOL_OUTPUT_KEEP).0,
                }),
                RunEvent::ContextUsage { metrics: latest } => metrics = latest,
                RunEvent::Finished { result } => finished = Some(result),
                RunEvent::Incomplete { result } => error = Some(format!("incomplete: {result}")),
                RunEvent::Failed { error: failure } => error = Some(failure),
                _ => {}
            }
        }
        (tools, outputs, finished, error, metrics)
    });
    let task = format!("Memory check. Earlier material in this conversation was compacted; the saved originals are under {} (project-relative). Answer the question below. The checkpoint is only a lossy summary: when it does not state the exact detail, search and read the saved originals with your tools (at most {max_calls} tool calls) instead of guessing. Reply as you would to the user: the answer in one short sentence, or exactly UNKNOWN if the original material never says. For a quote question copy the wording exactly. Do not mention files, paths, record numbers or how you searched.\n\nQuestion: {question}",
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
    let (tools, tool_outputs, finished, event_error, metrics) = collector.await?;
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
        tool_outputs,
        denied: hook.denied.load(Ordering::SeqCst),
        refused_paths: hook
            .refused_paths
            .lock()
            .map(|paths| paths.clone())
            .unwrap_or_default(),
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
            display: None,
            tool: tool.into(),
            summary: String::new(),
            risk,
            command: command.iter().map(|part| (*part).to_owned()).collect(),
        }
    }

    fn policy(project: &std::path::Path, max_calls: usize) -> ReadPolicy {
        ReadPolicy {
            project: project.canonicalize().unwrap(),
            refused_paths: Default::default(),
            used: Default::default(),
            denied: Default::default(),
            max_calls,
        }
    }

    #[test]
    fn only_non_writing_programs_run_and_calls_are_capped() {
        let scratch = tempfile::tempdir().unwrap();
        let hook = policy(scratch.path(), 2);
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

    #[test]
    fn git_is_refused_even_for_read_subcommands() {
        let scratch = tempfile::tempdir().unwrap();
        let hook = policy(scratch.path(), 8);
        for args in [
            &["log", "--output=/tmp/x"][..],
            &["show", "--output=x"],
            &["diff", "--no-index", "/etc/passwd", "x"],
            &["status"],
        ] {
            let mut tool_action = action("git", RiskLevel::Read, &[]);
            tool_action.summary = format!("run git {}", args.join(" "));
            assert_eq!(hook.approve(&tool_action), Approval::Deny, "{args:?}");
        }
    }

    #[test]
    fn shell_arguments_cannot_leave_the_scratch_project() {
        let scratch = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "answer key").unwrap();
        std::fs::create_dir_all(scratch.path().join(".themis/context/t")).unwrap();
        std::fs::write(scratch.path().join(".themis/context/t/s.jsonl"), "{}").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), scratch.path().join("link")).unwrap();
        let hook = policy(scratch.path(), 100);
        let shell = |command: &[&str]| hook.approve(&action("shell", RiskLevel::Execute, command));
        let secret = outside.path().join("secret.txt");
        let secret = secret.to_str().unwrap();
        for refused in [
            &["cat", secret][..],
            &["grep", "-r", "key", outside.path().to_str().unwrap()],
            &["rg", "--files", "/"],
            &["cat", "../secret.txt"],
            &["cat", ".themis/../../x"],
            &["cat", "~/.zsh_history"],
            &["grep", "-f", secret, "x"],
            &["grep", "--file=/etc/passwd", "x"],
            &["grep", "-f/etc/passwd", "x"],
            &["head", "--lines=3", "/etc/hosts"],
            #[cfg(unix)]
            &["cat", "link/secret.txt"],
            #[cfg(unix)]
            &["grep", "-r", "key", "link"],
        ] {
            assert_eq!(shell(refused), Approval::Deny, "{refused:?}");
        }
        assert!(!hook.refused_paths.lock().unwrap().is_empty());
        for allowed in [
            &["grep", "-rn", "merge", ".themis/context/t"][..],
            &["cat", ".themis/context/t/s.jsonl"],
            &["rg", "-n", "--max-count=3", "needle", "."],
            &[
                "wc",
                "-l",
                scratch
                    .path()
                    .join(".themis/context/t/s.jsonl")
                    .to_str()
                    .unwrap(),
            ],
            &["grep", "-c", "-A", "3", "pattern/with/slash", ".themis"],
        ] {
            assert_eq!(shell(allowed), Approval::AllowOnce, "{allowed:?}");
        }
    }
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
