use std::io::Write;
use std::path::Path;

use themis_core::configuration::ApprovalMode;

use super::channel_log::{read_page, take_bytes_read, ChannelLine};
use super::*;
use crate::state::AppState;

fn state_with_channel(dir: &Path) -> (AppState, String) {
    let state = AppState::new_for_test(dir.join("settings.json"));
    let channel = state
        .channel_save(
            Channel {
                id: String::new(),
                name: "general".into(),
                topic: String::new(),
                project_root: dir.join("projects").join("docs"),
                approval_mode: ApprovalMode::Auto,
                members: Vec::new(),
                revision: String::new(),
            },
            None,
        )
        .unwrap();
    (state, channel.id)
}

fn person(text: &str) -> ChannelEntry {
    ChannelEntry::Person { text: text.into() }
}

fn seqs(page: &ChannelPage) -> Vec<u64> {
    page.entries.iter().map(|line| line.seq).collect()
}

#[test]
fn channel_pages_walk_back_by_seq_within_max_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (state, channel) = state_with_channel(dir.path());
    for index in 1..=10 {
        let seq = state
            .channel_append(&channel, person(&format!("message {index:02}")))
            .unwrap();
        assert_eq!(seq, index);
    }
    // The longest line written above, with its newline (at_ms has 13 digits until 2286).
    let one_line = serde_json::to_vec(&ChannelLine {
        seq: 10,
        at_ms: 1_700_000_000_000,
        entry: person("message 10"),
    })
    .unwrap()
    .len()
        + 1;

    // Newest page first, oldest entry first within the page.
    let page = state.channel_page(&channel, None, one_line * 3).unwrap();
    assert_eq!(seqs(&page), vec![8, 9, 10]);
    assert_eq!(page.next_before, Some(8));
    assert!(
        matches!(&page.entries[2].entry, ChannelEntry::Person { text } if text == "message 10")
    );

    let page = state
        .channel_page(&channel, page.next_before, one_line * 3)
        .unwrap();
    assert_eq!(seqs(&page), vec![5, 6, 7]);
    let mut walked = seqs(&page);
    let mut before = page.next_before;
    while let Some(seq) = before {
        let page = state
            .channel_page(&channel, Some(seq), one_line * 3)
            .unwrap();
        walked.splice(0..0, seqs(&page));
        before = page.next_before;
    }
    assert_eq!(walked, (1..=7).collect::<Vec<_>>());

    // A page always holds one entry, however small the limit.
    let page = state.channel_page(&channel, Some(4), 1).unwrap();
    assert_eq!(seqs(&page), vec![3]);
    assert_eq!(page.next_before, Some(3));
    let page = state.channel_page(&channel, Some(2), 1).unwrap();
    assert_eq!(seqs(&page), vec![1]);
    assert_eq!(page.next_before, None);

    // Sequence numbers carry on after a restart.
    drop(state);
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    assert_eq!(state.channel_append(&channel, person("again")).unwrap(), 11);
    let page = state.channel_page(&channel, None, usize::MAX).unwrap();
    assert_eq!(seqs(&page), (1..=11).collect::<Vec<_>>());
    assert_eq!(page.next_before, None);
}

#[test]
fn appends_refuse_unknown_channels_and_unsafe_ids() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = state_with_channel(dir.path());
    assert!(state.channel_append("missing", person("hi")).is_err());
    assert!(state.channel_append("../general", person("hi")).is_err());
    assert!(state.channel_page("../general", None, 1000).is_err());
    let empty = state.channel_page("missing", None, 1000).unwrap();
    assert!(empty.entries.is_empty() && empty.next_before.is_none());
}

#[test]
fn a_torn_last_line_is_skipped_and_the_next_append_starts_clean() {
    let dir = tempfile::tempdir().unwrap();
    let (state, channel) = state_with_channel(dir.path());
    state.channel_append(&channel, person("one")).unwrap();
    let path = dir
        .path()
        .join(VISIONS_DIR)
        .join(CHANNEL_LOGS_DIR)
        .join(format!("{channel}.jsonl"));
    drop(state);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(b"{\"seq\":2,\"at_m").unwrap();
    drop(file);

    let state = AppState::new_for_test(dir.path().join("settings.json"));
    assert_eq!(state.channel_append(&channel, person("two")).unwrap(), 2);
    let page = state.channel_page(&channel, None, usize::MAX).unwrap();
    assert_eq!(seqs(&page), vec![1, 2]);
}

/// Writes `count` entries straight to a log, as appends would.
fn write_log(path: &Path, count: usize) {
    let mut bytes = Vec::new();
    for seq in 1..=count as u64 {
        let line = ChannelLine {
            seq,
            at_ms: 1_700_000_000_000 + seq,
            entry: person(&format!("message number {seq} with some words in it")),
        };
        serde_json::to_writer(&mut bytes, &line).unwrap();
        bytes.push(b'\n');
    }
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn channel_page_reads_stay_bounded_by_page_not_log_size() {
    let samples = crate::scale::measure(&[1_000, 50_000], |size| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        write_log(&path, size);
        take_bytes_read();
        let newest = read_page(&path, None, 4096).unwrap();
        assert_eq!(
            newest.entries.last().map(|line| line.seq),
            Some(size as u64)
        );
        let middle = read_page(&path, Some(size as u64 / 2), 4096).unwrap();
        assert_eq!(
            middle.entries.last().map(|line| line.seq),
            Some(size as u64 / 2 - 1)
        );
        let oldest = read_page(&path, Some(5), 4096).unwrap();
        assert_eq!(seqs(&oldest), vec![1, 2, 3, 4]);
        take_bytes_read()
    });
    let rate = crate::scale::growth_rate(&samples);
    assert!(rate < 0.2, "page reads grew with the log: {samples:?}");
}
