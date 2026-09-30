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

    /// Uses the live connection without opening, migrating, or writing a store.
    pub fn diagnostics(
        &self,
        deep: bool,
        thread_ids: &std::collections::HashSet<String>,
    ) -> Vec<crate::doctor::Check> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        session_checks(&connection, deep, Some(thread_ids))
    }

    pub fn diagnose_read_only(
        path: &Path,
        deep: bool,
        thread_ids: Option<&std::collections::HashSet<String>>,
    ) -> Vec<crate::doctor::Check> {
        use crate::doctor::{Check, Status};
        if !path.exists() {
            return vec![Check::new(
                "sessions",
                Status::Warn,
                "Session database is missing; no database was created",
                "Expected before first use; recover from backup if history existed",
            )];
        }
        match Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(connection) => {
                let _ = connection.busy_timeout(std::time::Duration::from_millis(500));
                session_checks(&connection, deep, thread_ids)
            }
            Err(_) => vec![Check::new(
                "sessions",
                Status::Fail,
                "Session database cannot be opened read-only",
                "Back up the database and check permissions or recover it",
            )],
        }
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
            let first: Option<(i64, String)> = connection
                .query_row(
                    "SELECT seq,text FROM context_turns WHERE thread_id=?1 AND seq<=?2 AND role='user' ORDER BY seq LIMIT 1",
                    params![thread_id, through_seq],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| format!("read first user request: {e}"))?;
            let mut statement = connection
                .prepare("SELECT seq,text FROM context_turns WHERE thread_id=?1 AND seq<=?2 AND role='user' ORDER BY seq DESC LIMIT 2")
                .map_err(|e| format!("read checkpoint user requests: {e}"))?;
            let mut requests: Vec<(i64, String)> = statement
                .query_map(params![thread_id, through_seq], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .map_err(|e| format!("read checkpoint user requests: {e}"))?
                .collect::<Result<_, _>>()
                .map_err(|e| format!("read checkpoint user requests: {e}"))?;
            requests.extend(first);
            requests.sort_by_key(|request| request.0);
            requests.dedup_by_key(|request| request.0);
            turns.push(ConversationTurn {
                role: ConversationRole::Assistant,
                text: format!(
                    "Earlier context checkpoint (historical; later user requests take precedence):\n{summary}\nVerbatim first and recent user requests (historical):\n{}",
                    requests.into_iter().map(|request| request.1).collect::<Vec<_>>().join("\n")
                ),
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

fn session_checks(
    connection: &Connection,
    deep: bool,
    thread_ids: Option<&std::collections::HashSet<String>>,
) -> Vec<crate::doctor::Check> {
    use crate::doctor::{Check, Status};
    let result = (|| -> Result<Vec<Check>, String> {
        let transaction = connection
            .unchecked_transaction()
            .map_err(|_| "Cannot acquire a read snapshot")?;
        let mut checks = Vec::new();
        let version: i64 = transaction
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|_| "Cannot read schema version")?;
        if version != 1 {
            return Err(format!(
                "Unsupported schema version {version}; migration was not attempted"
            ));
        }
        for sql in [
            "SELECT seq,thread_id,item FROM history LIMIT 0",
            "SELECT seq,thread_id,role,text FROM context_turns LIMIT 0",
            "SELECT thread_id,through_seq,summary FROM context_checkpoints LIMIT 0",
        ] {
            transaction
                .prepare(sql)
                .map_err(|_| "Session schema is incomplete")?;
        }
        let integrity = if deep {
            "PRAGMA integrity_check"
        } else {
            "PRAGMA quick_check"
        };
        let mut statement = transaction
            .prepare(integrity)
            .map_err(|_| "Cannot check database integrity")?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| "Cannot check database integrity")?;
        for row in rows {
            if row.map_err(|_| "Cannot read integrity result")? != "ok" {
                return Err("SQLite detected integrity violations".to_owned());
            }
        }
        checks.push(Check::new(
            "sqlite_integrity",
            Status::Pass,
            if deep {
                "Full integrity_check passed; schema version 1"
            } else {
                "quick_check passed; schema version 1"
            },
            "",
        ));
        let invalid_context: i64 = transaction.query_row("SELECT COUNT(*) FROM context_turns c LEFT JOIN history h ON h.seq=c.seq AND h.thread_id=c.thread_id WHERE h.seq IS NULL OR c.role NOT IN ('user','assistant')", [], |row| row.get(0)).map_err(|_| "Cannot check context references")?;
        if invalid_context != 0 {
            return Err(format!(
                "{invalid_context} invalid or orphaned context rows"
            ));
        }
        let mut statement = transaction
            .prepare("SELECT seq,thread_id,item FROM history ORDER BY seq")
            .map_err(|_| "Cannot read history")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|_| "Cannot read history")?;
        let mut checkpoints = std::collections::HashMap::new();
        let mut count = 0;
        let mut orphan_threads = std::collections::HashSet::new();
        // ponytail: scan persisted rows without retaining transcript contents; add incremental validation if stores become large.
        for row in rows {
            let (seq, id, json) = row.map_err(|_| "Cannot decode history row")?;
            let item: HistoryItem = serde_json::from_str(&json)
                .map_err(|_| "Invalid history JSON; contents omitted")?;
            if let HistoryItem::Event { envelope } = &item {
                if envelope.thread_id != id {
                    return Err("History event refers to a different thread".to_owned());
                }
            }
            let context: Option<(String, String)> = transaction
                .query_row(
                    "SELECT role,text FROM context_turns WHERE seq=?1 AND thread_id=?2",
                    params![seq, id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|_| "Cannot compare saved context")?;
            if let HistoryItem::Event {
                envelope:
                    ThreadEventEnvelope {
                        event: ThreadEvent::ContextCheckpoint { summary },
                        ..
                    },
            } = &item
            {
                checkpoints.insert(id.clone(), (seq, summary.clone()));
            }
            let expected =
                context_turn(&item).map(|turn| (role_name(&turn.role).to_owned(), turn.text));
            if context != expected {
                return Err("Persisted context differs from conversation history".to_owned());
            }
            if thread_ids.is_some_and(|ids| !ids.contains(&id)) {
                orphan_threads.insert(id);
            }
            count += 1;
        }
        let mut statement = transaction.prepare("SELECT c.thread_id,c.summary,h.item,c.through_seq FROM context_checkpoints c LEFT JOIN history h ON h.seq=c.through_seq AND h.thread_id=c.thread_id").map_err(|_| "Cannot read checkpoints")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|_| "Cannot read checkpoints")?;
        for row in rows {
            let (id, summary, json, seq) = row.map_err(|_| "Cannot decode checkpoint")?;
            if checkpoints.remove(&id) != Some((seq, summary.clone())) {
                return Err("Checkpoint is stale or differs from recorded history".to_owned());
            }
            let item: HistoryItem =
                serde_json::from_str(&json.ok_or("Checkpoint refers to missing history")?)
                    .map_err(|_| "Invalid checkpoint history JSON")?;
            if !matches!(item, HistoryItem::Event { envelope: ThreadEventEnvelope { thread_id, event: ThreadEvent::ContextCheckpoint { summary: saved }, .. } } if thread_id == id && saved == summary)
            {
                return Err("Checkpoint differs from its recorded history event".to_owned());
            }
        }
        if !checkpoints.is_empty() {
            return Err("Recorded checkpoints are missing from saved context".to_owned());
        }
        checks.push(Check::new(
            "session_consistency",
            Status::Pass,
            format!("{count} history rows decoded; context and checkpoints match"),
            "",
        ));
        if !orphan_threads.is_empty() {
            checks.push(Check::new(
                "thread_history_references",
                Status::Warn,
                format!(
                    "{} history thread identifiers are absent from the registry snapshot",
                    orphan_threads.len()
                ),
                "Review recovery notes and rerun when no threads are being modified",
            ));
        }
        if thread_ids.is_none() {
            checks.push(Check::new(
                "thread_history_references",
                Status::Unverified,
                "Registry unavailable; thread references could not be checked",
                "Recover the thread registry",
            ));
        }
        Ok(checks)
    })();
    result.unwrap_or_else(|detail| vec![Check::new("sessions", Status::Fail, detail, "Back up the data directory and recover the store; no repair or migration was attempted")])
}
