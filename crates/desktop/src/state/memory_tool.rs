//! `search_memory`: the agent's read-only way into what Themis remembers, beside the index and
//! the few memories every run gets in full. A query finds index lines across this project's and
//! the global memories and the project's plan records; a name reads one of them in full. It
//! never writes, reads no other project's memory, and is offered only while memory is on. The
//! app server's `search_memory` method (`themis memory search|show`) does the same work through
//! [`lookup`].

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use themis_core::configuration::{ApprovalMode, PolicyAction};
use themis_core::memory::MemoryScope;
use themis_core::tools::{async_trait, RiskLevel, ToolAction, ToolCallError, ToolRuntime, ToolT};

use super::integration_tools::RunApproval;
use super::AppState;

pub(super) struct SearchMemoryTool {
    pub(super) state: AppState,
    pub(super) project_root: PathBuf,
    pub(super) thread_id: String,
    pub(super) run: RunApproval,
}

impl std::fmt::Debug for SearchMemoryTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchMemoryTool").finish_non_exhaustive()
    }
}

fn error(message: impl ToString) -> ToolCallError {
    ToolCallError::RuntimeError(message.to_string().into())
}

const NAME: &str = "search_memory";

/// Whether the run's approval rules deny `search_memory` (in YOLO only a written deny rule).
/// Otherwise it approves itself: it only reads.
pub(super) fn denied_by(run: &RunApproval) -> bool {
    let action = ToolAction {
        display: None,
        command: Vec::new(),
        tool: NAME.to_owned(),
        summary: NAME.to_owned(),
        risk: RiskLevel::Read,
    };
    let decision = if run.mode == ApprovalMode::Yolo {
        run.policy.yolo_decision(&action)
    } else {
        run.policy.decision(&action)
    };
    decision == PolicyAction::Deny
}

impl SearchMemoryTool {
    /// Refused when an approval rule denies it.
    fn denied(&self) -> Option<ToolCallError> {
        denied_by(&self.run)
            .then(|| error("approval denied for 'search_memory': the approval rules deny it"))
    }
}

/// What `search_memory` answers for `args` (`query` or `name`) in `project_root`: the work both
/// the tool and the app server's `search_memory` method do. `via` (`tool` or `cli`) goes in the
/// diagnostics event with the chat id, never the words.
pub(super) fn lookup(
    state: &AppState,
    project_root: &Path,
    thread_id: &str,
    args: &Value,
    via: &str,
) -> Result<Value, String> {
    let name = args["name"].as_str().map(str::trim).unwrap_or_default();
    let query = args["query"].as_str().map(str::trim).unwrap_or_default();
    let result = match (name.is_empty(), query.is_empty()) {
        (false, _) => read(state, project_root, name),
        (true, false) => find(state, project_root, query),
        (true, true) => Err("pass a query or a name".to_owned()),
    };
    let _ = state.inner.diagnostics.append(
        "memory",
        "searched",
        "info",
        json!({"thread_id": thread_id, "by": if name.is_empty() { "query" } else { "name" },
            "via": via, "ok": result.is_ok()}),
    );
    result
}

/// The index lines that share words with `query`, across memories and plan records.
fn find(state: &AppState, project_root: &Path, query: &str) -> Result<Value, String> {
    let project = state
        .memory_store(MemoryScope::Project, Some(project_root))?
        .index();
    let global = state.memory_store(MemoryScope::Global, None)?.index();
    let plans = state
        .plan_store(project_root)
        .map(|store| store.index())
        .unwrap_or_default();
    let found = themis_core::memory::search(
        query,
        &[
            ("project memory", &project),
            ("global memory", &global),
            ("plan record", &plans),
        ],
    );
    let mut matches: Vec<Value> = found
        .into_iter()
        .map(|(source, line)| json!({"source": source, "line": line}))
        .collect();
    // And the project's tasks outside any record (a record's tasks come with it by name).
    if let Ok(store) = state.plan_store(project_root) {
        let tasks = themis_core::memory::tasks::search(query, &store.tasks().read());
        // Tasks always get a few of the places, within the same total.
        let reserved = tasks.len().min(3);
        matches.truncate(themis_core::memory::MAX_SEARCH_RESULTS - reserved);
        let room = themis_core::memory::MAX_SEARCH_RESULTS - matches.len();
        matches.extend(
            tasks
                .into_iter()
                .take(room)
                .map(|line| json!({"source": "task", "line": line})),
        );
    }
    Ok(json!({"matches": matches}))
}

/// One memory or plan record in full, this project's before the global ones; `tasks` reads
/// the project's whole task list.
fn read(state: &AppState, project_root: &Path, name: &str) -> Result<Value, String> {
    if name == "tasks" {
        let tasks = state.plan_store(project_root)?.tasks().read();
        return Ok(json!({
            "source": "tasks",
            "text": themis_core::memory::tasks::render_list(&tasks),
        }));
    }
    for scope in [MemoryScope::Project, MemoryScope::Global] {
        let root = (scope == MemoryScope::Project).then_some(project_root);
        let store = state.memory_store(scope, root)?;
        if let Some(memory) = store.read(name).map_err(|e| e.to_string())? {
            return Ok(json!({
                "source": format!("{} memory", scope.as_str()),
                "text": themis_core::memory::serialize(&memory),
            }));
        }
    }
    if let Ok(store) = state.plan_store(project_root) {
        if let Some(record) = store.read(name).map_err(|e| e.to_string())? {
            return Ok(json!({
                "source": "plan record",
                "text": themis_core::memory::plans::serialize(&record),
            }));
        }
    }
    Err(format!(
        "nothing named '{name}' is remembered; search with a query to find names"
    ))
}

#[async_trait]
impl ToolRuntime for SearchMemoryTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        if let Some(refused) = self.denied() {
            return Err(refused);
        }
        lookup(
            &self.state,
            &self.project_root,
            &self.thread_id,
            &args,
            "tool",
        )
        .map_err(error)
    }
}

impl ToolT for SearchMemoryTool {
    fn name(&self) -> &str {
        "search_memory"
    }
    fn description(&self) -> &str {
        themis_core::prompts::SEARCH_MEMORY_TOOL
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{
            "query":{"type":"string","description":"Words about what you need, to find memories and plan records"},
            "name":{"type":"string","description":"A memory or plan record name, to read it in full"}
        },"additionalProperties":false})
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
}
