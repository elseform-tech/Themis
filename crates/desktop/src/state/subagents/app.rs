//! The app's view of subagents: listing, transcripts, stopping and forgetting.

use super::*;
use crate::state::chat_grants;

impl AppState {
    pub(super) fn remember_subagent(&self, thread_id: &str, agent: &Arc<Subagent>) {
        let mut all = self
            .inner
            .subagents
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let list = all.entry(thread_id.to_owned()).or_default();
        list.push(Arc::clone(agent));
        while list.len() > KEPT_PER_CHAT {
            match list.iter().position(|agent| agent.ended()) {
                Some(index) => {
                    list.remove(index);
                }
                None => break,
            }
        }
    }

    /// Where a subagent's events are saved.
    pub(in crate::state) fn subagent_transcript_path(
        &self,
        thread_id: &str,
        id: &str,
    ) -> Option<PathBuf> {
        (chat_grants::safe_chat_id(thread_id) && id.bytes().all(|b| b.is_ascii_hexdigit())).then(
            || {
                self.app_data_dir()
                    .join(SUBAGENTS_DIR)
                    .join(thread_id)
                    .join(format!("{id}.jsonl"))
            },
        )
    }

    /// The chat's subagents, oldest first, with what each is doing or how it ended.
    pub fn list_subagents(&self, thread_id: &str) -> Vec<Value> {
        self.inner
            .subagents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
            .map(|list| {
                list.iter()
                    .map(|agent| {
                        let progress = agent.progress();
                        let (status, result) = progress
                            .ended
                            .unwrap_or_else(|| ("running".into(), String::new()));
                        json!({"id": agent.id, "name": agent.name, "brief": agent.brief,
                            "model": agent.model, "write_paths": agent.write_paths, "connections": agent.mcp.connections, "web": !agent.web.tools.is_empty(),
                    "parent": agent.parent, "shell_scopes": agent.shell_scopes(), "confined": agent.confined, "profile": agent.profile.as_ref().map(|named| &named.profile.name), "ran_on": progress.ran_on, "status": status, "text": progress.status,
                            "result": result, "tool_calls": agent.tool_calls.load(Ordering::SeqCst),
                            "seconds": agent.started.lock().unwrap_or_else(|e| e.into_inner()).elapsed().as_secs()})
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn stop_one_subagent(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        agent: &Subagent,
    ) {
        agent.stop.store(true, Ordering::SeqCst);
        // What it started stops with it, approvals included.
        let mut stopping = vec![agent.id.clone()];
        if let Some(list) = self
            .inner
            .subagents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
        {
            for child in list {
                if child.parent.as_deref() == Some(agent.id.as_str()) {
                    child.stop.store(true, Ordering::SeqCst);
                    stopping.push(child.id.clone());
                }
            }
        }
        let mut pending = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|approval_id, approval| {
            let mine = approval.thread_id == thread_id
                && approval
                    .request
                    .subagent
                    .as_ref()
                    .is_some_and(|label| stopping.contains(&label.id));
            if mine {
                let _ = approval.sender.send(Approval::Deny);
                sink.emit_approval_expired(&ApprovalExpired {
                    thread_id: thread_id.to_owned(),
                    approval_id: approval_id.clone(),
                    steered: false,
                });
            }
            !mine
        });
    }

    /// One of the chat's subagents, while it is running.
    pub(in crate::state) fn running_subagent(
        &self,
        thread_id: &str,
        id: &str,
    ) -> Option<Arc<Subagent>> {
        self.inner
            .subagents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)?
            .iter()
            .find(|agent| agent.id == id && !agent.ended())
            .cloned()
    }

    /// Stops one subagent of the chat (Workspace › Tasks); the main agent gets what it
    /// had so far. Completed work is never undone.
    pub fn stop_subagent(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        subagent_id: &str,
    ) -> Result<(), String> {
        let agent = self
            .inner
            .subagents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
            .and_then(|list| list.iter().find(|agent| agent.id == subagent_id).cloned())
            .ok_or_else(|| "This subagent is not running".to_owned())?;
        if !agent.ended() {
            self.stop_one_subagent(sink, thread_id, &agent);
        }
        Ok(())
    }

    /// A subagent's saved steps, oldest first: at most the last 512 KiB, whole lines only.
    pub fn get_subagent_transcript(
        &self,
        thread_id: &str,
        subagent_id: &str,
    ) -> Result<Vec<ThreadEvent>, String> {
        use std::io::{Read, Seek, SeekFrom};
        const LIMIT: u64 = 512 * 1024;
        let path = self
            .subagent_transcript_path(thread_id, subagent_id)
            .ok_or_else(|| "Unknown subagent".to_owned())?;
        let Ok(mut file) = std::fs::File::open(&path) else {
            return Ok(Vec::new());
        };
        let length = file.metadata().map(|m| m.len()).unwrap_or(0);
        let start = length.saturating_sub(LIMIT);
        file.seek(SeekFrom::Start(start))
            .map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        file.take(LIMIT)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        let text = String::from_utf8_lossy(&bytes);
        let mut lines = text.lines();
        if start > 0 {
            // The first line may be cut.
            lines.next();
        }
        Ok(lines
            .filter_map(|line| serde_json::from_str::<ThreadEventEnvelope>(line).ok())
            .map(|envelope| envelope.event)
            .collect())
    }

    /// Stops every running subagent of the chat (its run ended).
    pub(in crate::state) fn stop_subagents(&self, thread_id: &str) {
        if let Some(list) = self
            .inner
            .subagents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
        {
            for agent in list {
                agent.stop.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Forgets deleted chats' subagents and their transcripts.
    pub(in crate::state) fn forget_subagents(&self, thread_ids: &[String]) {
        let mut all = self
            .inner
            .subagents
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for id in thread_ids {
            if let Some(list) = all.remove(id) {
                for agent in list {
                    agent.stop.store(true, Ordering::SeqCst);
                }
            }
            if chat_grants::safe_chat_id(id) {
                let _ = std::fs::remove_dir_all(self.app_data_dir().join(SUBAGENTS_DIR).join(id));
            }
        }
    }
}
