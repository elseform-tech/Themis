//! Shared-checkout diffs and worktree change lifecycle.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use crate::diff::{
    git_diff_cached, git_diff_head, git_diff_worktree, git_head, git_untracked, is_git_repo,
    is_readable_change, untracked_diff_file, MAX_DIFF_FILES, MAX_LISTED_DIFF_BYTES,
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
    if candidate.has_root()
        || candidate
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(format!(
            "invalid path '{path}': must be relative to the project root"
        ));
    }
    Ok(())
}

/// The whole listing stays bounded too: once it holds MAX_LISTED_DIFF_BYTES of text, later
/// files are listed without lines.
pub(super) fn bound_listed_lines(files: &mut [crate::types::DiffFile]) {
    let mut room = MAX_LISTED_DIFF_BYTES;
    for file in files {
        let bytes: usize = file
            .hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .map(|line| line.text.len() + 1)
            .sum();
        if bytes > room {
            file.hunks.clear();
            file.too_large = true;
        } else {
            room -= bytes;
        }
    }
}

/// Puts a tracked file back as the last commit has it.
fn restore_from_head(root: &Path, path: &str) -> Result<(), String> {
    let out = crate::diff::guarded_git(root)?
        .args(["checkout", "HEAD", "--", path])
        .output()
        .map_err(|err| format!("failed to run git checkout: {err}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "git checkout failed for '{path}': {}",
            stderr.trim()
        ));
    }
    Ok(())
}

