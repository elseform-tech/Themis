//! Durable per-thread conversation and run event log.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use themis_core::runtime::{checkpoint_parts, ChatRole, ConversationRole, ConversationTurn};

use crate::types::{ThreadEvent, ThreadEventEnvelope};

mod context_turn;
pub mod history_pages;
#[cfg(test)]
mod scale_tests;
mod source_exports;
use context_turn::{context_turn, role_name};

/// Most rows [`Transcript::last_reply_text`] reads.
pub const REPLY_ROWS: usize = 64;

/// The whole reply before the latest message, from turns newest first (`(is_assistant,
/// text)`): the person's trailing messages are skipped, then the assistant messages up to the
/// message before them are joined oldest first.
pub fn reply_before<'a>(newest_first: impl Iterator<Item = (bool, &'a str)>) -> String {
    let mut parts: Vec<&str> = newest_first
        .skip_while(|(assistant, _)| !assistant)
        .take_while(|(assistant, _)| *assistant)
        .map(|(_, text)| text.trim())
        .filter(|text| !text.is_empty())
        .collect();
    parts.reverse();
    parts.join("\n\n")
}

// Items live briefly while being read or saved, and most of them are events.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryItem {
    User {
        run_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<String>,
        /// The Vision that sent it (directly or from the queue); absent for the person's own
        /// message, and in transcripts saved before Visions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vision_id: Option<String>,
    },
    Event {
        envelope: ThreadEventEnvelope,
    },
    Legacy {
        message: serde_json::Value,
    },
}

pub struct TranscriptStore {
    connection: Mutex<Connection>,
}

/// Shortest excerpt that counts as already present when it is merely contained in a turn.
const MIN_SUBSTRING_DEDUPE_CHARS: usize = 40;

/// Newest history rows read to find how a chat's last run ended.
const OUTCOME_ROWS: i64 = 64;

