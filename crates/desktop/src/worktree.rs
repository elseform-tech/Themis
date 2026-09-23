//! Per-thread git worktrees: branch + worktree lifecycle and apply-based merge.
//!
//! Every helper shells out to the `git` binary via argv (never through a
//! shell). All paths are explicit parameters so tests can point at temp dirs
//! instead of the real app data dir.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Branch prefix for thread branches (`themis/<short-id>`).
pub const BRANCH_PREFIX: &str = "themis/";
/// Thread-id chars embedded in the branch name.
pub const BRANCH_SHORT_LEN: usize = 8;

/// A thread's backing worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    /// Absolute checkout path (`<worktrees-root>/<thread-id>`).
    pub path: PathBuf,
    /// Thread branch (`themis/<short-id>`).
    pub branch: String,
    /// Base revision the branch forked from (branch name, or the `HEAD` SHA
    /// when the checkout is detached).
    pub base_branch: String,
}

/// Returns true when the `git` binary runs.
#[must_use]
pub fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Branch name for `thread_id` (`themis/` + its first 8 chars).
#[must_use]
pub fn branch_for_thread(thread_id: &str) -> String {
    let short: String = thread_id.chars().take(BRANCH_SHORT_LEN).collect();
    format!("{BRANCH_PREFIX}{short}")
}

/// Worktree checkout path for `thread_id` under `root`.
#[must_use]
pub fn worktree_path_for(root: &Path, thread_id: &str) -> PathBuf {
    root.join(thread_id)
}

