//! Coding toolset: root-scoped filesystem, shell, patch, and git tools.
//!
//! Every tool in this module is scoped to a project root: any path that
//! escapes the root — via `..`, an absolute path, or a symlink — is rejected.
//! Every tool call goes through an [`ApprovalHook`] first: the custom
//! `shell`/`apply_patch`/`git` tools gate internally, and the third-party
//! filesystem tools are wrapped in [`GatedTool`] (their own `execute`
//! never consults our hook, so ungated use would bypass approvals).
//!
//! Start here: [`boxed_tools`] builds the full set as
//! `Vec<Box<dyn ToolT>>`, which plugs directly into a ReAct run via
//! `Context::with_tools(...)` without the `#[agent(...)]` macro (see
//! [`boxed_tools`] docs for both wirings).

mod action;
mod apply_patch;
mod approval;
mod argv;
pub mod background;
mod destructive;
mod gated;
mod git;
mod patch;
mod paths;
pub mod read_only;
pub mod recall;
mod runner;
pub mod sensitive;
mod shell;
pub mod shell_scope;
pub mod site_permissions;
pub mod themis_cli;
pub mod web;
mod wrapped;

pub use action::*;
pub use apply_patch::*;
pub use approval::*;
pub use argv::*;
pub use destructive::*;
pub use gated::*;
pub use git::*;
use patch::*;
pub use paths::*;
pub use runner::*;
pub use shell::*;

use crate::sandbox::SandboxPlan;

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use autoagents::async_trait;
use autoagents_derive::{tool, ToolInput};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use autoagents::core::tool::ToolCallError;
pub use autoagents::core::tool::{ToolRuntime, ToolT};
pub use autoagents_toolkit::tools::filesystem::{
    CopyFile, CreateDir, DeleteFile, ListDir, MoveFile, ReadFile, SearchFile, WriteFile,
};

// ---------------------------------------------------------------------------
// Composition entry points
// ---------------------------------------------------------------------------

/// Build the full coding toolset for `root` as boxed SDK tools.
///
/// Returns, in order: `list_dir`, `read_file`, `write_file`, `copy_file`,
/// `move_file`, `delete_file`, `create_dir`, `search_file` (all
/// `autoagents-toolkit` filesystem tools constructed with
/// `new_with_root_dir`), plus the custom `shell`, `apply_patch`, and `git`
/// tools wired to `approvals`.
///
/// Every toolkit tool is wrapped in [`GatedTool`] so its calls consult
/// `approvals` first — ungated, their `execute` would bypass approvals
/// entirely. Gate risks: `list_dir`/`read_file`/`search_file` gate as
/// [`RiskLevel::Read`]; `write_file`/`copy_file`/`move_file`/`create_dir` as
/// [`RiskLevel::Write`]; `delete_file` as [`RiskLevel::Destructive`]
/// (regardless of its `recursive` flag). The custom tools gate internally
/// and are not double-wrapped.
///
/// `root` must exist; it is canonicalized once here so every tool shares
/// one scope.
///
/// # Dynamic wiring (recommended)
///
/// The result plugs directly into a ReAct run — no `#[agent]` macro needed:
///
/// ```ignore
/// use autoagents::core::agent::Context;
/// use themis_core::tools::{AllowAllHook, boxed_tools};
/// use std::sync::Arc;
///
/// let tools = boxed_tools(project_root, Arc::new(AllowAllHook))?;
/// let context = Context::new(llm, None).with_tools(tools);
/// // ... run ReActAgent with `context`
/// ```
///
/// # Macro-style wiring
///
/// `#[agent(tools = [..])]` expands bare paths to struct literals (`Tool {}`),
/// so root/hook configuration must come from an expression evaluated inside
/// the generated `tools(&self)`. An agent struct that carries the root and
/// hook can expose builders, e.g. `tools = [self.make_shell(), ...]`, or —
/// simpler — skip the macro and use `Context::with_tools` with
/// [`boxed_tools`] as above.
pub fn boxed_tools(
    root: &Path,
    approvals: Arc<dyn ApprovalHook>,
) -> anyhow::Result<Vec<Box<dyn ToolT>>> {
    boxed_tools_with_mode(root, approvals, false)
}