impl TranscriptStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create session directory: {e}"))?;
        }
        let connection = Connection::open(path).map_err(|e| format!("open sessions: {e}"))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS history (
               seq INTEGER PRIMARY KEY,
               thread_id TEXT NOT NULL,
               item TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS history_thread ON history(thread_id, seq);",
            )
            .map_err(|e| format!("initialize sessions: {e}"))?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS context_turns (seq INTEGER PRIMARY KEY, thread_id TEXT NOT NULL, role TEXT NOT NULL, text TEXT NOT NULL); CREATE INDEX IF NOT EXISTS context_turns_thread ON context_turns(thread_id, seq); CREATE TABLE IF NOT EXISTS context_checkpoints (thread_id TEXT PRIMARY KEY, through_seq INTEGER NOT NULL, summary TEXT NOT NULL); CREATE TABLE IF NOT EXISTS context_source_exports (thread_id TEXT NOT NULL,directory TEXT NOT NULL,last_seq INTEGER NOT NULL,last_file TEXT NOT NULL,PRIMARY KEY(thread_id,directory));").map_err(|e| format!("initialize context: {e}"))?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS attachment_paths (seq INTEGER PRIMARY KEY,thread_id TEXT NOT NULL,path TEXT NOT NULL,UNIQUE(thread_id,path));").map_err(|e| format!("initialize attachments: {e}"))?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS document_capsules (thread_id TEXT NOT NULL,source_sha256 TEXT NOT NULL,source_path TEXT NOT NULL,capsule TEXT NOT NULL,PRIMARY KEY(thread_id,source_sha256,source_path));").map_err(|e| format!("initialize document capsules: {e}"))?;
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| format!("read session version: {e}"))?;
        if version == 0 {
            let mut statement = connection
                .prepare("SELECT seq, thread_id, item FROM history ORDER BY seq")
                .map_err(|e| format!("migrate context: {e}"))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| format!("migrate context: {e}"))?;
            for row in rows {
                let (seq, thread_id, json) = row.map_err(|e| format!("migrate context: {e}"))?;
                let item: HistoryItem =
                    serde_json::from_str(&json).map_err(|e| format!("decode session item: {e}"))?;
                if let Some(turn) = context_turn(&item) {
                    connection.execute("INSERT OR IGNORE INTO context_turns(seq,thread_id,role,text) VALUES (?1,?2,?3,?4)", params![seq, thread_id, role_name(&turn.role), turn.text]).map_err(|e| format!("migrate context: {e}"))?;
                }
            }
            connection
                .execute_batch("PRAGMA user_version=1")
                .map_err(|e| format!("finish context migration: {e}"))?;
        }
        if version < 2 {
            migrate_attachment_paths(&connection)?;
        }
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Durable ordering across conversations, without reading message bodies.
    /// Whether this chat has ever had a goal, from its goal turns or a `/goal` message.
    pub fn had_goal(&self, thread_id: &str) -> bool {
        self.connection
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM context_turns WHERE thread_id = ?1 AND role = 'user' \
                 AND (instr(text, ?2) > 0 OR text LIKE '/goal%'))",
                params![thread_id, themis_core::goal::GOAL_TURN_MARKER],
                |row| row.get(0),
            )
            .unwrap_or(false)
    }

    pub fn last_activity_seq(&self, thread_id: &str) -> Result<i64, String> {
        self.connection
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM history WHERE thread_id = ?1",
                [thread_id],
                |row| row.get(0),
            )
            .map_err(|error| format!("read conversation activity: {error}"))
    }

    /// How the chat's last run ended, read from its newest history rows only (at most
    /// [`OUTCOME_ROWS`]): `done`, `incomplete`, `failed` or `stopped`; `None` with no run end
    /// among them.
    pub fn last_outcome(&self, thread_id: &str) -> Result<Option<&'static str>, String> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut statement = connection
            .prepare_cached(
                "SELECT item FROM history WHERE thread_id = ?1 ORDER BY seq DESC LIMIT ?2",
            )
            .map_err(|error| format!("read last outcome: {error}"))?;
        let rows = statement
            .query_map(rusqlite::params![thread_id, OUTCOME_ROWS], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|error| format!("read last outcome: {error}"))?;
        for item in rows.flatten() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&item) else {
                continue;
            };
            let event = &value["envelope"]["event"];
            match event["kind"].as_str() {
                Some("finished") => return Ok(Some("done")),
                Some("incomplete") => return Ok(Some("incomplete")),
                Some("failed") => {
                    let stopped = event["error"]
                        .as_str()
                        .is_some_and(|error| error.starts_with("Stopped by you."));
                    return Ok(Some(if stopped { "stopped" } else { "failed" }));
                }
                _ => {}
            }
        }
        Ok(None)
    }

    pub(crate) fn save_document_capsule(
        &self,
        thread_id: &str,
        capsule: &themis_core::runtime::documents::DocumentCapsule,
    ) -> Result<(), String> {
        let text = serde_json::to_string(capsule).map_err(|e| e.to_string())?;
        if text.len() > 1024 * 1024 {
            return Err("Document capsule exceeds its storage bound".into());
        }
        self.connection.lock().unwrap_or_else(|e| e.into_inner()).execute(
            "INSERT OR REPLACE INTO document_capsules(thread_id,source_sha256,source_path,capsule) VALUES (?1,?2,?3,?4)",
            params![thread_id,capsule.source_sha256,capsule.source_path,text]).map_err(|e|format!("save document capsule: {e}"))?;
        Ok(())
    }

    pub(crate) fn load_document_capsule(
        &self,
        thread_id: &str,
        wanted: &themis_core::runtime::documents::DocumentCapsule,
    ) -> Result<Option<themis_core::runtime::documents::DocumentCapsule>, String> {
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        let body:Option<String> = connection.query_row("SELECT capsule FROM document_capsules WHERE thread_id=?1 AND source_sha256=?2 AND source_path=?3 AND length(capsule)<=1048576",params![thread_id,wanted.source_sha256,wanted.source_path],|row|row.get(0)).optional().map_err(|e|e.to_string())?;
        Ok(body
            .and_then(|body| {
                serde_json::from_str::<themis_core::runtime::documents::DocumentCapsule>(&body).ok()
            })
            .filter(|c| c.compatible_with(wanted)))
    }

    pub(crate) fn document_context(
        &self,
        thread_id: &str,
        request: &str,
        max_bytes: usize,
        current_paths: &[String],
    ) -> Result<Vec<ConversationTurn>, String> {
        use themis_core::runtime::documents::DocumentCapsule;
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        // Match names/identities across the entire catalog before the bounded body read.
        // A source path keeps its latest version; distinct uploads remain distinct associations.
        let mut statement = connection.prepare("WITH ranked AS (SELECT rowid AS seq,*,row_number() OVER (PARTITION BY source_path ORDER BY rowid DESC) AS newest FROM document_capsules WHERE thread_id=?1 AND length(capsule)<=1048576) SELECT capsule FROM ranked WHERE newest=1 OR instr(?2,source_sha256)>0 OR instr(?2,source_path)>0 ORDER BY (instr(lower(?2),lower(json_extract(capsule,'$.source_name')))>0 OR instr(?2,source_sha256)>0 OR instr(?2,source_path)>0) DESC,seq DESC LIMIT 32").map_err(|e|e.to_string())?;
        let rows = statement
            .query_map(params![thread_id, request], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        let capsules = rows
            .filter_map(|row| match row {
                Ok(text) => serde_json::from_str::<DocumentCapsule>(&text)
                    .ok()
                    .filter(|c| {
                        c.format_version == 1
                            && c.pipeline_version
                                == themis_core::runtime::documents::PIPELINE_VERSION
                            && !current_paths.contains(&c.source_path)
                    })
                    .map(Ok),
                Err(error) => Some(Err(error.to_string())),
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut remaining = max_bytes;
        let mut turns = Vec::new();
        for capsule in capsules {
            let text = capsule.context();
            if text.len() <= remaining {
                remaining -= text.len();
                turns.push(ConversationTurn {
                    role: ConversationRole::Assistant,
                    text,
                });
            } else {
                let inventory = format!("Historical document capsule omitted from this bounded selection. Original source (untrusted): {}",serde_json::json!({"source_path":capsule.source_path,"source_sha256":capsule.source_sha256,"original_snapshot":capsule.original_snapshot,"summary_available_in_transcript":true}));
                if inventory.len() <= remaining {
                    remaining -= inventory.len();
                    turns.push(ConversationTurn {
                        role: ConversationRole::Assistant,
                        text: inventory,
                    });
                }
            }
        }
        Ok(turns)
    }

    fn append(&self, thread_id: &str, item: &HistoryItem) -> Result<(), String> {
        let item = serde_json::to_string(item).map_err(|e| format!("encode session item: {e}"))?;
        let mut connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = connection
            .transaction()
            .map_err(|e| format!("save session item: {e}"))?;
        transaction
            .execute(
                "INSERT INTO history(thread_id, item) VALUES (?1, ?2)",
                params![thread_id, item],
            )
            .map_err(|e| format!("save session item: {e}"))?;
        let seq = transaction.last_insert_rowid();
        let decoded: HistoryItem =
            serde_json::from_str(&item).map_err(|e| format!("decode session item: {e}"))?;
        if let Some(turn) = context_turn(&decoded) {
            transaction
                .execute(
                    "INSERT INTO context_turns(seq,thread_id,role,text) VALUES (?1,?2,?3,?4)",
                    params![seq, thread_id, role_name(&turn.role), turn.text],
                )
                .map_err(|e| format!("save context turn: {e}"))?;
        }
        if let HistoryItem::User { attachments, .. } = &decoded {
            remember_attachment_paths(&transaction, thread_id, attachments)
                .map_err(|e| format!("save attachments: {e}"))?;
        }
        if let HistoryItem::Event {
            envelope:
                ThreadEventEnvelope {
                    event: ThreadEvent::ContextCheckpoint { summary },
                    ..
                },
        } = decoded
        {
            transaction.execute("INSERT INTO context_checkpoints(thread_id,through_seq,summary) VALUES (?1,?2,?3) ON CONFLICT(thread_id) DO UPDATE SET through_seq=excluded.through_seq, summary=excluded.summary", params![thread_id,seq,summary]).map_err(|e| format!("save context checkpoint: {e}"))?;
        }
        transaction
            .commit()
            .map_err(|e| format!("commit session item: {e}"))?;
        Ok(())
    }

    pub fn append_user(&self, thread_id: &str, run_id: &str, text: &str) -> Result<(), String> {
        self.append_user_with_attachments(thread_id, run_id, text, Vec::new())
    }

    pub fn append_user_with_attachments(
        &self,
        thread_id: &str,
        run_id: &str,
        text: &str,
        attachments: Vec<String>,
    ) -> Result<(), String> {
        self.append_user_from(thread_id, run_id, text, attachments, None)
    }

    /// Saves a user message, as the Vision `vision_id`'s when it sent it.
    pub fn append_user_from(
        &self,
        thread_id: &str,
        run_id: &str,
        text: &str,
        attachments: Vec<String>,
        vision_id: Option<String>,
    ) -> Result<(), String> {
        self.append(
            thread_id,
            &HistoryItem::User {
                run_id: run_id.to_owned(),
                text: text.to_owned(),
                attachments,
                vision_id,
            },
        )
    }

    pub fn append_event(&self, envelope: &ThreadEventEnvelope) -> Result<(), String> {
        self.append(
            &envelope.thread_id,
            &HistoryItem::Event {
                envelope: envelope.clone(),
            },
        )
    }

    pub fn import_legacy(
        &self,
        thread_id: &str,
        messages: &[serde_json::Value],
    ) -> Result<(), String> {
        if messages.len() > 10_000
            || messages.iter().any(|message| {
                !matches!(
                    message.get("role").and_then(|value| value.as_str()),
                    Some("user" | "assistant" | "system")
                ) || message
                    .get("text")
                    .and_then(|value| value.as_str())
                    .is_none_or(|text| text.len() > 131_072)
            })
        {
            return Err("legacy conversation is invalid or too large".to_owned());
        }
        let mut connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = connection
            .transaction()
            .map_err(|e| format!("import conversation: {e}"))?;
        let count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM history WHERE thread_id = ?1",
                [thread_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("check conversation: {e}"))?;
        if count != 0 {
            return Ok(());
        }
        for message in messages {
            let item = serde_json::to_string(&HistoryItem::Legacy {
                message: message.clone(),
            })
            .map_err(|e| format!("encode conversation: {e}"))?;
            transaction
                .execute(
                    "INSERT INTO history(thread_id, item) VALUES (?1, ?2)",
                    params![thread_id, item],
                )
                .map_err(|e| format!("import conversation: {e}"))?;
            let seq = transaction.last_insert_rowid();
            if let Some(turn) = context_turn(&HistoryItem::Legacy {
                message: message.clone(),
            }) {
                transaction
                    .execute(
                        "INSERT INTO context_turns(seq,thread_id,role,text) VALUES (?1,?2,?3,?4)",
                        params![seq, thread_id, role_name(&turn.role), turn.text],
                    )
                    .map_err(|e| format!("import context: {e}"))?;
            }
        }
        transaction
            .commit()
            .map_err(|e| format!("commit conversation: {e}"))
    }

    /// Distinct attachment paths this chat's user messages referenced, oldest first.
    pub fn attachment_paths(&self, thread_id: &str) -> Result<Vec<String>, String> {
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        let mut statement = connection
            .prepare("SELECT path FROM attachment_paths WHERE thread_id = ?1 ORDER BY seq")
            .map_err(|e| format!("read attachments: {e}"))?;
        let rows = statement
            .query_map([thread_id], |row| row.get::<_, String>(0))
            .map_err(|e| format!("read attachments: {e}"))?;
        rows.map(|row| row.map_err(|e| format!("read attachments: {e}")))
            .collect()
    }

    pub fn history(&self, thread_id: &str) -> Result<Vec<HistoryItem>, String> {
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        let mut statement = connection
            .prepare("SELECT item FROM history WHERE thread_id = ?1 ORDER BY seq")
            .map_err(|e| format!("read session: {e}"))?;
        let rows = statement
            .query_map([thread_id], |row| row.get::<_, String>(0))
            .map_err(|e| format!("read session: {e}"))?;
        rows.map(|row| {
            let text = row.map_err(|e| format!("read session: {e}"))?;
            serde_json::from_str(&text).map_err(|e| format!("decode session item: {e}"))
        })
        .collect()
    }

    /// Up to `limit` history items after `seq` (exclusive), oldest first,
    /// each with its position: one page for callers that keep their own
    /// watermark and page forward from the last position read.
    pub fn history_after(
        &self,
        thread_id: &str,
        seq: i64,
        limit: usize,
    ) -> Result<Vec<(i64, HistoryItem)>, String> {
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        let mut statement = connection
            .prepare("SELECT seq,item FROM history WHERE thread_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3")
            .map_err(|e| format!("read session: {e}"))?;
        let rows = statement
            .query_map(
                params![thread_id, seq, i64::try_from(limit).unwrap_or(i64::MAX)],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|e| format!("read session: {e}"))?;
        rows.map(|row| {
            let (seq, text) = row.map_err(|e| format!("read session: {e}"))?;
            serde_json::from_str(&text)
                .map(|item| (seq, item))
                .map_err(|e| format!("decode session item: {e}"))
        })
        .collect()
    }

    /// The chat's latest saved message from `role` ("user" or "assistant"), if any.
    pub fn last_turn_text(&self, thread_id: &str, role: &str) -> Option<String> {
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        connection
            .query_row(
                "SELECT text FROM context_turns WHERE thread_id=?1 AND role=?2 ORDER BY seq DESC LIMIT 1",
                params![thread_id, role],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    /// The whole assistant reply before the person's latest message: every assistant message of
    /// that turn, in order (a reply is saved as several, its closing summary last). Reads at most
    /// [`REPLY_ROWS`] rows.
    pub fn last_reply_text(&self, thread_id: &str) -> String {
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(mut statement) = connection.prepare(
            "SELECT role,text FROM context_turns WHERE thread_id=?1 ORDER BY seq DESC LIMIT ?2",
        ) else {
            return String::new();
        };
        let rows: Vec<(String, String)> = statement
            .query_map(params![thread_id, REPLY_ROWS as i64], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default();
        reply_before(
            rows.iter()
                .map(|(role, text)| (role == "assistant", text.as_str())),
        )
    }

    /// The chat's latest checkpoint summary, if it has one.
    pub fn checkpoint_summary(&self, thread_id: &str) -> Option<String> {
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        connection
            .query_row(
                "SELECT summary FROM context_checkpoints WHERE thread_id=?1",
                [thread_id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn delete_thread(&self, thread_id: &str) -> Result<(), String> {
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        connection
            .execute(
                "DELETE FROM document_capsules WHERE thread_id=?1",
                [thread_id],
            )
            .map_err(|e| e.to_string())?;
        connection
            .execute(
                "DELETE FROM context_source_exports WHERE thread_id=?1",
                [thread_id],
            )
            .map_err(|e| e.to_string())?;
        connection
            .execute(
                "DELETE FROM attachment_paths WHERE thread_id=?1",
                [thread_id],
            )
            .map_err(|e| e.to_string())?;
        connection
            .execute("DELETE FROM history WHERE thread_id = ?1", [thread_id])
            .map_err(|e| format!("delete session: {e}"))?;
        connection
            .execute(
                "DELETE FROM context_turns WHERE thread_id = ?1",
                [thread_id],
            )
            .map_err(|e| format!("delete context: {e}"))?;
        connection
            .execute(
                "DELETE FROM context_checkpoints WHERE thread_id = ?1",
                [thread_id],
            )
            .map_err(|e| format!("delete checkpoint: {e}"))?;
        Ok(())
    }
}

/// A path keeps the position of its first mention, so a chat's list stays oldest first.
fn remember_attachment_paths(
    connection: &Connection,
    thread_id: &str,
    paths: &[String],
) -> rusqlite::Result<()> {
    for path in paths {
        connection.execute(
            "INSERT OR IGNORE INTO attachment_paths(thread_id,path) VALUES (?1,?2)",
            params![thread_id, path],
        )?;
    }
    Ok(())
}

/// Fills the attachment list from older history, once, all or nothing.
fn migrate_attachment_paths(connection: &Connection) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|e| format!("migrate attachments: {e}"))?;
    {
        let mut statement = transaction
            .prepare(
                "SELECT thread_id,item FROM history WHERE item LIKE '%\"attachments\"%' ORDER BY seq",
            )
            .map_err(|e| format!("migrate attachments: {e}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("migrate attachments: {e}"))?;
        for row in rows {
            let (thread_id, text) = row.map_err(|e| format!("migrate attachments: {e}"))?;
            if let Ok(HistoryItem::User { attachments, .. }) = serde_json::from_str(&text) {
                remember_attachment_paths(&transaction, &thread_id, &attachments)
                    .map_err(|e| format!("migrate attachments: {e}"))?;
            }
        }
    }
    transaction
        .execute_batch("PRAGMA user_version=2")
        .map_err(|e| format!("finish attachment migration: {e}"))?;
    transaction
        .commit()
        .map_err(|e| format!("finish attachment migration: {e}"))
}

mod diagnostics;
mod request_context;
#[cfg(test)]
use request_context::newest_turns_with_terms;

#[cfg(test)]
mod document_tests {
    use super::*;
    use themis_core::runtime::documents::{DocumentCapsule, PIPELINE_VERSION};

    fn capsule(index: usize) -> DocumentCapsule {
        DocumentCapsule {
            format_version: 1,
            pipeline_version: PIPELINE_VERSION.into(),
            summary_model: "summary-model".into(),
            policy_sha256: "policy-one".into(),
            source_name: format!("story-{index}.md"),
            source_path: format!("/staged/{index}/story-{index}.md"),
            source_sha256: format!("{index:064x}"),
            source_bytes: 123,
            original_snapshot: format!("/evidence/{index}.jsonl").into(),
            summary: format!("Complete story {index}: approved ending survives."),
        }
    }

    #[test]
    fn capsule_restart_selection_compatibility_and_deletion() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("transcript.sqlite");
        let store = TranscriptStore::open(&path).unwrap();
        for index in 0..50 {
            store
                .save_document_capsule("thread", &capsule(index))
                .unwrap();
        }
        drop(store);
        let store = TranscriptStore::open(&path).unwrap();
        let context = store
            .document_context("thread", "Resume story-0.md", 4000, &[])
            .unwrap();
        assert!(
            context[0].text.contains("Complete story 0:"),
            "named older source must rank before catalog limit"
        );
        assert!(context.iter().map(|turn| turn.text.len()).sum::<usize>() <= 4000);
        let wanted = capsule(0);
        assert!(store
            .load_document_capsule("thread", &wanted)
            .unwrap()
            .is_some());
        let mut incompatible = wanted.clone();
        incompatible.policy_sha256 = "policy-two".into();
        assert!(store
            .load_document_capsule("thread", &incompatible)
            .unwrap()
            .is_none());
        incompatible = wanted.clone();
        incompatible.pipeline_version = "obsolete".into();
        assert!(store
            .load_document_capsule("thread", &incompatible)
            .unwrap()
            .is_none());
        let mut revised = wanted.clone();
        revised.source_sha256 = "f".repeat(64);
        revised.summary = "Revised approved ending".into();
        store.save_document_capsule("thread", &revised).unwrap();
        let selected = store
            .document_context("thread", "Resume story-0.md", 4000, &[])
            .unwrap();
        assert!(selected[0].text.contains("Revised approved ending"));
        assert!(!selected
            .iter()
            .any(|t| t.text.contains("Complete story 0:")));
        let excluded = store
            .document_context(
                "thread",
                "Resume story-0.md",
                4000,
                &["/staged/0/story-0.md".into()],
            )
            .unwrap();
        assert!(!excluded
            .iter()
            .any(|t| t.text.contains("Revised approved ending")));
        // Distinct uploaded documents can have the same basename.
        let mut distinct = revised;
        distinct.source_path = "/staged/other/story-0.md".into();
        distinct.summary = "A separate story".into();
        store.save_document_capsule("thread", &distinct).unwrap();
        let selected = store
            .document_context("thread", "Resume story-0.md", 10000, &[])
            .unwrap();
        assert!(selected
            .iter()
            .any(|t| t.text.contains("Revised approved ending")));
        assert!(selected.iter().any(|t| t.text.contains("A separate story")));
        let compare = store
            .document_context(
                "thread",
                "Compare story-0.md",
                10000,
                &["/staged/other/story-0.md".into()],
            )
            .unwrap();
        assert!(
            compare
                .iter()
                .any(|t| t.text.contains("Revised approved ending")),
            "new attachment must not hide a distinct older same-name source"
        );
        store.delete_thread("thread").unwrap();
        assert!(store
            .document_context("thread", "story-0.md", 10000, &[])
            .unwrap()
            .is_empty());
        assert!(store
            .load_document_capsule("thread", &wanted)
            .unwrap()
            .is_none());
    }
}

#[cfg(test)]
mod history_after_tests {
    use super::*;

    /// The goal check's paged read: only items after the position, at most
    /// `limit` per page, oldest first, each with its position.
    #[test]
    fn history_after_pages_forward_from_a_position() {
        let dir = tempfile::tempdir().unwrap();
        let store = TranscriptStore::open(&dir.path().join("sessions.sqlite3")).unwrap();
        for text in ["one", "two", "three", "four"] {
            store.append_user("chat", "run", text).unwrap();
        }
        store.append_user("other", "run", "elsewhere").unwrap();
        let all = store.history_after("chat", 0, 100).unwrap();
        let texts = |items: &[(i64, HistoryItem)]| {
            items
                .iter()
                .map(|(_, item)| match item {
                    HistoryItem::User { text, .. } => text.clone(),
                    _ => String::new(),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(texts(&all), ["one", "two", "three", "four"]);
        assert!(all.windows(2).all(|pair| pair[0].0 < pair[1].0));
        let after_second = store.history_after("chat", all[1].0, 100).unwrap();
        assert_eq!(texts(&after_second), ["three", "four"]);
        let first_page = store.history_after("chat", 0, 2).unwrap();
        assert_eq!(texts(&first_page), ["one", "two"]);
        let next_page = store.history_after("chat", first_page[1].0, 2).unwrap();
        assert_eq!(texts(&next_page), ["three", "four"]);
        assert!(store
            .history_after("chat", all[3].0, 100)
            .unwrap()
            .is_empty());
    }
}

#[cfg(test)]
mod reply_tests {
    use super::reply_before;

    /// The approved plan is the whole reply before the person's message, not its closing
    /// paragraph alone.
    #[test]
    fn the_reply_before_the_latest_message_is_whole() {
        // Newest first: the person's approval, then the plan reply's parts, then older turns.
        let rows = [
            (false, "Approved, make it a goal."),
            (true, "This turn only delivered the plan."),
            (true, ""),
            (true, "## Plan\n1. Waves\n2. HUD"),
            (false, "Plan an FPS."),
            (true, "Older reply."),
        ];
        assert_eq!(
            reply_before(rows.into_iter()),
            "## Plan\n1. Waves\n2. HUD\n\nThis turn only delivered the plan."
        );
        assert_eq!(reply_before([(false, "hi")].into_iter()), "");
        assert_eq!(reply_before(std::iter::empty()), "");
        // No message after it: the latest reply.
        assert_eq!(
            reply_before([(true, "b"), (true, "a"), (false, "q")].into_iter()),
            "a\n\nb"
        );
    }
}

#[cfg(test)]
mod outcome_tests {
    use super::*;

    fn end(store: &TranscriptStore, thread: &str, event: ThreadEvent) {
        store
            .append_event(&ThreadEventEnvelope {
                thread_id: thread.into(),
                run_id: "run".into(),
                event,
            })
            .unwrap();
    }

    #[test]
    fn last_outcome_reads_how_the_latest_run_ended() {
        let directory = tempfile::tempdir().unwrap();
        let store = TranscriptStore::open(&directory.path().join("t.sqlite")).unwrap();
        assert_eq!(store.last_outcome("chat").unwrap(), None);
        store.append_user("chat", "run", "hello").unwrap();
        assert_eq!(store.last_outcome("chat").unwrap(), None);
        end(
            &store,
            "chat",
            ThreadEvent::Failed {
                error: "Stopped by you.".into(),
            },
        );
        assert_eq!(store.last_outcome("chat").unwrap(), Some("stopped"));
        end(
            &store,
            "chat",
            ThreadEvent::Failed {
                error: "network".into(),
            },
        );
        assert_eq!(store.last_outcome("chat").unwrap(), Some("failed"));
        end(
            &store,
            "chat",
            ThreadEvent::Incomplete {
                model: None,
                result: "partly".into(),
            },
        );
        assert_eq!(store.last_outcome("chat").unwrap(), Some("incomplete"));
        end(
            &store,
            "chat",
            ThreadEvent::Finished {
                model: None,
                result: "done".into(),
            },
        );
        // Rows after the end (such as a later memory note) do not hide it.
        store.append_user("chat", "run2", "next").unwrap();
        assert_eq!(store.last_outcome("chat").unwrap(), Some("done"));
        assert_eq!(store.last_outcome("other").unwrap(), None);
    }

    #[test]
    fn last_outcome_reads_a_bounded_tail() {
        // Cost stays flat however long the chat: only the newest OUTCOME_ROWS rows are read.
        let directory = tempfile::tempdir().unwrap();
        let store = TranscriptStore::open(&directory.path().join("t.sqlite")).unwrap();
        end(
            &store,
            "chat",
            ThreadEvent::Finished {
                model: None,
                result: "old".into(),
            },
        );
        for index in 0..OUTCOME_ROWS {
            store
                .append_user("chat", "run", &format!("row {index}"))
                .unwrap();
        }
        assert_eq!(store.last_outcome("chat").unwrap(), None);
    }
}
