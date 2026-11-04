//! Tasks the memory model keeps track of along the way, so a long chat or goal never loses what
//! is still to do, what is under way and what is done. Tasks of approved work live in that
//! work's plan record; the rest live in one project file, `plans/TASKS.md`. Old tasks stay:
//! done and cancelled ones keep their outcome and date (the oldest go only past a cap), and an
//! open task is never pushed out by a new one. Runs get open tasks in full and done ones as a
//! count; `search_memory` reads the rest. Everything here treats its text as untrusted.

use serde::Serialize;
use serde_json::Value;

use super::{read_bounded, strip_secrets, MemoryError, WRITING};

/// The project's file of tasks outside any plan record, in the plans folder.
pub const TASKS_FILE: &str = "TASKS.md";
/// Most open or doing tasks one list keeps; a new one past it is refused, never an old one
/// dropped.
pub const MAX_OPEN_TASKS: usize = 40;
/// Most done or cancelled tasks one list keeps; the oldest go first.
pub const MAX_CLOSED_TASKS: usize = 60;
/// Most characters of a task's title.
pub const MAX_TASK_TITLE: usize = 120;
/// Most characters of a task's note (what done means, the person's words, or the outcome).
pub const MAX_TASK_NOTE: usize = 300;
/// Most task ops one pass applies.
pub const MAX_TASK_OPS: usize = 12;
/// Most bytes of the tasks file.
const MAX_TASKS_BYTES: u64 = 48 * 1024;
/// Most done tasks a run is shown by title.
const DONE_SHOWN: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Open,
    Doing,
    Done,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Doing => "doing",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "open" => Some(Self::Open),
            "doing" => Some(Self::Doing),
            "done" => Some(Self::Done),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    fn closed(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Task {
    pub title: String,
    pub status: TaskStatus,
    pub note: String,
    /// The day it last changed (`YYYY-MM-DD`).
    pub updated: String,
}

/// One line of text: no breaks or control characters, secrets hidden, within `max`
/// characters. The separators a task line uses become plain dashes, so a line reads back.
fn clean(text: &str, max: usize) -> String {
    crate::goal::one_line(&strip_secrets(text).replace(" — ", " - "), max)
}

/// `- [status] title — note (YYYY-MM-DD)`, the note left out when empty.
pub fn render(task: &Task) -> String {
    let note = if task.note.is_empty() {
        String::new()
    } else {
        format!(" — {}", task.note)
    };
    format!(
        "- [{}] {}{note} ({})",
        task.status.as_str(),
        task.title,
        task.updated
    )
}

/// One task line back, or `None` for anything else.
pub fn parse_line(line: &str) -> Option<Task> {
    let rest = line.trim_end().strip_prefix("- [")?;
    let (status, rest) = rest.split_once("] ")?;
    let status = TaskStatus::parse(status)?;
    let (rest, updated) = rest.strip_suffix(')')?.rsplit_once(" (")?;
    if updated.len() != 10 || !updated.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        return None;
    }
    let (title, note) = rest.split_once(" — ").unwrap_or((rest, ""));
    let title = clean(title, MAX_TASK_TITLE);
    (!title.is_empty()).then(|| Task {
        title,
        status,
        note: clean(note, MAX_TASK_NOTE),
        updated: updated.to_owned(),
    })
}

/// Every task line of `text`, in order; other lines are skipped.
pub fn parse_list(text: &str) -> Vec<Task> {
    let mut tasks: Vec<Task> = text.lines().filter_map(parse_line).collect();
    fit(&mut tasks);
    tasks
}

pub fn render_list(tasks: &[Task]) -> String {
    tasks
        .iter()
        .map(|task| format!("{}\n", render(task)))
        .collect()
}

/// Keeps a list within its caps: open tasks beyond the cap (only a hand-edited file has them)
/// and the oldest closed tasks past theirs go.
fn fit(tasks: &mut Vec<Task>) {
    let mut open = 0;
    tasks.retain(|task| {
        if task.status.closed() {
            return true;
        }
        open += 1;
        open <= MAX_OPEN_TASKS
    });
    let closed = tasks.iter().filter(|task| task.status.closed()).count();
    let mut extra = closed.saturating_sub(MAX_CLOSED_TASKS);
    tasks.retain(|task| {
        if extra > 0 && task.status.closed() {
            extra -= 1;
            return false;
        }
        true
    });
}

/// A change the memory model proposes. `work` names the plan record it belongs to, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskOp {
    Add {
        title: String,
        note: String,
        work: Option<String>,
    },
    Update {
        title: String,
        status: TaskStatus,
        note: String,
        work: Option<String>,
    },
}

impl TaskOp {
    pub fn work(&self) -> Option<&str> {
        match self {
            Self::Add { work, .. } | Self::Update { work, .. } => work.as_deref(),
        }
    }
}

