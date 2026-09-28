use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PatchOp {
    Create,
    Modify,
    Delete,
}

impl std::fmt::Display for PatchOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchOp::Create => write!(f, "create"),
            PatchOp::Modify => write!(f, "modify"),
            PatchOp::Delete => write!(f, "delete"),
        }
    }
}

pub(super) struct PlannedFile {
    pub(super) relative: String,
    pub(super) path: PathBuf,
    pub(super) op: PatchOp,
    pub(super) new_content: Option<String>,
}

#[derive(Debug)]
pub(super) enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

pub(super) struct Hunk {
    pub(super) old_start: usize,
    pub(super) old_count: usize,
    pub(super) lines: Vec<HunkLine>,
    /// True when the file's new content has no trailing newline.
    pub(super) new_lacks_trailing_newline: bool,
}

pub(super) fn plan_patch(root: &Path, patch: &str) -> anyhow::Result<Vec<PlannedFile>> {
    let root_canon = root
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("project root '{}' is not accessible: {e}", root.display()))?;
    let lines: Vec<&str> = patch.lines().collect();
    let mut i = 0;
    let mut plan = Vec::new();
    while i < lines.len() {
        let line = lines[i];
        if let Some(rest) = line.strip_prefix("--- ") {
            let old_path = strip_path_prefix(rest.trim_end(), "a/");
            i += 1;
            if i >= lines.len() {
                anyhow::bail!("malformed patch: '---' without a following '+++' line");
            }
            let plus = lines[i];
            let Some(new_rest) = plus.strip_prefix("+++ ") else {
                anyhow::bail!("malformed patch: expected '+++' line, found '{plus}'");
            };
            let new_path = strip_path_prefix(new_rest.trim_end(), "b/");
            i += 1;

            let mut hunks = Vec::new();
            while i < lines.len() && lines[i].starts_with("@@ ") {
                let (hunk, next) = parse_hunk(&lines, i)?;
                hunks.push(hunk);
                i = next;
            }
            if hunks.is_empty() {
                anyhow::bail!("malformed patch: no hunks for '{new_path}'");
            }
            plan.push(plan_file(&root_canon, old_path, new_path, &hunks)?);
        } else {
            // Skip `diff --git`, `index`, context, and blank lines.
            i += 1;
        }
    }
    Ok(plan)
}

pub(super) fn strip_path_prefix<'a>(path: &'a str, prefix: &str) -> &'a str {
    let path = path.split('\t').next().unwrap_or(path).trim();
    path.strip_prefix(prefix).unwrap_or(path)
}

pub(super) fn parse_hunk(lines: &[&str], at: usize) -> anyhow::Result<(Hunk, usize)> {
    let header = lines[at];
    let rest = header
        .strip_prefix("@@")
        .ok_or_else(|| anyhow::anyhow!("malformed hunk header '{header}'"))?;
    let (ranges, _) = rest
        .split_once("@@")
        .ok_or_else(|| anyhow::anyhow!("malformed hunk header '{header}'"))?;
    let mut parts = ranges.split_whitespace();
    let old_range = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("malformed hunk header '{header}'"))?;
    let new_range = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("malformed hunk header '{header}'"))?;
    let (old_start, old_count) = parse_range(old_range, '-')?;
    let (_, new_count) = parse_range(new_range, '+')?;

    let mut hunk_lines = Vec::new();
    let mut last_side_is_new = false;
    let mut marker_after_new_at_end = false;
    let mut i = at + 1;
    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("@@ ") || line.starts_with("--- ") || line.starts_with("diff --git ") {
            break;
        }
        if line.starts_with('\\') {
            if line != "\\ No newline at end of file" {
                anyhow::bail!("malformed patch: unexpected line '{line}'");
            }
            if last_side_is_new {
                marker_after_new_at_end = true;
            }
            i += 1;
            continue;
        }
        let hunk_line = parse_hunk_line(line)?;
        last_side_is_new = !matches!(hunk_line, HunkLine::Remove(_));
        hunk_lines.push(hunk_line);
        marker_after_new_at_end = false;
        i += 1;
    }

    let old_seen = hunk_lines
        .iter()
        .filter(|l| matches!(l, HunkLine::Context(_) | HunkLine::Remove(_)))
        .count();
    let new_seen = hunk_lines
        .iter()
        .filter(|l| matches!(l, HunkLine::Context(_) | HunkLine::Add(_)))
        .count();
    if old_seen != old_count || new_seen != new_count {
        anyhow::bail!(
            "malformed patch: hunk header '{header}' does not match its body ({old_seen}/{old_count}, {new_seen}/{new_count})"
        );
    }
    Ok((
        Hunk {
            old_start,
            old_count,
            lines: hunk_lines,
            new_lacks_trailing_newline: marker_after_new_at_end,
        },
        i,
    ))
}

pub(super) fn parse_hunk_line(line: &str) -> anyhow::Result<HunkLine> {
    let marker = line
        .chars()
        .next()
        .ok_or_else(|| anyhow::anyhow!("malformed patch: unexpected hunk line '{line}'"))?;
    let text = &line[marker.len_utf8()..];
    match marker {
        ' ' => Ok(HunkLine::Context(text.to_owned())),
        '-' => Ok(HunkLine::Remove(text.to_owned())),
        '+' => Ok(HunkLine::Add(text.to_owned())),
        _ => anyhow::bail!("malformed patch: unexpected hunk line '{line}'"),
    }
}

