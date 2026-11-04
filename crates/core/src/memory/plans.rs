//! Plan records: one per piece of work the person approved, in the project's memory folder
//! (`plans/<name>.md`), beside the memories but apart from them. A record keeps the approved
//! plan word for word, the changes the person made since (with the steps they touched
//! rewritten), the status, and once the work is done its results, the evidence (copied as the
//! agent gave it) and the lessons worth keeping. Events drive it: the memory models name a plan
//! the person approved and propose changes and the done record; Themis copies the plan text and
//! a goal's evidence itself, so neither is summarized away. Everything that reads a record
//! treats it as untrusted text.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use super::{fail, read_bounded, strip_secrets, valid_name, MemoryError, MemoryStore, WRITING};

/// The folder, inside a project's memory folder, that holds its plan records.
pub const PLANS_FOLDER: &str = "plans";
/// The index file of the plan records, one line each.
pub const PLANS_INDEX_FILE: &str = "PLANS.md";
/// Most plan records one project keeps.
pub const MAX_PLANS: usize = 50;
/// Most bytes of the plan text kept; past it the end is cut.
pub const MAX_PLAN_BYTES: usize = 8 * 1024;
/// Most bytes of a record file; a larger file is not read. A record trims itself to fit (old
/// closed tasks, decisions and changes first), so a growing piece of work never passes it.
pub const MAX_RECORD_BYTES: u64 = 64 * 1024;
/// Most decision lines one record keeps; the oldest go first.
pub const MAX_DECISIONS: usize = 12;
/// Most "Change: …" lines one record keeps; the oldest go first.
pub const MAX_CHANGES: usize = 20;
/// Most characters of one change, evidence or lesson line.
pub const MAX_LINE_CHARS: usize = 400;
/// Most bytes of the results.
pub const MAX_RESULTS_BYTES: usize = 2 * 1024;
/// Most characters of one evidence line: a proof can be long.
pub const MAX_EVIDENCE_CHARS: usize = 1000;
/// Most evidence lines.
pub const MAX_EVIDENCE: usize = 12;
/// Most lessons.
pub const MAX_LESSONS: usize = 3;
/// Most step edits one change applies.
pub const MAX_EDITS: usize = 8;
/// Most characters of a record's title.
pub const MAX_TITLE_CHARS: usize = 120;

const HEADINGS: [&str; 7] = [
    "## Plan",
    "## Changes",
    "## Decisions",
    "## Tasks",
    "## Results",
    "## Evidence",
    "## Lessons",
];
/// Sections a record written before they existed may lack.
const OPTIONAL: [&str; 2] = ["## Decisions", "## Tasks"];

/// Where the work stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    /// The person approved it; no goal is building it.
    Approved,
    /// A goal is building it.
    InProgress,
    /// Built.
    Done,
}

impl PlanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::InProgress => "in progress",
            Self::Done => "done",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "approved" => Some(Self::Approved),
            "in progress" => Some(Self::InProgress),
            "done" => Some(Self::Done),
            _ => None,
        }
    }
}

/// One plan record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanRecord {
    /// Lowercase kebab-case slug; also the file name.
    pub name: String,
    /// One line saying what the work is.
    pub title: String,
    pub status: PlanStatus,
    /// The day it last changed (`YYYY-MM-DD`).
    pub updated: String,
    /// The chat whose goal builds it, while one does.
    pub chat: Option<String>,
    /// The approved plan, word for word, with the steps later changes touched rewritten.
    pub plan: String,
    /// One line per change the person made, oldest first.
    pub changes: Vec<String>,
    /// Choices made for this work, by the person or the agent, with why; oldest first.
    pub decisions: Vec<String>,
    /// The work's tasks, old and new, with their status.
    pub tasks: Vec<super::tasks::Task>,
    /// What was built, where, how to run and test it.
    pub results: String,
    /// One line per requirement: what proves it.
    pub evidence: Vec<String>,
    /// Rules that would save later work the same trouble, with why.
    pub lessons: Vec<String>,
}

/// One line: control characters and line breaks become spaces, within `max` characters.
fn line(text: &str, max: usize) -> String {
    crate::goal::one_line(text, max)
}

/// `text` within `max` bytes, cut at a character boundary with a mark when it is longer.
fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    const MARK: &str = "\n[…]";
    let mut end = max.saturating_sub(MARK.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{MARK}", &text[..end])
}