/// The answer's `"tasks"` ops (untrusted): at most [`MAX_TASK_OPS`], each checked; returns the
/// ops and how many were refused (malformed, or holding a secret).
pub fn parse_ops(value: &Value) -> (Vec<TaskOp>, usize) {
    let Some(entries) = value.as_array() else {
        return (Vec::new(), 0);
    };
    let mut refused = entries.len().saturating_sub(MAX_TASK_OPS);
    let mut ops = Vec::new();
    for entry in entries.iter().take(MAX_TASK_OPS) {
        let text = |key: &str| entry[key].as_str().map(str::trim).unwrap_or_default();
        let raw = [text("title"), text("note")];
        if raw.iter().any(|part| strip_secrets(part) != *part) {
            refused += 1;
            continue;
        }
        let title = clean(text("title"), MAX_TASK_TITLE);
        let note = clean(text("note"), MAX_TASK_NOTE);
        let work = Some(text("work"))
            .filter(|work| super::valid_name(work))
            .map(str::to_owned);
        let op = match (text("op"), TaskStatus::parse(text("status"))) {
            _ if title.is_empty() => None,
            ("add", _) => Some(TaskOp::Add { title, note, work }),
            ("update", Some(status)) => Some(TaskOp::Update {
                title,
                status,
                note,
                work,
            }),
            _ => None,
        };
        match op {
            Some(op) => ops.push(op),
            None => refused += 1,
        }
    }
    (ops, refused)
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Applies `op` to `tasks`: an add of a title already listed updates its note (and reopens it
/// when it was closed), an update needs the exact title. Returns whether anything changed.
pub fn apply(tasks: &mut Vec<Task>, op: &TaskOp, today: &str) -> bool {
    match op {
        TaskOp::Add { title, note, .. } => {
            let (title, note) = (&clean(title, MAX_TASK_TITLE), &clean(note, MAX_TASK_NOTE));
            if title.is_empty() {
                return false;
            }
            if let Some(task) = tasks.iter_mut().find(|task| same(&task.title, title)) {
                if task.status.closed() {
                    task.status = TaskStatus::Open;
                }
                if !note.is_empty() {
                    task.note = note.clone();
                }
                task.updated = today.to_owned();
                return true;
            }
            let open = tasks.iter().filter(|task| !task.status.closed()).count();
            if open >= MAX_OPEN_TASKS {
                return false;
            }
            tasks.push(Task {
                title: title.clone(),
                status: TaskStatus::Open,
                note: note.clone(),
                updated: today.to_owned(),
            });
        }
        TaskOp::Update {
            title,
            status,
            note,
            ..
        } => {
            let note = &clean(note, MAX_TASK_NOTE);
            let Some(task) = tasks.iter_mut().find(|task| same(&task.title, title)) else {
                return false;
            };
            task.status = *status;
            if !note.is_empty() {
                task.note = note.clone();
            }
            task.updated = today.to_owned();
        }
    }
    fit(tasks);
    true
}

/// What a run is shown of `tasks`: open and doing ones in full, then the done count with the
/// latest few titles. Empty when there are none.
pub fn run_view(tasks: &[Task]) -> String {
    let mut view: String = tasks
        .iter()
        .filter(|task| !task.status.closed())
        .map(|task| format!("{}\n", render(task)))
        .collect();
    let done: Vec<&Task> = tasks.iter().filter(|task| task.status.closed()).collect();
    if !done.is_empty() {
        let latest: Vec<&str> = done
            .iter()
            .rev()
            .take(DONE_SHOWN)
            .map(|task| task.title.as_str())
            .collect();
        view.push_str(&format!(
            "- {} done or cancelled (latest: {})\n",
            done.len(),
            latest.join("; ")
        ));
    }
    view
}

/// The project's tasks outside any plan record (`plans/TASKS.md`).
pub struct TaskFile {
    path: std::path::PathBuf,
}

impl TaskFile {
    /// `plans_dir` is the project's plans folder.
    pub fn new(plans_dir: &std::path::Path) -> Self {
        Self {
            path: plans_dir.join(TASKS_FILE),
        }
    }

    pub fn read(&self) -> Vec<Task> {
        if !std::fs::symlink_metadata(&self.path).is_ok_and(|meta| meta.file_type().is_file()) {
            return Vec::new();
        }
        read_bounded(&self.path, MAX_TASKS_BYTES)
            .map(|text| parse_list(&text))
            .unwrap_or_default()
    }

    pub fn write(&self, tasks: &[Task]) -> Result<(), MemoryError> {
        let _writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        if std::fs::symlink_metadata(&self.path).is_ok_and(|meta| !meta.file_type().is_file()) {
            return Err(MemoryError("the tasks file is not a regular file".into()));
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|_| MemoryError("the plans folder could not be made".into()))?;
        }
        crate::chat_grants::write_atomic(&self.path, &render_list(tasks))
            .map_err(|_| MemoryError("the tasks file could not be saved".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    fn add(title: &str, note: &str) -> TaskOp {
        TaskOp::Add {
            title: title.into(),
            note: note.into(),
            work: None,
        }
    }

    fn update(title: &str, status: TaskStatus, note: &str) -> TaskOp {
        TaskOp::Update {
            title: title.into(),
            status,
            note: note.into(),
            work: None,
        }
    }

    /// Old and new tasks stay together: a new one never pushes out an open one, a done one keeps
    /// its outcome and date, and a cancelled one says why.
    #[test]
    fn old_and_new_tasks_are_kept_with_their_status() {
        let mut tasks = Vec::new();
        assert!(apply(
            &mut tasks,
            &add(
                "Pause with P or Esc",
                "the person's words: \"P or Esc pauses\""
            ),
            "2026-11-03"
        ));
        assert!(apply(&mut tasks, &add("Touch controls", ""), "2026-11-03"));
        assert!(apply(
            &mut tasks,
            &add("Aliens that shoot", ""),
            "2026-11-04"
        ));
        assert!(apply(
            &mut tasks,
            &update(
                "pause with p or esc",
                TaskStatus::Done,
                "checked in the browser: the clock stops"
            ),
            "2026-11-04"
        ));
        assert!(apply(
            &mut tasks,
            &update(
                "Touch controls",
                TaskStatus::Cancelled,
                "the person dropped it"
            ),
            "2026-11-04"
        ));
        assert!(!apply(
            &mut tasks,
            &update("No such task", TaskStatus::Done, ""),
            "2026-11-04"
        ));
        assert_eq!(tasks.len(), 3);
        assert_eq!(
            render(&tasks[0]),
            "- [done] Pause with P or Esc — checked in the browser: the clock stops (2026-11-04)"
        );
        let view = run_view(&tasks);
        assert!(
            view.starts_with("- [open] Aliens that shoot (2026-11-04)\n"),
            "{view}"
        );
        assert!(
            view.ends_with("- 2 done or cancelled (latest: Touch controls; Pause with P or Esc)\n"),
            "{view}"
        );
        assert_eq!(parse_list(&render_list(&tasks)), tasks);
        // Adding a closed one again reopens it.
        apply(
            &mut tasks,
            &add("Touch controls", "asked again"),
            "2026-11-05",
        );
        assert_eq!(tasks[1].status, TaskStatus::Open);
    }

    #[test]
    fn caps_never_drop_an_open_task_for_a_new_one() {
        let mut tasks = Vec::new();
        for n in 0..MAX_OPEN_TASKS {
            assert!(apply(
                &mut tasks,
                &add(&format!("task {n}"), ""),
                "2026-11-04"
            ));
        }
        assert!(!apply(&mut tasks, &add("one too many", ""), "2026-11-04"));
        assert_eq!(tasks.len(), MAX_OPEN_TASKS);
        assert_eq!(tasks[0].title, "task 0");
        // Closed ones past their cap go oldest first.
        for n in 0..MAX_OPEN_TASKS {
            apply(
                &mut tasks,
                &update(&format!("task {n}"), TaskStatus::Done, ""),
                "2026-11-04",
            );
        }
        for n in 0..MAX_CLOSED_TASKS {
            apply(&mut tasks, &add(&format!("later {n}"), ""), "2026-11-05");
            apply(
                &mut tasks,
                &update(&format!("later {n}"), TaskStatus::Done, ""),
                "2026-11-05",
            );
        }
        assert_eq!(tasks.len(), MAX_CLOSED_TASKS);
        assert_eq!(tasks[0].title, "later 0");
    }

    #[test]
    fn ops_are_checked_and_secrets_refused() {
        let (ops, refused) = parse_ops(&json!([
            {"op":"add","title":"Sound on hit","note":"beep","work":"dodger-asteroid-dodger"},
            {"op":"update","title":"Sound on hit","status":"done","note":"heard by the person"},
            {"op":"update","title":"x","status":"finished"},
            {"op":"add","title":""},
            {"op":"add","title":"Use key sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789"},
            "not an object"
        ]));
        assert_eq!(ops.len(), 2, "{ops:?}");
        assert_eq!(ops[0].work(), Some("dodger-asteroid-dodger"));
        assert_eq!(refused, 4);
        assert_eq!(parse_ops(&json!(null)), (Vec::new(), 0));
    }

    #[test]
    fn a_tasks_file_round_trips_and_skips_other_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file = TaskFile::new(&dir.path().join("plans"));
        assert!(file.read().is_empty());
        let mut tasks = Vec::new();
        apply(
            &mut tasks,
            &add("Write the README", "with run steps"),
            "2026-11-04",
        );
        file.write(&tasks).unwrap();
        assert_eq!(file.read(), tasks);
        std::fs::write(
            dir.path().join("plans").join(TASKS_FILE),
            "# notes\n- [open] Kept (2026-11-04)\nnoise\n- [bad] x (2026-11-04)\n",
        )
        .unwrap();
        assert_eq!(file.read().len(), 1);
    }

    proptest! {
        /// Untrusted lines never panic, and what parses renders back to itself.
        #[test]
        fn any_line_parses_safely(line in ".{0,400}") {
            if let Some(task) = parse_line(&line) {
                prop_assert_eq!(parse_line(&render(&task)), Some(task));
            }
        }

        #[test]
        fn any_task_round_trips(title in "[^\\n]{1,80}", note in "[^\\n]{0,120}") {
            let mut tasks = Vec::new();
            if apply(&mut tasks, &add(&title, &note), "2026-11-04") {
                prop_assert_eq!(parse_list(&render_list(&tasks)), tasks);
            }
        }
    }
}
