//! Diffs, changed files, attached media and file comments.

use serde::{Deserialize, Serialize};

/// One diff line kind (`DiffLineKind` in types.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffLineKind {
    Context,
    Add,
    Del,
}

/// One diff line (`DiffLine` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

/// One hunk (`DiffHunk` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<DiffLine>,
}

/// One file a run changed (`ChangedFile` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    pub status: DiffStatus,
    /// Lines added and removed against the last commit; none when they can't be counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<u32>,
    /// A part of the path starts with a dot (`.github/…`, `.gitignore`); the reply folds these
    /// into one "N hidden files changed" line.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// One file shown under a reply (`AttachedMedia` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedMedia {
    /// Absolute path inside the project's `.themis/attachments`.
    pub path: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    /// `image`, `video` or `pdf`.
    pub kind: String,
}

/// A screenshot or recording a tool call saved (`ToolMedia` in types.ts):
/// metadata only, never the file's bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolMedia {
    /// Absolute path inside the project's `.themis/attachments`.
    pub path: String,
    /// `image` or `video`.
    pub kind: String,
    /// Pixel size, when the tool reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

/// File change status (`DiffStatus` in types.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
}

/// One changed file (`DiffFile` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffFile {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: DiffStatus,
    pub preexisting: bool,
    pub hunks: Vec<DiffHunk>,
    /// Lines added and removed in the whole change, however few `hunks` hold.
    #[serde(default)]
    pub added: usize,
    #[serde(default)]
    pub removed: usize,
    /// The change is too large to show whole (a downloaded dataset, a
    /// generated file): `hunks` hold only its first lines.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub too_large: bool,
    /// A part of the path starts with a dot; listings put these last and Changes folds them
    /// into one "N hidden files changed" line.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// Diff listing (`DiffState` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffState {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub files: Vec<DiffFile>,
}

/// A comment pinned to a file (`ThreadComment` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadComment {
    pub id: String,
    pub path: String,
    pub comment: String,
}
