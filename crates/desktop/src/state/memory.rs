//! Memory across chats: the stores under app data `memory/` (global, and one folder per
//! project), the agent's `memory` tool, the index each run starts with, and Settings › Memory.
//! The format and its checks live in `themis_core::memory`. Diagnostics carry scope, type and
//! counts only, never a memory's text.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use themis_core::configuration::{ApprovalMode, PolicyAction};
use themis_core::memory::{
    Memory, MemoryKind, MemoryScope, MemoryStore, SaveMode, GLOBAL_FOLDER, MAX_MEMORIES,
};
use themis_core::tools::{async_trait, RiskLevel, ToolAction, ToolCallError, ToolRuntime, ToolT};

use super::integration_tools::RunApproval;
use super::AppState;
use crate::sink::EventSink;
use crate::types::{ThreadEvent, ThreadEventEnvelope};

/// Directory name under the app data directory.
pub(super) const MEMORY_DIR: &str = "memory";

impl AppState {
    fn memory_root(&self) -> PathBuf {
        self.app_data_dir().join(MEMORY_DIR)
    }

    /// The store of `scope`; a project scope needs the project's root.
    pub(crate) fn memory_store(
        &self,
        scope: MemoryScope,
        project_root: Option<&Path>,
    ) -> Result<MemoryStore, String> {
        let folder = match scope {
            MemoryScope::Global => GLOBAL_FOLDER.to_owned(),
            MemoryScope::Project => themis_core::memory::project_folder(
                project_root.ok_or("project memories need a project")?,
            ),
        };
        MemoryStore::new(&self.memory_root(), &folder).map_err(|e| e.to_string())
    }

    /// What a run in `project_root` starts with: the global and this project's indexes (never
    /// another project's), or nothing when memory is off or empty.
    pub(super) fn memory_note(&self, enabled: bool, project_root: &Path) -> String {
        if !enabled {
            return String::new();
        }
        let index = |scope| {
            self.memory_store(scope, Some(project_root))
                .map(|store| store.index())
                .unwrap_or_default()
        };
        themis_core::memory::run_note(&index(MemoryScope::Global), &index(MemoryScope::Project))
    }

    /// Settings › Memory: one scope's memories, its index use and the revision a delete names.
    pub fn list_memories(&self, scope: &str, project_root: Option<&str>) -> Result<Value, String> {
        let scope = MemoryScope::parse(scope).ok_or("scope must be project or global")?;
        let root = project_root.map(PathBuf::from);
        let store = self.memory_store(scope, root.as_deref())?;
        let listing = store.list().map_err(|e| e.to_string())?;
        let enabled = self
            .user_memory_view()
            .map(|(memory, _)| memory.enabled)
            .unwrap_or(true);
        Ok(json!({
            "memories": listing.memories,
            "skipped": listing.skipped,
            "index_lines": store.index().lines().count(),
            "max_lines": MAX_MEMORIES,
            "revision": store.revision(),
            "enabled": enabled,
        }))
    }

    /// Settings › Memory's Delete: refused when the scope changed since `revision`.
    pub fn delete_memory(
        &self,
        scope: &str,
        project_root: Option<&str>,
        name: &str,
        revision: Option<&str>,
    ) -> Result<Value, String> {
        let scope = MemoryScope::parse(scope).ok_or("scope must be project or global")?;
        let root = project_root.map(PathBuf::from);
        let store = self.memory_store(scope, root.as_deref())?;
        let memory = store.delete(name, revision).map_err(|e| e.to_string())?;
        self.note_memory("deleted", scope, memory.kind, &store, "settings");
        Ok(json!({"revision": store.revision()}))
    }

    /// One bounded diagnostics line: what happened, where and how many remain.
    fn note_memory(
        &self,
        event: &str,
        scope: MemoryScope,
        kind: MemoryKind,
        store: &MemoryStore,
        by: &str,
    ) {
        let count = store.index().lines().count();
        let _ = self.inner.diagnostics.append(
            "memory",
            event,
            "info",
            json!({"scope": scope.as_str(), "type": kind.as_str(), "count": count, "by": by}),
        );
    }
}

/// The `memory` tool. The main agent saves, updates, deletes and reads; a subagent only reads.
pub(super) struct MemoryTool {
    pub(super) state: AppState,
    pub(super) sink: Arc<dyn EventSink>,
    pub(super) thread_id: String,
    pub(super) run_id: String,
    pub(super) project_root: PathBuf,
    /// `memory.enabled` as this run captured it.
    pub(super) enabled: bool,
    pub(super) run: RunApproval,
    /// A subagent's copy: `read` only.
    pub(super) read_only: bool,
}

impl std::fmt::Debug for MemoryTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryTool").finish_non_exhaustive()
    }
}

fn error(message: impl ToString) -> ToolCallError {
    ToolCallError::RuntimeError(message.to_string().into())
}

fn text_arg(args: &Value, key: &str) -> Option<String> {
    args[key]
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(themis_core::memory::strip_secrets)
}

/// The memory a save or update describes, secrets stripped.
fn memory_from(args: &Value, name: &str) -> Result<Memory, ToolCallError> {
    let kind = args["type"]
        .as_str()
        .and_then(MemoryKind::parse)
        .ok_or_else(|| error("type must be user, feedback, project or reference"))?;
    let description = text_arg(args, "description")
        .ok_or_else(|| error("description is needed: one line saying what the memory is"))?
        .replace(['\n', '\r'], " ");
    let fact = text_arg(args, "body").ok_or_else(|| error("body is needed: the fact itself"))?;
    let mut lines = Vec::new();
    if let Some(why) = text_arg(args, "why") {
        lines.push(format!("Why: {}", why.replace('\n', " ")));
    }
    if let Some(how) = text_arg(args, "how_to_apply") {
        lines.push(format!("How to apply: {}", how.replace('\n', " ")));
    }
    let body = if lines.is_empty() {
        fact
    } else {
        format!("{fact}\n\n{}", lines.join("\n"))
    };
    let memory = Memory {
        name: name.to_owned(),
        description,
        kind,
        saved: themis_core::memory::today(),
        source: text_arg(args, "source").map(|source| source.replace(['\n', '\r'], " ")),
        body,
    };
    memory.validate().map_err(error)?;
    Ok(memory)
}

