//! Git diff support: repo probing, `git diff` parsing, untracked listing.
//!
//! [`parse_diff`] is pure (text in, [`DiffFile`]s out) so fixtures can test it
//! without a repo; the `git_*` helpers shell out to the `git` binary.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::process::Command;

use crate::types::{ChangedFile, DiffFile, DiffHunk, DiffLine, DiffLineKind, DiffStatus};

/// Maximum files returned by one diff listing (older entries are dropped and
/// the state's `reason` notes the truncation).
pub const MAX_DIFF_FILES: usize = 200;
/// Most bytes of a new (untracked) file read for its diff.
pub const MAX_SHOWN_FILE_BYTES: u64 = 1024 * 1024;
/// Most diff lines shown for one file; a larger change shows its first lines
/// and is marked `too_large`.
pub const MAX_SHOWN_DIFF_LINES: usize = 2_000;
/// Most diff text in one listing across all its files, in bytes.
pub const MAX_LISTED_DIFF_BYTES: usize = 2 * 1024 * 1024;

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

/// Working-tree state of readable changed files: porcelain code and a content
/// hash (`None` when the file is gone).
pub type ChangeFingerprint = HashMap<String, (u8, Option<u64>)>;

/// Captures every readable file that differs from `HEAD`, including untracked
/// ones, so two captures can tell which files a run touched. Not a Git repo or
/// a Git failure yields an empty capture.
#[must_use]
pub fn change_fingerprint(root: &Path) -> ChangeFingerprint {
    use std::hash::{Hash, Hasher};
    let Ok(bytes) = git_output(
        root,
        &["status", "--porcelain", "-z", "--untracked-files=all"],
    ) else {
        return HashMap::new();
    };
    let mut fingerprint = HashMap::new();
    let mut chunks = bytes.split(|byte| *byte == 0);
    while let Some(chunk) = chunks.next() {
        if chunk.len() < 4 {
            continue;
        }
        if chunk[0] == b'R' || chunk[1] == b'R' {
            chunks.next();
        }
        let path = String::from_utf8_lossy(&chunk[3..]).into_owned();
        if !is_readable_change(root, &path) {
            continue;
        }
        let code = if chunk[0] == b' ' { chunk[1] } else { chunk[0] };
        let hash = std::fs::read(root.join(&path)).ok().map(|content| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            content.hash(&mut hasher);
            hasher.finish()
        });
        fingerprint.insert(path, (code, hash));
    }
    fingerprint
}

/// Files whose content differs between two captures, sorted by path. A file
/// that left the capture was restored to `HEAD` (modified) or removed.
#[must_use]
pub fn changed_between(
    root: &Path,
    before: &ChangeFingerprint,
    after: &ChangeFingerprint,
) -> Vec<ChangedFile> {
    let paths: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    paths
        .into_iter()
        .filter(|path| {
            before.get(*path).map(|entry| entry.1) != after.get(*path).map(|entry| entry.1)
        })
        .map(|path| {
            let status = match after.get(path) {
                Some((_, None)) => DiffStatus::Deleted,
                Some((b'?' | b'A', _)) => DiffStatus::Added,
                Some(_) => DiffStatus::Modified,
                None if root.join(path).exists() => DiffStatus::Modified,
                None => DiffStatus::Deleted,
            };
            ChangedFile {
                path: path.clone(),
                status,
            }
        })
        .collect()
}

/// Runs `git diff HEAD --no-color -U3` and parses it into [`DiffFile`]s.
pub fn git_diff_head(root: &Path) -> Result<Vec<DiffFile>, String> {
    let bytes = git_output(root, &["diff", "HEAD", "--no-color", "-U3"])?;
    Ok(parse_diff(&String::from_utf8_lossy(&bytes)))
}

