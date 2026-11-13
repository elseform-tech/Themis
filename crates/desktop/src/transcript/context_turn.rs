//! Which saved history items the model sees as conversation turns.

use super::*;

pub(super) fn role_name(role: &ConversationRole) -> &'static str {
    match role {
        ConversationRole::User => "user",
        ConversationRole::Assistant | ConversationRole::Checkpoint => "assistant",
    }
}

pub(super) fn context_turn(item: &HistoryItem) -> Option<ConversationTurn> {
    match item {
        HistoryItem::Event {
            envelope:
                ThreadEventEnvelope {
                    event: ThreadEvent::ContextSuspended { progress },
                    ..
                },
        } => Some(ConversationTurn {
            role: ConversationRole::Assistant,
            text: progress.clone(),
        }),
        // A steered message is the user's words, in the order the model saw them.
        HistoryItem::Event {
            envelope:
                ThreadEventEnvelope {
                    event: ThreadEvent::Steered { text },
                    ..
                },
        } => Some(ConversationTurn {
            role: ConversationRole::User,
            text: text.clone(),
        }),
        HistoryItem::User { text, .. } => Some(ConversationTurn {
            role: ConversationRole::User,
            text: text.clone(),
        }),
        HistoryItem::Event {
            envelope:
                ThreadEventEnvelope {
                    event:
                        ThreadEvent::Finished { result, .. } | ThreadEvent::Incomplete { result, .. },
                    ..
                },
        } => Some(ConversationTurn {
            role: ConversationRole::Assistant,
            text: result.clone(),
        }),
        // Without it the next turn sees the stopped request unanswered and does it again.
        HistoryItem::Event {
            envelope:
                ThreadEventEnvelope {
                    event: ThreadEvent::Failed { error },
                    ..
                },
        } if error.starts_with(themis_core::runtime::STOPPED_BY_YOU) => Some(ConversationTurn {
            role: ConversationRole::Assistant,
            text: themis_core::prompts::RUNTIME_STOPPED_TURN.to_owned(),
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

/// Whether `item` ends a run that failed for any reason but the person's stop.
fn failed_reply(item: &HistoryItem) -> bool {
    matches!(
        item,
        HistoryItem::Event {
            envelope: ThreadEventEnvelope {
                event: ThreadEvent::Failed { error },
                ..
            },
        } if !error.starts_with(themis_core::runtime::STOPPED_BY_YOU)
    )
}

/// The turn saved for history item `seq`: its own turn, or for a failed run a note standing in
/// for the reply it never gave. The note follows only an unanswered user turn, so a failure
/// with no request of its own (a queued message that could not go out) adds nothing, and the
/// provider's error text never enters the conversation.
pub(super) fn stored_turn(
    connection: &Connection,
    thread_id: &str,
    seq: i64,
    item: &HistoryItem,
) -> Result<Option<ConversationTurn>, String> {
    if !failed_reply(item) {
        return Ok(context_turn(item));
    }
    let previous: Option<String> = connection
        .query_row(
            "SELECT role FROM context_turns WHERE thread_id=?1 AND seq<?2 ORDER BY seq DESC LIMIT 1",
            params![thread_id, seq],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("read context turn: {e}"))?;
    Ok(
        (previous.as_deref() == Some("user")).then(|| ConversationTurn {
            role: ConversationRole::Assistant,
            text: themis_core::prompts::RUNTIME_FAILED_TURN.to_owned(),
        }),
    )
}

/// Saves the conversation turn for history item `seq`, if it has one; `or_ignore` keeps a row
/// already there (migrations), otherwise a second row for `seq` is an error.
pub(super) fn save_stored_turn(
    connection: &Connection,
    thread_id: &str,
    seq: i64,
    item: &HistoryItem,
    or_ignore: bool,
) -> Result<(), String> {
    let Some(turn) = stored_turn(connection, thread_id, seq, item)? else {
        return Ok(());
    };
    let insert = if or_ignore {
        "INSERT OR IGNORE INTO context_turns(seq,thread_id,role,text) VALUES (?1,?2,?3,?4)"
    } else {
        "INSERT INTO context_turns(seq,thread_id,role,text) VALUES (?1,?2,?3,?4)"
    };
    connection
        .execute(
            insert,
            params![seq, thread_id, role_name(&turn.role), turn.text],
        )
        .map(|_| ())
        .map_err(|e| format!("save context turn: {e}"))
}

/// Runs `each` on every history row whose item matches the SQL `filter`, oldest first, so
/// each saved turn sees the turns before it.
pub(super) fn for_each_history_row(
    connection: &Connection,
    filter: &str,
    mut each: impl FnMut(i64, &str, &HistoryItem) -> Result<(), String>,
) -> Result<(), String> {
    let mut statement = connection
        .prepare("SELECT seq,thread_id,item FROM history WHERE item LIKE ?1 ORDER BY seq")
        .map_err(|e| format!("read history: {e}"))?;
    let mut rows = statement
        .query(params![filter])
        .map_err(|e| format!("read history: {e}"))?;
    while let Some(row) = rows.next().map_err(|e| format!("read history: {e}"))? {
        let seq: i64 = row.get(0).map_err(|e| format!("read history: {e}"))?;
        let thread_id: String = row.get(1).map_err(|e| format!("read history: {e}"))?;
        let json: String = row.get(2).map_err(|e| format!("read history: {e}"))?;
        let item: HistoryItem =
            serde_json::from_str(&json).map_err(|e| format!("decode session item: {e}"))?;
        each(seq, &thread_id, &item)?;
    }
    Ok(())
}

/// Saves the notes stores made before failed runs (and, before 2026-11-07, stopped runs) never
/// got. Rows already there stay.
pub(super) fn backfill_run_end_turns(connection: &Connection) -> Result<(), String> {
    for_each_history_row(connection, "%\"failed\"%", |seq, thread_id, item| {
        save_stored_turn(connection, thread_id, seq, item, true)
    })
}

/// The turn a store at schema `version` should hold for item `seq`. Stores before version 3
/// have no failed-run notes yet; an unreadable earlier turn expects nothing, so it shows as
/// differing context.
pub(super) fn expected_turn(
    connection: &Connection,
    version: i64,
    thread_id: &str,
    seq: i64,
    item: &HistoryItem,
) -> Option<ConversationTurn> {
    if version < 3 {
        return context_turn(item);
    }
    stored_turn(connection, thread_id, seq, item).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::Status;

    const CHAT: &str = "chat";

    fn failed(store: &TranscriptStore, error: &str) {
        store
            .append_event(&ThreadEventEnvelope {
                thread_id: CHAT.into(),
                run_id: "run".into(),
                event: ThreadEvent::Failed {
                    error: error.into(),
                },
            })
            .unwrap();
    }

    fn turns(store: &TranscriptStore) -> Vec<(String, String)> {
        let connection = store.connection.lock().unwrap();
        let mut statement = connection
            .prepare("SELECT role,text FROM context_turns WHERE thread_id=?1 ORDER BY seq")
            .unwrap();
        statement
            .query_map(params![CHAT], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn doctor_passes(path: &Path) {
        let ids = std::collections::HashSet::from([CHAT.to_owned()]);
        let checks = TranscriptStore::diagnose_read_only(path, true, Some(&ids));
        assert!(
            checks.iter().all(|check| check.status != Status::Fail),
            "{checks:?}"
        );
    }

    /// A failed run answers its request with the note; a failure with no request of its own
    /// (a queued message that could not go out) adds nothing; a stop keeps its own note.
    #[test]
    fn a_failure_after_a_request_leaves_the_note_and_only_then() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("t.sqlite");
        let store = TranscriptStore::open(&path).unwrap();
        store.append_user(CHAT, "run", "first").unwrap();
        failed(&store, "provider refused: secret details");
        failed(&store, "queued message not sent");
        store.append_user(CHAT, "run", "second").unwrap();
        failed(
            &store,
            &format!("{}.", themis_core::runtime::STOPPED_BY_YOU),
        );
        let failed_note = themis_core::prompts::RUNTIME_FAILED_TURN.to_owned();
        let stopped_note = themis_core::prompts::RUNTIME_STOPPED_TURN.to_owned();
        assert_eq!(
            turns(&store),
            [
                ("user".to_owned(), "first".to_owned()),
                ("assistant".to_owned(), failed_note),
                ("user".to_owned(), "second".to_owned()),
                ("assistant".to_owned(), stopped_note),
            ]
        );
        drop(store);
        doctor_passes(&path);
    }

    /// A store from before the notes gets them when this version opens it, and the doctor
    /// accepts it both before and after.
    #[test]
    fn opening_an_older_store_adds_the_missing_notes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("t.sqlite");
        let store = TranscriptStore::open(&path).unwrap();
        store.append_user(CHAT, "run", "first").unwrap();
        failed(&store, "network");
        store.append_user(CHAT, "run", "second").unwrap();
        failed(
            &store,
            &format!("{}.", themis_core::runtime::STOPPED_BY_YOU),
        );
        let current = turns(&store);
        drop(store);
        let make_older = |notes: &str| {
            let connection = Connection::open(&path).unwrap();
            let sql = format!(
                "DELETE FROM context_turns WHERE role='assistant' AND text LIKE '{notes}'; \
                 PRAGMA user_version=2;"
            );
            connection.execute_batch(&sql).unwrap();
        };
        // A version 2 store had stop notes (2026-11-07) but no failure notes.
        make_older("[This reply failed%");
        doctor_passes(&path);
        let store = TranscriptStore::open(&path).unwrap();
        assert_eq!(turns(&store), current);
        drop(store);
        doctor_passes(&path);

        // Stops saved before 2026-11-07 left no note either; the same step adds it.
        make_older("[%");
        let store = TranscriptStore::open(&path).unwrap();
        assert_eq!(turns(&store), current);
        drop(store);
        doctor_passes(&path);
    }
}
