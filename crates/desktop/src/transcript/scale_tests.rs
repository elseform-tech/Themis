//! Scale tests for the transcript store. They count SQLite's own steps through its progress
//! hook, which sees every statement on the connection without any change to the code under test.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::history_pages::HistoryCursor;
use super::*;
use crate::scale::{growth_rate, measure};

const SIZES: [usize; 2] = [1000, 8000];
const THREAD: &str = "chat";

/// A chat of `size` turns, half typed and half steered. Durability is off only to fill it fast.
fn chat(size: usize) -> (tempfile::TempDir, TranscriptStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = TranscriptStore::open(&dir.path().join("sessions.sqlite3")).unwrap();
    store
        .connection
        .lock()
        .unwrap()
        .execute_batch("PRAGMA synchronous=OFF")
        .unwrap();
    for index in 0..size {
        let text = format!("turn {index}: {}", "words people actually write ".repeat(8));
        if index % 2 == 0 {
            store.append_user(THREAD, "run", &text).unwrap();
        } else {
            store
                .append_event(&ThreadEventEnvelope {
                    thread_id: THREAD.into(),
                    run_id: "run".into(),
                    event: ThreadEvent::Steered { text },
                })
                .unwrap();
        }
    }
    (dir, store)
}

/// A checkpoint at the end leaves only the recall search reading the old turns.
fn checkpoint(store: &TranscriptStore) {
    store
        .append_event(&ThreadEventEnvelope {
            thread_id: THREAD.into(),
            run_id: "run".into(),
            event: ThreadEvent::ContextCheckpoint {
                summary: "Summary with no topic facts.".into(),
            },
        })
        .unwrap();
}

/// What `work` returns, and how many steps SQLite took doing it.
fn sqlite_steps<T>(store: &TranscriptStore, work: impl FnOnce() -> T) -> (T, u64) {
    let steps = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&steps);
    store.connection.lock().unwrap().progress_handler(
        1,
        Some(move || {
            counter.fetch_add(1, Ordering::Relaxed);
            false
        }),
    );
    let result = work();
    store
        .connection
        .lock()
        .unwrap()
        .progress_handler(0, None::<fn() -> bool>);
    (result, steps.load(Ordering::Relaxed))
}

#[test]
fn attachment_lookup_stays_flat_as_a_chat_without_attachments_grows() {
    let samples = measure(&SIZES, |size| {
        let (_dir, store) = chat(size);
        let (paths, steps) = sqlite_steps(&store, || store.attachment_paths(THREAD).unwrap());
        assert!(paths.is_empty());
        steps
    });
    eprintln!("attachment_paths steps: {samples:?}");
    let rate = growth_rate(&samples);
    assert!(rate < 0.1, "growth {rate}: {samples:?}");
}

/// The list as it was once worked out from the raw history, kept here to check the stored one.
fn paths_from_raw_history(store: &TranscriptStore, thread_id: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for item in store.history(thread_id).unwrap() {
        if let HistoryItem::User { attachments, .. } = item {
            for path in attachments {
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
    }
    paths
}

/// Empties the stored list and marks the store as made before it existed.
fn make_store_older(path: &Path) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch("DROP TABLE attachment_paths; PRAGMA user_version=1;")
        .unwrap();
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(24))]

    /// One pass over a chat finds exactly what one SQLite search per term finds.
    #[test]
    fn recall_matches_what_sqlite_finds_term_by_term(
        turns in proptest::collection::vec("[aAbBéÉßİ\\x{0} ]{0,12}", 0..16),
        terms in proptest::collection::vec("[abéßi̇]{1,3}", 1..5),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = TranscriptStore::open(&dir.path().join("sessions.sqlite3")).unwrap();
        for text in &turns {
            store.append_user(THREAD, "run", text).unwrap();
        }
        store.append_user("other", "run", "ab").unwrap();
        let connection = store.connection.lock().unwrap();
        let found = newest_turns_with_terms(&connection, THREAD, &terms).unwrap();
        for (term, seqs) in terms.iter().zip(found) {
            let mut statement = connection.prepare("SELECT seq FROM context_turns WHERE thread_id=?1 AND instr(lower(text),?2)>0 ORDER BY seq DESC LIMIT 4").unwrap();
            let expected: Vec<i64> = statement
                .query_map(params![THREAD, term], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            proptest::prop_assert_eq!(seqs, expected);
        }
    }

    /// The stored list says what the raw history says, for new and for migrated stores.
    #[test]
    fn stored_attachment_paths_match_the_raw_history(
        messages in proptest::collection::vec(
            (proptest::bool::ANY, proptest::collection::vec("[ab]/[xyz]\\.txt", 0..4)),
            0..10,
        ),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        let store = TranscriptStore::open(&path).unwrap();
        for (first_chat, attachments) in messages {
            let thread = if first_chat { "one" } else { "two" };
            // A message may name the key without having attachments of its own.
            store.append_user(thread, "run", "the \"attachments\" key").unwrap();
            store.append_user_with_attachments(thread, "run", "look", attachments).unwrap();
        }
        let expected = ["one", "two"].map(|thread| paths_from_raw_history(&store, thread));
        proptest::prop_assert_eq!(store.attachment_paths("one").unwrap(), expected[0].clone());
        proptest::prop_assert_eq!(store.attachment_paths("two").unwrap(), expected[1].clone());
        drop(store);
        make_store_older(&path);
        let store = TranscriptStore::open(&path).unwrap();
        proptest::prop_assert_eq!(store.attachment_paths("one").unwrap(), expected[0].clone());
        proptest::prop_assert_eq!(store.attachment_paths("two").unwrap(), expected[1].clone());
    }
}

