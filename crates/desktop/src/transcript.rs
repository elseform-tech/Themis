//! Durable per-thread conversation and run event log.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use themis_core::runtime::{ConversationRole, ConversationTurn};

use crate::types::{ThreadEvent, ThreadEventEnvelope};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryItem {
    User { run_id: String, text: String },
    Event { envelope: ThreadEventEnvelope },
    Legacy { message: serde_json::Value },
}

pub struct TranscriptStore {
    connection: Mutex<Connection>,
}

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
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn append(&self, thread_id: &str, item: &HistoryItem) -> Result<(), String> {
        let item = serde_json::to_string(item).map_err(|e| format!("encode session item: {e}"))?;
        self.connection
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .execute(
                "INSERT INTO history(thread_id, item) VALUES (?1, ?2)",
                params![thread_id, item],
            )
            .map_err(|e| format!("save session item: {e}"))?;
        Ok(())
    }

    pub fn append_user(&self, thread_id: &str, run_id: &str, text: &str) -> Result<(), String> {
        self.append(
            thread_id,
            &HistoryItem::User {
                run_id: run_id.to_owned(),
                text: text.to_owned(),
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
        }
        transaction
            .commit()
            .map_err(|e| format!("commit conversation: {e}"))
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

    pub fn context(
        &self,
        thread_id: &str,
        max_messages: usize,
    ) -> Result<Vec<ConversationTurn>, String> {
        let mut turns = Vec::new();
        for item in self.history(thread_id)? {
            match item {
                HistoryItem::User { text, .. } => turns.push(ConversationTurn {
                    role: ConversationRole::User,
                    text,
                }),
                HistoryItem::Event {
                    envelope:
                        ThreadEventEnvelope {
                            event: ThreadEvent::Finished { result },
                            ..
                        },
                } => {
                    turns.push(ConversationTurn {
                        role: ConversationRole::Assistant,
                        text: result,
                    });
                }
                HistoryItem::Legacy { message } => {
                    let role = match message.get("role").and_then(|value| value.as_str()) {
                        Some("user") => Some(ConversationRole::User),
                        Some("assistant")
                            if message.get("final").and_then(|value| value.as_bool())
                                == Some(true) =>
                        {
                            Some(ConversationRole::Assistant)
                        }
                        _ => None,
                    };
                    if let (Some(role), Some(text)) =
                        (role, message.get("text").and_then(|value| value.as_str()))
                    {
                        turns.push(ConversationTurn {
                            role,
                            text: text.to_owned(),
                        });
                    }
                }
                _ => {}
            }
        }
        let skip = turns.len().saturating_sub(max_messages);
        Ok(turns.into_iter().skip(skip).collect())
    }

    pub fn delete_thread(&self, thread_id: &str) -> Result<(), String> {
        self.connection
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .execute("DELETE FROM history WHERE thread_id = ?1", [thread_id])
            .map_err(|e| format!("delete session: {e}"))?;
        Ok(())
    }
}