/// Current branch of `repo`, or `None` when detached or unreadable.
#[must_use]
pub fn current_branch(repo: &Path) -> Option<String> {
    let out = git_in(repo)
        .args(["branch", "--show-current"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Base revision for a new thread branch: the current branch, else the `HEAD`
/// SHA (detached checkouts).
pub fn base_revision(repo: &Path) -> Option<String> {
    if let Some(branch) = current_branch(repo) {
        return Some(branch);
    }
    crate::diff::git_head(repo)
}

/// Returns true when `branch` exists in `repo`.
#[must_use]
pub fn branch_exists(repo: &Path, branch: &str) -> bool {
    git_in(repo)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Creates branch `themis/<short-id>` plus a worktree at `<root>/<thread-id>`.
///
/// The worktree starts at `HEAD` (or as an orphan branch when the repo has no
/// commits yet). On any failure nothing is left behind: a half-created branch
/// or directory is removed again before the error returns.
pub fn create_worktree(
    repo: &Path,
    worktrees_root: &Path,
    thread_id: &str,
) -> Result<WorktreeInfo, String> {
    if thread_id.trim().is_empty()
        || thread_id.contains('/')
        || thread_id.contains('\\')
        || thread_id.contains("..")
    {
        return Err("thread id is not a plain directory name".to_owned());
    }
    std::fs::create_dir_all(worktrees_root).map_err(|err| {
        format!(
            "failed to create worktrees dir '{}': {err}",
            worktrees_root.display()
        )
    })?;
    let path = worktree_path_for(worktrees_root, thread_id);
    if path.exists() {
        return Err(format!("worktree path '{}' already exists", path.display()));
    }
    let base = base_revision(repo)
        .ok_or_else(|| format!("cannot determine a base revision in '{}'", repo.display()))?;
    let mut branch = branch_for_thread(thread_id);
    if branch_exists(repo, &branch) {
        // Practically unreachable (uuid prefix), but never clobber a branch.
        let extra: String = thread_id
            .chars()
            .filter(|char| *char != '-')
            .skip(BRANCH_SHORT_LEN)
            .take(4)
            .collect();
        branch = format!("{branch}-{extra}");
        if branch_exists(repo, &branch) {
            return Err(format!("branch '{branch}' already exists"));
        }
    }
    let has_head = crate::diff::git_head(repo).is_some();
    let mut cmd = git_in(repo);
    cmd.arg("worktree")
        .arg("add")
        .arg("-b")
        .arg(&branch)
        .arg(&path);
    if has_head {
        cmd.arg("HEAD");
    }
    if let Err(err) = run_git(&mut cmd, "git worktree add") {
        cleanup_after_failed_create(repo, &path, &branch);
        return Err(err);
    }
    if !path.is_dir() {
        cleanup_after_failed_create(repo, &path, &branch);
        return Err(format!(
            "git worktree add reported success but '{}' is missing",
            path.display()
        ));
    }
    Ok(WorktreeInfo {
        path,
        branch,
        base_branch: base,
    })
}

/// Removes `worktree` from `repo` (already-gone worktrees are a success).
pub fn remove_worktree(repo: &Path, worktree: &Path) -> Result<(), String> {
    let mut cmd = git_in(repo);
    cmd.arg("worktree")
        .arg("remove")
        .arg("--force")
        .arg(worktree);
    let result = run_git(&mut cmd, "git worktree remove");
    if worktree.exists() {
        result?;
        if worktree.exists() {
            return Err(format!(
                "worktree '{}' still exists after removal",
                worktree.display()
            ));
        }
    }
    Ok(())
}

/// Forgets stale worktree metadata in `repo` (best-effort cleanup helper).
pub fn prune_worktrees(repo: &Path) {
    let _ = run_git(
        git_in(repo).args(["worktree", "prune"]),
        "git worktree prune",
    );
}

/// Deletes `branch` in `repo` (already-gone branches are a success).
pub fn delete_branch(repo: &Path, branch: &str) -> Result<(), String> {
    let result = run_git(git_in(repo).args(["branch", "-D", branch]), "git branch -D");
    if let Err(err) = result {
        if branch_exists(repo, branch) {
            return Err(err);
        }
    }
    Ok(())
}

/// SHA of the empty tree (universal across repos).
const EMPTY_TREE_SHA: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Full patch for merging `worktree` into its checkout, plus the touched files.
///
/// The worktree is diffed against its fork point (`merge-base(base, branch)`),
/// not against the base ref directly: the user's checkout may have advanced
/// past the creation commit, and diffing against the moved ref would silently
/// drop (or revert) the checkout's own commits. Untracked files are recorded
/// with `git add -N` (index-only, no commits) so they appear as additions.
///
/// Orphan mode (the repo had no commits when the thread was created, so the
/// worktree `HEAD` is unborn): the whole worktree content versus nothing is
/// the patch, whatever the checkout did meanwhile. When both sides committed
/// from an orphan start (no common ancestor), the patch is the committed
/// part versus the empty tree plus the uncommitted part versus the tip. A
/// deleted base or thread branch is a loud error, never a guessed diff.
pub fn merge_patch(
    worktree: &Path,
    base: &str,
    branch: &str,
) -> Result<(Vec<u8>, Vec<String>), String> {
    run_git(git_in(worktree).args(["add", "-N", "-A"]), "git add -N")?;
    if let Some(fork) = merge_base(worktree, base, branch) {
        return diff_against(worktree, Some(&fork));
    }
    // No fork point: find out why before diffing anything.
    if crate::diff::git_head(worktree).is_none() {
        // Orphan mode: nothing was ever committed on the thread side.
        return diff_against(worktree, None);
    }
    if !rev_is_commit(worktree, branch) {
        return Err(format!(
            "cannot merge: thread branch '{branch}' no longer exists"
        ));
    }
    if !rev_is_commit(worktree, base) {
        return Err(format!(
            "cannot merge: base branch '{base}' no longer exists"
        ));
    }
    // Both sides have commits but no common ancestor: committed-vs-empty plus
    // uncommitted-vs-tip, concatenated (sequential `diff --git` sections
    // apply as one patch).
    let (mut patch, mut files) = diff_range(worktree, EMPTY_TREE_SHA, branch)?;
    let (uncommitted, uncommitted_files) = diff_against(worktree, Some(branch))?;
    patch.extend_from_slice(&uncommitted);
    for file in uncommitted_files {
        if !files.contains(&file) {
            files.push(file);
        }
    }
    Ok((patch, files))
}

/// Diffs the worktree (tracked changes plus intent-to-add untracked files)
/// against `rev`, or against nothing when `rev` is `None`.
fn diff_against(worktree: &Path, rev: Option<&str>) -> Result<(Vec<u8>, Vec<String>), String> {
    let mut diff = git_in(worktree);
    diff.arg("diff").arg("--no-color");
    if let Some(rev) = rev {
        diff.arg(rev);
    }
    let patch = run_git(&mut diff, "git diff")?;
    let mut names = git_in(worktree);
    names.arg("diff").arg("--name-only").arg("-z");
    if let Some(rev) = rev {
        names.arg(rev);
    }
    let raw = run_git(&mut names, "git diff --name-only")?;
    Ok((patch, split_names(&raw)))
}

/// Diffs committed `old..new` (no working-tree content).
fn diff_range(worktree: &Path, old: &str, new: &str) -> Result<(Vec<u8>, Vec<String>), String> {
    let patch = run_git(
        git_in(worktree).args(["diff", "--no-color", old, new]),
        "git diff",
    )?;
    let raw = run_git(
        git_in(worktree).args(["diff", "--name-only", "-z", old, new]),
        "git diff --name-only",
    )?;
    Ok((patch, split_names(&raw)))
}

fn split_names(raw: &[u8]) -> Vec<String> {
    raw.split(|byte| *byte == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect()
}

fn merge_base(repo: &Path, left: &str, right: &str) -> Option<String> {
    let out = git_in(repo)
        .args(["merge-base", left, right])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}

fn rev_is_commit(repo: &Path, rev: &str) -> bool {
    git_in(repo)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ])
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Dry-runs `patch` against `checkout` (read-only).
///
/// Returns the conflicting paths, or an empty vec when the patch applies
/// cleanly. When git's stderr names no paths, `files` (the whole patch) is
/// reported so callers never get an empty conflict list for a failed check.
pub fn apply_check(checkout: &Path, patch: &[u8], files: &[String]) -> Result<Vec<String>, String> {
    let out = git_with_stdin(checkout, &["apply", "--check", "-"], patch)?;
    if out.status.success() {
        return Ok(Vec::new());
    }
    let mut conflicts = parse_apply_conflicts(&String::from_utf8_lossy(&out.stderr));
    if conflicts.is_empty() {
        conflicts = files.to_vec();
    }
    Ok(conflicts)
}

/// Applies `patch` to `checkout`'s working tree (uncommitted; `git apply` is
/// atomic, so a failure leaves the checkout untouched).
pub fn apply_patch(checkout: &Path, patch: &[u8]) -> Result<(), String> {
    let out = git_with_stdin(checkout, &["apply", "-"], patch)?;
    if !out.status.success() {
        return Err(format!(
            "git apply failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Extracts conflicting paths from `git apply --check` stderr.
///
/// Understands `error: patch failed: <path>:<hunk>` as well as `<path>: ...`
/// diagnostics such as `already exists in working directory`.
#[must_use]
pub fn parse_apply_conflicts(stderr: &str) -> Vec<String> {
    let mut conflicts = Vec::new();
    for line in stderr.lines() {
        let from_patch_failed = line
            .strip_prefix("error: patch failed: ")
            .and_then(|rest| rest.rsplit_once(':').map(|(path, _)| path));
        let path = from_patch_failed.or_else(|| {
            let rest = line.strip_prefix("error:")?.trim_start();
            let (candidate, _) = rest.split_once(':')?;
            let candidate = candidate.trim();
            if candidate.is_empty() || candidate.contains(' ') {
                None
            } else {
                Some(candidate)
            }
        });
        if let Some(path) = path.map(str::trim).filter(|path| !path.is_empty()) {
            if !conflicts.iter().any(|known: &String| known == path) {
                conflicts.push(path.to_owned());
            }
        }
    }
    conflicts
}

/// Parses the `.git` pointer file of a linked worktree (`gitdir: <path>`).
pub fn worktree_gitdir(worktree: &Path) -> Option<PathBuf> {
    let dotgit = worktree.join(".git");
    if !dotgit.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(&dotgit).ok()?;
    let target = text.trim().strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return None;
    }
    Some(PathBuf::from(target))
}

/// Returns true when `path` looks like a linked worktree (a `.git` pointer file).
#[must_use]
pub fn is_worktree_dir(path: &Path) -> bool {
    worktree_gitdir(path).is_some()
}

/// Removes one orphan worktree dir under `worktrees_root`.
///
/// Refuses anything that is not our exact naming (a uuid dir), is not a plain
/// directory name, or does not look like a linked worktree — those are left in
/// place with an explanatory error.
pub fn remove_orphan(worktrees_root: &Path, name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(format!(
            "refusing to prune '{name}': not a plain directory name"
        ));
    }
    if uuid::Uuid::parse_str(name).is_err() {
        return Err(format!(
            "refusing to prune '{name}': not one of our worktree names"
        ));
    }
    let dir = worktrees_root.join(name);
    if !dir.exists() {
        return Ok(());
    }
    if !dir.is_dir() || !is_worktree_dir(&dir) {
        return Err(format!(
            "refusing to prune '{name}': not a git worktree, leaving it in place"
        ));
    }
    // Resolve the main repo before removal so it can be told to forget us.
    let main_gitdir =
        worktree_gitdir(&dir).and_then(|gitdir| gitdir.parent()?.parent().map(Path::to_path_buf));
    let _ = run_git(
        git_in(&dir)
            .args(["worktree", "remove", "--force"])
            .arg(&dir),
        "git worktree remove",
    );
    if dir.exists() {
        // `remove` also fails when the main repo is gone; the dir is ours
        // (uuid-named, directly under our root), so drop it directly.
        std::fs::remove_dir_all(&dir)
            .map_err(|err| format!("failed to remove orphan worktree '{name}': {err}"))?;
    }
    if let Some(main) = main_gitdir.filter(|main| main.is_dir()) {
        let _ = Command::new("git")
            .arg("--git-dir")
            .arg(&main)
            .args(["worktree", "prune"])
            .output();
    }
    Ok(())
}

fn cleanup_after_failed_create(repo: &Path, path: &Path, branch: &str) {
    let _ = remove_worktree(repo, path);
    if branch_exists(repo, branch) {
        let _ = delete_branch(repo, branch);
    }
    if path.is_dir() {
        let _ = std::fs::remove_dir_all(path);
    }
}

fn git_in(repo: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo);
    cmd
}

fn run_git(cmd: &mut Command, what: &str) -> Result<Vec<u8>, String> {
    let out = cmd
        .output()
        .map_err(|err| format!("failed to run {what}: {err}"))?;
    if !out.status.success() {
        return Err(format!(
            "{what} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

fn git_with_stdin(dir: &Path, args: &[&str], input: &[u8]) -> Result<std::process::Output, String> {
    use std::io::Write;
    use std::process::Stdio;

    let what = format!("git {}", args.join(" "));
    let mut child = git_in(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to run {what}: {err}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input)
            .map_err(|err| format!("failed to write to {what}: {err}"))?;
    }
    child
        .wait_with_output()
        .map_err(|err| format!("failed to run {what}: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        std::fs::create_dir(&root).expect("mkdir");
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.email", "themis@test"]);
        git(&root, &["config", "user.name", "themis"]);
        std::fs::write(root.join("file.txt"), "line1\nline2\nline3\n").expect("write");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "init"]);
        (temp, root)
    }

    #[test]
    fn branch_names_embed_the_id_prefix() {
        assert_eq!(
            branch_for_thread("12345678-9abc-def0-1234-56789abcdef0"),
            "themis/12345678"
        );
        assert!(
            branch_for_thread("12345678-9abc-def0-1234-56789abcdef0").starts_with(BRANCH_PREFIX)
        );
    }

    #[test]
    fn conflict_parsing_covers_git_diagnostics() {
        let stderr = "error: patch failed: src/a.rs:12\nerror: src/a.rs: patch does not apply\n";
        assert_eq!(parse_apply_conflicts(stderr), vec!["src/a.rs"]);
        let stderr = "error: new.txt: already exists in working directory\n";
        assert_eq!(parse_apply_conflicts(stderr), vec!["new.txt"]);
        assert!(parse_apply_conflicts("").is_empty());
        // Multi-word diagnostics without a path shape are ignored.
        assert!(parse_apply_conflicts("error: some vague failure").is_empty());
    }

    #[test]
    fn create_and_remove_roundtrip() {
        if !git_available() {
            eprintln!("skipping: git binary not found");
            return;
        }
        let (_temp, repo) = init_repo();
        let root = _temp.path().join("worktrees");
        let id = uuid::Uuid::new_v4().to_string();
        let info = create_worktree(&repo, &root, &id).expect("create");
        assert_eq!(info.path, root.join(&id));
        assert_eq!(info.branch, branch_for_thread(&id));
        assert!(!info.base_branch.is_empty());
        assert!(info.path.join("file.txt").is_file());
        assert!(branch_exists(&repo, &info.branch));
        assert!(is_worktree_dir(&info.path));

        remove_worktree(&repo, &info.path).expect("remove");
        assert!(!info.path.exists());
        delete_branch(&repo, &info.branch).expect("delete branch");
        assert!(!branch_exists(&repo, &info.branch));
        // Second removal is a no-op success.
        remove_worktree(&repo, &info.path).expect("idempotent remove");
        delete_branch(&repo, &info.branch).expect("idempotent delete");
    }

    #[test]
    fn create_succeeds_on_repos_without_commits() {
        if !git_available() {
            eprintln!("skipping: git binary not found");
            return;
        }
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).expect("mkdir");
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "themis@test"]);
        git(&repo, &["config", "user.name", "themis"]);
        let id = uuid::Uuid::new_v4().to_string();
        let info = create_worktree(&repo, &temp.path().join("worktrees"), &id).expect("create");
        assert!(info.path.is_dir());
        // Unborn branches have no ref yet, so the worktree's checked-out
        // branch (not `branch_exists`) proves the branch was created.
        assert_eq!(
            current_branch(&info.path).as_deref(),
            Some(info.branch.as_str())
        );
    }

    #[test]
    fn create_rejects_bad_ids_and_existing_paths() {
        if !git_available() {
            eprintln!("skipping: git binary not found");
            return;
        }
        let (_temp, repo) = init_repo();
        let root = _temp.path().join("worktrees");
        for bad in ["", "  ", "../escape", "a/b", "a\\b"] {
            assert!(create_worktree(&repo, &root, bad).is_err(), "{bad:?}");
        }
        let id = uuid::Uuid::new_v4().to_string();
        create_worktree(&repo, &root, &id).expect("first create");
        let err = create_worktree(&repo, &root, &id).expect_err("second create");
        assert!(err.contains("already exists"), "{err}");
        // The failed retry left no extra branch behind.
        let out = git_in(&repo)
            .args(["branch", "--list", "themis/*"])
            .output()
            .expect("list");
        assert_eq!(String::from_utf8_lossy(&out.stdout).lines().count(), 1);
    }

    #[test]
    fn merge_patch_covers_tracked_and_untracked_changes() {
        if !git_available() {
            eprintln!("skipping: git binary not found");
            return;
        }
        let (_temp, repo) = init_repo();
        let id = uuid::Uuid::new_v4().to_string();
        let info = create_worktree(&repo, &_temp.path().join("worktrees"), &id).expect("create");
        std::fs::write(info.path.join("file.txt"), "changed\nline2\nline3\n").expect("write");
        std::fs::write(info.path.join("added.txt"), "new\n").expect("write");
        let (patch, files) =
            merge_patch(&info.path, &info.base_branch, &info.branch).expect("patch");
        assert!(!patch.is_empty());
        assert_eq!(files.len(), 2);
        assert!(files.contains(&"file.txt".to_owned()));
        assert!(files.contains(&"added.txt".to_owned()));
        // A clean checkout passes the check...
        assert_eq!(
            apply_check(&repo, &patch, &files).expect("check"),
            Vec::<String>::new()
        );
        // ...but a conflicting committed change does not.
        std::fs::write(repo.join("file.txt"), "other\nline2\nline3\n").expect("write");
        git(&repo, &["commit", "-qam", "conflict"]);
        let conflicts = apply_check(&repo, &patch, &files).expect("recheck");
        assert_eq!(conflicts, vec!["file.txt".to_owned()]);
    }

    #[test]
    fn merge_patch_tracks_the_fork_point_when_base_advances() {
        if !git_available() {
            eprintln!("skipping: git binary not found");
            return;
        }
        let (_temp, repo) = init_repo();
        let id = uuid::Uuid::new_v4().to_string();
        let info = create_worktree(&repo, &_temp.path().join("worktrees"), &id).expect("create");
        std::fs::write(info.path.join("file.txt"), "thread\nline2\nline3\n").expect("write");
        // The checkout advances past the creation commit (non-overlapping).
        std::fs::write(repo.join("other.txt"), "checkout\n").expect("write");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "advance"]);
        // The patch still carries the thread change (a naive `diff base`
        // would diff against the moved ref instead of the fork point)...
        let (patch, files) =
            merge_patch(&info.path, &info.base_branch, &info.branch).expect("patch");
        assert_eq!(files, vec!["file.txt".to_owned()]);
        // ... and it applies without reverting the checkout's own commit.
        assert_eq!(
            apply_check(&repo, &patch, &files).expect("check"),
            Vec::<String>::new()
        );
        apply_patch(&repo, &patch).expect("apply");
        assert_eq!(
            std::fs::read_to_string(repo.join("file.txt")).expect("read"),
            "thread\nline2\nline3\n"
        );
        assert!(repo.join("other.txt").is_file());
    }

    #[test]
    fn remove_orphan_is_conservative() {
        if !git_available() {
            eprintln!("skipping: git binary not found");
            return;
        }
        let (_temp, repo) = init_repo();
        let root = _temp.path().join("worktrees");
        std::fs::create_dir_all(&root).expect("mkdir");
        // Non-uuid names are refused and left alone.
        let foreign = root.join("someone-elses-dir");
        std::fs::create_dir(&foreign).expect("mkdir");
        let err = remove_orphan(&root, "someone-elses-dir").expect_err("refused");
        assert!(err.contains("not one of our"), "{err}");
        assert!(foreign.is_dir());
        // Uuid-named but not a worktree: refused too.
        let fake = uuid::Uuid::new_v4().to_string();
        std::fs::create_dir(root.join(&fake)).expect("mkdir");
        let err = remove_orphan(&root, &fake).expect_err("refused");
        assert!(err.contains("not a git worktree"), "{err}");
        assert!(root.join(&fake).is_dir());
        // A real orphaned worktree is removed.
        let orphan = uuid::Uuid::new_v4().to_string();
        git(
            &repo,
            &[
                "worktree",
                "add",
                &root.join(&orphan).to_string_lossy(),
                "HEAD",
            ],
        );
        assert!(root.join(&orphan).is_dir());
        remove_orphan(&root, &orphan).expect("pruned");
        assert!(!root.join(&orphan).exists());
    }
}
