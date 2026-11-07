//! Which files a chat may search: snapshots, history exports and attachments.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceKind {
    Snapshot,
    HistorySource,
    Attachment,
}

impl SourceKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::HistorySource => "history_source",
            Self::Attachment => "attachment",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Source {
    pub(super) kind: SourceKind,
    pub(super) path: PathBuf,
    pub(super) name: String,
    /// Newer is larger; snapshots rank ahead of history exports.
    pub(super) recency: u128,
}

fn parse_snapshot_name(name: &str) -> Option<u128> {
    let digits = name.strip_prefix("snapshot-")?.strip_suffix(".jsonl")?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

fn parse_history_name(name: &str) -> Option<u128> {
    // history-source-<first>-<last>-<uuid>.jsonl
    let rest = name
        .strip_prefix("history-source-")?
        .strip_suffix(".jsonl")?;
    let mut parts = rest.splitn(3, '-');
    let _first: u64 = parts.next()?.parse().ok()?;
    let last: u64 = parts.next()?.parse().ok()?;
    parts.next()?;
    Some(u128::from(last))
}

/// True for a regular, non-symlink file that resolves inside `root`.
fn confined_file(path: &Path, root: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    path.canonicalize()
        .is_ok_and(|resolved| resolved.starts_with(root))
}

/// Opens a source for reading without following a final symlink and checks the
/// opened handle (not just the earlier listing) is a regular file.
pub(super) fn open_regular(path: &Path) -> Option<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    #[cfg(not(unix))]
    if std::fs::symlink_metadata(path)
        .ok()?
        .file_type()
        .is_symlink()
    {
        return None;
    }
    Some(file)
}

pub(super) fn evidence_sources(scope: &RecallScope) -> (Vec<Source>, bool) {
    let mut sources = Vec::new();
    let Ok(directory) = scope.evidence_dir.canonicalize() else {
        return (sources, false);
    };
    if let Ok(entries) = std::fs::read_dir(&directory) {
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let (kind, recency) = if let Some(version) = parse_snapshot_name(&name) {
                (SourceKind::Snapshot, version.saturating_add(1 << 100))
            } else if let Some(last) = parse_history_name(&name) {
                (SourceKind::HistorySource, last)
            } else {
                continue;
            };
            let path = directory.join(&name);
            if confined_file(&path, &directory) {
                sources.push(Source {
                    kind,
                    path,
                    name,
                    recency,
                });
            }
        }
    }
    sources.sort_by(|a, b| (b.recency, &b.name).cmp(&(a.recency, &a.name)));
    let truncated = sources.len() > MAX_FILES;
    sources.truncate(MAX_FILES);
    (sources, truncated)
}

pub(super) fn attachment_sources(scope: &RecallScope) -> Vec<Source> {
    let Some(root) = scope
        .attachment_root
        .as_ref()
        .and_then(|root| root.canonicalize().ok())
    else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for path in &scope.attachments {
        let Ok(resolved) = path.canonicalize() else {
            continue;
        };
        if !seen.insert(resolved.clone()) || !confined_file(&resolved, &root) {
            continue;
        }
        let name = resolved
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("attachment")
            .to_owned();
        out.push(Source {
            kind: SourceKind::Attachment,
            path: resolved,
            name,
            recency: 0,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

pub(super) fn display_path(scope: &RecallScope, path: &Path) -> String {
    scope
        .project_root
        .as_ref()
        .and_then(|root| root.canonicalize().ok())
        .and_then(|root| path.strip_prefix(root).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
