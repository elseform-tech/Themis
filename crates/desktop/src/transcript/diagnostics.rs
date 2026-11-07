//! Session-store health checks for doctor and the diagnostics view.

use super::*;

impl TranscriptStore {
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
}

pub(super) fn session_checks(
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
        // Version 1 is a store this version of the app hasn't opened yet.
        if !(1..=2).contains(&version) {
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
        if version >= 2 {
            transaction
                .prepare("SELECT thread_id,path FROM attachment_paths LIMIT 0")
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
                format!("Full integrity_check passed; schema version {version}")
            } else {
                format!("quick_check passed; schema version {version}")
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
