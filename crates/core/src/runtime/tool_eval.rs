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
    /// Conversation turn (0 = the first prompt) in which the checkpoint was installed.
    pub turn: usize,
    /// `notes` calls made before this checkpoint (all turns).
    pub notes_calls_before: usize,
    /// The notes lines the checkpoint carries, if any.
    pub notes: Option<serde_json::Value>,
    /// Size of the durable checkpoint summary.
    pub summary_bytes: usize,
}

/// One turn of a multi-turn task.
#[derive(Debug, Clone, Serialize)]
pub struct TurnRecord {
    pub prompt: String,
    pub answer: String,
    pub error: Option<String>,
    /// Tool calls made in this turn.
    pub calls: usize,
    /// Loop warnings in this turn (three unchanged results in a row each).
    pub loop_warnings: usize,
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
    /// Each checkpoint with the notes it carries.
    pub checkpoints: Vec<CheckpointRecord>,
    /// The saved notes after every `notes` call in order, or `null` when it was rejected.
    pub notes_log: Vec<serde_json::Value>,
    /// The notes file at the end of the task.
    pub final_notes: Option<serde_json::Value>,
    /// Every turn: the prompt, then each follow-up.
    pub turns: Vec<TurnRecord>,
    /// Loop warnings over all turns.
    pub loop_warnings: usize,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub latency_ms: u128,
}

/// How one task runs.
#[derive(Debug, Clone)]
pub struct ToolTaskOptions {
    /// A summary standing in for compacted history.
    pub checkpoint: Option<String>,
    /// Saved-originals directory: offers `recall`, the evidence pointer and `notes`.
    pub evidence: Option<std::path::PathBuf>,
    pub max_turns: usize,
    /// Context budget in tokens; small values force compaction mid-task.
    pub budget: usize,
    pub recent_messages: usize,
    /// Ablation: `false` withholds `notes` and its instruction.
    pub offer_notes: bool,
    /// Later user messages in the same chat, each run after the previous answer with the
    /// chat history (latest checkpoint plus the text turns) and the same notes file.
    pub followups: Vec<String>,
    /// Copies the project to `<project>.turn-<n>` after each turn (without `.themis`, `.git`
    /// and caches) so a grader can check every stage, not only the end state.
    pub snapshots: bool,
}

