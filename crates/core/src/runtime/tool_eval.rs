//! Tool-use evals (cargo feature `eval`; never enabled in releases).
//!
//! Runs one task with the real chat toolset in a scratch project and records every tool
//! call: name, argument summary, approval decision, success and a bounded output. The
//! `evals/tools` harness sets up the project, checks the end state and measures first-try
//! success and wasted calls per tool.

use super::*;

/// Bytes of each tool output kept in the report.
const OUTPUT_KEEP: usize = 4_096;

/// One tool call the agent made during a task.
#[derive(Debug, Clone, Serialize, Default)]
pub struct CallRecord {
    pub tool: String,
    /// One-line summary of the call (its arguments, bounded).
    pub summary: String,
    /// `allow_once`, `deny`, ... when an approval was decided for it.
    pub decision: Option<String>,
    /// Whether the call succeeded (false for denials, bad arguments and failures).
    pub ok: bool,
    /// Bounded tool output or error text.
    pub output: String,
}

/// One context checkpoint installed during a task.
#[derive(Debug, Clone, Serialize)]
pub struct CheckpointRecord {
    /// `update_plan` calls made before this checkpoint.
    pub plan_calls_before: usize,
    /// The plan items the checkpoint's task ledger carries, if any.
    pub plan: Option<serde_json::Value>,
    /// Size of the durable checkpoint summary.
    pub summary_bytes: usize,
}

/// Result of one tool-use task.
#[derive(Debug, Clone, Serialize)]
pub struct ToolTaskReport {
    pub answer: String,
    pub finished: bool,
    pub error: Option<String>,
    pub calls: Vec<CallRecord>,
    pub tools_available: Vec<String>,
    pub requests: u64,
    /// Context checkpoints installed during the task.
    pub compactions: usize,
    /// Each checkpoint with the plan its ledger carries.
    pub checkpoints: Vec<CheckpointRecord>,
    /// Every `update_plan` call in order: its validated items, or `null` when rejected.
    pub plans: Vec<serde_json::Value>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub latency_ms: u128,
}

/// How one task runs.
#[derive(Debug, Clone)]
pub struct ToolTaskOptions {
    /// A summary standing in for compacted history.
    pub checkpoint: Option<String>,
    /// Saved-originals directory: offers `recall`, the evidence pointer and `update_plan`.
    pub evidence: Option<std::path::PathBuf>,
    pub max_turns: usize,
    /// Context budget in tokens; small values force compaction mid-task.
    pub budget: usize,
    pub recent_messages: usize,
    /// Ablation: `false` withholds `update_plan` and its instruction.
    pub offer_plan: bool,
}

impl Default for ToolTaskOptions {
    fn default() -> Self {
        Self {
            checkpoint: None,
            evidence: None,
            max_turns: 12,
            budget: 64_000,
            recent_messages: 20,
            offer_plan: true,
        }
    }
}

/// Custom mode with a person who allows every non-destructive action once: destructive
/// requests are refused, so a task can also check that the agent accepts "no".
struct AllowNonDestructive;

impl ApprovalHook for AllowNonDestructive {
    fn approve(&self, action: &ToolAction) -> Approval {
        if action.risk == crate::tools::RiskLevel::Destructive {
            Approval::Deny
        } else {
            Approval::AllowOnce
        }
    }
}

