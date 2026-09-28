//! Shared-checkout diffs and worktree change lifecycle.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use crate::diff::{
    git_diff_cached, git_diff_head, git_diff_worktree, git_head, git_untracked, is_git_repo,
    untracked_diff_file, MAX_DIFF_FILES,
};
use crate::types::{DiffState, MergeResult};
use crate::worktree;

use super::{AppState, ThreadRecord};

/// Diff and discard operations use the same shared checkout as agent tools.
fn diff_root_for(record: &ThreadRecord) -> Result<PathBuf, String> {
    Ok(record.project_root.clone())
}

/// Tracked-file diffs, with a staged+unstaged fallback for repos without `HEAD`.
fn tracked_diff_files(root: &Path) -> Result<Vec<crate::types::DiffFile>, String> {
    if git_head(root).is_some() {
        return git_diff_head(root);
    }
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for file in git_diff_cached(root)?
        .into_iter()
        .chain(git_diff_worktree(root)?)
    {
        if seen.insert(file.path.clone()) {
            files.push(file);
        }
    }
    Ok(files)
}

/// Rejects absolute paths and `..` escapes (diff paths must stay in-root).
fn ensure_root_relative(path: &str) -> Result<(), String> {
    let candidate = Path::new(path);
    if candidate.is_absolute()
        || candidate
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "invalid path '{path}': must be relative to the project root"
        ));
    }
    Ok(())
}

