use super::*;

// ---------------------------------------------------------------------------
// Root-scoped path resolution
// ---------------------------------------------------------------------------

/// Resolve a tool-supplied path to an absolute path inside `root`.
///
/// `user_path` may be relative (resolved against `root`) or absolute.
/// Rejects escapes via `..`, absolute paths outside `root`, and symlinks
/// inside `root` that point outside it (symlinks are resolved with
/// `canonicalize`, then contained with `strip_prefix`).
///
/// Nonexistent paths are supported (needed for file creation): the nearest
/// existing ancestor is canonicalized and the remainder is lexically
/// appended, then the containment check applies to the result.
///
/// Note: this is check-then-use (TOCTOU). It stops confused or malicious
/// model output from wandering off-root, but a hostile actor racing the
/// filesystem could still swap a symlink between check and use.
pub fn resolve_within_root(root: &Path, user_path: &str) -> anyhow::Result<PathBuf> {
    let root_canon = root
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("project root '{}' is not accessible: {e}", root.display()))?;
    let joined = {
        let candidate = Path::new(user_path);
        if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            root_canon.join(candidate)
        }
    };
    let candidate = lexical_normalize(&joined);

    // Walk up to the nearest existing ancestor, remembering the tail.
    let mut ancestor = candidate.as_path();
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => match ancestor.file_name() {
                Some(name) => {
                    tail.push(name);
                    ancestor = ancestor.parent().ok_or_else(|| {
                        anyhow::anyhow!("path '{user_path}' escapes the project root")
                    })?;
                }
                None => {
                    anyhow::bail!("path '{user_path}' escapes the project root");
                }
            },
            Err(e) => anyhow::bail!("cannot access path '{user_path}': {e}"),
        }
    }

    let mut resolved = ancestor
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("cannot resolve path '{user_path}': {e}"))?;
    for part in tail.iter().rev() {
        resolved.push(part);
    }
    resolved
        .strip_prefix(&root_canon)
        .map_err(|_| anyhow::anyhow!("path '{user_path}' escapes the project root"))?;
    Ok(resolved)
}

pub(super) fn resolve_tool_path(
    root: &Path,
    path: &str,
    unrestricted: bool,
) -> anyhow::Result<PathBuf> {
    if !unrestricted {
        return resolve_within_root(root, path);
    }
    let absolute = root.join(path);
    let filesystem_root = root
        .ancestors()
        .last()
        .ok_or_else(|| anyhow::anyhow!("filesystem root unavailable"))?;
    resolve_within_root(filesystem_root, &absolute.to_string_lossy())
}
pub(super) fn plan_tool_patch(
    root: &Path,
    patch: &str,
    unrestricted: bool,
) -> anyhow::Result<Vec<patch::PlannedFile>> {
    if !unrestricted {
        return plan_patch(root, patch);
    }
    let mut normalized = String::new();
    for line in patch.lines() {
        if let Some((prefix, path, strip)) = line
            .strip_prefix("--- ")
            .map(|p| ("--- ", p, "a/"))
            .or_else(|| line.strip_prefix("+++ ").map(|p| ("+++ ", p, "b/")))
        {
            normalized.push_str(prefix);
            if path == "/dev/null" {
                normalized.push_str(path);
            } else {
                normalized.push_str(
                    &resolve_tool_path(root, path.strip_prefix(strip).unwrap_or(path), true)?
                        .to_string_lossy(),
                );
            }
        } else {
            normalized.push_str(line);
        }
        normalized.push('\n');
    }
    let mut plan = plan_patch(
        root.ancestors()
            .last()
            .ok_or_else(|| anyhow::anyhow!("filesystem root unavailable"))?,
        &normalized,
    )?;
    // Planned against `/`, paths read as `private/tmp/...`: report them
    // relative to the project, or whole when outside it.
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    for file in &mut plan {
        file.relative = file
            .path
            .strip_prefix(&canonical_root)
            .or_else(|_| file.path.strip_prefix(root))
            .map_or_else(
                |_| file.path.to_string_lossy().into_owned(),
                // `/` on every platform, like the restricted tool's patch paths.
                |inside| {
                    inside
                        .components()
                        .map(|part| part.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/")
                },
            );
    }
    Ok(plan)
}

/// Collapse `.` and `..` lexically (no filesystem access).
pub(super) fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                out.push(component.as_os_str());
            }
        }
    }
    out
}
