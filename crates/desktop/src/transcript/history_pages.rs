//! Exact JSONL transport of durable history, including entries larger than RPC.
use super::*;

pub const MAX_PAGE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HistoryCursor {
    /// Last completely delivered row. Offsets address the next row in this chat.
    pub after_seq: i64,
    pub offset: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryPage {
    pub jsonl: String,
    pub through_seq: i64,
    pub next_cursor: Option<HistoryCursor>,
}

impl TranscriptStore {
    /// Bounded SQLite reads, not an in-memory slice of the complete transcript.
    /// A fixed watermark excludes concurrent appends until the next snapshot.
    pub fn history_page(
        &self,
        thread_id: &str,
        cursor: HistoryCursor,
        through_seq: Option<i64>,
        max_bytes: usize,
    ) -> Result<HistoryPage, String> {
        if !(16..=MAX_PAGE_BYTES).contains(&max_bytes)
            || cursor.after_seq < 0
            || cursor.offset > i32::MAX as usize
            || through_seq.is_some_and(|seq| seq < cursor.after_seq)
        {
            return Err("Invalid history page cursor or byte limit".into());
        }
        let connection = self.connection.lock().unwrap_or_else(|e| e.into_inner());
        let through_seq = through_seq
            .map(Ok)
            .unwrap_or_else(|| {
                connection.query_row(
                    "SELECT COALESCE(MAX(seq),0) FROM history WHERE thread_id=?1",
                    [thread_id],
                    |row| row.get(0),
                )
            })
            .map_err(|e| format!("read history watermark: {e}"))?;
        let mut cursor = cursor;
        let mut jsonl = String::new();
        let mut statement = connection.prepare(
            "SELECT seq FROM history WHERE thread_id=?1 AND seq>?2 AND seq<=?3 ORDER BY seq LIMIT 1"
        ).map_err(|e| format!("read history page: {e}"))?;
        for _ in 0..128 {
            let room = max_bytes - jsonl.len();
            if room < 5 {
                break;
            }
            let row: Option<i64> = statement
                .query_row(params![thread_id, cursor.after_seq, through_seq], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|e| format!("read history page: {e}"))?;
            let Some(seq) = row else {
                if cursor.offset != 0 {
                    return Err("History page cursor refers to a missing entry".into());
                }
                break;
            };
            // SQLite incremental I/O supports the existing TEXT column. SQL
            // substr/CAST would materialize the whole entry on every page.
            let blob = connection
                .blob_open(rusqlite::DatabaseName::Main, "history", "item", seq, true)
                .map_err(|e| format!("open history entry: {e}"))?;
            let total = blob.len();
            if cursor.offset >= total {
                return Err("History page offset exceeds entry".into());
            }
            let mut bytes = vec![0; (room - 1).min(total - cursor.offset)];
            blob.read_at_exact(&mut bytes, cursor.offset)
                .map_err(|e| format!("read history fragment: {e}"))?;
            let text = match std::str::from_utf8(&bytes) {
                Ok(text) => text,
                Err(error) if error.error_len().is_none() && error.valid_up_to() > 0 => {
                    std::str::from_utf8(&bytes[..error.valid_up_to()])
                        .map_err(|_| "Invalid history UTF-8")?
                }
                Err(_) => return Err("History page offset is not a UTF-8 boundary".into()),
            };
            jsonl.push_str(text);
            cursor.offset += text.len();
            if cursor.offset == total {
                jsonl.push('\n');
                cursor = HistoryCursor {
                    after_seq: seq,
                    offset: 0,
                };
            } else {
                break;
            }
        }
        let more: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM history WHERE thread_id=?1 AND seq>?2 AND seq<=?3)",
                params![thread_id, cursor.after_seq, through_seq],
                |row| row.get(0),
            )
            .map_err(|e| format!("read history continuation: {e}"))?;
        Ok(HistoryPage {
            jsonl,
            through_seq,
            next_cursor: more.then_some(cursor),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_pages_preserve_unicode_snapshot_and_restart_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        let store = TranscriptStore::open(&path).unwrap();
        let text = "é🌺\n\\\"".repeat(100);
        store.append_user("chat", "first", &text).unwrap();
        store.append_user("other", "other", "private").unwrap();
        store.append_user("chat", "second", "second").unwrap();
        let first = store
            .history_page("chat", HistoryCursor::default(), None, 17)
            .unwrap();
        let watermark = first.through_seq;
        store
            .append_user("chat", "later", "excluded from snapshot")
            .unwrap();
        drop(store);
        let store = TranscriptStore::open(&path).unwrap();
        let mut cursor = first.next_cursor.unwrap();
        let mut jsonl = first.jsonl;
        for _ in 0..1000 {
            let page = store
                .history_page("chat", cursor, Some(watermark), 17)
                .unwrap();
            assert!(page.jsonl.len() <= 17);
            assert_eq!(page.through_seq, watermark);
            jsonl.push_str(&page.jsonl);
            match page.next_cursor {
                Some(next) => {
                    assert!(next > cursor);
                    cursor = next;
                }
                None => break,
            }
        }
        let values: Vec<serde_json::Value> = jsonl
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["text"], text);
        assert_eq!(values[1]["text"], "second");
        let next = store
            .history_page(
                "chat",
                HistoryCursor {
                    after_seq: watermark,
                    offset: 0,
                },
                None,
                1024,
            )
            .unwrap();
        assert!(next.next_cursor.is_none());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(next.jsonl.trim()).unwrap()["text"],
            "excluded from snapshot"
        );
    }

    #[test]
    fn history_pages_bound_rows_wire_expansion_and_reject_invalid_cursors() {
        let dir = tempfile::tempdir().unwrap();
        let store = TranscriptStore::open(&dir.path().join("sessions.sqlite3")).unwrap();
        assert_eq!(
            store
                .history_page("empty", HistoryCursor::default(), None, 16)
                .unwrap()
                .jsonl,
            ""
        );
        for _ in 0..150 {
            store.append_user("many", "run", "x").unwrap();
        }
        let page = store
            .history_page("many", HistoryCursor::default(), None, MAX_PAGE_BYTES)
            .unwrap();
        assert_eq!(page.jsonl.lines().count(), 128);
        let tail = store
            .history_page(
                "many",
                page.next_cursor.unwrap(),
                Some(page.through_seq),
                MAX_PAGE_BYTES,
            )
            .unwrap();
        assert_eq!(tail.jsonl.lines().count(), 22);
        assert!(tail.next_cursor.is_none());
        store
            .append_user("escaped", "run", &"\u{1}🌺\\\"".repeat(200_000))
            .unwrap();
        let page = store
            .history_page("escaped", HistoryCursor::default(), None, MAX_PAGE_BYTES)
            .unwrap();
        assert!(page.jsonl.len() <= MAX_PAGE_BYTES);
        assert!(
            serde_json::to_vec(&serde_json::json!({"ok":true,"value":page,"error":null}))
                .unwrap()
                .len()
                < crate::server::MAX_FRAME
        );
        for (cursor, bytes) in [
            (HistoryCursor::default(), 15),
            (HistoryCursor::default(), MAX_PAGE_BYTES + 1),
            (
                HistoryCursor {
                    after_seq: -1,
                    offset: 0,
                },
                16,
            ),
            (
                HistoryCursor {
                    after_seq: 0,
                    offset: usize::MAX,
                },
                16,
            ),
            (
                HistoryCursor {
                    after_seq: 0,
                    offset: 999_999,
                },
                16,
            ),
        ] {
            assert!(store.history_page("many", cursor, None, bytes).is_err());
        }
        // The supplied offset lands inside the multibyte code point.
        let row: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT item FROM history WHERE thread_id='escaped'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let offset = row.find('🌺').unwrap() + 1;
        assert!(store
            .history_page(
                "escaped",
                HistoryCursor {
                    after_seq: 0,
                    offset
                },
                None,
                16
            )
            .is_err());
    }

    #[test]
    fn oversized_entry_page_does_not_materialize_the_whole_sqlite_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        let store = TranscriptStore::open(&path).unwrap();
        store
            .append_user("large", "run", &"x".repeat(8 * 1024 * 1024))
            .unwrap();
        drop(store);
        let store = TranscriptStore::open(&path).unwrap();
        {
            let connection = store.connection.lock().unwrap();
            // Reject any SQL value materialized above 4 KiB in this isolated
            // connection. Existing large entries must remain readable by chunk.
            unsafe {
                rusqlite::ffi::sqlite3_limit(
                    connection.handle(),
                    rusqlite::ffi::SQLITE_LIMIT_LENGTH,
                    4096,
                );
            }
        }
        let page = store
            .history_page("large", HistoryCursor::default(), None, 128)
            .expect("A bounded history page must not load the full oversized SQLite value");
        assert!(page.jsonl.len() <= 128);
        assert!(page.next_cursor.unwrap().offset > 0);
    }
}
