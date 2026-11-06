use super::*;

// ---------------------------------------------------------------------------
// Apply-patch tool
// ---------------------------------------------------------------------------

/// Arguments for [`ApplyPatchTool`].
#[derive(Debug, Clone, Serialize, Deserialize, ToolInput)]
pub struct ApplyPatchArgs {
    /// Unified diff to apply; `a/`/`b/` prefixes accepted.
    #[input(
        description = "Exact unified diff with ---/+++ paths and numbered @@ ranges. Example: --- a/file.txt\n+++ b/file.txt\n@@ -1,2 +1,2 @@\n old context\n-old value\n+new value\n. Count context+removed lines on the old side and context+added lines on the new side. Every body line needs a space, - or + prefix, including blank lines. Do not use *** Begin Patch / *** End Patch. For generated or large edits, use approved Python/bash and print selected verification facts."
    )]
    pub patch: String,
}

/// Applies a unified diff to files inside the project root.
///
/// Pure-Rust applier (no `patch` binary needed). Every hunk target is
/// resolved with [`resolve_within_root`] and out-of-root hunks abort the
/// whole patch before anything is written. Requires approval
/// ([`RiskLevel::Write`]).
///
/// Limitations: exact-match hunks only (no fuzz); UTF-8 text files;
/// `\r\n` files are rewritten with `\n` endings.
#[tool(
    name = "apply_patch",
    description = "Apply a unified diff to files inside the project root. Requires approval.",
    input = ApplyPatchArgs,
)]
pub struct ApplyPatchTool {
    root: PathBuf,
    approvals: Arc<dyn ApprovalHook>,
    unrestricted: bool,
    guard: Option<Arc<crate::sandbox::FileGuard>>,
}

impl ApplyPatchTool {
    pub fn with_unrestricted_paths(mut self, unrestricted: bool) -> Self {
        self.unrestricted = unrestricted;
        self
    }
    /// Create an apply-patch tool rooted at `root`.
    pub fn new(root: PathBuf, approvals: Arc<dyn ApprovalHook>) -> Self {
        Self {
            root,
            approvals,
            unrestricted: false,
            guard: None,
        }
    }

    /// Refuses patches touching paths the sandbox protects.
    #[must_use]
    pub fn with_guard(mut self, guard: Option<Arc<crate::sandbox::FileGuard>>) -> Self {
        self.guard = guard;
        self
    }

    /// The project root this tool is scoped to.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[async_trait]
impl ToolRuntime for ApplyPatchTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let raw = args.clone();
        let ApplyPatchArgs { patch } = parse_tool_args("apply_patch", args)?;
        // Plan first (validates every path and hunk), approve, then commit.
        let plan = plan_tool_patch(&self.root, &patch, self.unrestricted)
            .map_err(|e| tool_error(format!("apply_patch: {e}. No files were changed. Use --- a/path, +++ b/path and numbered @@ -old_start,old_count +new_start,new_count @@ hunks; prefix every body line. Count context on both sides. Do not use *** Begin Patch markers. For larger edits, generate a unified diff locally with Python difflib or edit with approved Python/bash.")))?;
        let summary = if plan.is_empty() {
            "apply empty patch (no changes)".to_string()
        } else {
            let files: Vec<String> = plan
                .iter()
                .map(|f| format!("{} ({})", f.relative, f.op))
                .collect();
            format!("apply patch to {}: {}", plan.len(), files.join(", "))
        };
        check_call_permitted(
            self.approvals.as_ref(),
            &ToolAction {
                display: None,
                command: Vec::new(),
                tool: "apply_patch".to_string(),
                summary,
                risk: RiskLevel::Write,
            },
            &raw,
        )?;
        if let Some(guard) = &self.guard {
            for file in &plan {
                guard
                    .check_write(&file.path)
                    .map_err(|e| tool_error(format!("apply_patch: {e}. No files were changed.")))?;
            }
        }
        let mut changed = Vec::with_capacity(plan.len());
        for file in &plan {
            commit_planned_file(file).map_err(|e| tool_error(format!("apply_patch: {e}")))?;
            changed.push(file.relative.clone());
        }
        Ok(json!({ "success": true, "files": changed }))
    }
}

/// Apply `patch` to `root` without an approval check.
///
/// Used by [`ApplyPatchTool`] after approval; also handy for tests and for
/// callers that gate approval at dispatch time. Returns the affected
/// root-relative paths. Two-phase: the whole patch is parsed and validated
/// before any file is written.
pub fn apply_unified_diff(root: &Path, patch: &str) -> anyhow::Result<Vec<String>> {
    let plan = plan_patch(root, patch)?;
    let mut changed = Vec::with_capacity(plan.len());
    for file in &plan {
        commit_planned_file(file)?;
        changed.push(file.relative.clone());
    }
    Ok(changed)
}