/// YOLO removes workspace and git-command restrictions; relative paths retain project semantics.
pub fn boxed_tools_with_mode(
    root: &Path,
    approvals: Arc<dyn ApprovalHook>,
    yolo: bool,
) -> anyhow::Result<Vec<Box<dyn ToolT>>> {
    boxed_tools_for_chat(root, approvals, yolo, None, None)
}

/// [`boxed_tools_with_mode`] for one chat: `background` lets `shell` start
/// background tasks and `background_task` manage them; `cancel` kills a
/// running blocking `shell` command when the user stops the run.
pub fn boxed_tools_for_chat(
    root: &Path,
    approvals: Arc<dyn ApprovalHook>,
    yolo: bool,
    background: Option<background::BackgroundScope>,
    cancel: Option<Arc<AtomicBool>>,
) -> anyhow::Result<Vec<Box<dyn ToolT>>> {
    boxed_tools_for_run(
        root,
        approvals,
        ToolOptions {
            yolo,
            background,
            cancel,
            sandbox: None,
            no_escalation: false,
        },
    )
}

/// Per-run options for [`boxed_tools_for_run`].
#[derive(Default)]
pub struct ToolOptions {
    /// YOLO: no approval prompts, unrestricted paths and git subcommands.
    pub yolo: bool,
    /// Lets `shell` start background tasks owned by this chat.
    pub background: Option<background::BackgroundScope>,
    /// Kills a running blocking `shell` command when set (Stop).
    pub cancel: Option<Arc<AtomicBool>>,
    /// The run's captured sandbox for `shell`, background tasks and `git`, and
    /// its file guard for the in-process file tools. In Custom mode a command
    /// the sandbox blocked asks to run outside it; in YOLO it never asks.
    pub sandbox: Option<Arc<SandboxPlan>>,
    /// Never ask to run a blocked command outside the sandbox (subagents).
    pub no_escalation: bool,
}