/// Runs `task` in `project` with the chat toolset (background tasks included). With
/// `evidence`, the run gets the saved-originals pointer and the `recall` tool for that
/// directory, after `checkpoint` (a summary standing in for compacted history).
#[allow(unexpected_cfgs)]
pub async fn run_tool_task(
    llm: &Arc<dyn LLMProvider>,
    project: &std::path::Path,
    task: &str,
    options: ToolTaskOptions,
) -> anyhow::Result<ToolTaskReport> {
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowNonDestructive);
    let registry = crate::tools::background::BackgroundRegistry::in_memory();
    #[allow(unused_mut)]
    let mut tools = crate::tools::boxed_tools_for_chat(
        project,
        approvals.clone(),
        false,
        Some(crate::tools::background::BackgroundScope::new(
            registry.clone(),
            "tool-eval",
        )),
        None,
    )?;
    #[cfg(eval_recall)]
    if let Some(evidence) = options.evidence.as_deref() {
        tools.push(Box::new(crate::tools::recall::RecallTool::new(
            crate::tools::recall::RecallScope {
                evidence_dir: evidence.to_owned(),
                attachments: Vec::new(),
                attachment_root: None,
                project_root: project.canonicalize().ok(),
            },
        )));
    }
    let tools_available = tools.iter().map(|tool| tool.name().to_owned()).collect();
    let history = options
        .checkpoint
        .as_ref()
        .map(|text| {
            vec![ConversationTurn {
                role: ConversationRole::Checkpoint,
                text: text.clone(),
            }]
        })
        .unwrap_or_default();
    let (events, mut sink) = tokio::sync::mpsc::channel(1024);
    let collector = tokio::spawn(async move {
        let (mut calls, mut finished, mut error, mut metrics, mut checkpoints) = (
            Vec::<CallRecord>::new(),
            None,
            None,
            serde_json::Value::Null,
            Vec::new(),
        );
        while let Some(event) = sink.recv().await {
            match event {
                RunEvent::ToolCallStarted { tool, summary } => calls.push(CallRecord {
                    tool,
                    summary: take_char_prefix(&summary, 600).0,
                    ..CallRecord::default()
                }),
                RunEvent::ApprovalDecided { decision, .. } => {
                    if let Some(call) = calls.last_mut() {
                        call.decision = serde_json::to_value(decision)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_owned));
                    }
                }
                RunEvent::ToolCallFinished { tool, ok, output } => {
                    let output = take_char_prefix(&output, OUTPUT_KEEP).0;
                    match calls.iter_mut().rev().find(|call| call.tool == tool) {
                        Some(call) if call.output.is_empty() && !call.ok => {
                            call.ok = ok;
                            call.output = output;
                        }
                        _ => calls.push(CallRecord {
                            tool,
                            ok,
                            output,
                            ..CallRecord::default()
                        }),
                    }
                }
                RunEvent::ContextUsage { metrics: latest } => metrics = latest,
                RunEvent::ContextCheckpoint { summary } => checkpoints.push(CheckpointRecord {
                    plan_calls_before: calls.iter().filter(|c| c.tool == ledger::PLAN_TOOL).count(),
                    plan: ledger_plan(&summary),
                    summary_bytes: summary.len(),
                }),
                RunEvent::Finished { result } => finished = Some(result),
                RunEvent::Incomplete { result } => error = Some(format!("incomplete: {result}")),
                RunEvent::Failed { error: failure } => error = Some(failure),
                _ => {}
            }
        }
        (calls, finished, error, metrics, checkpoints)
    });
    let plan_log = Arc::new(Mutex::new(Vec::new()));
    let started = std::time::Instant::now();
    let result = EVAL_PLAN_LOG
        .scope(
            plan_log.clone(),
            EVAL_OFFER_PLAN.scope(
                options.offer_plan,
                run_task_with_evidence(
                    llm.clone(),
                    tools,
                    task.to_owned(),
                    history,
                    approvals,
                    RunPolicy {
                        total_turns: options.max_turns,
                        context_token_budget: options.budget,
                        recent_messages: options.recent_messages,
                    },
                    events,
                    Arc::new(AtomicBool::new(false)),
                    String::new(),
                    options.evidence.clone(),
                ),
            ),
        )
        .await;
    let latency_ms = started.elapsed().as_millis();
    // Background tasks the agent left running must not outlive the eval.
    registry.remove_thread("tool-eval", std::time::Duration::from_secs(5));
    let (calls, finished, event_error, metrics, checkpoints) = collector.await?;
    let plans = std::mem::take(&mut *plan_log.lock().unwrap());
    let (answer, error) = match (result, finished) {
        (Ok(text), _) => (text, None),
        (Err(error), Some(text)) => (text, Some(error.to_string())),
        (Err(error), None) => (
            String::new(),
            Some(event_error.unwrap_or_else(|| error.to_string())),
        ),
    };
    Ok(ToolTaskReport {
        finished: error.is_none() && !answer.trim().is_empty(),
        answer: answer.trim().to_owned(),
        error,
        calls,
        tools_available,
        requests: metrics["total_requests"].as_u64().unwrap_or(0),
        compactions: checkpoints.len(),
        checkpoints,
        plans,
        input_tokens: metrics["provider_input_tokens"].as_u64(),
        output_tokens: metrics["provider_output_tokens"].as_u64(),
        latency_ms,
    })
}

/// The plan line of the task ledger inside a checkpoint summary (the last ledger block).
fn ledger_plan(summary: &str) -> Option<serde_json::Value> {
    let start = summary.rfind(ledger::START)? + ledger::START.len();
    let block = &summary[start..];
    let block = &block[..block.find(ledger::END).unwrap_or(block.len())];
    block
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| value["kind"] == "plan")
        .map(|value| value["items"].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_plan_reads_the_plan_line() {
        let summary = format!(
            "Goal: x{}{}\n{}{}",
            ledger::START,
            r#"{"kind":"plan","items":[{"step":"a","status":"done"}]}"#,
            r#"{"kind":"action"}"#,
            ledger::END
        );
        assert_eq!(
            ledger_plan(&summary),
            Some(serde_json::json!([{"step":"a","status":"done"}]))
        );
        assert_eq!(ledger_plan("no ledger here"), None);
    }
}