fn read_diff(root: &Path, preexisting: &HashSet<String>) -> Result<DiffState, String> {
    if !is_git_repo(root) {
        return Ok(DiffState {
            available: false,
            reason: Some("project is not a git repository".to_owned()),
            files: Vec::new(),
        });
    }
    let mut files = tracked_diff_files(root)?;
    // A path git reports as both changed and untracked counts once, as changed.
    let mut listed: HashSet<String> = files.iter().map(|file| file.path.clone()).collect();
    let before = files.len();
    files.retain(|file| is_readable_change(root, &file.path));
    let mut hidden = before - files.len();
    let mut total = files.len();
    // The counts need a look at every new file. Only the ones shown are read in full.
    for relative in git_untracked(root)? {
        crate::diff::count_listed_item();
        if listed.contains(&relative) {
            continue;
        }
        if !is_readable_change(root, &relative) {
            hidden += 1;
            continue;
        }
        total += 1;
        if files.len() < MAX_DIFF_FILES {
            files.push(untracked_diff_file(root, &relative));
        }
        listed.insert(relative);
    }
    for file in &mut files {
        file.preexisting = preexisting.contains(&file.path)
            || file
                .old_path
                .as_deref()
                .is_some_and(|old| preexisting.contains(old));
    }
    let mut notes = Vec::new();
    if total > MAX_DIFF_FILES {
        files.truncate(MAX_DIFF_FILES);
        notes.push(format!(
            "truncated: showing {MAX_DIFF_FILES} of {total} changed files"
        ));
    }
    bound_listed_lines(&mut files);
    if hidden > 0 {
        notes.push(format!(
            "{hidden} binary or generated file{} hidden",
            if hidden == 1 { "" } else { "s" }
        ));
    }
    let reason = (!notes.is_empty()).then(|| notes.join(" · "));
    Ok(DiffState {
        available: true,
        reason,
        files,
    })
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
        // Git and file reads block, and the live diff asks for this every few seconds.
        tokio::task::spawn_blocking(move || read_diff(&root, &preexisting))
            .await
            .map_err(|err| err.to_string())?
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
            let _ = crate::diff::guarded_git(&root)?
                .args(["rm", "--cached", "-q", "--", &path])
                .output();
            std::fs::remove_file(root.join(&path))
                .map_err(|err| format!("cannot delete '{path}': {err}"))?;
        } else {
            restore_from_head(&root, &path)?;
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

#[cfg(test)]
mod tests {
    /// Restoring a file runs outside the sandbox: a smudge filter the repository names doesn't start.
    #[cfg(unix)]
    #[test]
    fn restoring_a_file_starts_none_of_the_repositorys_programs() {
        use std::os::unix::fs::PermissionsExt;
        let repo = committed_repo();
        let marker = repo.path().join("program-ran");
        let script = repo.path().join(".git/program.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\ncat\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .status()
                .unwrap()
                .success());
        };
        git(&["config", "filter.x.smudge", &script.to_string_lossy()]);
        git(&["config", "filter.x.required", "true"]);
        std::fs::write(repo.path().join(".git/info/attributes"), "* filter=x\n").unwrap();
        std::fs::write(repo.path().join("kept.txt"), "edited\n").unwrap();
        restore_from_head(repo.path(), "kept.txt").unwrap();
        assert!(!marker.exists(), "restoring ran a program");
        assert_ne!(
            std::fs::read_to_string(repo.path().join("kept.txt")).unwrap(),
            "edited\n"
        );
    }

    use super::*;
    use crate::diff::scale_tests::committed_repo;
    use crate::diff::take_work;
    use crate::scale::{growth_rate, measure};
    use crate::types::DiffStatus;

    /// Visible files sort first, so each hidden one is checked against the whole visible list.
    fn dirty_repo(hidden: usize, visible: usize) -> tempfile::TempDir {
        let repo = committed_repo();
        for index in 0..visible {
            let line = format!("visible {index}\n");
            std::fs::write(repo.path().join(format!("a{index:05}.txt")), line).unwrap();
        }
        for index in 0..hidden {
            std::fs::write(repo.path().join(format!("b{index:05}.bin")), [0u8, 1]).unwrap();
        }
        repo
    }

    /// Changed files come first, then new ones by name, with both notes counting everything.
    #[test]
    fn listing_keeps_its_order_and_counts_past_the_cap() {
        let repo = dirty_repo(3, MAX_DIFF_FILES + 5);
        std::fs::write(repo.path().join("kept.txt"), "edited\n").unwrap();
        std::fs::write(repo.path().join("cache.pyc"), "generated\n").unwrap();
        let preexisting = HashSet::from(["a00000.txt".to_owned()]);
        let state = read_diff(repo.path(), &preexisting).unwrap();
        assert_eq!(state.files.len(), MAX_DIFF_FILES);
        assert_eq!(state.files[0].path, "kept.txt");
        assert_eq!(state.files[0].status, DiffStatus::Modified);
        assert!(!state.files[0].preexisting);
        assert_eq!(state.files[1].path, "a00000.txt");
        assert_eq!(state.files[1].status, DiffStatus::Added);
        assert_eq!(state.files[1].added, 1);
        assert!(state.files[1].preexisting);
        assert_eq!(state.files[199].path, "a00198.txt");
        assert_eq!(
            state.reason.as_deref(),
            Some(
                "truncated: showing 200 of 206 changed files · 4 binary or generated files hidden"
            )
        );
    }

    #[test]
    fn listing_many_hidden_files_stays_linear() {
        let samples = measure(&[625, 5000], |hidden| {
            let visible = hidden / 10;
            let repo = dirty_repo(hidden, visible);
            take_work();
            let state = read_diff(repo.path(), &HashSet::new()).unwrap();
            let work = take_work();
            eprintln!("read_diff {hidden} hidden, {visible} visible: {work:?}");
            assert_eq!(state.files.len(), visible.min(MAX_DIFF_FILES));
            assert!(state
                .reason
                .unwrap()
                .contains(&format!("{hidden} binary or generated files hidden")));
            work.items + work.opens
        });
        let rate = growth_rate(&samples);
        assert!(rate < 1.2, "growth {rate}: {samples:?}");
    }

    #[test]
    fn listing_builds_no_more_diffs_than_it_shows() {
        let samples = measure(&[625, 5000], |visible| {
            let repo = dirty_repo(0, visible);
            take_work();
            let state = read_diff(repo.path(), &HashSet::new()).unwrap();
            let work = take_work();
            eprintln!("read_diff {visible} visible: {work:?}");
            assert_eq!(state.files.len(), MAX_DIFF_FILES);
            assert_eq!(
                state.reason.as_deref(),
                Some(
                    format!("truncated: showing {MAX_DIFF_FILES} of {visible} changed files")
                        .as_str()
                )
            );
            assert!(
                work.built <= MAX_DIFF_FILES as u64,
                "built {} diffs to show {MAX_DIFF_FILES}",
                work.built
            );
            // One look at each file, then the shown ones read for their text and line count.
            let most_opens = (visible + 2 * MAX_DIFF_FILES) as u64;
            assert!(work.opens <= most_opens, "{} opens", work.opens);
            work.opens
        });
        let rate = growth_rate(&samples);
        assert!(rate < 1.2, "growth {rate}: {samples:?}");
    }

    proptest::proptest! {
        /// Accepted diff paths are relative and never step above the project.
        #[test]
        fn accepted_diff_paths_stay_relative(path in "(\\.\\./|/|[a-z]{1,4}/|\\./){0,4}[a-z.]{0,6}") {
            if ensure_root_relative(&path).is_ok() {
                let candidate = Path::new(&path);
                proptest::prop_assert!(!candidate.has_root());
                proptest::prop_assert!(!candidate.components().any(|part| matches!(part, Component::ParentDir | Component::Prefix(_))));
            }
        }
    }
}
