//! What the agent remembers across chats: one small Markdown file per memory with a
//! frontmatter header, and an index (`MEMORY.md`) of one line per memory that runs read at
//! their start, with the memories relevant to the message in full. The memory models keep
//! them (see [`auto`] and [`jobs`]); the chat's own model only reads them. Memories live in the app data folder, never in a project: one folder for
//! memories that apply everywhere and one per project. Files are untrusted on read (a person
//! may edit them): every field is validated, sizes are bounded, and symlinks are refused.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub mod auto;
pub mod jobs;
pub mod plans;

use serde::{Deserialize, Serialize};
use sha2::Digest;

/// The index file in each scope's folder.
pub const INDEX_FILE: &str = "MEMORY.md";
/// Most memories (and so index lines) one scope holds.
pub const MAX_MEMORIES: usize = 200;
/// Most bytes of one scope's index, which is what reaches every run.
pub const MAX_INDEX_BYTES: usize = 24 * 1024;
pub const MAX_NAME_CHARS: usize = 64;
pub const MAX_DESCRIPTION_CHARS: usize = 160;
pub const MAX_SOURCE_CHARS: usize = 200;
pub const MAX_BODY_BYTES: usize = 4 * 1024;
/// Most bytes a memory file may have; a larger file is not read.
pub const MAX_FILE_BYTES: u64 = 8 * 1024;

/// What a memory is about. Shown in Settings › Memory as About you, Your feedback, Project
/// facts and Pointers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKind {
    User,
    Feedback,
    Project,
    Reference,
}

impl MemoryKind {
    pub const ALL: [MemoryKind; 4] = [Self::User, Self::Feedback, Self::Project, Self::Reference];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Project => "project",
            Self::Reference => "reference",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    /// Only memories about the person and their general feedback may apply everywhere;
    /// project facts and pointers always belong to one project.
    pub fn may_be_global(self) -> bool {
        matches!(self, Self::User | Self::Feedback)
    }
}

/// Where a memory applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryScope {
    /// The current project only.
    Project,
    /// Every project ("Everywhere").
    Global,
}

impl MemoryScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Global => "global",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "project" => Some(Self::Project),
            "global" => Some(Self::Global),
            _ => None,
        }
    }
}

/// One memory, as saved in `<name>.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memory {
    /// Lowercase kebab-case slug; also the file name.
    pub name: String,
    /// One line saying what it is.
    pub description: String,
    #[serde(rename = "type")]
    pub kind: MemoryKind,
    /// The day it was saved or last updated, `YYYY-MM-DD`.
    pub saved: String,
    /// What it was built from when that was observed content (a web page, a file), never the
    /// person: such a memory is information, never instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The fact, with `Why:` and `How to apply:` lines for feedback and project facts.
    pub body: String,
}

/// Why a memory file or a requested memory is refused. Messages are fixed text, never the
/// offending content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryError(pub String);

impl std::fmt::Display for MemoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MemoryError {}

fn fail<T>(message: impl Into<String>) -> Result<T, MemoryError> {
    Err(MemoryError(message.into()))
}

/// A lowercase kebab-case slug of at most [`MAX_NAME_CHARS`]: `free-model-for-tests`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_CHARS
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_date(text: &str) -> bool {
    chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").is_ok() && text.len() == 10
}

/// One line of plain text: no line breaks or other control characters.
fn one_line(text: &str) -> bool {
    !text.chars().any(char::is_control)
}

impl Memory {
    /// Checks every field; a memory that passes serializes to a file [`parse`] reads back.
    pub fn validate(&self) -> Result<(), MemoryError> {
        if !valid_name(&self.name) {
            return fail(format!(
                "name must be a lowercase kebab-case slug of at most {MAX_NAME_CHARS} characters, such as free-model-for-tests"
            ));
        }
        let description = self.description.trim();
        if description.is_empty()
            || description != self.description
            || self.description.chars().count() > MAX_DESCRIPTION_CHARS
            || !one_line(&self.description)
        {
            return fail(format!(
                "description must be one line of at most {MAX_DESCRIPTION_CHARS} characters"
            ));
        }
        if !valid_date(&self.saved) {
            return fail("saved must be a date such as 2026-11-02");
        }
        if let Some(source) = &self.source {
            if source.trim().is_empty()
                || source.trim() != source
                || source.chars().count() > MAX_SOURCE_CHARS
                || !one_line(source)
            {
                return fail(format!(
                    "source must be one line of at most {MAX_SOURCE_CHARS} characters"
                ));
            }
        }
        if self.body.trim().is_empty() || self.body.len() > MAX_BODY_BYTES {
            return fail(format!(
                "the memory's text must be 1 to {MAX_BODY_BYTES} bytes"
            ));
        }
        if self
            .body
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            return fail("the memory's text may not hold control characters");
        }
        Ok(())
    }

    /// The line this memory has in the index.
    pub fn index_line(&self) -> String {
        match &self.source {
            Some(source) => format!(
                "- {} ({}, {}, from {source}): {}",
                self.name,
                self.kind.as_str(),
                self.saved,
                self.description
            ),
            None => format!(
                "- {} ({}, {}): {}",
                self.name,
                self.kind.as_str(),
                self.saved,
                self.description
            ),
        }
    }
}

