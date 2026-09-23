//! Durable per-thread conversation and run event log.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
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
        connection.execute_batch("CREATE TABLE IF NOT EXISTS context_turns (seq INTEGER PRIMARY KEY, thread_id TEXT NOT NULL, role TEXT NOT NULL, text TEXT NOT NULL); CREATE INDEX IF NOT EXISTS context_turns_thread ON context_turns(thread_id, seq); CREATE TABLE IF NOT EXISTS context_checkpoints (thread_id TEXT PRIMARY KEY, through_seq INTEGER NOT NULL, summary TEXT NOT NULL);").map_err(|e| format!("initialize context: {e}"))?;
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
        Ok(Self {
            connection: Mutex::new(connection),
        })
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
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        let checkpoint: Option<(i64, String)> = connection
            .query_row(
                "SELECT through_seq,summary FROM context_checkpoints WHERE thread_id=?1",
                [thread_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("read context checkpoint: {e}"))?;
        let through_seq = checkpoint.as_ref().map_or(0, |checkpoint| checkpoint.0);
        let mut statement = connection
            .prepare(
                "SELECT role,text FROM context_turns WHERE thread_id=?1 AND seq>?2 ORDER BY seq",
            )
            .map_err(|e| format!("read context: {e}"))?;
        let rows = statement
            .query_map(params![thread_id, through_seq], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("read context: {e}"))?;
        let mut turns = Vec::new();
        if let Some((_, summary)) = checkpoint {
            turns.push(ConversationTurn {
                role: ConversationRole::Assistant,
                text: format!("Earlier context checkpoint:\n{summary}"),
            });
        }
        for row in rows {
            let (role, text) = row.map_err(|e| format!("read context: {e}"))?;
            turns.push(ConversationTurn {
                role: if role == "user" {
                    ConversationRole::User
                } else {
                    ConversationRole::Assistant
                },
                text,
            });
        }
        let skip = turns.len().saturating_sub(max_messages);
        Ok(turns.into_iter().skip(skip).collect())
    }

    pub fn delete_thread(&self, thread_id: &str) -> Result<(), String> {
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
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

fn role_name(role: &ConversationRole) -> &'static str {
    match role {
        ConversationRole::User => "user",
        ConversationRole::Assistant => "assistant",
    }
}

fn context_turn(item: &HistoryItem) -> Option<ConversationTurn> {
    match item {
        HistoryItem::User { text, .. } => Some(ConversationTurn {
            role: ConversationRole::User,
            text: text.clone(),
        }),
        HistoryItem::Event {
            envelope:
                ThreadEventEnvelope {
                    event: ThreadEvent::Finished { result },
                    ..
                },
        } => Some(ConversationTurn {
            role: ConversationRole::Assistant,
            text: result.clone(),
        }),
        HistoryItem::Legacy { message } => {
            let role = match message.get("role")?.as_str()? {
                "user" => ConversationRole::User,
                "assistant" if message.get("final")?.as_bool()? => ConversationRole::Assistant,
                _ => return None,
            };
            Some(ConversationTurn {
                role,
                text: message.get("text")?.as_str()?.to_owned(),
            })
        }
        _ => None,
    }
}
