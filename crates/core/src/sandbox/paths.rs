//! Path handling: home and configured paths, canonical resolution, temporary folders and subagent write paths.

use super::*;

/// The user's home folder from the environment.
pub fn home_dir() -> Option<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// Expands a validated configured path (`~/x` or absolute).
pub fn expand_path(path: &str, home: Option<&Path>) -> Option<PathBuf> {
    if path == "~" {
        return home.map(Path::to_path_buf);
    }
    match path.strip_prefix("~/") {
        Some(rest) => home.map(|home| home.join(rest)),
        None => Some(PathBuf::from(path)),
    }
}

/// Canonical form of `path`: the nearest existing ancestor is canonicalized
/// (symlinks resolved, `/tmp` -> `/private/tmp` on macOS) and the missing
/// remainder appended. `.`/`..` are resolved lexically first.
pub fn resolve_existing(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            other => normal.push(other.as_os_str()),
        }
    }
    let mut ancestor = normal.as_path();
    let mut tail = Vec::new();
    loop {
        if let Ok(canonical) = ancestor.canonicalize() {
            let mut resolved = canonical;
            for part in tail.iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
        match (ancestor.file_name(), ancestor.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name.to_owned());
                ancestor = parent;
            }
            _ => return normal,
        }
    }
}

pub(super) fn push_unique(list: &mut Vec<PathBuf>, path: PathBuf) {
    if !list.contains(&path) {
        list.push(path);
    }
}

/// Temporary folders sandboxed commands may write to.
pub(super) fn temp_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let temp = resolve_existing(&std::env::temp_dir());
    // macOS keeps a per-user cache folder (`.../C`) beside the temp folder
    // (`.../T`); compilers and tools use it like a temp folder.
    if cfg!(target_os = "macos") && temp.file_name().is_some_and(|name| name == "T") {
        if let Some(cache) = temp.parent().map(|parent| parent.join("C")) {
            if cache.is_dir() {
                push_unique(&mut dirs, resolve_existing(&cache));
            }
        }
    }
    push_unique(&mut dirs, temp);
    if cfg!(unix) {
        for fixed in ["/tmp", "/var/tmp"] {
            if Path::new(fixed).is_dir() {
                push_unique(&mut dirs, resolve_existing(Path::new(fixed)));
            }
        }
    }
    if cfg!(target_os = "linux") && Path::new("/dev/shm").is_dir() {
        push_unique(&mut dirs, PathBuf::from("/dev/shm"));
    }
    dirs
}

/// A subagent's `write_paths` entry in its stored form: project-relative, with a trailing `/` for
/// a folder. Refuses globs, anything that leaves the project, and `.themis/`, `.git/` and
/// `protected` folders.
pub fn normalize_write_path(
    root: &Path,
    raw: &str,
    protected: &[PathBuf],
) -> std::result::Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return Err("a write path can't be empty".into());
    }
    if raw.contains(['*', '?', '[', ']', '{', '}', '\\']) {
        return Err(format!(
            "'{raw}': write paths are folders or files, never globs"
        ));
    }
    let relative = Path::new(raw);
    let mut parts = Vec::new();
    for part in relative.components() {
        match part {
            Component::Normal(name) => parts.push(name.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return Err(format!("'{raw}' must stay inside the project")),
        }
    }
    let Some(first) = parts.first() else {
        return Err(format!(
            "'{raw}' is the whole project; name a folder or file in it"
        ));
    };
    if first.eq_ignore_ascii_case(".themis") || first.eq_ignore_ascii_case(".git") {
        return Err(format!("'{raw}': {first}/ is never writable by a subagent"));
    }
    let joined = parts.join("/");
    let root = resolve_existing(root);
    let resolved = resolve_existing(&root.join(&joined));
    if !resolved.starts_with(&root) {
        return Err(format!("'{raw}' must stay inside the project"));
    }
    if protected.iter().any(|folder| resolved.starts_with(folder)) {
        return Err(format!("'{raw}' holds Themis settings or permissions"));
    }
    Ok(if raw.ends_with('/') || resolved.is_dir() {
        format!("{joined}/")
    } else {
        joined
    })
}

/// Whether two normalized write paths share any file: one is the other or holds it.
pub fn write_paths_overlap(a: &str, b: &str) -> bool {
    let parts = |path: &str| {
        path.trim_end_matches('/')
            .split('/')
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (a, b) = (parts(a), parts(b));
    // Ignoring case: on macOS and Windows `src/` and `Src/` are one folder.
    a.iter()
        .zip(&b)
        .all(|(left, right)| left.eq_ignore_ascii_case(right))
}