/// The coding toolset for one run (see [`boxed_tools`]).
pub fn boxed_tools_for_run(
    root: &Path,
    approvals: Arc<dyn ApprovalHook>,
    options: ToolOptions,
) -> anyhow::Result<Vec<Box<dyn ToolT>>> {
    let ToolOptions {
        yolo,
        background,
        cancel,
        sandbox,
        no_escalation,
    } = options;
    let guard = sandbox
        .as_ref()
        .and_then(|plan| plan.file_guard(yolo))
        .map(Arc::new);
    let canonical = root
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("project root '{}' is not accessible: {e}", root.display()))?;
    let root_string = canonical.to_string_lossy().into_owned();
    let gate = |tool: Box<dyn ToolT>, risk: RiskLevel| -> Box<dyn ToolT> {
        let tool = GatedTool::new(tool, Arc::clone(&approvals), risk)
            .with_base_dir(canonical.clone())
            .with_guard(guard.clone());
        Box::new(if yolo {
            tool.with_relative_root(canonical.clone())
        } else {
            tool
        })
    };
    let mut tools: Vec<Box<dyn ToolT>> = vec![
        gate(
            Box::new(if yolo {
                ListDir::new_unrestricted()
            } else {
                ListDir::new_with_root_dir(root_string.clone())
            }),
            RiskLevel::Read,
        ),
        gate(
            Box::new(if yolo {
                ReadFile::new_unrestricted()
            } else {
                ReadFile::new_with_root_dir(root_string.clone())
            }),
            RiskLevel::Read,
        ),
        gate(
            Box::new(if yolo {
                WriteFile::new_unrestricted()
            } else {
                WriteFile::new_with_root_dir(root_string.clone())
            }),
            RiskLevel::Write,
        ),
        gate(
            Box::new(if yolo {
                CopyFile::new_unrestricted()
            } else {
                CopyFile::new_with_root_dir(root_string.clone())
            }),
            RiskLevel::Write,
        ),
        gate(
            Box::new(if yolo {
                MoveFile::new_unrestricted()
            } else {
                MoveFile::new_with_root_dir(root_string.clone())
            }),
            RiskLevel::Write,
        ),
        gate(
            Box::new(if yolo {
                DeleteFile::new_unrestricted()
            } else {
                DeleteFile::new_with_root_dir(root_string.clone())
            }),
            RiskLevel::Destructive,
        ),
        gate(
            Box::new(if yolo {
                CreateDir::new_unrestricted()
            } else {
                CreateDir::new_with_root_dir(root_string.clone())
            }),
            RiskLevel::Write,
        ),
        gate(
            Box::new(if yolo {
                SearchFile::new_unrestricted(100)
            } else {
                SearchFile::new_with_root_dir(root_string.clone())
            }),
            RiskLevel::Read,
        ),
    ];
    let mut shell =
        ShellTool::new(canonical.clone(), Arc::clone(&approvals)).with_unrestricted_paths(yolo);
    if let Some(scope) = background.clone() {
        shell = shell.with_background(scope);
    }
    if let Some(cancel) = cancel {
        shell = shell.with_cancel(cancel);
    }
    let mut git =
        GitTool::new(canonical.clone(), approvals.clone()).with_unrestricted_commands(yolo);
    if let Some(plan) = sandbox {
        shell = shell.with_sandbox(Arc::clone(&plan), !yolo && !no_escalation);
        git = git.with_sandbox(plan, !yolo && !no_escalation);
    }
    tools.push(Box::new(shell));
    tools.push(Box::new(background::BackgroundTool::new(background)));
    tools.push(Box::new(
        ApplyPatchTool::new(canonical, Arc::clone(&approvals))
            .with_unrestricted_paths(yolo)
            .with_guard(guard),
    ));
    tools.push(Box::new(git));
    Ok(tools)
}

/// Build the full toolset as shared (`Arc`) handles.
///
/// Convert to boxes with the SDK's `shared_tools_to_boxes` when an API wants
/// `Vec<Box<dyn ToolT>>` but the handles must stay shared across agents.
pub fn shared_tools(
    root: &Path,
    approvals: Arc<dyn ApprovalHook>,
) -> anyhow::Result<Vec<Arc<dyn ToolT>>> {
    Ok(boxed_tools(root, approvals)?
        .into_iter()
        .map(Arc::from)
        .collect())
}

/// Entry points for the `fuzz/` cargo-fuzz targets; not a stable API.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing {
    /// Commands found inside wrappers such as `sh -c "..."` or `sudo` (see
    /// [`super::commands_run_by`]).
    pub fn sensitive_names(
        patterns: &[String],
        tool: &str,
        args: &serde_json::Value,
    ) -> Vec<String> {
        // No root: names alone, so a fuzzer's paths touch neither the disk nor Git.
        super::sensitive::SensitiveMatcher::new(patterns, None)
            .classify(tool, args)
            .map(|access| access.names())
            .unwrap_or_default()
    }

    pub fn wrapped_commands(command: &[String]) -> Option<Vec<Vec<String>>> {
        super::wrapped::wrapped_commands(command)
    }

    /// Parses every `@@` hunk in `patch` and applies them to `old` (the hunk parser and the
    /// forgiving applier `apply_patch` uses, without touching files).
    pub fn apply_hunk_text(old: &str, patch: &str) -> Result<Vec<String>, String> {
        let lines: Vec<&str> = patch.lines().collect();
        let mut hunks = Vec::new();
        let mut at = 0;
        while at < lines.len() {
            if lines[at].starts_with("@@ ") {
                let (hunk, next) = super::parse_hunk(&lines, at).map_err(|e| e.to_string())?;
                hunks.push(hunk);
                at = next;
            } else {
                at += 1;
            }
        }
        let old: Vec<String> = old.lines().map(str::to_owned).collect();
        super::apply_hunks(&old, &hunks, "fuzz").map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests;
