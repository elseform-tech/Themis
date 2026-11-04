//! Memory across chats: the stores under app data `memory/` (global, and one folder per
//! project), the memory block each run starts with, the chat's memory line, and Settings ›
//! Memory. The memory models keep the stores (`memory_auto`); no agent has a memory tool. The
//! format and its checks live in `themis_core::memory`. Diagnostics carry scope, type and
//! counts only, never a memory's text.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use themis_core::memory::{MemoryKind, MemoryScope, MemoryStore, GLOBAL_FOLDER, MAX_MEMORIES};

use super::AppState;
use crate::sink::EventSink;
use crate::types::{MemoryChange, ThreadEvent, ThreadEventEnvelope};

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

    /// This project's plan records.
    pub(crate) fn plan_store(
        &self,
        project_root: &Path,
    ) -> Result<themis_core::memory::plans::PlanStore, String> {
        themis_core::memory::plans::PlanStore::new(
            &self.memory_root(),
            &themis_core::memory::project_folder(project_root),
        )
        .map_err(|e| e.to_string())
    }

    /// The plan record a chat is about: the one its goal builds, else the one `about` names
    /// best. Reads the index and at most one record.
    pub(crate) fn plan_record_for(
        &self,
        project_root: &Path,
        thread_id: &str,
        about: &str,
    ) -> (String, Option<themis_core::memory::plans::PlanRecord>) {
        let Ok(store) = self.plan_store(project_root) else {
            return (String::new(), None);
        };
        let index = store.index();
        if index.is_empty() {
            return (index, None);
        }
        let record = store.for_chat(thread_id).or_else(|| {
            themis_core::memory::plans::relevant_name(about, &index)
                .and_then(|name| store.read(&name).ok().flatten())
        });
        (index, record)
    }

    /// What a run in `project_root` starts with for `message`: the global and this project's
    /// indexes (never another project's) and the memories relevant to the message in full, or
    /// nothing when memory is off or empty. Reads two index files and at most
    /// [`themis_core::memory::MAX_RELEVANT`] memories.
    pub(super) fn memory_note(
        &self,
        enabled: bool,
        (project_root, thread_id): (&Path, &str),
        message: &str,
    ) -> String {
        if !enabled {
            return String::new();
        }
        // Then the plan records: their index and the one this chat is about, in full.
        let (plans_index, record) = self.plan_record_for(project_root, thread_id, message);
        let mut plans = themis_core::memory::plans::run_note(&plans_index, record.as_ref());
        // And the project's tasks outside any record: open ones in full, done ones counted.
        let tasks = self
            .plan_store(project_root)
            .map(|store| themis_core::memory::tasks::run_view(&store.tasks().read()))
            .unwrap_or_default();
        if !tasks.is_empty() {
            if !plans.is_empty() {
                plans.push('\n');
            }
            plans.push_str(&themis_core::prompts::fill(
                themis_core::prompts::SYSTEM_MEMORY_TASKS,
                &[("tasks", tasks.trim_end())],
            ));
        }
        let Ok(global) = self.memory_store(MemoryScope::Global, None) else {
            return String::new();
        };
        let project = self
            .memory_store(MemoryScope::Project, Some(project_root))
            .ok();
        let memories = themis_core::memory::memory_block(&global, project.as_ref(), message);
        match (memories.is_empty(), plans.is_empty()) {
            (_, true) => memories,
            (true, false) => plans,
            (false, false) => format!("{memories}\n{plans}"),
        }
    }

    /// Settings › Memory: one scope's memories, its index use, the revision a delete names, and
    /// whether memory jobs are waiting for the memory models.
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
            "waiting": self.memory_waiting(),
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

    /// Puts the memory line ("Saved … · Updated …") under `run_id`'s reply, saved with it.
    pub(super) fn announce_memory(
        &self,
        sink: &dyn EventSink,
        (thread_id, run_id): (&str, &str),
        changes: Vec<MemoryChange>,
    ) {
        if changes.is_empty() {
            return;
        }
        let envelope = ThreadEventEnvelope {
            thread_id: thread_id.to_owned(),
            run_id: run_id.to_owned(),
            event: ThreadEvent::MemoryChanged { changes },
        };
        if let Err(problem) = self.inner.transcript.append_event(&envelope) {
            self.record_error("save_thread_event", problem);
        }
        sink.emit_thread_event(&envelope);
    }

    /// Tells the chat where its memory pass stands ([`ThreadEvent::MemoryWorking`]); live only.
    pub(super) fn memory_working(
        &self,
        sink: &dyn EventSink,
        (thread_id, run_id): (&str, &str),
        step: &str,
    ) {
        sink.emit_thread_event(&ThreadEventEnvelope {
            thread_id: thread_id.to_owned(),
            run_id: run_id.to_owned(),
            event: ThreadEvent::MemoryWorking {
                step: step.to_owned(),
            },
        });
    }

    /// One bounded diagnostics line: what happened, where and how many remain.
    pub(super) fn note_memory(
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