impl Default for ToolTaskOptions {
    fn default() -> Self {
        Self {
            checkpoint: None,
            evidence: None,
            max_turns: 12,
            budget: 64_000,
            recent_messages: 20,
            offer_notes: true,
            followups: Vec::new(),
            snapshots: false,
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

/// Runs `task` (then each follow-up) in `project` with the chat toolset (background tasks
/// included). With `evidence`, the run gets the saved-originals pointer, `recall` and `notes`
/// for that directory, after `checkpoint` (a summary standing in for compacted history).
pub async fn run_tool_task(
    llm: &Arc<dyn LLMProvider>,
    project: &std::path::Path,
    task: &str,
    options: ToolTaskOptions,
) -> anyhow::Result<ToolTaskReport> {
    let mut history = options
        .checkpoint
        .as_ref()
        .map(|text| {
            vec![ConversationTurn {
                role: ConversationRole::Checkpoint,
                text: text.clone(),
            }]
        })
        .unwrap_or_default();
    let prompts: Vec<String> = std::iter::once(task.to_owned())
        .chain(options.followups.iter().cloned())
        .collect();
    let mut report = ToolTaskReport {
        answer: String::new(),
        finished: true,
        error: None,
        calls: Vec::new(),
        tools_available: Vec::new(),
        requests: 0,
        compactions: 0,
        checkpoints: Vec::new(),
        notes_log: Vec::new(),
        final_notes: None,
        turns: Vec::new(),
        loop_warnings: 0,
        input_tokens: None,
        output_tokens: None,
        latency_ms: 0,
    };
    let started = std::time::Instant::now();
    for (turn, prompt) in prompts.iter().enumerate() {
        let run = run_turn(llm, project, prompt, history.clone(), &options).await?;
        let notes_before = report.notes_log.len();
        report
            .checkpoints
            .extend(
                run.checkpoints
                    .iter()
                    .map(|(calls_before, summary)| CheckpointRecord {
                        turn,
                        notes_calls_before: notes_before + calls_before,
                        notes: checkpoint_notes(summary),
                        summary_bytes: summary.len(),
                    }),
            );
        // The chat history the desktop sends next: the latest checkpoint, then text turns.
        if let Some((_, summary)) = run.checkpoints.last() {
            history = vec![ConversationTurn {
                role: ConversationRole::Checkpoint,
                text: summary.clone(),
            }];
        }
        history.push(ConversationTurn {
            role: ConversationRole::User,
            text: prompt.clone(),
        });
        history.push(ConversationTurn {
            role: ConversationRole::Assistant,
            text: run.answer.clone(),
        });
        report.turns.push(TurnRecord {
            prompt: prompt.clone(),
            answer: run.answer.clone(),
            error: run.error.clone(),
            calls: run.calls.len(),
            loop_warnings: run.loop_warnings,
        });
        report.loop_warnings += run.loop_warnings;
        report.finished &= run.error.is_none() && !run.answer.trim().is_empty();
        report.answer = run.answer.trim().to_owned();
        report.error = report.error.take().or(run.error);
        report.calls.extend(run.calls);
        report.notes_log.extend(run.notes_log);
        report.tools_available = run.tools_available;
        if options.snapshots {
            let mut target = project.as_os_str().to_owned();
            target.push(format!(".turn-{}", turn + 1));
            snapshot(project, std::path::Path::new(&target))?;
        }
        report.requests += run.metrics["total_requests"].as_u64().unwrap_or(0);
        for (total, key) in [
            (&mut report.input_tokens, "provider_input_tokens"),
            (&mut report.output_tokens, "provider_output_tokens"),
        ] {
            if let Some(value) = run.metrics[key].as_u64() {
                *total = Some(total.unwrap_or(0) + value);
            }
        }
    }
    report.compactions = report.checkpoints.len();
    report.latency_ms = started.elapsed().as_millis();
    report.final_notes = options
        .evidence
        .as_deref()
        .map(notes::Notes::load)
        .filter(|notes| !notes.lines.is_empty())
        .and_then(|notes| serde_json::to_value(notes.lines).ok());
    Ok(report)
}

struct TurnRun {
    answer: String,
    error: Option<String>,
    calls: Vec<CallRecord>,
    tools_available: Vec<String>,
    metrics: serde_json::Value,
    /// (`notes` calls before it in this turn, durable summary) per checkpoint.
    checkpoints: Vec<(usize, String)>,
    notes_log: Vec<serde_json::Value>,
    loop_warnings: usize,
}

#[allow(unexpected_cfgs)]
async fn run_turn(
    llm: &Arc<dyn LLMProvider>,
    project: &std::path::Path,
    task: &str,
    history: Vec<ConversationTurn>,
    options: &ToolTaskOptions,
) -> anyhow::Result<TurnRun> {
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
                RunEvent::ContextCheckpoint { summary } => checkpoints.push((
                    calls.iter().filter(|c| c.tool == notes::TOOL).count(),
                    summary,
                )),
                RunEvent::Finished { result } => finished = Some(result),
                RunEvent::Incomplete { result } => error = Some(format!("incomplete: {result}")),
                RunEvent::Failed { error: failure } => error = Some(failure),
                _ => {}
            }
        }
        (calls, finished, error, metrics, checkpoints)
    });
    let notes_log = Arc::new(Mutex::new(Vec::new()));
    let loop_warnings = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let result = EVAL_LOOP_WARNINGS
        .scope(
            loop_warnings.clone(),
            EVAL_NOTES_LOG.scope(
                notes_log.clone(),
                EVAL_OFFER_NOTES.scope(
                    options.offer_notes,
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
            ),
        )
        .await;
    // Background tasks the agent left running must not outlive the eval.
    registry.remove_thread("tool-eval", std::time::Duration::from_secs(5));
    let (calls, finished, event_error, metrics, checkpoints) = collector.await?;
    let notes_log = std::mem::take(&mut *notes_log.lock().unwrap());
    let (answer, error) = match (result, finished) {
        // Ending at the turn limit returns a handoff summary: keep it as the answer and
        // record why, so the eval does not count it as a finished task.
        (Ok(text), _) => (
            text,
            event_error.filter(|error| error.starts_with("incomplete:")),
        ),
        (Err(error), Some(text)) => (text, Some(error.to_string())),
        (Err(error), None) => (
            String::new(),
            Some(event_error.unwrap_or_else(|| error.to_string())),
        ),
    };
    Ok(TurnRun {
        answer,
        error,
        calls,
        tools_available,
        metrics,
        checkpoints,
        notes_log,
        loop_warnings: loop_warnings.load(std::sync::atomic::Ordering::SeqCst),
    })
}