/// Free text kept in a record: secrets hidden, control characters (but line breaks and tabs)
/// removed, and no line that could pass for one of the record's own headings.
fn block(text: &str, max: usize) -> String {
    let text: String = strip_secrets(text)
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    let text = text
        .lines()
        .map(|row| {
            if HEADINGS.contains(&row.trim_end()) {
                format!("#{row}")
            } else {
                row.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    cut(&edges(&text), max)
}

/// `text` without blank lines at either end, keeping its first line's indentation: an edit of
/// an indented step (a file tree, a code block) must keep it, saved and read back.
fn edges(text: &str) -> String {
    let start = text
        .lines()
        .take_while(|row| row.trim().is_empty())
        .map(|row| row.len() + 1)
        .sum::<usize>()
        .min(text.len());
    text[start..].trim_end().to_owned()
}

impl PlanRecord {
    /// A new record of `plan` (the approved reply), kept word for word within its limit.
    pub fn new(name: &str, title: &str, plan: &str, chat: Option<&str>, today: &str) -> Self {
        let mut record = Self {
            name: name.to_owned(),
            title: line(&strip_secrets(title), MAX_TITLE_CHARS),
            status: if chat.is_some() {
                PlanStatus::InProgress
            } else {
                PlanStatus::Approved
            },
            updated: today.to_owned(),
            chat: chat.map(str::to_owned),
            plan: block(plan, MAX_PLAN_BYTES),
            changes: Vec::new(),
            decisions: Vec::new(),
            tasks: Vec::new(),
            results: String::new(),
            evidence: Vec::new(),
            lessons: Vec::new(),
        };
        record.seed_tasks(today);
        record
    }

    /// Records a change: each edit whose `old` text occurs exactly once in the plan is applied
    /// (the plan stays within its limit), then the change line is added. Returns how many edits
    /// were applied and how many were not.
    pub fn change(
        &mut self,
        change: &str,
        edits: &[(String, String)],
        today: &str,
    ) -> (usize, usize) {
        let (mut applied, mut missed) = (0, 0);
        for (old, new) in edits.iter().take(MAX_EDITS) {
            let new = block(new, MAX_PLAN_BYTES);
            let edited = (!old.trim().is_empty() && self.plan.matches(old.as_str()).count() == 1)
                .then(|| self.plan.replacen(old.as_str(), &new, 1))
                .filter(|plan| plan.len() <= MAX_PLAN_BYTES);
            match edited {
                Some(plan) => {
                    self.plan = plan;
                    applied += 1;
                }
                None => missed += 1,
            }
        }
        missed += edits.len().saturating_sub(MAX_EDITS);
        let change = line(&strip_secrets(change), MAX_LINE_CHARS);
        if !change.is_empty() {
            self.changes.push(change);
            let over = self.changes.len().saturating_sub(MAX_CHANGES);
            self.changes.drain(..over);
        }
        self.updated = today.to_owned();
        self.fit();
        (applied, missed)
    }

    /// Marks the work done. `evidence` replaces the evidence when it holds any; results and
    /// lessons replace theirs when given.
    pub fn done(&mut self, results: &str, evidence: &[String], lessons: &[String], today: &str) {
        self.status = PlanStatus::Done;
        if !results.trim().is_empty() {
            self.results = block(results, MAX_RESULTS_BYTES);
        }
        let lines = |items: &[String], max: usize, chars: usize| -> Vec<String> {
            items
                .iter()
                .map(|item| line(&strip_secrets(item), chars))
                .filter(|item| !item.is_empty())
                .take(max)
                .collect()
        };
        let evidence = lines(evidence, MAX_EVIDENCE, MAX_EVIDENCE_CHARS);
        if !evidence.is_empty() {
            self.evidence = evidence;
        }
        let lessons = lines(lessons, MAX_LESSONS, MAX_LINE_CHARS);
        if !lessons.is_empty() {
            self.lessons = lessons;
        }
        self.updated = today.to_owned();
        self.fit();
    }

    /// Keeps the record within [`MAX_RECORD_BYTES`], whatever its characters: the oldest
    /// changes go first, then the last evidence lines, the last lessons, and finally the
    /// results. The plan itself (at most [`MAX_PLAN_BYTES`]) always stays.
    fn fit(&mut self) {
        while serialize(self).len() as u64 > MAX_RECORD_BYTES {
            let closed = self.tasks.iter().position(|task| {
                matches!(
                    task.status,
                    super::tasks::TaskStatus::Done | super::tasks::TaskStatus::Cancelled
                )
            });
            if let Some(at) = closed {
                self.tasks.remove(at);
            } else if !self.decisions.is_empty() {
                self.decisions.remove(0);
            } else if !self.changes.is_empty() {
                self.changes.remove(0);
            } else if !self.evidence.is_empty() {
                self.evidence.pop();
            } else if !self.lessons.is_empty() {
                self.lessons.pop();
            } else if !self.results.is_empty() {
                self.results.clear();
            } else {
                break;
            }
        }
    }

    /// Records a decision: `who` (the person or the agent) chose `what`, because `why`.
    pub fn decide(&mut self, who: &str, what: &str, why: &str, today: &str) {
        let what = line(&strip_secrets(what), MAX_LINE_CHARS);
        if what.is_empty() {
            return;
        }
        let why = line(&strip_secrets(why), MAX_LINE_CHARS);
        let mut entry = format!("{who}: {what}");
        if !why.is_empty() {
            entry.push_str(&format!(" — because {why}"));
        }
        self.decisions
            .push(line(&format!("{entry} ({today})"), MAX_LINE_CHARS));
        let over = self.decisions.len().saturating_sub(MAX_DECISIONS);
        self.decisions.drain(..over);
        self.updated = today.to_owned();
        self.fit();
    }

    /// Adds an open task for each requirement of the plan that has none yet: each bullet or
    /// numbered step outside code blocks, a `**Label:**` step titled by its label. Themis does
    /// this itself (the memory models only move statuses), so a record's tasks cover its whole
    /// plan. Returns how many were added.
    pub fn seed_tasks(&mut self, today: &str) -> usize {
        let mut added = 0;
        for (title, note) in plan_requirements(&self.plan) {
            if self
                .tasks
                .iter()
                .any(|task| task.title.eq_ignore_ascii_case(&title))
            {
                continue;
            }
            let op = super::tasks::TaskOp::Add {
                title,
                note,
                status: super::tasks::TaskStatus::Open,
                work: None,
            };
            if super::tasks::apply(&mut self.tasks, &op, today) {
                added += 1;
            }
        }
        if added > 0 {
            self.fit();
        }
        added
    }

    /// Applies a task op to the work's tasks; whether anything changed.
    pub fn apply_task(&mut self, op: &super::tasks::TaskOp, today: &str) -> bool {
        let changed = super::tasks::apply(&mut self.tasks, op, today);
        if changed {
            self.updated = today.to_owned();
            self.fit();
        }
        changed
    }

    /// What a run is shown of the record: the plan, the changes, decisions, open tasks (done
    /// ones as a count) and lessons. Results and evidence stay in the file for `search_memory`,
    /// so the block stays the same size however long the work runs.
    pub fn run_view(&self) -> String {
        let mut text = format!(
            "{} ({}, {}): {}\n## Plan\n{}\n",
            self.name,
            self.status.as_str(),
            self.updated,
            self.title,
            self.plan.trim_end()
        );
        let lines = |items: &[String]| -> String {
            items.iter().map(|item| format!("- {item}\n")).collect()
        };
        for (heading, body) in [
            ("## Changes", lines(&self.changes)),
            ("## Decisions", lines(&self.decisions)),
            ("## Tasks", super::tasks::run_view(&self.tasks)),
            ("## Lessons", lines(&self.lessons)),
        ] {
            if !body.is_empty() {
                text.push_str(&format!("{heading}\n{body}"));
            }
        }
        if !self.results.is_empty() || !self.evidence.is_empty() {
            text.push_str(
                "(Results and evidence are in the full record: search_memory with its name.)\n",
            );
        }
        text
    }

    /// The record's index line: `- name (status, date): title`.
    pub fn index_line(&self) -> String {
        format!(
            "- {} ({}, {}): {}\n",
            self.name,
            self.status.as_str(),
            self.updated,
            self.title
        )
    }

    pub fn validate(&self) -> Result<(), MemoryError> {
        if !valid_name(&self.name) {
            return fail("a plan record's name is a lowercase kebab-case slug");
        }
        if self.title.trim().is_empty() || self.plan.trim().is_empty() {
            return fail("a plan record needs a title and its plan");
        }
        if self.chat.as_deref().is_some_and(|chat| {
            chat.is_empty() || !chat.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        }) {
            return fail("a plan record's chat is an id");
        }
        if serialize(self).len() as u64 > MAX_RECORD_BYTES {
            return fail("the plan record is too large");
        }
        Ok(())
    }
}

/// The requirements of an approved plan, as (title, note): each `- `, `* ` or `N. ` line
/// outside code blocks, without Markdown emphasis; a `**Label:** rest` line is titled by its
/// label, others by their opening words.
fn plan_requirements(plan: &str) -> Vec<(String, String)> {
    let mut fenced = false;
    let mut found = Vec::new();
    for row in plan.lines() {
        let row = row.trim();
        if row.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let item = row
            .strip_prefix("- ")
            .or_else(|| row.strip_prefix("* "))
            .or_else(|| {
                let (number, rest) = row.split_once(". ")?;
                (!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())).then_some(rest)
            });
        let Some(item) = item.map(str::trim).filter(|item| !item.is_empty()) else {
            continue;
        };
        let labelled = item
            .strip_prefix("**")
            .and_then(|rest| rest.split_once("**"))
            .map(|(label, rest)| {
                (
                    label.trim().trim_end_matches(':').to_owned(),
                    rest.trim_start_matches(':').trim().to_owned(),
                )
            });
        let plain = |text: &str| text.replace("**", "").replace('`', "");
        let (title, note) = match labelled {
            Some((label, rest)) if !label.is_empty() => (plain(&label), plain(&rest)),
            _ => {
                let text = plain(item);
                let title: String = text
                    .split_whitespace()
                    .take(10)
                    .collect::<Vec<_>>()
                    .join(" ");
                // A short step is its own title; a note would only repeat it.
                let note = if title == text.trim() {
                    String::new()
                } else {
                    text
                };
                (title, note)
            }
        };
        found.push((title, note));
    }
    found
}

/// The record's file text.
pub fn serialize(record: &PlanRecord) -> String {
    let mut text = format!(
        "---\nname: {}\ntitle: {}\nstatus: {}\nupdated: {}\n",
        record.name,
        record.title,
        record.status.as_str(),
        record.updated
    );
    if let Some(chat) = &record.chat {
        text.push_str(&format!("chat: {chat}\n"));
    }
    text.push_str("---\n");
    let list = |items: &[String]| {
        if items.is_empty() {
            "(none)".to_owned()
        } else {
            items
                .iter()
                .map(|item| format!("- {item}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    };
    let results = if record.results.is_empty() {
        "(none)".to_owned()
    } else {
        record.results.clone()
    };
    let tasks = if record.tasks.is_empty() {
        "(none)".to_owned()
    } else {
        super::tasks::render_list(&record.tasks)
    };
    for (heading, body) in HEADINGS.iter().zip([
        record.plan.clone(),
        list(&record.changes),
        list(&record.decisions),
        tasks,
        results,
        list(&record.evidence),
        list(&record.lessons),
    ]) {
        text.push_str(&format!("{heading}\n{}\n\n", body.trim_end()));
    }
    text.trim_end().to_owned() + "\n"
}

/// Reads a record's file text. Untrusted: a file someone edited may say anything, so
/// whatever doesn't fit the shape is refused, never guessed at.
pub fn parse(text: &str) -> Result<PlanRecord, MemoryError> {
    let text = text.replace("\r\n", "\n");
    let rest = text
        .strip_prefix("---\n")
        .ok_or_else(|| MemoryError("a plan record starts with its header".into()))?;
    let (header, mut body) = rest
        .split_once("\n---\n")
        .ok_or_else(|| MemoryError("a plan record's header has no end".into()))?;
    let mut fields = std::collections::BTreeMap::new();
    for row in header.lines() {
        let (key, value) = row
            .split_once(": ")
            .ok_or_else(|| MemoryError("a plan record's header line is not `key: value`".into()))?;
        fields.insert(key.trim(), value.trim().to_owned());
    }
    let field = |key: &str| {
        fields
            .get(key)
            .cloned()
            .ok_or_else(|| MemoryError(format!("a plan record needs `{key}`")))
    };
    // The sections are found from the end, so a plan that quotes a heading keeps it.
    let mut sections: Vec<String> = Vec::new();
    for heading in HEADINGS.iter().skip(1).rev() {
        let marker = format!("\n{heading}\n");
        let Some(at) = body.rfind(&marker) else {
            if OPTIONAL.contains(heading) {
                sections.push(String::new());
                continue;
            }
            return Err(MemoryError(format!("a plan record needs `{heading}`")));
        };
        sections.push(edges(&body[at + marker.len()..]));
        body = &body[..at];
    }
    sections.reverse();
    let plan = body
        .trim_start_matches('\n')
        .strip_prefix("## Plan\n")
        .ok_or_else(|| MemoryError("a plan record needs `## Plan`".into()))
        .map(edges)?;
    let list = |section: &str, chars: usize| -> Vec<String> {
        section
            .lines()
            .filter_map(|row| row.strip_prefix("- "))
            .map(|item| line(item, chars))
            .filter(|item| !item.is_empty())
            .collect()
    };
    // Changes, Decisions, Tasks, Results, Evidence, Lessons.
    let results = if sections[3] == "(none)" {
        String::new()
    } else {
        sections[3].clone()
    };
    let record = PlanRecord {
        name: field("name")?,
        title: line(&field("title")?, MAX_TITLE_CHARS),
        status: PlanStatus::parse(&field("status")?).ok_or_else(|| {
            MemoryError("a plan record's status is approved, in progress or done".into())
        })?,
        updated: field("updated")?,
        chat: fields.get("chat").cloned(),
        plan,
        changes: list(&sections[0], MAX_LINE_CHARS)
            .into_iter()
            .take(MAX_CHANGES)
            .collect(),
        decisions: list(&sections[1], MAX_LINE_CHARS)
            .into_iter()
            .take(MAX_DECISIONS)
            .collect(),
        tasks: super::tasks::parse_list(&sections[2]),
        results: cut(&results, MAX_RESULTS_BYTES),
        evidence: list(&sections[4], MAX_EVIDENCE_CHARS)
            .into_iter()
            .take(MAX_EVIDENCE)
            .collect(),
        lessons: list(&sections[5], MAX_LINE_CHARS)
            .into_iter()
            .take(MAX_LESSONS)
            .collect(),
    };
    record.validate()?;
    Ok(record)
}

/// One project's plan records.
pub struct PlanStore {
    store: MemoryStore,
}

impl PlanStore {
    /// The plan records of the project memory folder `folder` under `root` (the app data's
    /// `memory` folder). Nothing is created until something is saved.
    pub fn new(root: &Path, folder: &str) -> Result<Self, MemoryError> {
        Ok(Self {
            store: MemoryStore::new(root, &format!("{folder}/{PLANS_FOLDER}"))?,
        })
    }

    fn path_of(&self, name: &str) -> Result<PathBuf, MemoryError> {
        if !valid_name(name) {
            return fail("no plan record has that name: names are lowercase kebab-case slugs");
        }
        Ok(self.store.dir().join(format!("{name}.md")))
    }

    /// Every valid record, newest first then by name; at most [`MAX_PLANS`].
    pub fn list(&self) -> Result<Vec<PlanRecord>, MemoryError> {
        let Some(dir) = self.store.checked_dir(false)? else {
            return Ok(Vec::new());
        };
        let entries = std::fs::read_dir(&dir)
            .map_err(|_| MemoryError("plans folder is unreadable".into()))?;
        let mut records = Vec::new();
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(stem) = file_name.to_str().and_then(|name| name.strip_suffix(".md")) else {
                continue;
            };
            if records.len() >= MAX_PLANS * 2 {
                break;
            }
            if let Some(Ok(record)) =
                read_bounded(&entry.path(), MAX_RECORD_BYTES).map(|text| parse(&text))
            {
                if record.name == stem {
                    records.push(record);
                }
            }
        }
        records.sort_by(|a: &PlanRecord, b| {
            b.updated.cmp(&a.updated).then_with(|| a.name.cmp(&b.name))
        });
        records.truncate(MAX_PLANS);
        Ok(records)
    }

    /// One record, `None` when there is none by that name.
    pub fn read(&self, name: &str) -> Result<Option<PlanRecord>, MemoryError> {
        let path = self.path_of(name)?;
        if self.store.checked_dir(false)?.is_none() {
            return Ok(None);
        }
        match read_bounded(&path, MAX_RECORD_BYTES) {
            None if std::fs::symlink_metadata(&path).is_err() => Ok(None),
            None => fail("that plan record's file is unreadable"),
            Some(text) => parse(&text).map(Some),
        }
    }

    /// The project's tasks outside any plan record, beside the records.
    pub fn tasks(&self) -> super::tasks::TaskFile {
        super::tasks::TaskFile::new(self.store.dir())
    }

    /// The record a goal in `chat` is building, if any.
    pub fn for_chat(&self, chat: &str) -> Option<PlanRecord> {
        self.list()
            .ok()?
            .into_iter()
            .find(|record| record.chat.as_deref() == Some(chat))
    }

    /// The saved index (one line per record), as runs read it.
    pub fn index(&self) -> String {
        if !matches!(self.store.checked_dir(false), Ok(Some(_))) {
            return String::new();
        }
        let Some(text) = read_bounded(&self.store.dir().join(PLANS_INDEX_FILE), 8 * 1024) else {
            return String::new();
        };
        text.lines()
            .filter(|row| row.starts_with("- "))
            .take(MAX_PLANS)
            .map(|row| format!("{row}\n"))
            .collect()
    }

    /// Saves `record` (validated; a new one only while the project has room) and rewrites the
    /// index. `create` refuses a name in use.
    pub fn save(&self, record: &PlanRecord, create: bool) -> Result<(), MemoryError> {
        record.validate()?;
        let _writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        self.store.checked_dir(true)?;
        let path = self.path_of(&record.name)?;
        let exists = std::fs::symlink_metadata(&path).is_ok();
        if exists && !std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
            return fail("that plan record's file is not a regular file");
        }
        if create && exists {
            return fail(format!(
                "a plan record named {} already exists",
                record.name
            ));
        }
        if !create && !exists {
            return fail(format!("no plan record is named {}", record.name));
        }
        if create && self.list()?.len() >= MAX_PLANS {
            return fail(format!(
                "plan records are full ({MAX_PLANS}): delete one first"
            ));
        }
        crate::chat_grants::write_atomic(&path, &serialize(record))
            .map_err(|_| MemoryError("plan record could not be saved".into()))?;
        self.write_index()
    }

    /// Deletes a record and rewrites the index.
    pub fn delete(&self, name: &str) -> Result<PlanRecord, MemoryError> {
        let _writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        let Some(record) = self.read(name)? else {
            return fail(format!("no plan record is named {name}"));
        };
        std::fs::remove_file(self.path_of(name)?)
            .map_err(|_| MemoryError("plan record could not be deleted".into()))?;
        self.write_index()?;
        Ok(record)
    }

    fn write_index(&self) -> Result<(), MemoryError> {
        let index: String = self.list()?.iter().map(PlanRecord::index_line).collect();
        crate::chat_grants::write_atomic(&self.store.dir().join(PLANS_INDEX_FILE), &index)
            .map_err(|_| MemoryError("plans index could not be saved".into()))
    }
}

/// The record in `index` that `message` is most about: the line sharing the most words of at
/// least four letters with it (at least two), newest first on a tie. No model, no file read.
pub fn relevant_name(message: &str, index: &str) -> Option<String> {
    let wanted = super::terms(message);
    index
        .lines()
        .filter_map(super::auto::index_entry)
        .map(|(name, title)| {
            (
                super::terms(&format!("{name} {title}"))
                    .intersection(&wanted)
                    .count(),
                name,
            )
        })
        .filter(|(score, _)| *score >= 2)
        .fold(None::<(usize, &str)>, |best, (score, name)| match best {
            Some((top, _)) if top >= score => best,
            _ => Some((score, name)),
        })
        .map(|(_, name)| name.to_owned())
}

/// What a run is told about plan records: the index, then `record` in full; nothing when the
/// project has none.
pub fn run_note(index: &str, record: Option<&PlanRecord>) -> String {
    if index.trim().is_empty() {
        return String::new();
    }
    let mut note = crate::prompts::fill(
        crate::prompts::SYSTEM_MEMORY_PLANS,
        &[("index", index.trim_end())],
    );
    if let Some(record) = record {
        note.push('\n');
        note.push_str(&crate::prompts::fill(
            crate::prompts::SYSTEM_MEMORY_PLAN,
            &[("record", record.run_view().trim_end())],
        ));
    }
    note
}

/// What the memory models proposed for a plan record, validated (secrets refused).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanOp {
    /// The reply the person answered is a plan they approved: record it under `name`.
    Create { name: String, title: String },
    /// The person changed the plan: rewrite the steps it touches (`old` → `new`, exact text)
    /// and add the change line.
    Change {
        name: String,
        change: String,
        edits: Vec<(String, String)>,
    },
    /// The work is done: what was built and how to run and test it, the evidence when no
    /// goal gave any, and lessons.
    Done {
        name: String,
        results: String,
        evidence: Vec<String>,
        lessons: Vec<String>,
    },
    /// The person said to forget it.
    Delete { name: String },
}

impl PlanOp {
    pub fn name(&self) -> &str {
        match self {
            Self::Create { name, .. }
            | Self::Change { name, .. }
            | Self::Done { name, .. }
            | Self::Delete { name } => name,
        }
    }
}

/// Why a plan op was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpRefused {
    Invalid,
    Secret,
}

fn text(entry: &Value, key: &str) -> Option<String> {
    let value: String = entry[key]
        .as_str()?
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn texts(entry: &Value, key: &str, max: usize) -> Vec<String> {
    entry[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|item| line(item, MAX_LINE_CHARS))
        .filter(|item| !item.is_empty())
        .take(max)
        .collect()
}

/// Reads the answer's `"plan"` entry (`{op, name, …}`); `None` when there is none. Untrusted.
pub fn parse_op(entry: &Value) -> Option<Result<PlanOp, OpRefused>> {
    if entry.is_null() {
        return None;
    }
    let name = match entry["name"].as_str().map(str::trim) {
        Some(name) if valid_name(name) => name.to_owned(),
        _ => return Some(Err(OpRefused::Invalid)),
    };
    let op = match entry["op"].as_str() {
        Some("create") => match text(entry, "title") {
            Some(title) => PlanOp::Create { name, title },
            None => return Some(Err(OpRefused::Invalid)),
        },
        Some("change") => {
            let edits: Vec<(String, String)> = entry["edits"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|edit| {
                    Some((
                        edit["old"].as_str()?.to_owned(),
                        edit["new"].as_str()?.to_owned(),
                    ))
                })
                .take(MAX_EDITS)
                .collect();
            match text(entry, "change") {
                Some(change) => PlanOp::Change {
                    name,
                    change,
                    edits,
                },
                None => return Some(Err(OpRefused::Invalid)),
            }
        }
        Some("done") => PlanOp::Done {
            name,
            results: text(entry, "results").unwrap_or_default(),
            evidence: texts(entry, "evidence", MAX_EVIDENCE).into_iter().collect(),
            lessons: texts(entry, "lessons", MAX_LESSONS),
        },
        Some("delete") => PlanOp::Delete { name },
        _ => return Some(Err(OpRefused::Invalid)),
    };
    let words: Vec<&str> = match &op {
        PlanOp::Create { title, .. } => vec![title],
        PlanOp::Change { change, edits, .. } => std::iter::once(change.as_str())
            .chain(edits.iter().map(|(_, new)| new.as_str()))
            .collect(),
        PlanOp::Done {
            results,
            evidence,
            lessons,
            ..
        } => std::iter::once(results.as_str())
            .chain(evidence.iter().map(String::as_str))
            .chain(lessons.iter().map(String::as_str))
            .collect(),
        PlanOp::Delete { .. } => Vec::new(),
    };
    if words.iter().any(|word| strip_secrets(word) != *word) {
        return Some(Err(OpRefused::Secret));
    }
    Some(Ok(op))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const PLAN: &str = "1. People: add and remove.\n2. Amounts in USD, leftover cent to the first person.\n3. JSON export and import.\n\n## Changes\nquoted heading stays in the plan";

    fn record() -> PlanRecord {
        PlanRecord::new(
            "splitter-app",
            "Expense splitter in splitter/",
            PLAN,
            Some("chat-1"),
            "2026-11-03",
        )
    }

    #[test]
    fn a_record_keeps_the_plan_word_for_word_and_reads_back() {
        let record = record();
        assert_eq!(record.status, PlanStatus::InProgress);
        assert!(
            record.plan.contains("## Changes\nquoted")
                || record.plan.contains("### Changes\nquoted")
        );
        let text = serialize(&record);
        assert_eq!(parse(&text).unwrap(), record);
        assert_eq!(
            record.index_line(),
            "- splitter-app (in progress, 2026-11-03): Expense splitter in splitter/\n"
        );
    }

    #[test]
    fn a_change_rewrites_only_the_steps_it_touches_and_adds_its_line() {
        let mut record = record();
        let (applied, missed) = record.change(
            "Amounts in euros only; the leftover cent goes to the payer.",
            &[
                (
                    "2. Amounts in USD, leftover cent to the first person.".to_owned(),
                    "2. Amounts in euros only, leftover cent to the payer.".to_owned(),
                ),
                ("no such step".to_owned(), "x".to_owned()),
            ],
            "2026-11-04",
        );
        assert_eq!((applied, missed), (1, 1));
        assert!(record
            .plan
            .contains("2. Amounts in euros only, leftover cent to the payer."));
        assert!(record.plan.contains("1. People: add and remove.\n"));
        assert!(record.plan.contains("3. JSON export and import."));
        assert_eq!(
            record.changes,
            ["Amounts in euros only; the leftover cent goes to the payer."]
        );
        assert_eq!(parse(&serialize(&record)).unwrap(), record);
    }

    /// A record keeps the work's decisions and tasks, old and new; a record written before
    /// those sections existed still reads; a run sees open tasks and decisions but not results.
    #[test]
    fn a_record_keeps_decisions_and_tasks_and_older_records_still_read() {
        use super::super::tasks::{TaskOp, TaskStatus};
        let old = "---\nname: old\ntitle: Old work\nstatus: done\nupdated: 2026-11-01\n---\n## Plan\n1. Build it.\n\n## Changes\n(none)\n\n## Results\nBuilt it.\n\n## Evidence\n- built: ls\n\n## Lessons\n(none)\n";
        let record = parse(old).unwrap();
        assert!(record.decisions.is_empty() && record.tasks.is_empty());
        assert_eq!(parse(&serialize(&record)).unwrap(), record);

        let mut record = record;
        record.decide(
            "agent",
            "pool asteroid meshes",
            "creating them per frame leaks GPU memory",
            "2026-11-04",
        );
        record.decide("person", "keep the mute key M", "", "2026-11-04");
        let add = |title: &str| TaskOp::Add {
            title: title.into(),
            note: String::new(),
            status: TaskStatus::Open,
            work: None,
        };
        assert!(record.apply_task(&add("Sound on hit"), "2026-11-04"));
        assert!(record.apply_task(&add("Back/forward cache check"), "2026-11-04"));
        assert!(record.apply_task(
            &TaskOp::Update {
                title: "Sound on hit".into(),
                status: TaskStatus::Done,
                note: "heard by the person".into(),
                work: None
            },
            "2026-11-04"
        ));
        let back = parse(&serialize(&record)).unwrap();
        assert_eq!(back, record);
        assert_eq!(back.decisions[0], "agent: pool asteroid meshes — because creating them per frame leaks GPU memory (2026-11-04)");
        let view = record.run_view();
        assert!(
            view.contains("## Decisions\n- agent: pool asteroid meshes"),
            "{view}"
        );
        assert!(
            view.contains("- [open] Back/forward cache check (2026-11-04)"),
            "{view}"
        );
        assert!(view.contains("- 1 done: Sound on hit"), "{view}");
        assert!(!view.contains("Built it."), "results stay out: {view}");
        assert!(view.contains("search_memory"), "{view}");
    }

    /// A record that would pass its size drops old closed tasks before anything else.
    #[test]
    fn a_full_record_drops_old_closed_tasks_first() {
        use super::super::tasks::{TaskOp, TaskStatus};
        let mut record = record();
        let long = "x".repeat(290);
        for n in 0..200 {
            let title = format!("task {n} {}", "y".repeat(100));
            record.apply_task(
                &TaskOp::Add {
                    title: title.clone(),
                    note: long.clone(),
                    status: TaskStatus::Open,
                    work: None,
                },
                "2026-11-04",
            );
            record.apply_task(
                &TaskOp::Update {
                    title,
                    status: TaskStatus::Done,
                    note: long.clone(),
                    work: None,
                },
                "2026-11-04",
            );
        }
        record.apply_task(
            &TaskOp::Add {
                title: "still open".into(),
                note: long.clone(),
                status: TaskStatus::Open,
                work: None,
            },
            "2026-11-04",
        );
        assert!(serialize(&record).len() as u64 <= MAX_RECORD_BYTES);
        assert!(record.tasks.iter().any(|task| task.title == "still open"));
        assert_eq!(record.changes.len(), 0);
        assert!(record.plan.contains("1. People"), "{}", record.plan);
        assert_eq!(parse(&serialize(&record)).unwrap(), record);
    }

    /// Every plan step becomes a task Themis tracks, titled by its label; code blocks and
    /// prose are not steps; a second seeding adds only what is missing.
    #[test]
    fn a_plans_steps_become_its_tasks() {
        let mut record = PlanRecord::new(
            "dodger",
            "Dodger",
            "## Gameplay\n- **Ship:** arrow keys or WASD move it.\n- **Lives:** three, with a `1.5 s` blink.\n\nSome prose.\n```\n- not a step\n```\n## How we test it\n1. **Unit tests:** node --test dodger/spec\n2. Browser check over http with a clean console",
            None,
            "2026-11-04",
        );
        let titles: Vec<&str> = record
            .tasks
            .iter()
            .map(|task| task.title.as_str())
            .collect();
        assert_eq!(
            titles,
            [
                "Ship",
                "Lives",
                "Unit tests",
                "Browser check over http with a clean console"
            ],
            "{:?}",
            record.tasks
        );
        assert_eq!(record.tasks[1].note, "three, with a 1.5 s blink.");
        assert_eq!(record.seed_tasks("2026-11-04"), 0);
        record.plan.push_str("\n3. Sound by ear");
        assert_eq!(record.seed_tasks("2026-11-05"), 1);
        assert_eq!(parse(&serialize(&record)).unwrap(), record);
    }

    /// An edit of an indented line keeps its indentation: a live run's edit of a file tree moved
    /// `score.js` out of its folder when the new text was trimmed.
    #[test]
    fn an_edit_of_an_indented_line_keeps_its_indentation() {
        let mut record = PlanRecord::new(
            "tree",
            "A tree",
            "```\ngame/\n    state.js   phases\n    score.js   scoring\n```",
            None,
            "2026-11-04",
        );
        let (applied, missed) = record.change(
            "Added aliens.js",
            &[(
                "    score.js   scoring".to_owned(),
                "    score.js   scoring\n    aliens.js  invaders".to_owned(),
            )],
            "2026-11-04",
        );
        assert_eq!((applied, missed), (1, 0));
        assert!(
            record.plan.contains(
                "\n    state.js   phases\n    score.js   scoring\n    aliens.js  invaders\n"
            ),
            "{}",
            record.plan
        );
        assert_eq!(parse(&serialize(&record)).unwrap(), record);
    }

    #[test]
    fn done_keeps_results_evidence_and_lessons() {
        let mut record = record();
        record.done(
            "Built splitter/ (index.html, split.js); run: node --test splitter/spec",
            &["Balances sum to zero: node --test: 24 pass".to_owned()],
            &["Keep money in integer cents: floats drifted by a cent".to_owned()],
            "2026-11-04",
        );
        // Done keeps the chat link: that chat may go on about the work.
        assert_eq!(
            (record.status, record.chat.as_deref()),
            (PlanStatus::Done, Some("chat-1"))
        );
        let back = parse(&serialize(&record)).unwrap();
        assert_eq!(back, record);
        assert_eq!(back.evidence.len(), 1);
        // A later done without evidence keeps the evidence already there.
        record.done("", &[], &[], "2026-11-05");
        assert_eq!(record.evidence.len(), 1);
    }

    #[test]
    fn secrets_never_reach_a_record() {
        let record = PlanRecord::new(
            "x",
            "t",
            "use key sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            None,
            "2026-11-03",
        );
        assert!(!record.plan.contains("sk-ant-api03-AAAA"));
        let op = parse_op(
            &serde_json::json!({"op":"done","name":"x","results":"token ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}),
        );
        assert_eq!(op, Some(Err(OpRefused::Secret)));
    }

    #[test]
    fn ops_are_read_and_bad_ones_refused() {
        assert_eq!(parse_op(&Value::Null), None);
        assert_eq!(
            parse_op(&serde_json::json!({"op":"create","name":"splitter-app","title":"Splitter"})),
            Some(Ok(PlanOp::Create {
                name: "splitter-app".into(),
                title: "Splitter".into()
            }))
        );
        for bad in [
            serde_json::json!({"op":"create","name":"Bad Name","title":"x"}),
            serde_json::json!({"op":"create","name":"ok"}),
            serde_json::json!({"op":"change","name":"ok"}),
            serde_json::json!({"op":"rename","name":"ok"}),
        ] {
            assert_eq!(parse_op(&bad), Some(Err(OpRefused::Invalid)), "{bad}");
        }
    }

    #[test]
    fn the_store_saves_lists_finds_the_chat_and_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let store = PlanStore::new(dir.path(), "project-abc").unwrap();
        assert!(store.list().unwrap().is_empty());
        assert_eq!(store.index(), "");
        store.save(&record(), true).unwrap();
        assert!(store.save(&record(), true).is_err());
        assert_eq!(store.for_chat("chat-1").unwrap().name, "splitter-app");
        assert!(store.index().starts_with("- splitter-app (in progress"));
        let mut done = record();
        done.done("Built.", &[], &[], "2026-11-04");
        store.save(&done, false).unwrap();
        assert_eq!(store.for_chat("chat-1").unwrap().status, PlanStatus::Done);
        assert!(store.index().contains("(done, 2026-11-04)"));
        store.delete("splitter-app").unwrap();
        assert_eq!(store.index(), "");
    }

    #[test]
    fn the_largest_record_the_limits_allow_still_saves_and_reads_back() {
        let wide = |n: usize| "é".repeat(n);
        let mut record =
            PlanRecord::new("big", &wide(500), &wide(9000), Some("chat-1"), "2026-11-03");
        for _ in 0..30 {
            record.change(&wide(900), &[], "2026-11-04");
        }
        let many = |count: usize, chars: usize| vec![wide(chars); count];
        record.done(&wide(5000), &many(20, 2000), &many(9, 900), "2026-11-05");
        assert_eq!(record.evidence[0].chars().count(), MAX_EVIDENCE_CHARS);
        record.validate().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = PlanStore::new(dir.path(), "project-abc").unwrap();
        store.save(&record, true).unwrap();
        assert_eq!(store.read("big").unwrap().unwrap(), record);
    }

    #[test]
    fn a_run_reads_the_index_and_the_record_it_is_about() {
        let index = "- splitter-app (in progress, 2026-11-03): Expense splitter web app\n- fps-game (done, 2026-11-02): FPS arena game\n";
        assert_eq!(
            relevant_name("add a clear button to the expense splitter", index).as_deref(),
            Some("splitter-app")
        );
        assert_eq!(relevant_name("hello there", index), None);
        assert_eq!(run_note("", None), "");
        let note = run_note(index, Some(&record()));
        assert!(note.contains("- fps-game (done, 2026-11-02): FPS arena game"));
        assert!(note.contains("## Plan\n1. People: add and remove."));
    }

    proptest! {
        // Untrusted file text: reading never panics, and what reads back saves to the same.
        #[test]
        fn any_file_text_is_read_or_refused(text in "(?s).{0,600}") {
            if let Ok(record) = parse(&text) {
                prop_assert_eq!(parse(&serialize(&record)).unwrap(), record);
            }
        }

        #[test]
        fn any_plan_and_changes_round_trip(plan in "(?s)[^\r]{1,400}", change in "[^\r\n]{0,80}", old in "[^\r]{0,30}", new in "[^\r]{0,30}") {
            let mut record = PlanRecord::new("p", "Title", &format!("x{plan}"), None, "2026-11-03");
            record.change(&change, &[(old, new)], "2026-11-04");
            record.done(&plan, std::slice::from_ref(&change), std::slice::from_ref(&change), "2026-11-05");
            let text = serialize(&record);
            prop_assert_eq!(parse(&text).unwrap(), record);
        }
    }
}
