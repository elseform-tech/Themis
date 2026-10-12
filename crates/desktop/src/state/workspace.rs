//! Read-only workspace monitor: branch, branches, pull requests, directory
//! listing and file contents. Nothing here writes to the project or runs
//! git actions that change state.

use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use super::{canonical_project_dir, AppState};

/// Directory entries returned per listing.
const MAX_ENTRIES: usize = 500;
/// Bytes of a file returned for viewing.
const MAX_FILE_BYTES: usize = 256 * 1024;
const PR_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BranchInfo {
    pub name: String,
    pub current: bool,
    pub upstream: Option<String>,
    /// e.g. "ahead 2, behind 1" as reported by git.
    pub track: Option<String>,
    pub updated: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PullRequestInfo {
    pub number: u64,
    pub title: String,
    pub branch: String,
    pub state: String,
    pub draft: bool,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceInfo {
    pub root: String,
    pub name: String,
    pub is_git: bool,
    pub branch: Option<String>,
    pub branches: Vec<BranchInfo>,
    pub pull_requests: Vec<PullRequestInfo>,
    /// Why pull requests are unavailable (no GitHub CLI, not signed in, no remote).
    pub pull_requests_unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirEntry {
    pub name: String,
    /// Project-relative path with `/` separators.
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirListing {
    pub path: String,
    pub entries: Vec<DirEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileView {
    pub path: String,
    pub content: Option<String>,
    pub size: u64,
    pub truncated: bool,
    /// True when the bytes are not UTF-8 text; content is then omitted.
    pub binary: bool,
}

/// Resolves a project-relative path inside `root`, rejecting absolute paths,
/// `..` components and symlinks that escape the project.
pub fn resolve_inside(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(relative);
    if candidate.has_root()
        || candidate
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(format!(
            "invalid path '{relative}': must stay inside the project"
        ));
    }
    let resolved = root
        .join(candidate)
        .canonicalize()
        .map_err(|err| format!("cannot open '{relative}': {err}"))?;
    if !resolved.starts_with(root) {
        return Err(format!("invalid path '{relative}': leaves the project"));
    }
    Ok(resolved)
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn branches(root: &Path) -> Vec<BranchInfo> {
    let Some(text) = git(
        root,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--count=50",
            "--format=%(HEAD)\t%(refname:short)\t%(upstream:short)\t%(upstream:track,nobracket)\t%(committerdate:relative)",
            "refs/heads",
        ],
    ) else {
        return Vec::new();
    };
    let present = |value: &str| (!value.is_empty()).then(|| value.to_owned());
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let head = fields.next()?;
            let name = fields.next()?.to_owned();
            Some(BranchInfo {
                current: head == "*",
                name,
                upstream: present(fields.next().unwrap_or_default()),
                track: present(fields.next().unwrap_or_default()),
                updated: present(fields.next().unwrap_or_default()),
            })
        })
        .collect()
}

/// Lists open pull requests with the GitHub CLI when it is installed and signed in.
fn pull_requests(root: &Path) -> Result<Vec<PullRequestInfo>, String> {
    let child = Command::new("gh")
        .current_dir(root)
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "20",
            "--json",
            "number,title,headRefName,state,isDraft,url",
        ])
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|_| "GitHub CLI (gh) is not installed".to_owned())?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(child.wait_with_output());
    });
    let out = receiver
        .recv_timeout(PR_TIMEOUT)
        .map_err(|_| "GitHub did not answer in time".to_owned())?
        .map_err(|err| err.to_string())?;
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr);
        let first = detail.lines().next().unwrap_or("GitHub CLI failed").trim();
        return Err(first.chars().take(160).collect());
    }
    let items: Vec<serde_json::Value> =
        serde_json::from_slice(&out.stdout).map_err(|err| err.to_string())?;
    Ok(items
        .iter()
        .map(|item| PullRequestInfo {
            number: item["number"].as_u64().unwrap_or_default(),
            title: item["title"].as_str().unwrap_or_default().to_owned(),
            branch: item["headRefName"].as_str().unwrap_or_default().to_owned(),
            state: item["state"].as_str().unwrap_or_default().to_owned(),
            draft: item["isDraft"].as_bool().unwrap_or(false),
            url: item["url"].as_str().unwrap_or_default().to_owned(),
        })
        .collect())
}