/// Copies readable project files for a per-turn grade. Symlinks are skipped, so a snapshot
/// never follows a link out of the scratch project.
fn snapshot(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if [".themis", ".git", "__pycache__", "node_modules", "target"]
            .iter()
            .any(|skip| name == *skip)
        {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            snapshot(&entry.path(), &to.join(&name))?;
        } else if kind.is_file() && entry.metadata()?.len() <= 2 << 20 {
            std::fs::copy(entry.path(), to.join(&name))?;
        }
    }
    Ok(())
}

/// The notes lines a checkpoint summary carries (its last saved-notes block), parsed back
/// from the rendered `- key [status]: text` lines.
fn checkpoint_notes(summary: &str) -> Option<serde_json::Value> {
    let start = summary.rfind(notes::START)? + notes::START.len();
    let block = &summary[start..];
    let block = &block[..block.find(notes::END).unwrap_or(block.len())];
    let lines: Vec<serde_json::Value> = block
        .lines()
        .filter_map(|line| {
            let (label, text) = line.strip_prefix("- ")?.split_once(": ")?;
            let (key, status) = match label.rsplit_once(" [") {
                Some((key, status)) => (key, status.strip_suffix(']')),
                None => (label, None),
            };
            Some(serde_json::json!({"key":key,"text":text,"status":status}))
        })
        .collect();
    Some(serde_json::Value::Array(lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_copies_project_files_but_not_agent_state_or_links() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir_all(project.join("src/pkg")).unwrap();
        std::fs::create_dir_all(project.join(".themis/context")).unwrap();
        std::fs::create_dir_all(project.join("src/__pycache__")).unwrap();
        std::fs::write(project.join("src/pkg/a.py"), "x = 1\n").unwrap();
        std::fs::write(project.join(".themis/context/notes.json"), "{}").unwrap();
        std::fs::write(project.join("src/__pycache__/a.pyc"), "c").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.path(), project.join("escape")).unwrap();
        let target = root.path().join("project.turn-1");
        snapshot(&project, &target).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("src/pkg/a.py")).unwrap(),
            "x = 1\n"
        );
        assert!(!target.join(".themis").exists());
        assert!(!target.join("src/__pycache__").exists());
        assert!(!target.join("escape").exists());
    }

    #[test]
    fn checkpoint_notes_reads_the_rendered_lines() {
        let notes = notes::Notes::default()
            .apply(&serde_json::json!({"set":[
                {"key":"goal","text":"ship: v2"},
                {"key":"mod-01","text":"4 functions","status":"done"}]}))
            .unwrap();
        let summary = format!("Goal: x{}ledger", notes.render());
        assert_eq!(
            checkpoint_notes(&summary),
            Some(serde_json::json!([
                {"key":"goal","text":"ship: v2","status":null},
                {"key":"mod-01","text":"4 functions","status":"done"}]))
        );
        assert_eq!(checkpoint_notes("no notes here"), None);
    }
}