/// What a branch adds since it left the current one: `git diff HEAD...refs/heads/<branch>`.
pub fn git_diff_branch(root: &Path, branch: &str) -> Result<Vec<DiffFile>, String> {
    let range = format!("HEAD...refs/heads/{branch}");
    let bytes = git_output(root, &["diff", "--no-color", "-U3", &range, "--"])?;
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

/// Folders whose contents are build or tool caches, never hand-written code.
const GENERATED_DIRS: &[&str] = &[
    "__pycache__",
    "node_modules",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".venv",
    "venv",
    ".gradle",
    ".next",
];
/// Compiled or system artifacts that are never worth reading as a diff.
const GENERATED_EXTENSIONS: &[&str] = &[
    "pyc", "pyo", "class", "o", "obj", "so", "dylib", "dll", "exe", "a", "lib",
];
/// Bytes sampled to decide whether a file is readable text.
const TEXT_SAMPLE_BYTES: usize = 8 * 1024;

/// True for files a reader can follow in the monitor: not inside a cache
/// folder, not a compiled artifact, and (when present on disk) UTF-8 text.
/// Deleted files are kept so removals stay visible.
#[must_use]
pub fn is_readable_change(root: &Path, relative: &str) -> bool {
    let path = Path::new(relative);
    let generated_dir = path.components().any(|part| {
        GENERATED_DIRS
            .iter()
            .any(|dir| part.as_os_str() == std::ffi::OsStr::new(dir))
    });
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let generated_file = name == ".DS_Store"
        || path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                GENERATED_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
            });
    if generated_dir || generated_file {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(root.join(path)) else {
        return true;
    };
    let mut sample = vec![0; TEXT_SAMPLE_BYTES];
    let read = std::io::Read::read(&mut file, &mut sample).unwrap_or(0);
    sample.truncate(read);
    if sample.contains(&0) {
        return false;
    }
    match std::str::from_utf8(&sample) {
        Ok(_) => true,
        // A full sample may cut a multi-byte character at its end.
        Err(error) => read == TEXT_SAMPLE_BYTES && error.error_len().is_none(),
    }
}

