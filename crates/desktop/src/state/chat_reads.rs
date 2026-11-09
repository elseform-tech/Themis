//! A chat's read-only lookups as app-server methods: `recall` (the chat's saved originals) and
//! `search_memory` (what Themis remembers in the chat's project), the same work, limits and
//! results as the built-in tools of the same names. `themis recall` and `themis memory` call
//! them; an agent's credential reaches them only for its own chat and only when its run has the
//! tool (see `crate::server::agent`).

use std::path::Path;

use serde_json::{json, Value};
use themis_core::agent_cli::RunScope;
use themis_core::configuration::{ApprovalPolicy, PolicyAction};
use themis_core::tools::recall::{self, RecallArgs, RecallScope};
use themis_core::tools::{RiskLevel, ToolAction, ToolT};

use super::AppState;

/// The run's `recall` tool over `scope`, unless `policy` denies every call to it (a tool every
/// call to which would be denied is not advertised; the prompt then falls back to the read_file
/// pointer). YOLO runs keep it whatever the rules say.
pub(super) fn recall_tool(
    yolo: bool,
    policy: &ApprovalPolicy,
    scope: RecallScope,
) -> Option<Box<dyn ToolT>> {
    let probe = ToolAction {
        display: None,
        command: Vec::new(),
        tool: "recall".into(),
        summary: String::new(),
        risk: RiskLevel::Read,
    };
    let denied = !yolo && policy.decision(&probe) == PolicyAction::Deny;
    (!denied).then(|| Box::new(recall::RecallTool::new(scope)) as Box<dyn ToolT>)
}

/// The scope a run's `themis` credential carries: the run's project and network block, and the
/// read tools of `tools` (the run's, after skill grants) it may reach through `themis`
/// (`search_memory` only when no rule denies it, as the tool itself refuses then).
pub(super) fn run_scope(
    work_root: &Path,
    sandbox: &themis_core::sandbox::SandboxPlan,
    tools: &[Box<dyn ToolT>],
    memory_denied: bool,
) -> RunScope {
    let reads = tools
        .iter()
        .map(|tool| tool.name())
        .filter(|name| !(memory_denied && *name == "search_memory"));
    RunScope::new(work_root, Some(sandbox)).with_reads(reads)
}

impl AppState {
    /// The `recall` or `search_memory` method: `args` name the chat (`threadId`) and carry the
    /// tool's arguments.
    pub(crate) async fn chat_read(&self, method: &str, args: Value) -> Result<Value, String> {
        let thread_id = args["threadId"]
            .as_str()
            .ok_or("missing threadId")?
            .to_owned();
        if method == "recall" {
            self.recall(&thread_id, args).await
        } else {
            self.search_memory(&thread_id, &args).await
        }
    }

    /// What `recall` may read for `thread_id` working in `work_root`: the chat's evidence
    /// directory (made when missing), the attachment originals it referenced and the project's
    /// attachment folder they must live in.
    pub(super) fn recall_scope(
        &self,
        work_root: &Path,
        thread_id: &str,
    ) -> Result<RecallScope, String> {
        let evidence_dir =
            super::attachments::project_storage_directory(work_root, &["context", thread_id])?;
        let attachments = self
            .inner
            .transcript
            .attachment_paths(thread_id)?
            .into_iter()
            .map(std::path::PathBuf::from)
            .collect();
        Ok(RecallScope {
            evidence_dir,
            attachments,
            attachment_root: Some(work_root.join(".themis/attachments")),
            project_root: work_root.canonicalize().ok(),
        })
    }

    /// The chat's working folder (its worktree when it has one).
    async fn chat_root(&self, thread_id: &str) -> Result<std::path::PathBuf, String> {
        self.thread_project_root(thread_id)
            .await
            .map(std::path::PathBuf::from)
            .ok_or_else(|| format!("unknown thread '{thread_id}'"))
    }

    /// `recall` over `thread_id`'s saved originals: `args` are the tool's (`query`, `limit`,
    /// `kinds`, `source`, `around`), and so are the limits and the bounded result.
    pub async fn recall(&self, thread_id: &str, args: Value) -> Result<Value, String> {
        let args: RecallArgs =
            serde_json::from_value(args).map_err(|error| format!("recall: {error}"))?;
        let root = self.chat_root(thread_id).await?;
        let scope = self.recall_scope(&root, thread_id)?;
        let by = if args.around.trim().is_empty() {
            "query"
        } else {
            "around"
        };
        let result = tokio::task::spawn_blocking(move || {
            recall::recall(&scope, &args, &recall::Limits::default())
        })
        .await
        .map_err(|error| format!("recall: {error}"))?
        .map_err(|error| format!("recall: {error}"));
        let _ = self.inner.diagnostics.append(
            "recall",
            "searched",
            "info",
            json!({"thread_id": thread_id, "by": by, "via": "cli", "ok": result.is_ok()}),
        );
        result
    }

    /// `search_memory` in `thread_id`'s project: `query` finds index lines across this
    /// project's and the global memories, plan records and tasks; `name` reads one in full.
    pub async fn search_memory(&self, thread_id: &str, args: &Value) -> Result<Value, String> {
        let root = self.chat_root(thread_id).await?;
        super::memory_tool::lookup(self, &root, thread_id, args, "cli")
    }
}