#[test]
fn attachment_migration_runs_once_and_leaves_a_store_the_doctor_accepts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    let store = TranscriptStore::open(&path).unwrap();
    store
        .append_user_with_attachments(THREAD, "run", "look", vec!["a.txt".into()])
        .unwrap();
    drop(store);
    make_store_older(&path);
    let ids = std::collections::HashSet::from([THREAD.to_owned()]);
    // The doctor never migrates, so a store not yet opened by this version stays valid to it.
    let older = TranscriptStore::diagnose_read_only(&path, true, Some(&ids));
    assert!(
        older
            .iter()
            .all(|check| check.status != crate::doctor::Status::Fail),
        "{older:?}"
    );
    let store = TranscriptStore::open(&path).unwrap();
    assert_eq!(store.attachment_paths(THREAD).unwrap(), ["a.txt"]);
    drop(store);
    let current = TranscriptStore::diagnose_read_only(&path, true, Some(&ids));
    assert!(
        current
            .iter()
            .all(|check| check.status != crate::doctor::Status::Fail),
        "{current:?}"
    );
    let store = TranscriptStore::open(&path).unwrap();
    store
        .append_user_with_attachments(THREAD, "run", "again", vec!["a.txt".into(), "b.txt".into()])
        .unwrap();
    drop(store);
    let store = TranscriptStore::open(&path).unwrap();
    assert_eq!(store.attachment_paths(THREAD).unwrap(), ["a.txt", "b.txt"]);
    store.delete_thread(THREAD).unwrap();
    assert!(store.attachment_paths(THREAD).unwrap().is_empty());
}

/// Words no turn contains, the case where every term reads the whole chat.
fn terms_matching_nothing(count: usize) -> String {
    let words: Vec<String> = (0..count)
        .map(|index| format!("zqx{}", char::from(b'a' + index as u8)))
        .collect();
    let request = words.join(" ");
    assert_eq!(
        themis_core::social::content_terms(&request.to_lowercase()).len(),
        count
    );
    request
}

fn recall_steps(store: &TranscriptStore, request: &str) -> u64 {
    let (_, steps) = sqlite_steps(store, || {
        store.context_for_request(THREAD, request, 8000).unwrap()
    });
    steps
}

#[test]
fn recall_reads_a_chat_once_however_many_terms_a_request_has() {
    let samples = measure(&[SIZES[1]], |size| {
        let (_dir, store) = chat(size);
        checkpoint(&store);
        let one = recall_steps(&store, &terms_matching_nothing(1));
        let twelve = recall_steps(&store, &terms_matching_nothing(12));
        eprintln!("recall steps at {size}: one term {one}, twelve terms {twelve}");
        assert!(twelve < one * 2, "one term {one}, twelve terms {twelve}");
        twelve
    });
    assert_eq!(samples.len(), 1);
}

#[test]
fn recall_stays_linear_as_a_chat_grows() {
    let samples = measure(&SIZES, |size| {
        let (_dir, store) = chat(size);
        checkpoint(&store);
        recall_steps(&store, &terms_matching_nothing(12))
    });
    eprintln!("recall steps: {samples:?}");
    let rate = growth_rate(&samples);
    assert!(rate < 1.2, "growth {rate}: {samples:?}");
}

#[test]
fn one_history_page_costs_the_same_however_long_the_chat_is() {
    let samples = measure(&SIZES, |size| {
        let (_dir, store) = chat(size);
        let (page, first) = sqlite_steps(&store, || {
            store
                .history_page(THREAD, HistoryCursor::default(), None, 64 * 1024)
                .unwrap()
        });
        let middle = HistoryCursor {
            after_seq: (size / 2) as i64,
            offset: 0,
        };
        let (_, later) = sqlite_steps(&store, || {
            store
                .history_page(THREAD, middle, Some(page.through_seq), 64 * 1024)
                .unwrap()
        });
        eprintln!("history page steps at {size}: first {first}, later {later}");
        first.max(later)
    });
    let rate = growth_rate(&samples);
    assert!(rate < 0.1, "growth {rate}: {samples:?}");
}