impl AppState {
    /// Lists the shared project checkout diff (git repos only).
    pub async fn list_diff(&self, thread_id: String) -> Result<DiffState, String> {
        let (root, preexisting) = {
            let threads = self.inner.threads.read().await;
            let record = threads
                .get(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            match diff_root_for(record) {
                Ok(root) => (root, record.preexisting.clone()),
                Err(reason) => {
                    return Ok(DiffState {
                        available: false,
                        reason: Some(reason),
                        files: Vec::new(),
                    });
                }
            }
        };
        if !is_git_repo(&root) {
            return Ok(DiffState {
                available: false,
                reason: Some("project is not a git repository".to_owned()),
                files: Vec::new(),
            });
        }
        let mut files = tracked_diff_files(&root)?;
        let mut seen: HashSet<String> = files.iter().map(|file| file.path.clone()).collect();
        for relative in git_untracked(&root)? {
            if seen.insert(relative.clone()) {
                files.push(untracked_diff_file(&root, &relative));
            }
        }
        for file in &mut files {
            file.preexisting = preexisting.contains(&file.path)
                || file
                    .old_path
                    .as_deref()
                    .is_some_and(|old| preexisting.contains(old));
        }
        let total = files.len();
        let reason = if total > MAX_DIFF_FILES {
            files.truncate(MAX_DIFF_FILES);
            Some(format!(
                "truncated: showing {MAX_DIFF_FILES} of {total} changed files"
            ))
        } else {
            None
        };
        Ok(DiffState {
            available: true,
            reason,
            files,
        })
    }

    /// Records acceptance of a changed file (in-memory; the disk already
    /// carries the change).
    pub async fn accept_file(&self, thread_id: String, path: String) -> Result<(), String> {
        let mut threads = self.inner.threads.write().await;
        let record = threads
            .get_mut(&thread_id)
            .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
        record.accepted.insert(path);
        Ok(())
    }

    /// Discards a thread-made change: tracked files are restored from `HEAD`,
    /// added files are deleted. Preexisting files are refused.
    pub async fn discard_file(&self, thread_id: String, path: String) -> Result<(), String> {
        ensure_root_relative(&path)?;
        let root = {
            let threads = self.inner.threads.read().await;
            let record = threads
                .get(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            if record.preexisting.contains(&path) {
                return Err(format!(
                    "refusing to discard '{path}': it had preexisting changes \
                     before the thread started"
                ));
            }
            diff_root_for(record).map_err(|reason| format!("cannot discard: {reason}"))?
        };
        if !is_git_repo(&root) {
            return Err("cannot discard: project is not a git repository".to_owned());
        }
        if git_untracked(&root)?.iter().any(|name| name == &path) {
            std::fs::remove_file(root.join(&path))
                .map_err(|err| format!("cannot delete '{path}': {err}"))?;
        } else if git_head(&root).is_none() {
            // No commits yet: "tracked" means staged-new, so unstage and delete.
            let _ = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["rm", "--cached", "-q", "--", &path])
                .output();
            std::fs::remove_file(root.join(&path))
                .map_err(|err| format!("cannot delete '{path}': {err}"))?;
        } else {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["checkout", "HEAD", "--", &path])
                .output()
                .map_err(|err| format!("failed to run git checkout: {err}"))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(format!(
                    "git checkout failed for '{path}': {}",
                    stderr.trim()
                ));
            }
        }
        if let Some(record) = self.inner.threads.write().await.get_mut(&thread_id) {
            record.accepted.remove(&path);
        }
        Ok(())
    }

    /// Merges a thread's worktree changes into the user's checkout, apply-based.
    ///
    /// Computes the full working-tree-vs-base patch (tracked changes plus
    /// untracked files), dry-runs it with `git apply --check`, and only then
    /// applies it to the checkout's working tree — uncommitted, for the user
    /// to review. History is never mutated. On conflicts nothing is touched
    /// and the conflicting paths are returned. On success the worktree and
    /// branch are retired; the thread itself stays (marked merged internally).
    pub async fn merge_thread(&self, thread_id: String) -> Result<MergeResult, String> {
        let (checkout, worktree, branch, base) = {
            let threads = self.inner.threads.read().await;
            let record = threads
                .get(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            if record.running {
                return Err(format!(
                    "thread '{thread_id}' is busy: wait for the run to finish before merging"
                ));
            }
            let Some(worktree) = record.worktree_path.clone() else {
                if record.merged {
                    return Err(format!("thread '{thread_id}' is already merged"));
                }
                return Err(format!(
                    "cannot merge thread '{thread_id}': it has no git worktree (non-git project)"
                ));
            };
            let branch = record.branch.clone().ok_or_else(|| {
                format!("cannot merge thread '{thread_id}': no thread branch recorded")
            })?;
            let base = record.base_branch.clone().ok_or_else(|| {
                format!("cannot merge thread '{thread_id}': no base revision recorded")
            })?;
            (record.project_root.clone(), worktree, branch, base)
        };
        if !worktree.is_dir() {
            return Err(format!(
                "cannot merge thread '{thread_id}': worktree missing \
                 (expected at '{}')",
                worktree.display()
            ));
        }
        let (patch, files) = worktree::merge_patch(&worktree, &base, &branch)?;
        if files.is_empty() {
            // Nothing to apply; still retire the worktree and branch.
            self.retire_worktree(&thread_id, &checkout, &worktree, &branch)
                .await;
            return Ok(MergeResult {
                applied: true,
                applied_files: Vec::new(),
                conflicts: Vec::new(),
            });
        }
        let conflicts = worktree::apply_check(&checkout, &patch, &files)?;
        if !conflicts.is_empty() {
            return Ok(MergeResult {
                applied: false,
                applied_files: Vec::new(),
                conflicts,
            });
        }
        if let Err(err) = worktree::apply_patch(&checkout, &patch) {
            // The checkout moved between check and apply: re-check to report
            // conflicts precisely (`git apply` is atomic, so nothing landed).
            let recheck = worktree::apply_check(&checkout, &patch, &files).unwrap_or_default();
            if !recheck.is_empty() {
                return Ok(MergeResult {
                    applied: false,
                    applied_files: Vec::new(),
                    conflicts: recheck,
                });
            }
            return Err(err);
        }
        self.retire_worktree(&thread_id, &checkout, &worktree, &branch)
            .await;
        Ok(MergeResult {
            applied: true,
            applied_files: files,
            conflicts: Vec::new(),
        })
    }

    /// Retires a merged thread's worktree and branch (best-effort git cleanup;
    /// the thread record itself is always updated).
    async fn retire_worktree(
        &self,
        thread_id: &str,
        checkout: &Path,
        worktree_path: &Path,
        branch: &str,
    ) {
        let _ = worktree::remove_worktree(checkout, worktree_path);
        if worktree_path.exists() && worktree_path.starts_with(&self.inner.worktrees_root) {
            let _ = std::fs::remove_dir_all(worktree_path);
        }
        worktree::prune_worktrees(checkout);
        let _ = worktree::delete_branch(checkout, branch);
        if let Some(record) = self.inner.threads.write().await.get_mut(thread_id) {
            record.merged = true;
            record.worktree_path = None;
            record.branch = None;
        }
        self.persist_registry().await;
    }
}
