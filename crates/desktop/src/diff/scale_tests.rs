//! Scale tests for change fingerprints: they count the bytes read, not the time taken.

use std::process::Command;

use super::*;
use crate::scale::{growth_rate, measure};

pub(crate) fn committed_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        assert!(Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    };
    git(&["init", "-q"]);
    std::fs::write(dir.path().join("kept.txt"), "one\n").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "user.email=t@t",
        "-c",
        "user.name=t",
        "commit",
        "-qm",
        "init",
    ]);
    dir
}

const MIB: usize = 1024 * 1024;

fn text_of(bytes: usize) -> Vec<u8> {
    b"some text in a line\n".repeat(bytes / 20)
}

#[test]
fn fingerprint_reads_do_not_grow_with_the_size_of_a_dirty_file() {
    let samples = measure(&[2, 16], |mib| {
        let repo = committed_repo();
        std::fs::write(repo.path().join("big.log"), text_of(mib * MIB)).unwrap();
        take_work();
        let fingerprint = change_fingerprint(repo.path());
        assert!(fingerprint.contains_key("big.log"));
        take_work().bytes
    });
    eprintln!("change_fingerprint bytes: {samples:?}");
    let rate = growth_rate(&samples);
    assert!(rate < 0.1, "growth {rate}: {samples:?}");
}

/// A dirty file `size` bytes long, replaced in place by `edit` with its modified time kept, and
/// whether a later capture tells the two apart.
fn edit_is_seen(size: usize, edit: impl FnOnce(&mut Vec<u8>)) -> bool {
    let repo = committed_repo();
    let path = repo.path().join("file.txt");
    let mut content = text_of(size);
    std::fs::write(&path, &content).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let before = change_fingerprint(repo.path());
    edit(&mut content);
    std::fs::write(&path, &content).unwrap();
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(modified).unwrap();
    drop(file);
    let after = change_fingerprint(repo.path());
    !changed_between(repo.path(), &before, &after).is_empty()
}

#[test]
fn a_small_file_is_compared_by_its_whole_content() {
    assert!(edit_is_seen(100 * 1024, |content| {
        let middle = content.len() / 2;
        content[middle] = b'X';
    }));
    assert!(!edit_is_seen(100 * 1024, |_| {}));
}

#[test]
fn a_large_file_edited_with_its_size_and_time_kept_is_seen_at_either_end_only() {
    let size = 2 * MIB;
    assert!(edit_is_seen(size, |content| content[10] = b'X'));
    assert!(edit_is_seen(size, |content| {
        let near_end = content.len() - 10;
        content[near_end] = b'X';
    }));
    assert!(edit_is_seen(size, |content| content.push(b'\n')));
    // The known cost of not reading the whole file.
    assert!(!edit_is_seen(size, |content| {
        let middle = content.len() / 2;
        content[middle] = b'X';
    }));
}

/// The ordinary case: any write moves the modified time, so a middle edit is seen. Saving a large
/// file unchanged counts too, where a small one is compared by content and doesn't.
#[test]
fn a_new_modified_time_counts_as_a_change_in_a_large_file_only() {
    let repo = committed_repo();
    let save_with_later_time = |name: &str, bytes: usize| {
        let path = repo.path().join(name);
        std::fs::write(&path, text_of(bytes)).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let before = change_fingerprint(repo.path());
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(modified + std::time::Duration::from_secs(60))
            .unwrap();
        drop(file);
        let after = change_fingerprint(repo.path());
        changed_between(repo.path(), &before, &after)
    };
    assert!(save_with_later_time("small.txt", 1024).is_empty());
    let changed = save_with_later_time("large.txt", 2 * MIB);
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].path, "large.txt");
}

#[test]
fn an_unreadable_path_still_counts_as_gone() {
    let repo = committed_repo();
    assert_eq!(content_fingerprint(&repo.path().join("missing.txt")), None);
    assert_eq!(content_fingerprint(repo.path()), None);
}
