//! Deciding whether a failed sandboxed command was blocked by the sandbox.
//!
//! No backend reports its denials to the parent directly (Seatbelt writes to
//! the system log, bubblewrap and Landlock only return errors to the program),
//! so the signal is the program's own output, read conservatively:
//!
//! * the command failed (non-zero exit), and
//! * a line carries a permission error (`Operation not permitted`, `Permission
//!   denied`, `Read-only file system`) and names a path that this plan blocks:
//!   a secret location, a protected folder, or a folder outside the writable
//!   ones that the operating system itself would let us write; or a line under
//!   a secret location says `No such file or directory` (bubblewrap hides them);
//! * or the network is blocked and a line carries a typical network failure.
//!
//! Anything else (no path, a path the sandbox allows, a path the OS itself
//! refuses) is not treated as a sandbox denial, so no escalation is offered.

use super::{resolve_existing, NetworkPolicy, SandboxPlan};
use std::path::{Path, PathBuf};

/// Lines examined, and bytes per line.
const MAX_LINES: usize = 400;
const MAX_LINE_BYTES: usize = 4096;
/// Path candidates examined per line.
const MAX_CANDIDATES: usize = 16;

const PERMISSION_MARKERS: &[&str] = &[
    "operation not permitted",
    "permission denied",
    "read-only file system",
    "eperm",
    "eacces",
    "erofs",
];

const NETWORK_MARKERS: &[&str] = &[
    "could not resolve host",
    "couldn't connect to server",
    "failed to connect",
    "network is unreachable",
    "temporary failure in name resolution",
    "name or service not known",
    "nodename nor servname",
    "getaddrinfo",
    "could not resolve",
    "connection refused",
    "unable to access",
    "network access is blocked",
];

fn lines(output: &str) -> impl Iterator<Item = &str> {
    output.lines().take(MAX_LINES).map(|line| {
        let mut end = line.len().min(MAX_LINE_BYTES);
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        &line[..end]
    })
}

/// Whether any line mentions a permission or (when blocked) network failure.
pub(super) fn has_marker(plan: &SandboxPlan, output: &str) -> bool {
    lines(output).any(|line| {
        let lower = line.to_ascii_lowercase();
        PERMISSION_MARKERS
            .iter()
            .any(|marker| lower.contains(marker))
            || (plan.network() == NetworkPolicy::Block
                && NETWORK_MARKERS.iter().any(|marker| lower.contains(marker)))
    })
}

pub(super) fn denial(plan: &SandboxPlan, output: &str, cwd: &Path) -> Option<String> {
    for line in lines(output) {
        let lower = line.to_ascii_lowercase();
        if plan.network() == NetworkPolicy::Block
            && plan.network_enforced
            && NETWORK_MARKERS.iter().any(|marker| lower.contains(marker))
        {
            return Some("network access is blocked (sandbox.network is \"block\")".to_owned());
        }
        let permission = PERMISSION_MARKERS
            .iter()
            .any(|marker| lower.contains(marker));
        let missing = lower.contains("no such file or directory");
        if !permission && !missing {
            continue;
        }
        // The path an error is about sits next to its message
        // (`sh: 1: cannot create /x: Read-only file system`), while the
        // program's own path starts the line; look at the nearest first.
        let marker = PERMISSION_MARKERS
            .iter()
            .chain(std::iter::once(&"no such file or directory"))
            .filter_map(|marker| lower.find(marker))
            .min()
            .unwrap_or(0);
        let mut candidates = path_candidates(line);
        candidates.sort_by_key(|candidate| {
            let start = line.find(candidate.as_str()).unwrap_or(0);
            let end = start + candidate.len();
            if end <= marker {
                marker - end
            } else {
                start.saturating_sub(marker)
            }
        });
        for candidate in candidates {
            let Some(path) = absolute(&candidate, cwd, plan) else {
                continue;
            };
            // Resolving an automounted path (`/net/...`, `/home/...` on
            // macOS) can wait on the network; compare those lexically.
            let resolved = if automounted(&path) {
                path.clone()
            } else {
                resolve_existing(&path)
            };
            let inside = |list: &[PathBuf]| {
                list.iter()
                    .any(|root| resolved.starts_with(root) || path.starts_with(root))
            };
            if inside(plan.secrets()) {
                return Some(format!(
                    "{candidate} is a protected secret location the sandbox does not let commands read or change"
                ));
            }
            if !permission {
                continue;
            }
            if inside(plan.protected()) {
                return Some(format!(
                    "writing to {candidate} is blocked: Themis settings and permissions are protected"
                ));
            }
            if !inside(plan.writable()) && os_allows_write(&resolved) {
                return Some(format!(
                    "writing to {candidate} is blocked: only the project, temporary folders and sandbox.writable_paths are writable"
                ));
            }
        }
    }
    None
}

/// Absolute form of a path candidate: `~/x` expands the plan's home, a relative
/// path containing `/` resolves against the command's working folder.
fn absolute(candidate: &str, cwd: &Path, plan: &SandboxPlan) -> Option<PathBuf> {
    if let Some(rest) = candidate.strip_prefix("~/") {
        return plan.home.as_ref().map(|home| home.join(rest));
    }
    let path = Path::new(candidate);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    candidate.contains('/').then(|| cwd.join(path))
}

fn automounted(path: &Path) -> bool {
    cfg!(target_os = "macos")
        && ["/net", "/home", "/Network"]
            .iter()
            .any(|mount| path.starts_with(mount))
}

/// Path-like words in one output line: quoted strings and whitespace-separated
/// words with surrounding punctuation removed, that start with `/`, `~/` or
/// contain a `/`. Bounded; never panics.
pub fn path_candidates(line: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let push = |text: &str, found: &mut Vec<String>| {
        let trimmed = text.trim_matches(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '\'' | '"'
                        | '`'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '<'
                        | '>'
                        | ','
                        | ';'
                        | ':'
                        | '‘'
                        | '’'
                        | '“'
                        | '”'
                )
        });
        if trimmed.len() > 1
            && (trimmed.starts_with('/') || trimmed.starts_with("~/") || trimmed.contains('/'))
            && !trimmed.contains("://")
            && found.len() < MAX_CANDIDATES
            && !found.iter().any(|seen| seen == trimmed)
        {
            found.push(trimmed.to_owned());
        }
    };
    for quote in ['\'', '"', '`'] {
        for (index, part) in line.split(quote).enumerate() {
            if index % 2 == 1 {
                push(part, &mut found);
            }
        }
    }
    for (open, close) in [('‘', '’'), ('“', '”')] {
        let mut rest = line;
        while let Some(start) = rest.find(open) {
            let after = &rest[start + open.len_utf8()..];
            let Some(end) = after.find(close) else { break };
            push(&after[..end], &mut found);
            rest = &after[end + close.len_utf8()..];
        }
    }
    for word in line.split_whitespace() {
        push(word, &mut found);
    }
    found
}

/// Whether the operating system itself would let this process write `path`
/// (or create it in its nearest existing folder). A permission error on a path
/// the OS refuses anyway is not the sandbox's doing.
#[cfg(unix)]
fn os_allows_write(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let mut probe = path;
    loop {
        if std::fs::symlink_metadata(probe).is_ok() {
            break;
        }
        match probe.parent() {
            Some(parent) => probe = parent,
            None => return false,
        }
    }
    let Ok(text) = std::ffi::CString::new(probe.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `text` is a valid NUL-terminated path for the duration of the call.
    unsafe { libc::access(text.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(not(unix))]
fn os_allows_write(_path: &Path) -> bool {
    true
}