/// Builds an "added" [`DiffFile`] for an untracked file: full content as one
/// hunk of additions (empty files get no hunks).
#[must_use]
pub fn untracked_diff_file(root: &Path, relative: &str) -> DiffFile {
    let mut bytes = Vec::new();
    let mut too_large = false;
    if let Ok(file) = std::fs::File::open(root.join(relative)) {
        let _ = std::io::Read::read_to_end(
            &mut std::io::Read::take(file, MAX_SHOWN_FILE_BYTES + 1),
            &mut bytes,
        );
        if bytes.len() as u64 > MAX_SHOWN_FILE_BYTES {
            too_large = true;
            // Keep whole lines only.
            bytes.truncate(bytes.iter().rposition(|b| *b == b'\n').unwrap_or(0));
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<DiffLine> = text
        .lines()
        .map(|text| DiffLine {
            kind: DiffLineKind::Add,
            text: text.to_owned(),
        })
        .collect();
    if lines.len() > MAX_SHOWN_DIFF_LINES {
        lines.truncate(MAX_SHOWN_DIFF_LINES);
        too_large = true;
    }
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
        too_large,
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
        let (hunks, too_large) = first_lines(self.hunks, MAX_SHOWN_DIFF_LINES);
        DiffFile {
            path,
            old_path,
            status,
            preexisting: false,
            hunks,
            too_large,
        }
    }
}

/// The hunks holding a file's first `max` diff lines, and whether any were cut.
/// A cut hunk's counts describe the lines it keeps.
fn first_lines(hunks: Vec<DiffHunk>, max: usize) -> (Vec<DiffHunk>, bool) {
    let mut kept = Vec::new();
    let mut room = max;
    for mut hunk in hunks {
        if room == 0 {
            return (kept, true);
        }
        if hunk.lines.len() > room {
            hunk.lines.truncate(room);
            hunk.old_lines = hunk
                .lines
                .iter()
                .filter(|line| line.kind != DiffLineKind::Add)
                .count();
            hunk.new_lines = hunk
                .lines
                .iter()
                .filter(|line| line.kind != DiffLineKind::Del)
                .count();
            kept.push(hunk);
            return (kept, true);
        }
        room -= hunk.lines.len();
        kept.push(hunk);
    }
    (kept, false)
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
    fn oversized_changes_are_listed_without_their_lines() {
        // A 34 MB downloaded catalog made the change list exceed 8 MiB.
        let root = tempfile::tempdir().unwrap();
        let big = "a,b,c\n".repeat(usize::try_from(MAX_SHOWN_FILE_BYTES).unwrap() / 6 + 1);
        std::fs::write(root.path().join("stars.csv"), &big).unwrap();
        std::fs::write(root.path().join("small.txt"), "one\ntwo\n").unwrap();
        let file = untracked_diff_file(root.path(), "stars.csv");
        // Its first lines are shown, whole, within the bounds.
        assert!(file.too_large);
        assert_eq!(file.status, DiffStatus::Added);
        assert_eq!(file.hunks[0].lines.len(), MAX_SHOWN_DIFF_LINES);
        assert!(file.hunks[0].lines.iter().all(|line| line.text == "a,b,c"));
        let small = untracked_diff_file(root.path(), "small.txt");
        assert!(!small.too_large);
        assert_eq!(small.hunks[0].lines.len(), 2);

        let added: String = (0..=MAX_SHOWN_DIFF_LINES)
            .map(|n| format!("+row {n}\n"))
            .collect();
        let text = format!(
            "diff --git a/data.csv b/data.csv\n--- a/data.csv\n+++ b/data.csv\n@@ -0,0 +1,{} @@\n{added}",
            MAX_SHOWN_DIFF_LINES + 1
        );
        let files = parse_diff(&text);
        assert!(files[0].too_large);
        assert_eq!(files[0].hunks[0].lines.len(), MAX_SHOWN_DIFF_LINES);
        assert_eq!(files[0].hunks[0].new_lines, MAX_SHOWN_DIFF_LINES);
        assert_eq!(files[0].path, "data.csv");
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
    fn fingerprints_report_only_files_changed_between_captures() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-q"]);
        std::fs::write(root.join("kept.txt"), "one\n").unwrap();
        std::fs::write(root.join("gone.txt"), "bye\n").unwrap();
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
        std::fs::write(root.join("kept.txt"), "edited before the run\n").unwrap();
        let before = change_fingerprint(root);
        std::fs::write(root.join("new.py"), "print(1)\n").unwrap();
        std::fs::write(root.join("blob.bin"), [0u8, 1, 2]).unwrap();
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        let after = change_fingerprint(root);
        let changed = changed_between(root, &before, &after);
        assert_eq!(
            changed,
            vec![
                ChangedFile {
                    path: "gone.txt".into(),
                    status: DiffStatus::Deleted
                },
                ChangedFile {
                    path: "new.py".into(),
                    status: DiffStatus::Added
                },
            ],
            "an edit made before the run and binary files are not reported"
        );
    }

    use proptest::prelude::*;

    /// One generated file diff: path plus hunks of (kind, text) lines.
    fn file_diff() -> impl Strategy<Value = (String, Vec<Vec<(u8, String)>>)> {
        (
            "[a-z]{1,8}(/[a-z]{1,8}){0,2}\\.rs",
            proptest::collection::vec(
                proptest::collection::vec((0u8..3, "[ -~]{0,30}"), 1..6),
                1..4,
            ),
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        /// Arbitrary text never panics the diff parser or the path decoder.
        #[test]
        fn diff_parser_accepts_any_text(text in any::<String>()) {
            let _ = parse_diff(&text);
            let _ = decode_git_quoted_path(&text);
        }

        /// Well-formed git output parses back to the same files, kinds and lines.
        #[test]
        fn generated_diffs_round_trip(files in proptest::collection::vec(file_diff(), 1..4)) {
            let mut text = String::new();
            for (path, hunks) in &files {
                text.push_str(&format!("diff --git a/{path} b/{path}\nindex 1111111..2222222 100644\n--- a/{path}\n+++ b/{path}\n"));
                let mut start = 1;
                for lines in hunks {
                    let old = lines.iter().filter(|(kind, _)| *kind != 1).count();
                    let new = lines.iter().filter(|(kind, _)| *kind != 2).count();
                    text.push_str(&format!("@@ -{start},{old} +{start},{new} @@\n"));
                    for (kind, line) in lines {
                        text.push_str(&format!("{}{line}\n", [' ', '+', '-'][*kind as usize]));
                    }
                    start += old + 10;
                }
            }
            let parsed = parse_diff(&text);
            prop_assert_eq!(parsed.len(), files.len());
            for (file, (path, hunks)) in parsed.iter().zip(&files) {
                prop_assert_eq!(&file.path, path);
                prop_assert_eq!(file.hunks.len(), hunks.len());
                for (hunk, lines) in file.hunks.iter().zip(hunks) {
                    let kinds: Vec<(u8, &str)> = hunk.lines.iter().map(|line| (match line.kind { DiffLineKind::Context => 0, DiffLineKind::Add => 1, DiffLineKind::Del => 2 }, line.text.as_str())).collect();
                    let expected: Vec<(u8, &str)> = lines.iter().map(|(kind, text)| (*kind, text.as_str())).collect();
                    prop_assert_eq!(kinds, expected);
                }
            }
        }

        /// Reported files are exactly those whose content changed, sorted and unique.
        #[test]
        fn changed_files_are_exactly_the_differing_ones(
            before in proptest::collection::hash_map("[a-c]{1,2}", (prop_oneof![Just(b'M'), Just(b'?'), Just(b'A')], proptest::option::of(0u64..4)), 0..6),
            after in proptest::collection::hash_map("[a-c]{1,2}", (prop_oneof![Just(b'M'), Just(b'?'), Just(b'A')], proptest::option::of(0u64..4)), 0..6),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let changed = changed_between(dir.path(), &before, &after);
            let paths: Vec<&str> = changed.iter().map(|file| file.path.as_str()).collect();
            let mut expected: Vec<&str> = before.keys().chain(after.keys()).map(String::as_str)
                .filter(|path| before.get(*path).map(|entry| entry.1) != after.get(*path).map(|entry| entry.1))
                .collect();
            expected.sort_unstable();
            expected.dedup();
            prop_assert_eq!(paths, expected);
        }

        /// Files judged readable are NUL-free UTF-8 in their sampled prefix.
        #[test]
        fn readable_files_are_text(content in proptest::collection::vec(any::<u8>(), 0..9000), text in any::<bool>(), unicode in any::<String>()) {
            let dir = tempfile::tempdir().unwrap();
            let bytes = if text { unicode.into_bytes() } else { content };
            std::fs::write(dir.path().join("sample.txt"), &bytes).unwrap();
            if is_readable_change(dir.path(), "sample.txt") {
                let sample = &bytes[..bytes.len().min(TEXT_SAMPLE_BYTES)];
                prop_assert!(!sample.contains(&0));
                prop_assert!(std::str::from_utf8(sample).is_ok() || sample.len() == TEXT_SAMPLE_BYTES);
            }
        }
    }

    #[test]
    fn only_readable_text_changes_are_monitored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("pkg/__pycache__")).unwrap();
        std::fs::write(root.join("pkg/app.py"), "print('hi')\n").unwrap();
        std::fs::write(root.join("pkg/__pycache__/app.cpython-314.pyc"), "x").unwrap();
        std::fs::write(root.join("logo.png"), [0x89, b'P', b'N', b'G', 0, 1, 2]).unwrap();
        std::fs::write(root.join("latin1.txt"), [b'c', b'a', b'f', 0xe9]).unwrap();
        std::fs::write(root.join(".DS_Store"), "x").unwrap();
        assert!(is_readable_change(root, "pkg/app.py"));
        assert!(
            is_readable_change(root, "removed.rs"),
            "deleted files stay visible"
        );
        for hidden in [
            "pkg/__pycache__/app.cpython-314.pyc",
            "logo.png",
            "latin1.txt",
            ".DS_Store",
            "build/lib.o",
        ] {
            assert!(!is_readable_change(root, hidden), "{hidden}");
        }
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