impl AppState {
    /// Branch, local branches and open pull requests for a project.
    pub async fn get_workspace(
        &self,
        project_root: String,
        include_pull_requests: bool,
    ) -> Result<WorkspaceInfo, String> {
        let root = canonical_project_dir(&project_root)?;
        let is_git = crate::diff::is_git_repo(&root);
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        tokio::task::spawn_blocking(move || {
            let (pull_requests, unavailable) = if !is_git {
                (Vec::new(), Some("Not a Git repository".to_owned()))
            } else if !include_pull_requests {
                (Vec::new(), None)
            } else {
                match pull_requests(&root) {
                    Ok(items) => (items, None),
                    Err(reason) => (Vec::new(), Some(reason)),
                }
            };
            WorkspaceInfo {
                root: root.to_string_lossy().into_owned(),
                name,
                is_git,
                branch: is_git
                    .then(|| crate::worktree::current_branch(&root))
                    .flatten(),
                branches: if is_git { branches(&root) } else { Vec::new() },
                pull_requests,
                pull_requests_unavailable: unavailable,
            }
        })
        .await
        .map_err(|err| err.to_string())
    }

    /// Lists one project directory: folders first, `.git` hidden, bounded.
    pub async fn list_project_dir(
        &self,
        project_root: String,
        path: String,
    ) -> Result<DirListing, String> {
        let root = canonical_project_dir(&project_root)?;
        let directory = resolve_inside(&root, &path)?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&directory).map_err(|err| err.to_string())? {
            let entry = entry.map_err(|err| err.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                continue;
            }
            let metadata = entry.metadata().map_err(|err| err.to_string())?;
            entries.push(DirEntry {
                path: relative_display(&root, &entry.path()),
                is_dir: metadata.is_dir(),
                size: metadata.is_file().then_some(metadata.len()),
                name,
            });
        }
        entries.sort_by(|left, right| {
            right
                .is_dir
                .cmp(&left.is_dir)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        let truncated = entries.len() > MAX_ENTRIES;
        entries.truncate(MAX_ENTRIES);
        Ok(DirListing {
            path: relative_display(&root, &directory),
            entries,
            truncated,
        })
    }

    /// Reads a project file for viewing, bounded to 256 KiB of UTF-8 text.
    pub async fn read_project_file(
        &self,
        project_root: String,
        path: String,
    ) -> Result<FileView, String> {
        let root = canonical_project_dir(&project_root)?;
        let file = resolve_inside(&root, &path)?;
        let metadata = std::fs::metadata(&file).map_err(|err| err.to_string())?;
        if !metadata.is_file() {
            return Err(format!("'{path}' is not a file"));
        }
        let mut bytes = std::fs::read(&file).map_err(|err| err.to_string())?;
        let truncated = bytes.len() > MAX_FILE_BYTES;
        bytes.truncate(MAX_FILE_BYTES);
        let text = match String::from_utf8(bytes) {
            Ok(text) => Some(text),
            // A cut can split a multi-byte character; keep the valid prefix.
            Err(error) if truncated && error.utf8_error().error_len().is_none() => {
                let valid = error.utf8_error().valid_up_to();
                String::from_utf8(error.into_bytes()[..valid].to_vec()).ok()
            }
            Err(_) => None,
        };
        Ok(FileView {
            path: relative_display(&root, &file),
            binary: text.is_none(),
            content: text,
            size: metadata.len(),
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]
        /// No combination of names, `.`/`..`, separators or symlinks resolves
        /// outside the project; accepted paths exist inside it.
        #[cfg(unix)]
        #[test]
        fn resolved_paths_never_leave_the_project(
            parts in proptest::collection::vec(
                proptest::sample::select(vec!["src", "inner", "..", ".", "out", "inside_link", "a.txt", "", "missing", "\\", "src\\inner"]),
                0..6,
            ),
            absolute in proptest::prelude::any::<bool>(),
        ) {
            let outside = tempfile::tempdir().unwrap();
            let project = tempfile::tempdir().unwrap();
            let root = project.path().canonicalize().unwrap();
            std::fs::create_dir_all(root.join("src/inner")).unwrap();
            std::fs::write(root.join("src/a.txt"), "a").unwrap();
            std::fs::write(outside.path().join("a.txt"), "secret").unwrap();
            std::os::unix::fs::symlink(outside.path(), root.join("out")).unwrap();
            std::os::unix::fs::symlink(root.join("src"), root.join("inside_link")).unwrap();
            let relative = format!("{}{}", if absolute { "/" } else { "" }, parts.join("/"));
            if let Ok(resolved) = resolve_inside(&root, &relative) {
                proptest::prop_assert!(resolved.starts_with(&root), "{relative} -> {}", resolved.display());
                proptest::prop_assert!(resolved.exists());
                proptest::prop_assert!(!relative.split('/').any(|part| part == ".."), "{relative} accepted with ..");
            }
        }
    }

    #[test]
    fn paths_cannot_leave_the_project() {
        let outside = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();
        assert!(resolve_inside(&root, "src").is_ok());
        assert!(resolve_inside(&root, "../").is_err());
        assert!(resolve_inside(&root, "/etc").is_err());
        #[cfg(unix)]
        assert!(resolve_inside(&root, "link/secret.txt")
            .unwrap_err()
            .contains("leaves the project"));
    }
}