impl MemoryTool {
    fn denied(&self, risk: RiskLevel) -> bool {
        let action = ToolAction {
            display: None,
            command: Vec::new(),
            tool: "memory".to_owned(),
            summary: "memory".to_owned(),
            risk,
        };
        let decision = if self.run.mode == ApprovalMode::Yolo {
            self.run.policy.yolo_decision(&action)
        } else {
            self.run.policy.decision(&action)
        };
        decision == PolicyAction::Deny
    }

    fn announce(&self, action: &str, scope: MemoryScope, memory: &Memory) {
        let envelope = ThreadEventEnvelope {
            thread_id: self.thread_id.clone(),
            run_id: self.run_id.clone(),
            event: ThreadEvent::MemoryChanged {
                action: action.to_owned(),
                scope: scope.as_str().to_owned(),
                name: memory.name.clone(),
                description: memory.description.clone(),
                project_root: (scope == MemoryScope::Project)
                    .then(|| self.project_root.to_string_lossy().into_owned()),
            },
        };
        if let Err(problem) = self.state.inner.transcript.append_event(&envelope) {
            self.state.record_error("save_thread_event", problem);
        }
        self.sink.emit_thread_event(&envelope);
    }
}

#[async_trait]
impl ToolRuntime for MemoryTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        if !self.enabled {
            return Err(error(
                "memory is off (Settings › Memory): nothing is saved or read",
            ));
        }
        let action = if self.read_only {
            "read"
        } else {
            args["action"].as_str().unwrap_or_default()
        };
        if !["save", "update", "delete", "read"].contains(&action) {
            return Err(error("action must be save, update, delete or read"));
        }
        let scope = match args["scope"].as_str() {
            None | Some("") => MemoryScope::Project,
            Some(text) => {
                MemoryScope::parse(text).ok_or_else(|| error("scope must be project or global"))?
            }
        };
        let name = args["name"]
            .as_str()
            .map(str::trim)
            .filter(|name| themis_core::memory::valid_name(name))
            .ok_or_else(|| {
                error("name must be a lowercase kebab-case slug, such as free-model-for-tests")
            })?
            .to_owned();
        let risk = if action == "read" {
            RiskLevel::Read
        } else {
            RiskLevel::Write
        };
        if self.denied(risk) {
            return Err(error(
                "approval denied for 'memory': the approval rules deny it",
            ));
        }
        let store = self
            .state
            .memory_store(scope, Some(&self.project_root))
            .map_err(error)?;
        match action {
            "read" => {
                let memory = store.read(&name).map_err(error)?.ok_or_else(|| {
                    error(format!("no {} memory is named {name}", scope.as_str()))
                })?;
                Ok(json!({
                    "name": memory.name,
                    "scope": scope.as_str(),
                    "type": memory.kind.as_str(),
                    "saved": memory.saved,
                    "source": memory.source,
                    "description": memory.description,
                    "text": memory.body,
                }))
            }
            "delete" => {
                let memory = store.delete(&name, None).map_err(error)?;
                self.state
                    .note_memory("deleted", scope, memory.kind, &store, "agent");
                self.announce("forgot", scope, &memory);
                Ok(json!({"deleted": name, "scope": scope.as_str()}))
            }
            _ => {
                let memory = memory_from(&args, &name)?;
                if scope == MemoryScope::Global && !memory.kind.may_be_global() {
                    return Err(error(format!(
                        "a {} memory belongs to this project: save it with scope \"project\"",
                        memory.kind.as_str()
                    )));
                }
                let mode = if action == "save" {
                    SaveMode::Create
                } else {
                    SaveMode::Update
                };
                let count = store.save(&memory, mode).map_err(error)?;
                let verb = if action == "save" { "saved" } else { "updated" };
                self.state
                    .note_memory(verb, scope, memory.kind, &store, "agent");
                self.announce(verb, scope, &memory);
                Ok(json!({
                    "status": verb,
                    "name": memory.name,
                    "scope": scope.as_str(),
                    "memories_in_scope": count,
                }))
            }
        }
    }
}

impl ToolT for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }
    fn description(&self) -> &str {
        if self.read_only {
            themis_core::prompts::MEMORY_TOOL_READ
        } else {
            themis_core::prompts::MEMORY_TOOL
        }
    }
    fn args_schema(&self) -> Value {
        let scope = json!({"type":"string","enum":["project","global"],"description":"Where it applies; project by default."});
        let name = json!({"type":"string","description":"Lowercase kebab-case slug, at most 64 characters."});
        if self.read_only {
            return json!({"type":"object","properties":{"name":name,"scope":scope},"required":["name"],"additionalProperties":false});
        }
        json!({"type":"object","properties":{
            "action":{"type":"string","enum":["save","update","delete","read"]},
            "name":name,
            "scope":scope,
            "type":{"type":"string","enum":["user","feedback","project","reference"]},
            "description":{"type":"string","description":"One line, at most 160 characters."},
            "body":{"type":"string","description":"The fact itself."},
            "why":{"type":"string"},
            "how_to_apply":{"type":"string"},
            "source":{"type":"string","description":"The web page or file it was built from, when not from the person."}
        },"required":["action","name"],"additionalProperties":false})
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
}
