//! Git diff support: repo probing, `git diff` parsing, untracked listing.
//!
//! [`parse_diff`] is pure (text in, [`DiffFile`]s out) so fixtures can test it
//! without a repo; the `git_*` helpers shell out to the `git` binary.

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

use crate::types::{DiffFile, DiffHunk, DiffLine, DiffLineKind, DiffStatus};

/// Maximum files returned by one diff listing (older entries are dropped and
/// the state's `reason` notes the truncation).
pub const MAX_DIFF_FILES: usize = 200;

/// Returns true when `root` is inside a git work tree.
#[must_use]
pub fn is_git_repo(root: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .is_ok_and(|out| {
            out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true"
        })
}

/// Resolves `HEAD` to a SHA, or `None` when the repo has no commits (or git
/// fails).
#[must_use]
pub fn git_head(root: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn git_output(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|err| format!("failed to run git {}: {err}", args.join(" ")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("git {} failed: {}", args.join(" "), stderr.trim()));
    }
    Ok(out.stdout)
}

/// Parses `git status --porcelain -z` into the set of changed paths.
///
/// Rename entries contribute both sides so preexisting checks stay
/// conservative.
pub fn git_status_names(root: &Path) -> Result<HashSet<String>, String> {
    let bytes = git_output(root, &["status", "--porcelain", "-z"])?;
    let mut names = HashSet::new();
    let mut chunks = bytes.split(|byte| *byte == 0);
    while let Some(chunk) = chunks.next() {
        if chunk.len() < 4 {
            continue;
        }
        let renamed = chunk[0] == b'R' || chunk[1] == b'R';
        names.insert(String::from_utf8_lossy(&chunk[3..]).into_owned());
        if renamed {
            if let Some(other) = chunks.next().filter(|next| !next.is_empty()) {
                names.insert(String::from_utf8_lossy(other).into_owned());
            }
        }
    }
    Ok(names)
}

/// Runs `git diff HEAD --no-color -U3` and parses it into [`DiffFile`]s.
pub fn git_diff_head(root: &Path) -> Result<Vec<DiffFile>, String> {
    let bytes = git_output(root, &["diff", "HEAD", "--no-color", "-U3"])?;
    Ok(parse_diff(&String::from_utf8_lossy(&bytes)))
}

/// Runs `git diff --cached --no-color -U3` (staged changes; used when the repo
/// has no `HEAD` yet) and parses it.
pub fn git_diff_cached(root: &Path) -> Result<Vec<DiffFile>, String> {
    let bytes = git_output(root, &["diff", "--cached", "--no-color", "-U3"])?;
    Ok(parse_diff(&String::from_utf8_lossy(&bytes)))
}

/// Runs `git diff --no-color -U3` (unstaged changes; used with
/// [`git_diff_cached`] when the repo has no `HEAD` yet) and parses it.
pub fn git_diff_worktree(root: &Path) -> Result<Vec<DiffFile>, String> {
    let bytes = git_output(root, &["diff", "--no-color", "-U3"])?;
    Ok(parse_diff(&String::from_utf8_lossy(&bytes)))
}

/// Lists untracked files (`git ls-files --others --exclude-standard`).
pub fn git_untracked(root: &Path) -> Result<Vec<String>, String> {
    let bytes = git_output(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    Ok(bytes
        .split(|byte| *byte == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect())
}

/// Builds an "added" [`DiffFile`] for an untracked file: full content as one
/// hunk of additions (empty files get no hunks).
#[must_use]
pub fn untracked_diff_file(root: &Path, relative: &str) -> DiffFile {
    let bytes = std::fs::read(root.join(relative)).unwrap_or_default();
    let lines: Vec<DiffLine> = String::from_utf8_lossy(&bytes)
        .lines()
        .map(|text| DiffLine {
            kind: DiffLineKind::Add,
            text: text.to_owned(),
        })
        .collect();
    let hunks = if lines.is_empty() {
        Vec::new()
    } else {
        vec![DiffHunk {
            old_start: 0,
            old_lines: 0,
            new_start: 1,
            new_lines: lines.len(),
            lines,
        }]
    };
    DiffFile {
        path: relative.to_owned(),
        old_path: None,
        status: DiffStatus::Added,
        preexisting: false,
        hunks,
    }
}

/// Parses `git diff` output (one `diff --git` section per file) into files.
///
/// Handles modified/added/deleted/renamed files, multi-hunk bodies, and
/// binary files (reported with no hunks). Malformed hunk headers are skipped
/// defensively rather than failing the whole listing.
#[must_use]
pub fn parse_diff(text: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut current: Option<FileBuilder> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(done) = current.take() {
                files.push(done.build());
            }
            current = Some(FileBuilder::new(rest));
        } else if let Some(builder) = current.as_mut() {
            builder.push_line(line);
        }
    }
    if let Some(done) = current.take() {
        files.push(done.build());
    }
    files
}

