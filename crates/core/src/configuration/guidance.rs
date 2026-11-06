use super::*;

/// Load a bounded prompt relative to the file that declares it; root containment is enforced.
pub fn load_prompt(config: &mut RuntimeConfig, directory: &std::path::Path) -> Result<()> {
    if let Some(path) = &config.compaction.prompt_file {
        let root = directory.canonicalize()?;
        let path = root
            .join(path)
            .canonicalize()
            .context("compaction.prompt_file is unavailable")?;
        ensure!(
            path.starts_with(root),
            "compaction.prompt_file escapes its configuration directory"
        );
        ensure!(
            std::fs::metadata(&path)?.len() <= 32768,
            "compaction.prompt_file exceeds 32 KiB"
        );
        config.compaction.prompt = Some(std::fs::read_to_string(path)?);
    }
    Ok(())
}

/// Read declared repository guidance, bounded and confined to the project root.
pub fn load_instructions(config: &RuntimeConfig, project: &std::path::Path) -> Result<String> {
    validate_relative_paths(&config.instructions)?;
    if config.instructions.is_empty() {
        return Ok(String::new());
    }
    let root = project.canonicalize()?;
    let mut text = String::new();
    for reference in &config.instructions {
        let path = root
            .join(reference)
            .canonicalize()
            .with_context(|| format!("instruction file {reference} is unavailable"))?;
        ensure!(
            path.starts_with(&root),
            "instruction file escapes the project root"
        );
        ensure!(
            std::fs::metadata(&path)?.len() <= 65536,
            "instruction file exceeds 64 KiB"
        );
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("instruction file {reference} is not UTF-8 text"))?;
        let heading = crate::prompts::fill(
            crate::prompts::RUNTIME_REPOSITORY_GUIDANCE_FILE,
            &[("reference", reference)],
        );
        text.push_str(&format!("\n{heading}\n{content}\n"));
        ensure!(
            text.len() <= 65536,
            "combined instruction guidance exceeds 64 KiB"
        );
    }
    Ok(text)
}

/// The most documents named in the map of a project's docs.
pub(super) const MAX_MAPPED_DOCS: usize = 40;
/// The most folder entries looked at to make that map, however big the project is.
pub(super) const MAX_MAP_VISITS: usize = 2000;
/// How many folders below the project root the map looks.
const MAX_MAP_DEPTH: usize = 3;
const MAX_GUIDANCE_BYTES: usize = 65536;
/// Generated or third-party folders, whose documents say nothing about this project.
const NOT_PROJECT_DOCS: [&str; 7] = [
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    "coverage",
    "venv",
];

/// How agents should work in this project, read fresh at the start of each run: the configured
/// instructions, the project's AGENTS.md or else its CLAUDE.md, and a map of its other docs,
/// because the main file often says too little. Never fails a run: a file it can't read is left out.
pub fn project_guidance(config: &RuntimeConfig, project: &std::path::Path) -> String {
    let mut text = load_instructions(config, project).unwrap_or_default();
    if !config.project_guidance {
        return with_guidance_heading(text);
    }
    let Ok(root) = project.canonicalize() else {
        return with_guidance_heading(text);
    };
    let listed = |name: &str| config.instructions.iter().any(|item| item == name);
    // CLAUDE.md only stands in for an AGENTS.md the project doesn't have.
    let main = ["AGENTS.md", "CLAUDE.md"]
        .into_iter()
        .find(|name| root.join(name).is_file())
        .filter(|name| !listed(name));
    if let Some(name) = main {
        let room = MAX_GUIDANCE_BYTES.saturating_sub(text.len());
        if let Some(content) = read_bounded_guidance(&root, name, room) {
            let heading = crate::prompts::fill(
                crate::prompts::RUNTIME_REPOSITORY_GUIDANCE_FILE,
                &[("reference", name)],
            );
            text.push_str(&format!("\n{heading}\n{content}\n"));
        }
    }
    let docs: Vec<String> = docs_map(&root)
        .0
        .into_iter()
        .filter(|doc| !listed(doc) && main != Some(doc.as_str()))
        .map(|doc| format!("- {doc}"))
        .collect();
    if !docs.is_empty() {
        let map = crate::prompts::fill(
            crate::prompts::PROJECT_GUIDANCE_DOCS,
            &[("docs", &docs.join("\n"))],
        );
        text.push_str(&format!("\n{map}\n"));
    }
    with_guidance_heading(text)
}

fn with_guidance_heading(text: String) -> String {
    if text.is_empty() {
        return text;
    }
    format!("{}\n{text}", crate::prompts::DESKTOP_REPOSITORY_GUIDANCE)
}

/// A guidance file's text, cut to `room` bytes. A link leading out of the project, something that
/// isn't a file and text that isn't UTF-8 all count as no file.
fn read_bounded_guidance(root: &std::path::Path, name: &str, room: usize) -> Option<String> {
    use std::io::Read;
    let path = root.join(name).canonicalize().ok()?;
    if !path.starts_with(root) || !std::fs::metadata(&path).ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(room as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    let cut = bytes.len() == room;
    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(error) if cut && error.utf8_error().error_len().is_none() => {
            let valid = error.utf8_error().valid_up_to();
            let mut bytes = error.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).ok()
        }
        Err(_) => None,
    }
}

/// The project's documents, nearest the root first, and how many folder entries that took.
/// Doesn't follow links and skips hidden and generated folders.
pub(super) fn docs_map(root: &std::path::Path) -> (Vec<String>, usize) {
    let mut docs = Vec::new();
    let mut visited = 0;
    let mut folders = std::collections::VecDeque::from([(root.to_owned(), String::new(), 0)]);
    while let Some((folder, prefix, depth)) = folders.pop_front() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            visited += 1;
            if visited >= MAX_MAP_VISITS {
                break;
            }
            found.push(entry);
        }
        found.sort_by_key(std::fs::DirEntry::file_name);
        for entry in found {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let path = format!("{prefix}{name}");
            if kind.is_dir() {
                if depth < MAX_MAP_DEPTH && !NOT_PROJECT_DOCS.contains(&name.as_str()) {
                    folders.push_back((entry.path(), format!("{path}/"), depth + 1));
                }
            } else if kind.is_file()
                && [".md", ".mdx", ".rst"]
                    .iter()
                    .any(|end| name.ends_with(end))
                && docs.len() < MAX_MAPPED_DOCS
            {
                docs.push(path);
            }
        }
        if visited >= MAX_MAP_VISITS || docs.len() >= MAX_MAPPED_DOCS {
            break;
        }
    }
    (docs, visited)
}