/// A memory's body from its fact and optional `why` and `how_to_apply`: the fact, then one
/// `Why:` and one `How to apply:` line. A field replaces such lines the fact repeats (a model
/// updating a memory often passes the old body back with them); without the field the last of
/// them is kept. So an update never stacks a second `Why:`.
pub fn compose_body(fact: &str, why: Option<&str>, how: Option<&str>) -> String {
    let label = |line: &str, label: &str| {
        let line = line.trim_start();
        line.get(..label.len())
            .filter(|head| head.eq_ignore_ascii_case(label))
            .map(|_| line[label.len()..].trim().to_owned())
    };
    let mut kept_why = None;
    let mut kept_how = None;
    let mut rest = Vec::new();
    for line in fact.lines() {
        if let Some(text) = label(line, "Why:") {
            kept_why = Some(text);
        } else if let Some(text) = label(line, "How to apply:") {
            kept_how = Some(text);
        } else {
            rest.push(line);
        }
    }
    let one_line = |text: &str| text.replace(['\n', '\r'], " ").trim().to_owned();
    let why = why.map(one_line).or(kept_why).filter(|t| !t.is_empty());
    let how = how.map(one_line).or(kept_how).filter(|t| !t.is_empty());
    let mut body = rest.join("\n").trim().to_owned();
    let lines: Vec<String> = [
        why.map(|text| format!("Why: {text}")),
        how.map(|text| format!("How to apply: {text}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !lines.is_empty() {
        if !body.is_empty() {
            body.push_str("\n\n");
        }
        body.push_str(&lines.join("\n"));
    }
    body
}

/// The file text of a valid memory.
pub fn serialize(memory: &Memory) -> String {
    let mut text = format!(
        "---\nname: {}\ndescription: {}\ntype: {}\nsaved: {}\n",
        memory.name,
        memory.description,
        memory.kind.as_str(),
        memory.saved
    );
    if let Some(source) = &memory.source {
        text.push_str(&format!("source: {source}\n"));
    }
    text.push_str("---\n");
    text.push_str(memory.body.trim_end());
    text.push('\n');
    text
}

/// Reads a memory file: a `---` fenced header of `key: value` lines (name, description, type,
/// saved, optional source; no other key, none twice), then the body. Untrusted input: anything
/// else is refused, never guessed at.
pub fn parse(text: &str) -> Result<Memory, MemoryError> {
    if text.len() as u64 > MAX_FILE_BYTES {
        return fail("memory file is too large");
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return fail("memory file must start with a --- header");
    };
    let mut name = None;
    let mut description = None;
    let mut kind = None;
    let mut saved = None;
    let mut source = None;
    let mut body = None;
    let mut remaining = rest;
    while !remaining.is_empty() {
        let (line, after) = remaining.split_once('\n').unwrap_or((remaining, ""));
        remaining = after;
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line == "---" {
            body = Some(remaining);
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            return fail("memory header lines must be key: value");
        };
        let value = value.trim().to_owned();
        let slot = match key.trim() {
            "name" => &mut name,
            "description" => &mut description,
            "type" => &mut kind,
            "saved" => &mut saved,
            "source" => &mut source,
            _ => return fail("memory header has an unknown key"),
        };
        if slot.replace(value).is_some() {
            return fail("memory header names a key twice");
        }
    }
    let Some(body) = body else {
        return fail("memory header is not closed with ---");
    };
    let (Some(name), Some(description), Some(kind), Some(saved)) = (name, description, kind, saved)
    else {
        return fail("memory header needs name, description, type and saved");
    };
    let Some(kind) = MemoryKind::parse(&kind) else {
        return fail("memory type must be user, feedback, project or reference");
    };
    let memory = Memory {
        name,
        description,
        kind,
        saved,
        source,
        body: body.replace("\r\n", "\n").trim_end().to_owned(),
    };
    memory.validate()?;
    Ok(memory)
}

/// The index of `memories` (one line each, in the order given), at most [`MAX_MEMORIES`]
/// lines and [`MAX_INDEX_BYTES`] bytes; what does not fit is left out.
pub fn build_index(memories: &[Memory]) -> String {
    let mut index = String::new();
    for memory in memories.iter().take(MAX_MEMORIES) {
        let line = memory.index_line();
        if index.len() + line.len() + 1 > MAX_INDEX_BYTES {
            break;
        }
        index.push_str(&line);
        index.push('\n');
    }
    index
}

/// `text` with secrets hidden (the browser tools' redaction: token-like words, values after
/// names such as `password=`) and with control characters other than line breaks and tabs
/// removed. Applied to everything a memory is saved with.
pub fn strip_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let line = if crate::secret_look::looks_like_secret(line) {
            hide_secret_words(line)
        } else {
            line.to_owned()
        };
        out.push_str(&plain_redact(&line));
    }
    out.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

/// [`crate::browser::redact::redact_text`] without its JSON reformatting, so prose keeps its
/// shape.
fn plain_redact(line: &str) -> String {
    if line.trim_start().starts_with(['{', '[']) {
        // A JSON line is redacted as words, like any other line.
        return crate::browser::redact::redact_text(&format!("x {line}"))[2..].to_owned();
    }
    crate::browser::redact::redact_text(line)
}

/// Hides each word of `line` that looks like a secret on its own (a fixed key shape).
fn hide_secret_words(line: &str) -> String {
    line.split(' ')
        .map(|word| {
            if crate::secret_look::looks_like_secret(word) {
                crate::browser::redact::hidden(word)
            } else {
                word.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The folder, under the memory root, of a project's memories: a hash of its canonical path,
/// so names, spaces and renames never matter.
pub fn project_folder(project_root: &Path) -> String {
    let path = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let digest = sha2::Sha256::digest(path.to_string_lossy().as_bytes());
    format!("projects/{}", &format!("{digest:x}")[..16])
}

/// The folder of global memories under the memory root.
pub const GLOBAL_FOLDER: &str = "global";

/// Whether to save a new memory or change an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveMode {
    Create,
    Update,
}

/// Serializes writes to memory folders within the process.
static WRITING: Mutex<()> = Mutex::new(());

#[cfg(test)]
thread_local! {
    /// Files opened and bytes read by this module on this thread (scale tests).
    static WORK: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
}

#[cfg(test)]
fn count_read(bytes: usize) {
    WORK.with(|work| {
        let (files, read) = work.get();
        work.set((files + 1, read + bytes as u64));
    });
}

#[cfg(not(test))]
fn count_read(_bytes: usize) {}

/// Reads at most `limit` bytes of a regular file that is not a symlink; `None` when missing,
/// a symlink, too large or unreadable.
fn read_bounded(path: &Path, limit: u64) -> Option<String> {
    use std::io::Read;
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > limit {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(limit)
        .read_to_string(&mut text)
        .ok()?;
    count_read(text.len());
    Some(text)
}

/// One scope's memories: `<root>/<folder>/<name>.md` and the index beside them.
#[derive(Debug, Clone)]
pub struct MemoryStore {
    root: PathBuf,
    dir: PathBuf,
}

/// What a listing found: the valid memories, newest first, and how many files were skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub memories: Vec<Memory>,
    pub skipped: usize,
}

impl MemoryStore {
    /// The store of `folder` ([`GLOBAL_FOLDER`] or a [`project_folder`]) under `root` (the app
    /// data's `memory` folder). Nothing is created until something is saved.
    pub fn new(root: &Path, folder: &str) -> Result<Self, MemoryError> {
        let relative = Path::new(folder);
        let safe = relative.components().all(|part| {
            matches!(part, std::path::Component::Normal(name)
                if name.to_str().is_some_and(|name| name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')))
        });
        if !safe || folder.is_empty() {
            return fail("memory folder is not a plain relative path");
        }
        Ok(Self {
            root: root.to_path_buf(),
            dir: root.join(relative),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The folder, refused when it or any folder between it and the root is a symlink.
    fn checked_dir(&self, create: bool) -> Result<Option<PathBuf>, MemoryError> {
        if create {
            std::fs::create_dir_all(&self.dir)
                .map_err(|_| MemoryError("memory folder could not be created".into()))?;
        }
        let mut at = self.dir.as_path();
        loop {
            match std::fs::symlink_metadata(at) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return fail("memory folder may not be a symlink")
                }
                Ok(metadata) if !metadata.is_dir() => return fail("memory folder is not a folder"),
                Ok(_) => {}
                Err(_) if !create => return Ok(None),
                Err(_) => return fail("memory folder is unreadable"),
            }
            if at == self.root {
                break;
            }
            match at.parent() {
                Some(parent) if parent.starts_with(&self.root) => at = parent,
                _ => break,
            }
        }
        Ok(Some(self.dir.clone()))
    }

    fn path_of(&self, name: &str) -> Result<PathBuf, MemoryError> {
        if !valid_name(name) {
            return fail("no memory has that name: names are lowercase kebab-case slugs");
        }
        Ok(self.dir.join(format!("{name}.md")))
    }

    /// Every valid memory, newest first then by name. Invalid files, symlinks and anything
    /// past [`MAX_MEMORIES`] are skipped and counted.
    pub fn list(&self) -> Result<Listing, MemoryError> {
        let Some(dir) = self.checked_dir(false)? else {
            return Ok(Listing::default());
        };
        let entries = std::fs::read_dir(&dir)
            .map_err(|_| MemoryError("memory folder is unreadable".into()))?;
        let mut listing = Listing::default();
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            if file_name == INDEX_FILE {
                continue;
            }
            let Some(stem) = file_name.strip_suffix(".md") else {
                continue;
            };
            if listing.memories.len() >= MAX_MEMORIES * 2 {
                listing.skipped += 1;
                continue;
            }
            match read_bounded(&entry.path(), MAX_FILE_BYTES).map(|text| parse(&text)) {
                Some(Ok(memory)) if memory.name == stem => listing.memories.push(memory),
                _ => listing.skipped += 1,
            }
        }
        listing
            .memories
            .sort_by(|a, b| b.saved.cmp(&a.saved).then_with(|| a.name.cmp(&b.name)));
        if listing.memories.len() > MAX_MEMORIES {
            listing.skipped += listing.memories.len() - MAX_MEMORIES;
            listing.memories.truncate(MAX_MEMORIES);
        }
        Ok(listing)
    }

    /// One memory, `None` when there is none by that name.
    pub fn read(&self, name: &str) -> Result<Option<Memory>, MemoryError> {
        let path = self.path_of(name)?;
        if self.checked_dir(false)?.is_none() {
            return Ok(None);
        }
        match read_bounded(&path, MAX_FILE_BYTES) {
            None if std::fs::symlink_metadata(&path).is_err() => Ok(None),
            None => fail("that memory's file is unreadable"),
            Some(text) => parse(&text).map(Some),
        }
    }

    /// The saved index, as runs read it: at most [`MAX_INDEX_BYTES`], only its `- ` lines.
    pub fn index(&self) -> String {
        if !matches!(self.checked_dir(false), Ok(Some(_))) {
            return String::new();
        }
        let Some(text) = read_bounded(&self.dir.join(INDEX_FILE), MAX_INDEX_BYTES as u64) else {
            return String::new();
        };
        text.lines()
            .filter(|line| line.starts_with("- ") && one_line(line))
            .take(MAX_MEMORIES)
            .map(|line| format!("{line}\n"))
            .collect()
    }

    /// Changes whenever the index does; an edit from Settings names the one it saw.
    pub fn revision(&self) -> String {
        let digest = sha2::Sha256::digest(self.index().as_bytes());
        format!("{digest:x}")[..16].to_owned()
    }

    fn write_index(&self, listing: &Listing) -> Result<(), MemoryError> {
        crate::chat_grants::write_atomic(
            &self.dir.join(INDEX_FILE),
            &build_index(&listing.memories),
        )
        .map_err(|_| MemoryError("memory index could not be saved".into()))
    }

    /// Saves `memory` (validated) and rewrites the index. `Create` refuses a name in use and a
    /// full scope; `Update` refuses a name not in use. Returns how many memories the scope has.
    pub fn save(&self, memory: &Memory, mode: SaveMode) -> Result<usize, MemoryError> {
        memory.validate()?;
        let _writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        self.checked_dir(true)?;
        let path = self.path_of(&memory.name)?;
        let exists = std::fs::symlink_metadata(&path).is_ok();
        if std::fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_file()) {
            return fail("that memory's file is not a regular file");
        }
        match (mode, exists) {
            (SaveMode::Create, true) => {
                return fail(format!(
                    "a memory named {} already exists: update it, or pick another name",
                    memory.name
                ))
            }
            (SaveMode::Update, false) => {
                return fail(format!("no memory is named {}", memory.name))
            }
            _ => {}
        }
        if mode == SaveMode::Create && self.list()?.memories.len() >= MAX_MEMORIES {
            return fail(format!(
                "memory is full ({MAX_MEMORIES}): update or delete one first"
            ));
        }
        crate::chat_grants::write_atomic(&path, &serialize(memory))
            .map_err(|_| MemoryError("memory could not be saved".into()))?;
        let listing = self.list()?;
        self.write_index(&listing)?;
        Ok(listing.memories.len())
    }

    /// Updates the memory named `from` to `memory`, under `memory.name` when that differs: the
    /// file is rewritten in place and then renamed in one step, so the scope never holds both,
    /// and the index is rewritten. Refuses a `from` not in use and a new name already in use.
    pub fn rename(&self, from: &str, memory: &Memory) -> Result<usize, MemoryError> {
        if from == memory.name {
            return self.save(memory, SaveMode::Update);
        }
        memory.validate()?;
        let _writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        self.checked_dir(true)?;
        let old = self.path_of(from)?;
        let new = self.path_of(&memory.name)?;
        let original = match read_bounded(&old, MAX_FILE_BYTES) {
            Some(text)
                if std::fs::symlink_metadata(&old).is_ok_and(|m| m.file_type().is_file()) =>
            {
                text
            }
            _ => return fail(format!("no memory is named {from}")),
        };
        if std::fs::symlink_metadata(&new).is_ok() {
            return fail(format!(
                "a memory named {} already exists: update that one, or pick another name",
                memory.name
            ));
        }
        crate::chat_grants::write_atomic(&old, &serialize(memory))
            .map_err(|_| MemoryError("memory could not be saved".into()))?;
        if std::fs::rename(&old, &new).is_err() {
            // Put the old memory back rather than leave it under a name it doesn't carry.
            let _ = crate::chat_grants::write_atomic(&old, &original);
            return fail("memory could not be renamed");
        }
        let listing = self.list()?;
        self.write_index(&listing)?;
        Ok(listing.memories.len())
    }

    /// Deletes a memory and rewrites the index. With `revision`, refuses when the index changed
    /// since it was read. Returns the memory that was deleted.
    pub fn delete(&self, name: &str, revision: Option<&str>) -> Result<Memory, MemoryError> {
        let _writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        if revision.is_some_and(|expected| expected != self.revision()) {
            return fail("memories changed since they were read; reload and try again");
        }
        let Some(memory) = self.read(name)? else {
            return fail(format!("no memory is named {name}"));
        };
        std::fs::remove_file(self.path_of(name)?)
            .map_err(|_| MemoryError("memory could not be deleted".into()))?;
        let listing = self.list()?;
        self.write_index(&listing)?;
        Ok(memory)
    }
}

/// Most memories a run gets in full beside the index.
pub const MAX_RELEVANT: usize = 5;
/// Most bytes of those memories together.
pub const MAX_RELEVANT_BYTES: usize = 8 * 1024;
/// Shortest word that counts toward relevance.
const MIN_TERM_CHARS: usize = 4;
/// Words too common to tell memories apart.
const STOP_WORDS: [&str; 24] = [
    "about", "after", "again", "also", "been", "before", "could", "does", "from", "have", "into",
    "just", "like", "make", "more", "please", "should", "that", "them", "then", "there", "this",
    "what", "with",
];

/// The distinct lowercase words of `text` that can tell memories apart.
fn terms(text: &str) -> std::collections::BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= MIN_TERM_CHARS)
        .map(str::to_lowercase)
        .filter(|word| !STOP_WORDS.contains(&word.as_str()))
        .collect()
}

/// Which memories bear on `message`, by a cheap rule that needs no model: the index lines
/// (name and description) that share the most words of at least four letters with it, at most
/// [`MAX_RELEVANT`], this project's before the global ones on a tie, newest first.
pub fn relevant_names(
    message: &str,
    global_index: &str,
    project_index: &str,
) -> Vec<(MemoryScope, String)> {
    let wanted = terms(message);
    if wanted.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, usize, MemoryScope, String)> = Vec::new();
    for (scope, index) in [
        (MemoryScope::Project, project_index),
        (MemoryScope::Global, global_index),
    ] {
        for line in index.lines() {
            let Some((name, description)) = auto::index_entry(line) else {
                continue;
            };
            let have = terms(&format!("{name} {description}"));
            let score = have.intersection(&wanted).count();
            if score > 0 {
                scored.push((score, scored.len(), scope, name.to_owned()));
            }
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(MAX_RELEVANT)
        .map(|(_, _, scope, name)| (scope, name))
        .collect()
}

/// What a run is told about memory: the global and this project's indexes under the memory
/// note, then `relevant` in full (within [`MAX_RELEVANT_BYTES`]), or nothing when neither index
/// holds a memory.
pub fn run_note(
    global_index: &str,
    project_index: &str,
    relevant: &[(MemoryScope, Memory)],
) -> String {
    if global_index.trim().is_empty() && project_index.trim().is_empty() {
        return String::new();
    }
    let lines = |index: &str| {
        let index = index.trim_end();
        if index.is_empty() {
            "(none)".to_owned()
        } else {
            index.to_owned()
        }
    };
    let mut note = crate::prompts::fill(
        crate::prompts::SYSTEM_MEMORY,
        &[
            ("global", &lines(global_index)),
            ("project", &lines(project_index)),
        ],
    );
    let mut memories = String::new();
    for (scope, memory) in relevant {
        let entry = format!(
            "### {} ({})\n{}\n",
            memory.index_line().trim_start_matches("- "),
            scope.as_str(),
            memory.body.trim_end()
        );
        if memories.len() + entry.len() > MAX_RELEVANT_BYTES {
            continue;
        }
        memories.push_str(&entry);
    }
    if !memories.is_empty() {
        note.push('\n');
        note.push_str(&crate::prompts::fill(
            crate::prompts::SYSTEM_MEMORY_RELEVANT,
            &[("memories", memories.trim_end())],
        ));
    }
    note
}

/// The memory block a run in a project starts with, for `message`: both indexes (one bounded
/// file each) and the relevant memories (at most [`MAX_RELEVANT`] files), so its cost stays
/// the same whatever the memory count.
pub fn memory_block(global: &MemoryStore, project: Option<&MemoryStore>, message: &str) -> String {
    let global_index = global.index();
    let project_index = project.map(MemoryStore::index).unwrap_or_default();
    let relevant: Vec<(MemoryScope, Memory)> =
        relevant_names(message, &global_index, &project_index)
            .into_iter()
            .filter_map(|(scope, name)| {
                let store = match scope {
                    MemoryScope::Global => global,
                    MemoryScope::Project => project?,
                };
                Some((scope, store.read(&name).ok()??))
            })
            .collect();
    run_note(&global_index, &project_index, &relevant)
}

/// Today's date as memories record it.
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn memory(name: &str) -> Memory {
        Memory {
            name: name.to_owned(),
            description: "Use the free model for test runs".to_owned(),
            kind: MemoryKind::Feedback,
            saved: "2026-11-02".to_owned(),
            source: None,
            body: "Use the free model for test runs.\n\nWhy: said so.\nHow to apply: pick it."
                .to_owned(),
        }
    }

    #[test]
    fn a_memory_round_trips_through_its_file() {
        let mut saved = memory("free-model-for-tests");
        saved.source = Some("web page https://example.com/post".to_owned());
        let text = serialize(&saved);
        assert!(
            text.starts_with("---\nname: free-model-for-tests\n"),
            "{text}"
        );
        assert_eq!(parse(&text).unwrap(), saved);
        assert_eq!(parse(&text.replace('\n', "\r\n")).unwrap(), saved);
    }

    #[test]
    fn untrusted_files_are_refused_not_guessed() {
        let good = serialize(&memory("a-name"));
        for bad in [
            "no header".to_owned(),
            good.replace("type: feedback", "type: rule"),
            good.replace("name: a-name", "name: ../escape"),
            good.replace("name: a-name", "name: A-Name"),
            good.replace("saved: 2026-11-02", "saved: yesterday"),
            good.replace("saved: 2026-11-02", "saved: 2026-02-30"),
            good.replace("type: feedback\n", "type: feedback\ntype: user\n"),
            good.replace("type: feedback\n", "type: feedback\nextra: 1\n"),
            good.replace("type: feedback\n", ""),
            good.replacen("---\n", "", 2)
                .replacen("name", "---\nname", 1),
            good.replace("Use the free model for test runs.\n", "\u{7}bell\n"),
            format!("{good}{}", "x".repeat(MAX_FILE_BYTES as usize)),
            "---\nname: a\ndescription: d\ntype: user\nsaved: 2026-11-02\n".to_owned(),
            "---\nname: a\ndescription: d\ntype: user\nsaved: 2026-11-02\n---\n   \n".to_owned(),
        ] {
            assert!(parse(&bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn the_run_note_carries_both_indexes_and_relevant_memories_or_nothing() {
        assert_eq!(run_note("", "\n", &[]), "");
        let note = run_note("", "- a (project, 2026-11-02): A fact\n", &[]);
        assert!(note.starts_with("Memory from earlier chats"), "{note}");
        // Memory first: what the person wants wins over the files; facts about state are checked.
        assert!(
            note.contains("Start from memory before reading files"),
            "{note}"
        );
        assert!(
            note.contains("memory takes precedence over the files"),
            "{note}"
        );
        assert!(
            note.contains("check those against the current files"),
            "{note}"
        );
        assert!(note.contains("so you never save them"), "{note}");
        assert!(!note.contains("memory tool"), "no tool is offered: {note}");
        assert!(
            note.ends_with("Everywhere:\n(none)\nThis project:\n- a (project, 2026-11-02): A fact"),
            "{note}"
        );
        let full = run_note(
            "- free-model-for-tests (feedback, 2026-11-02): Use the free model for test runs\n",
            "",
            &[(MemoryScope::Global, memory("free-model-for-tests"))],
        );
        assert!(
            full.ends_with("in full:\n### free-model-for-tests (feedback, 2026-11-02): Use the free model for test runs (global)\nUse the free model for test runs.\n\nWhy: said so.\nHow to apply: pick it."),
            "{full}"
        );
    }

    #[test]
    fn relevant_memories_share_words_with_the_message() {
        let global = "- free-model-for-tests (feedback, 2026-11-02): Use the free model for test runs\n- short-replies (feedback, 2026-11-01): Keep replies short\n";
        let project = "- deploy-branch (project, 2026-11-02): Deploy from the release branch\n";
        assert_eq!(
            relevant_names("Which model should the tests use?", global, project),
            [(MemoryScope::Global, "free-model-for-tests".to_owned())]
        );
        assert_eq!(
            relevant_names("deploy it to the release", global, project),
            [(MemoryScope::Project, "deploy-branch".to_owned())]
        );
        // Short or common words alone pick nothing.
        assert!(relevant_names("hi, what is it?", global, project).is_empty());
        let many: String = (0..50)
            .map(|n| format!("- m-{n} (user, 2026-11-02): Tests run here\n"))
            .collect();
        assert_eq!(relevant_names("tests", &many, "").len(), MAX_RELEVANT);
    }

    /// The memory block reads two index files and at most [`MAX_RELEVANT`] memories whatever the
    /// memory count, so each run's cost stays flat as memory grows.
    #[test]
    fn the_memory_block_reads_a_bounded_number_of_files() {
        fn work(count: usize) -> ((u64, u64), usize) {
            let root = tempfile::tempdir().unwrap();
            let global = MemoryStore::new(root.path(), GLOBAL_FOLDER).unwrap();
            let project = MemoryStore::new(root.path(), "projects/abc").unwrap();
            for index in 0..count {
                for store in [&global, &project] {
                    let mut item = memory(&format!("m-{index}"));
                    item.description = format!("Tests fact number {index}");
                    std::fs::create_dir_all(store.dir()).unwrap();
                    std::fs::write(
                        store.dir().join(format!("{}.md", item.name)),
                        serialize(&item),
                    )
                    .unwrap();
                }
            }
            for store in [&global, &project] {
                store.write_index(&store.list().unwrap()).unwrap();
            }
            WORK.with(|work| work.set((0, 0)));
            let block = memory_block(&global, Some(&project), "how do the tests run?");
            (WORK.with(|work| work.replace((0, 0))), block.len())
        }
        let (small, small_bytes) = work(10);
        let (large, large_bytes) = work(200);
        eprintln!("memory block: 10 memories {small:?} {small_bytes} bytes, 200 memories {large:?} {large_bytes} bytes");
        assert_eq!(small.0, 2 + MAX_RELEVANT as u64);
        assert_eq!(large.0, 2 + MAX_RELEVANT as u64);
        assert!(large.1 <= (2 * MAX_INDEX_BYTES) as u64 + MAX_RELEVANT as u64 * MAX_FILE_BYTES);
        assert!(
            large_bytes
                <= 2 * MAX_INDEX_BYTES
                    + MAX_RELEVANT_BYTES
                    + crate::prompts::SYSTEM_MEMORY.len()
                    + crate::prompts::SYSTEM_MEMORY_RELEVANT.len()
        );
    }

    #[test]
    fn names_are_kebab_slugs() {
        for good in ["a", "free-model-2", "x1-y2"] {
            assert!(valid_name(good), "{good}");
        }
        for bad in [
            "",
            "-a",
            "a-",
            "a--b",
            "A",
            "a_b",
            "a/b",
            "..",
            "a.b",
            &"a".repeat(65),
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn global_is_only_for_the_person_and_their_feedback() {
        assert!(MemoryKind::User.may_be_global());
        assert!(MemoryKind::Feedback.may_be_global());
        assert!(!MemoryKind::Project.may_be_global());
        assert!(!MemoryKind::Reference.may_be_global());
    }

    #[test]
    fn secrets_are_stripped() {
        let cleaned = strip_secrets(
            "Deploy key is ghp_abcdefghijklmnopqrstuvwxyz0123456789AB and password=hunter2\nAPI_KEY=sk-abcdefghijklmnopqrstuvwxyz012345 here",
        );
        assert!(!cleaned.contains("ghp_abcdefghij"), "{cleaned}");
        assert!(!cleaned.contains("hunter2"), "{cleaned}");
        assert!(!cleaned.contains("sk-abcdefghij"), "{cleaned}");
        assert!(cleaned.contains("Deploy key is"), "{cleaned}");
        assert_eq!(
            strip_secrets("Plain words: keep them.\nWhy: asked twice."),
            "Plain words: keep them.\nWhy: asked twice."
        );
        assert_eq!(strip_secrets("bell\u{7} ok"), "bell ok");
    }

    #[test]
    fn the_store_saves_lists_indexes_and_deletes() {
        let root = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(root.path(), GLOBAL_FOLDER).unwrap();
        assert_eq!(store.index(), "");
        assert_eq!(store.save(&memory("one"), SaveMode::Create).unwrap(), 1);
        assert!(store.save(&memory("one"), SaveMode::Create).is_err());
        assert!(store.save(&memory("two"), SaveMode::Update).is_err());
        let mut newer = memory("two");
        newer.saved = "2026-11-03".to_owned();
        newer.kind = MemoryKind::User;
        store.save(&newer, SaveMode::Create).unwrap();
        assert_eq!(
            store.index(),
            "- two (user, 2026-11-03): Use the free model for test runs\n- one (feedback, 2026-11-02): Use the free model for test runs\n"
        );
        let revision = store.revision();
        assert!(store.delete("one", Some("stale")).is_err());
        assert_eq!(store.delete("one", Some(&revision)).unwrap().name, "one");
        assert_eq!(store.list().unwrap().memories.len(), 1);
        assert!(!store.index().contains("- one"));
        assert!(store.read("one").unwrap().is_none());
        assert!(store.read("../x").is_err());
    }

    #[test]
    fn an_update_can_rename_and_leaves_one_file() {
        let root = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(root.path(), GLOBAL_FOLDER).unwrap();
        store
            .save(&memory("deploy-via-staging-branch"), SaveMode::Create)
            .unwrap();
        store.save(&memory("other"), SaveMode::Create).unwrap();
        let corrected = memory("deploy-via-release-branch");
        // Onto a name in use, or from a name not in use: refused, nothing changes.
        assert!(store
            .rename("deploy-via-staging-branch", &memory("other"))
            .is_err());
        assert!(store.rename("missing", &corrected).is_err());
        assert!(store.rename("../x", &corrected).is_err());
        let mut invalid = corrected.clone();
        invalid.description = String::new();
        assert!(store.rename("deploy-via-staging-branch", &invalid).is_err());
        assert!(store.read("deploy-via-staging-branch").unwrap().is_some());
        assert_eq!(
            store
                .rename("deploy-via-staging-branch", &corrected)
                .unwrap(),
            2
        );
        assert!(store.read("deploy-via-staging-branch").unwrap().is_none());
        assert!(!store.dir().join("deploy-via-staging-branch.md").exists());
        assert_eq!(
            store.read("deploy-via-release-branch").unwrap(),
            Some(corrected.clone())
        );
        assert!(store.index().contains("- deploy-via-release-branch ("));
        assert!(!store.index().contains("staging"));
        // Renaming to its own name is a plain update.
        assert_eq!(
            store
                .rename("deploy-via-release-branch", &corrected)
                .unwrap(),
            2
        );
    }

    #[test]
    fn an_updated_body_keeps_one_why_and_one_how_line() {
        // The fields replace Why and How lines the body repeats.
        assert_eq!(
            compose_body(
                "Deploy via the release branch.\n\nWhy: staging is shared\nHow to apply: push to staging",
                Some("staging was retired"),
                Some("push to release"),
            ),
            "Deploy via the release branch.\n\nWhy: staging was retired\nHow to apply: push to release"
        );
        // Without the fields, only the last of repeated lines is kept, at the end.
        assert_eq!(
            compose_body(
                "Deploy via release.\nWhy: old\n\nWhy: new\nhow to apply: merge",
                None,
                None
            ),
            "Deploy via release.\n\nWhy: new\nHow to apply: merge"
        );
        assert_eq!(compose_body("Just a fact.", None, None), "Just a fact.");
        assert_eq!(
            compose_body("Fact.", Some("a\nb"), None),
            "Fact.\n\nWhy: a b"
        );
    }

    #[test]
    fn folders_must_be_plain_and_never_symlinks() {
        let root = tempfile::tempdir().unwrap();
        for bad in ["", "../x", "/abs", "a/../b", "a b"] {
            assert!(MemoryStore::new(root.path(), bad).is_err(), "{bad}");
        }
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("projects")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), root.path().join("projects/abc")).unwrap();
            let store = MemoryStore::new(root.path(), "projects/abc").unwrap();
            assert!(store.save(&memory("x"), SaveMode::Create).is_err());
            assert!(store.list().is_err());
            assert_eq!(store.index(), "");
            assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
            // A memory file that is a symlink is skipped, never followed.
            let store = MemoryStore::new(root.path(), GLOBAL_FOLDER).unwrap();
            store.save(&memory("real"), SaveMode::Create).unwrap();
            std::fs::write(outside.path().join("secret.md"), serialize(&memory("link"))).unwrap();
            std::os::unix::fs::symlink(
                outside.path().join("secret.md"),
                store.dir().join("link.md"),
            )
            .unwrap();
            let listing = store.list().unwrap();
            assert_eq!(listing.memories.len(), 1);
            assert_eq!(listing.skipped, 1);
            assert!(store.read("link").is_err());
        }
    }

    #[test]
    fn project_folders_differ_by_project_and_stay_plain() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        assert_ne!(project_folder(a.path()), project_folder(b.path()));
        assert_eq!(project_folder(a.path()), project_folder(a.path()));
        assert!(MemoryStore::new(a.path(), &project_folder(a.path())).is_ok());
    }

    #[test]
    fn the_index_is_capped_in_lines_and_bytes() {
        let many: Vec<Memory> = (0..MAX_MEMORIES + 50)
            .map(|index| memory(&format!("m-{index}")))
            .collect();
        let index = build_index(&many);
        assert!(index.lines().count() <= MAX_MEMORIES);
        assert!(index.len() <= MAX_INDEX_BYTES);
        let long: Vec<Memory> = (0..MAX_MEMORIES)
            .map(|index| Memory {
                description: "d".repeat(MAX_DESCRIPTION_CHARS),
                source: Some("s".repeat(MAX_SOURCE_CHARS)),
                ..memory(&format!("m-{index}"))
            })
            .collect();
        assert!(build_index(&long).len() <= MAX_INDEX_BYTES);
        // A full scope refuses another memory.
        let root = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(root.path(), GLOBAL_FOLDER).unwrap();
        for item in many.iter().take(MAX_MEMORIES) {
            std::fs::create_dir_all(store.dir()).unwrap();
            std::fs::write(
                store.dir().join(format!("{}.md", item.name)),
                serialize(item),
            )
            .unwrap();
        }
        assert!(store
            .save(&memory("one-more"), SaveMode::Create)
            .unwrap_err()
            .0
            .contains("full"));
    }

    /// Rebuilding the index reads each memory once, so it grows with the memories and no
    /// faster; reading it at the start of a run is one bounded file whatever the count.
    #[test]
    fn index_work_stays_linear_and_its_read_bounded() {
        fn work(count: usize) -> ((u64, u64), (u64, u64)) {
            let root = tempfile::tempdir().unwrap();
            let store = MemoryStore::new(root.path(), GLOBAL_FOLDER).unwrap();
            std::fs::create_dir_all(store.dir()).unwrap();
            for index in 0..count {
                let item = memory(&format!("m-{index}"));
                std::fs::write(
                    store.dir().join(format!("{}.md", item.name)),
                    serialize(&item),
                )
                .unwrap();
            }
            WORK.with(|work| work.set((0, 0)));
            let listing = store.list().unwrap();
            store.write_index(&listing).unwrap();
            let rebuild = WORK.with(|work| work.replace((0, 0)));
            let index = store.index();
            assert!(index.len() <= MAX_INDEX_BYTES);
            let read = WORK.with(|work| work.replace((0, 0)));
            (rebuild, read)
        }
        let (small_rebuild, small_read) = work(10);
        let (large_rebuild, large_read) = work(200);
        // Files opened and bytes read: linear, exactly one file per memory.
        assert_eq!(small_rebuild.0, 10);
        assert_eq!(large_rebuild.0, 200);
        let growth = (large_rebuild.1 as f64 / small_rebuild.1 as f64).ln() / 20f64.ln();
        assert!((0.9..1.1).contains(&growth), "rebuild grows at {growth}");
        // The run-start read: one file, bounded bytes.
        assert_eq!(small_read.0, 1);
        assert_eq!(large_read.0, 1);
        assert!(large_read.1 <= MAX_INDEX_BYTES as u64);
    }

    fn arbitrary_memory() -> impl Strategy<Value = Memory> {
        (
            "[a-z0-9]{1,8}(-[a-z0-9]{1,8}){0,3}",
            "[A-Za-z0-9 ,.:'-]{0,40}[A-Za-z0-9.]",
            0usize..4,
            prop::option::of("[A-Za-z0-9.:/]{1,30}"),
            "[A-Za-z0-9 .,:\n-]{0,200}[A-Za-z0-9.]",
        )
            .prop_map(|(name, description, kind, source, body)| Memory {
                name,
                description,
                kind: MemoryKind::ALL[kind],
                saved: "2026-11-02".to_owned(),
                source,
                body,
            })
    }

    proptest! {
        #[test]
        fn valid_memories_round_trip(memory in arbitrary_memory()) {
            prop_assume!(memory.validate().is_ok());
            prop_assert_eq!(parse(&serialize(&memory)).unwrap(), memory);
        }

        #[test]
        fn parse_never_panics_and_what_it_accepts_is_valid(text in "(---\n)?[ -~\n\r]{0,400}") {
            if let Ok(memory) = parse(&text) {
                prop_assert!(memory.validate().is_ok());
                prop_assert!(valid_name(&memory.name));
                prop_assert_eq!(parse(&serialize(&memory)).unwrap(), memory);
            }
        }

        #[test]
        fn header_like_text_never_escapes_a_valid_name(name in "[ -~]{0,30}") {
            let text = format!("---\nname: {name}\ndescription: d\ntype: user\nsaved: 2026-11-02\n---\nbody\n");
            if let Ok(memory) = parse(&text) {
                prop_assert!(!memory.name.contains('/') && !memory.name.contains('.'));
            }
        }

        #[test]
        fn stripping_secrets_never_panics_and_keeps_lines(text in "[ -~\n]{0,300}") {
            let cleaned = strip_secrets(&text);
            prop_assert!(!cleaned.chars().any(|c| c.is_control() && c != '\n' && c != '\t'));
            prop_assert_eq!(cleaned.split('\n').count(), text.split('\n').count());
        }

        #[test]
        fn a_composed_body_never_holds_two_why_or_how_lines(
            fact in "((Why|why|WHY|How to apply|how to apply): [a-zé ]{0,12}\n|[ -~é]{0,20}\n){0,8}",
            why in prop::option::of("[ -~é\n]{0,20}"),
            how in prop::option::of("[ -~é\n]{0,20}"),
        ) {
            let body = compose_body(&fact, why.as_deref(), how.as_deref());
            let count = |label: &str| body
                .lines()
                .filter(|line| line.trim_start().to_ascii_lowercase().starts_with(label))
                .count();
            prop_assert!(count("why:") <= 1, "{}", body);
            prop_assert!(count("how to apply:") <= 1, "{}", body);
            // Composing again changes nothing.
            prop_assert_eq!(compose_body(&body, None, None), body);
        }
    }
}