struct HunkBuilder {
    old_start: usize,
    old_lines: usize,
    new_start: usize,
    new_lines: usize,
    lines: Vec<DiffLine>,
}

impl HunkBuilder {
    fn build(self) -> DiffHunk {
        DiffHunk {
            old_start: self.old_start,
            old_lines: self.old_lines,
            new_start: self.new_start,
            new_lines: self.new_lines,
            lines: self.lines,
        }
    }
}

struct FileBuilder {
    old_path: String,
    new_path: String,
    rename_from: Option<String>,
    rename_to: Option<String>,
    new_file: bool,
    deleted_file: bool,
    minus_dev_null: bool,
    plus_dev_null: bool,
    hunks: Vec<DiffHunk>,
    current: Option<HunkBuilder>,
}

impl FileBuilder {
    fn new(header: &str) -> Self {
        let (old_raw, new_raw) = split_git_paths(header);
        Self {
            old_path: strip_ab_prefix(&old_raw),
            new_path: strip_ab_prefix(&new_raw),
            rename_from: None,
            rename_to: None,
            new_file: false,
            deleted_file: false,
            minus_dev_null: false,
            plus_dev_null: false,
            hunks: Vec::new(),
            current: None,
        }
    }

    fn push_line(&mut self, line: &str) {
        if let Some(header) = line.strip_prefix("@@ ") {
            self.flush_hunk();
            if let Some(hunk) = parse_hunk_header(header) {
                self.current = Some(hunk);
            }
            return;
        }
        if line.starts_with("new file mode") {
            self.new_file = true;
        } else if line.starts_with("deleted file mode") {
            self.deleted_file = true;
        } else if let Some(path) = line.strip_prefix("rename from ") {
            self.rename_from = Some(dequote(path));
        } else if let Some(path) = line.strip_prefix("rename to ") {
            self.rename_to = Some(dequote(path));
        } else if line == "--- /dev/null" {
            self.minus_dev_null = true;
        } else if line == "+++ /dev/null" {
            self.plus_dev_null = true;
        } else if let Some(hunk) = self.current.as_mut() {
            push_hunk_line(hunk, line);
        }
    }

    fn flush_hunk(&mut self) {
        if let Some(hunk) = self.current.take() {
            self.hunks.push(hunk.build());
        }
    }

    fn build(mut self) -> DiffFile {
        self.flush_hunk();
        let renamed = self.rename_from.is_some() || self.rename_to.is_some();
        let (status, path, old_path) = if renamed {
            (
                DiffStatus::Renamed,
                self.rename_to.clone().unwrap_or(self.new_path.clone()),
                Some(self.rename_from.clone().unwrap_or(self.old_path.clone())),
            )
        } else if self.new_file || self.minus_dev_null {
            (DiffStatus::Added, self.new_path.clone(), None)
        } else if self.deleted_file || self.plus_dev_null {
            (DiffStatus::Deleted, self.old_path.clone(), None)
        } else {
            (DiffStatus::Modified, self.new_path.clone(), None)
        };
        DiffFile {
            path,
            old_path,
            status,
            preexisting: false,
            hunks: self.hunks,
        }
    }
}

fn push_hunk_line(hunk: &mut HunkBuilder, line: &str) {
    if line.starts_with('\\') {
        return;
    }
    let Some(marker) = line.as_bytes().first() else {
        hunk.lines.push(DiffLine {
            kind: DiffLineKind::Context,
            text: String::new(),
        });
        return;
    };
    let (kind, text) = match marker {
        b' ' => (DiffLineKind::Context, &line[1..]),
        b'+' => (DiffLineKind::Add, &line[1..]),
        b'-' => (DiffLineKind::Del, &line[1..]),
        _ => return,
    };
    hunk.lines.push(DiffLine {
        kind,
        text: text.to_owned(),
    });
}

fn parse_hunk_header(header: &str) -> Option<HunkBuilder> {
    let ranges = header.split("@@").next()?;
    let mut parts = ranges.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let (old_start, old_lines) = parse_range(old)?;
    let (new_start, new_lines) = parse_range(new)?;
    Some(HunkBuilder {
        old_start,
        old_lines,
        new_start,
        new_lines,
        lines: Vec::new(),
    })
}