pub(super) fn parse_range(range: &str, sign: char) -> anyhow::Result<(usize, usize)> {
    let body = range
        .strip_prefix(sign)
        .ok_or_else(|| anyhow::anyhow!("malformed hunk range '{range}'"))?;
    match body.split_once(',') {
        Some((start, count)) => Ok((
            start
                .parse()
                .map_err(|_| anyhow::anyhow!("malformed hunk range '{range}'"))?,
            count
                .parse()
                .map_err(|_| anyhow::anyhow!("malformed hunk range '{range}'"))?,
        )),
        None => Ok((
            body.parse()
                .map_err(|_| anyhow::anyhow!("malformed hunk range '{range}'"))?,
            1,
        )),
    }
}

pub(super) fn plan_file(
    root_canon: &Path,
    old_path: &str,
    new_path: &str,
    hunks: &[Hunk],
) -> anyhow::Result<PlannedFile> {
    if new_path == "/dev/null" {
        if old_path == "/dev/null" {
            anyhow::bail!("malformed patch: both sides are /dev/null");
        }
        let path = resolve_within_root(root_canon, old_path)?;
        return Ok(PlannedFile {
            relative: old_path.to_string(),
            path,
            op: PatchOp::Delete,
            new_content: None,
        });
    }
    let path = resolve_within_root(root_canon, new_path)?;
    let relative = path
        .strip_prefix(root_canon)
        .map_err(|_| anyhow::anyhow!("path '{new_path}' escapes the project root"))?
        .to_string_lossy()
        .into_owned();

    let (old_lines, creating) = if old_path == "/dev/null" {
        (Vec::new(), true)
    } else {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("cannot read '{old_path}' for patching: {e}"))?;
        (
            content.lines().map(str::to_string).collect::<Vec<_>>(),
            false,
        )
    };
    let mut new_lines = apply_hunks(&old_lines, hunks, new_path)?;
    let trailing_newline = !hunks
        .last()
        .map(|h| h.new_lacks_trailing_newline)
        .unwrap_or(false);
    let mut new_content = String::new();
    for (n, line) in new_lines.drain(..).enumerate() {
        if n > 0 {
            new_content.push('\n');
        }
        new_content.push_str(&line);
    }
    if trailing_newline && (!new_content.is_empty() || !creating) {
        new_content.push('\n');
    }
    Ok(PlannedFile {
        relative,
        path,
        op: if creating {
            PatchOp::Create
        } else {
            PatchOp::Modify
        },
        new_content: Some(new_content),
    })
}

pub(super) fn apply_hunks(
    old: &[String],
    hunks: &[Hunk],
    file: &str,
) -> anyhow::Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut cursor = 0;
    for hunk in hunks {
        // `old_start` is 1-based, except 0 when the hunk touches no old lines.
        let start = if hunk.old_count == 0 {
            hunk.old_start
        } else {
            hunk.old_start.saturating_sub(1)
        };
        if start < cursor || start > old.len() {
            anyhow::bail!("patch does not apply to '{file}': hunk out of range");
        }
        out.extend_from_slice(&old[cursor..start]);
        cursor = start;
        for line in &hunk.lines {
            match line {
                HunkLine::Context(text) => {
                    let actual = old.get(cursor).ok_or_else(|| {
                        anyhow::anyhow!("patch does not apply to '{file}': context mismatch")
                    })?;
                    if actual != text {
                        anyhow::bail!(
                            "patch does not apply to '{file}': context mismatch at line {}",
                            cursor + 1
                        );
                    }
                    out.push(text.clone());
                    cursor += 1;
                }
                HunkLine::Remove(text) => {
                    let actual = old.get(cursor).ok_or_else(|| {
                        anyhow::anyhow!("patch does not apply to '{file}': context mismatch")
                    })?;
                    if actual != text {
                        anyhow::bail!(
                            "patch does not apply to '{file}': removal mismatch at line {}",
                            cursor + 1
                        );
                    }
                    cursor += 1;
                }
                HunkLine::Add(text) => out.push(text.clone()),
            }
        }
    }
    out.extend_from_slice(&old[cursor..]);
    Ok(out)
}

pub(super) fn commit_planned_file(file: &PlannedFile) -> anyhow::Result<()> {
    match file.op {
        PatchOp::Delete => {
            std::fs::remove_file(&file.path)
                .map_err(|e| anyhow::anyhow!("cannot delete '{}': {e}", file.relative))?;
        }
        PatchOp::Create | PatchOp::Modify => {
            if let Some(parent) = file.path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    anyhow::anyhow!("cannot create parent of '{}': {e}", file.relative)
                })?;
            }
            let content = file.new_content.as_deref().unwrap_or_default();
            std::fs::write(&file.path, content)
                .map_err(|e| anyhow::anyhow!("cannot write '{}': {e}", file.relative))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Git tool
// ---------------------------------------------------------------------------
