//! Splitting an allowed folder around excluded subtrees.
//!
//! Landlock can only allow, never carve an exception out of an allowed folder.
//! To allow `root` except some subtrees, the allowance is given to every entry
//! beside the path that leads to an excluded subtree, recursively; folders on
//! that path only get directory listing. Entries created later directly inside
//! such a listing-only folder are not covered (documented limit).

use std::path::{Path, PathBuf};

/// Deepest recursion followed while splitting.
const MAX_DEPTH: usize = 64;

/// Grants covering everything under `root` except the `excluded` subtrees.
///
/// Returns `(full, list_only)`: `full` paths may be granted recursively;
/// `list_only` folders lead to an excluded path and may only be listed. Paths
/// in `excluded` must be canonical. `budget` bounds the directory entries
/// visited; running out is an error rather than a partial (wider) grant.
/// Symlinked entries are granted by their resolved target, and only when that
/// target is unrelated to every excluded path.
pub fn split_around(
    root: &Path,
    excluded: &[PathBuf],
    budget: &mut usize,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), String> {
    let mut full = Vec::new();
    let mut list_only = Vec::new();
    walk(root, excluded, budget, 0, &mut full, &mut list_only)?;
    Ok((full, list_only))
}

fn walk(
    node: &Path,
    excluded: &[PathBuf],
    budget: &mut usize,
    depth: usize,
    full: &mut Vec<PathBuf>,
    list_only: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if excluded.iter().any(|path| node.starts_with(path)) {
        return Ok(());
    }
    if !excluded.iter().any(|path| path.starts_with(node)) {
        full.push(node.to_path_buf());
        return Ok(());
    }
    if depth >= MAX_DEPTH {
        return Err(format!("{} is nested too deeply to split", node.display()));
    }
    list_only.push(node.to_path_buf());
    let entries = match std::fs::read_dir(node) {
        Ok(entries) => entries,
        // An unreadable folder on the way to a secret: list it only.
        Err(_) => return Ok(()),
    };
    for entry in entries {
        if *budget == 0 {
            return Err("too many folders to split around protected paths".into());
        }
        *budget -= 1;
        let Ok(entry) = entry else { continue };
        let child = entry.path();
        let is_link = entry.file_type().is_ok_and(|kind| kind.is_symlink());
        if is_link {
            let Ok(target) = child.canonicalize() else {
                continue;
            };
            let related = excluded
                .iter()
                .any(|path| target.starts_with(path) || path.starts_with(&target));
            if !related {
                full.push(target);
            }
            continue;
        }
        walk(&child, excluded, budget, depth + 1, full, list_only)?;
    }
    Ok(())
}