fn parse_range(range: &str) -> Option<(usize, usize)> {
    match range.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

/// Splits the `diff --git <old> <new>` remainder into its two paths,
/// honoring git's C-style quoting for unusual names.
fn split_git_paths(header: &str) -> (String, String) {
    let mut paths = Vec::new();
    let bytes = header.as_bytes();
    let mut index = 0;
    while index < bytes.len() && paths.len() < 2 {
        while index < bytes.len() && bytes[index] == b' ' {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        if bytes[index] == b'"' {
            let mut end = index + 1;
            while end < bytes.len() {
                if bytes[end] == b'\\' {
                    end += 2;
                } else if bytes[end] == b'"' {
                    break;
                } else {
                    end += 1;
                }
            }
            paths.push(dequote(&header[index..=end.min(bytes.len() - 1)]));
            index = end + 1;
        } else {
            let mut end = index;
            while end < bytes.len() && bytes[end] != b' ' {
                end += 1;
            }
            paths.push(header[index..end].to_owned());
            index = end;
        }
    }
    match paths.len() {
        2 => (paths.remove(0), paths.remove(0)),
        1 => (paths[0].clone(), paths.remove(0)),
        _ => (String::new(), String::new()),
    }
}

/// Strips the `a/`/`b/` prefix git prepends to diff paths.
fn strip_ab_prefix(path: &str) -> String {
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
        .to_owned()
}

/// Removes surrounding quotes and unescapes git's C-style quoting.
fn dequote(path: &str) -> String {
    let path = path.trim();
    let Some(inner) = path
        .strip_prefix('"')
        .and_then(|path| path.strip_suffix('"'))
    else {
        return path.to_owned();
    };
    decode_git_quoted_path(inner)
}

fn decode_git_quoted_path(path: &str) -> String {
    let mut decoded = Vec::with_capacity(path.len());
    let mut bytes = path.bytes().peekable();
    while let Some(byte) = bytes.next() {
        if byte != b'\\' {
            decoded.push(byte);
            continue;
        }
        let Some(escaped) = bytes.next() else {
            decoded.push(b'\\');
            break;
        };
        match escaped {
            b'0'..=b'7' => decoded.push(decode_octal_escape(escaped, &mut bytes)),
            b'a' => decoded.push(0x07),
            b'b' => decoded.push(0x08),
            b'f' => decoded.push(0x0c),
            b'n' => decoded.push(b'\n'),
            b'r' => decoded.push(b'\r'),
            b't' => decoded.push(b'\t'),
            b'v' => decoded.push(0x0b),
            b'\\' | b'"' => decoded.push(escaped),
            other => {
                decoded.push(b'\\');
                decoded.push(other);
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn decode_octal_escape(first: u8, bytes: &mut std::iter::Peekable<std::str::Bytes<'_>>) -> u8 {
    let mut value = u16::from(first - b'0');
    for _ in 0..2 {
        let Some(next @ b'0'..=b'7') = bytes.peek().copied() else {
            break;
        };
        bytes.next();
        value = value * 8 + u16::from(next - b'0');
    }
    value as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODIFIED: &str = "\
diff --git a/src/main.rs b/src/main.rs
index 1111111..2222222 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
-    old();
+    new();
+    more();
 }
";

    const ADDED: &str = "\
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+one
+two
";

    const DELETED: &str = "\
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 4444444..0000000
--- a/gone.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-line1
-line2
";

    const RENAMED: &str = "\
diff --git a/old.rs b/new.rs
similarity index 90%
rename from old.rs
rename to new.rs
index 5555555..6666666 100644
--- a/old.rs
+++ b/new.rs
@@ -1 +1 @@
-fn old() {}
+fn new() {}
";

    #[test]
    fn parses_modified_file_with_hunk_positions() {
        let files = parse_diff(MODIFIED);
        assert_eq!(files.len(), 1);
        let file = &files[0];
        assert_eq!(file.path, "src/main.rs");
        assert_eq!(file.status, DiffStatus::Modified);
        assert!(!file.preexisting);
        assert!(file.old_path.is_none());
        assert_eq!(file.hunks.len(), 1);
        let hunk = &file.hunks[0];
        assert_eq!((hunk.old_start, hunk.old_lines), (1, 3));
        assert_eq!((hunk.new_start, hunk.new_lines), (1, 4));
        let kinds: Vec<DiffLineKind> = hunk.lines.iter().map(|line| line.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DiffLineKind::Context,
                DiffLineKind::Del,
                DiffLineKind::Add,
                DiffLineKind::Add,
                DiffLineKind::Context,
            ]
        );
        assert_eq!(hunk.lines[1].text, "    old();");
    }

    #[test]
    fn parses_added_deleted_and_renamed() {
        let added = &parse_diff(ADDED)[0];
        assert_eq!(added.status, DiffStatus::Added);
        assert_eq!(added.path, "new.txt");
        assert_eq!(added.hunks.len(), 1);
        assert_eq!((added.hunks[0].old_start, added.hunks[0].old_lines), (0, 0));
        assert!(added.hunks[0]
            .lines
            .iter()
            .all(|line| line.kind == DiffLineKind::Add));

        let deleted = &parse_diff(DELETED)[0];
        assert_eq!(deleted.status, DiffStatus::Deleted);
        assert_eq!(deleted.path, "gone.txt");
        assert!(deleted.hunks[0]
            .lines
            .iter()
            .all(|line| line.kind == DiffLineKind::Del));

        let renamed = &parse_diff(RENAMED)[0];
        assert_eq!(renamed.status, DiffStatus::Renamed);
        assert_eq!(renamed.path, "new.rs");
        assert_eq!(renamed.old_path.as_deref(), Some("old.rs"));
    }

    #[test]
    fn parses_multiple_files_and_skips_no_newline_markers() {
        let text = format!(
            "{MODIFIED}{ADDED}diff --git a/tail.txt b/tail.txt\n\
             new file mode 100644\n--- /dev/null\n+++ b/tail.txt\n\
             @@ -0,0 +1 @@\n+last\n\\ No newline at end of file\n"
        );
        let files = parse_diff(&text);
        assert_eq!(files.len(), 3);
        assert_eq!(files[2].path, "tail.txt");
        assert_eq!(files[2].hunks[0].lines.len(), 1);
    }

    #[test]
    fn binary_files_parse_without_hunks() {
        let files = parse_diff(
            "diff --git a/img.png b/img.png\n\
             index 7777777..8888888 100644\n\
             Binary files a/img.png and b/img.png differ\n",
        );
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].status, DiffStatus::Modified);
        assert!(files[0].hunks.is_empty());
    }

    #[test]
    fn quoted_paths_are_dequoted() {
        let files = parse_diff(
            "diff --git \"a/sp ace.txt\" \"b/sp ace.txt\"\n\
             new file mode 100644\n--- /dev/null\n+++ \"b/sp ace.txt\"\n\
             @@ -0,0 +1 @@\n+x\n",
        );
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "sp ace.txt");
    }

    #[test]
    fn quoted_octal_paths_decode_utf8_bytes() {
        let files = parse_diff(
            "diff --git \"a/caf\\303\\251.txt\" \"b/caf\\303\\251.txt\"\n\
             new file mode 100644\n--- /dev/null\n+++ \"b/caf\\303\\251.txt\"\n\
             @@ -0,0 +1 @@\n+x\n",
        );

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "café.txt");
    }

    #[test]
    fn truncation_cap_applies_at_two_hundred_files() {
        let mut text = String::new();
        for index in 0..250 {
            text.push_str(&format!(
                "diff --git a/f{index}.txt b/f{index}.txt\n\
                 new file mode 100644\n--- /dev/null\n+++ b/f{index}.txt\n\
                 @@ -0,0 +1 @@\n+x\n"
            ));
        }
        let mut files = parse_diff(&text);
        assert_eq!(files.len(), 250);
        // The listing layer truncates (mirrors AppState::list_diff behavior).
        files.truncate(MAX_DIFF_FILES);
        assert_eq!(files.len(), MAX_DIFF_FILES);
        assert_eq!(files[199].path, "f199.txt");
    }

    #[test]
    fn untracked_file_reports_full_content_as_added() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("note.txt"), "a\nb\n").expect("write");
        std::fs::write(dir.path().join("empty.txt"), "").expect("write");
        let file = untracked_diff_file(dir.path(), "note.txt");
        assert_eq!(file.status, DiffStatus::Added);
        assert_eq!(file.hunks.len(), 1);
        assert_eq!(file.hunks[0].new_lines, 2);
        assert!(file.hunks[0]
            .lines
            .iter()
            .all(|line| line.kind == DiffLineKind::Add));
        assert_eq!(file.hunks[0].lines[0].text, "a");
        let empty = untracked_diff_file(dir.path(), "empty.txt");
        assert!(empty.hunks.is_empty());
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
    }

    #[test]
    fn live_git_helpers_agree_on_a_temp_repo() {
        if !git_available() {
            eprintln!("skipping live_git_helpers: git binary not found");
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        assert!(!is_git_repo(root));
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .expect("git init");
        assert!(status.success());
        assert!(is_git_repo(root));
        assert!(git_head(root).is_none());
        assert!(git_status_names(root).expect("status").is_empty());
        std::fs::write(root.join("a.txt"), "hi\n").expect("write");
        assert_eq!(git_untracked(root).expect("untracked"), vec!["a.txt"]);
    }
}
