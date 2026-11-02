//! The agent's to-do list: what `todo_write` accepts, how a write merges into the list the
//! runtime keeps, and the bounded text the model and the goal check read back.
//!
//! The main agent keeps one list per chat, outside the chat history, so compaction can't lose it;
//! a subagent keeps its own for the length of its run. The agent rewrites the whole list on each
//! call. Items keep an `id` the runtime assigns, so a rewrite can keep an item's details without
//! repeating them, and a request the person made keeps their own words whatever the agent writes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Most items a write may hold, unless the list already holds more (the person's requests are
/// never dropped to make room).
pub const MAX_ITEMS: usize = 50;
pub const MAX_TITLE_CHARS: usize = 120;
/// A request's title: its first words.
const REQUEST_TITLE_CHARS: usize = 100;
/// Longest details the agent may write.
pub const MAX_DETAILS_CHARS: usize = 500;
/// Longest request of the person's kept word for word as an item's details.
pub const MAX_REQUEST_CHARS: usize = 2000;
pub const MAX_OUTCOME_CHARS: usize = 120;
/// Longest reason the check gives for reopening an item.
pub const MAX_REOPEN_CHARS: usize = 240;
/// Longest note the model reads back at the start of a turn and after a checkpoint, in characters.
pub const MAX_NOTE_CHARS: usize = 10_000;
/// Longest list the goal check reads, in characters.
pub const MAX_CHECK_CHARS: usize = 20_000;
/// Finished items the turn note names; older ones are counted.
const NOTE_DONE_SHOWN: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }
    fn parse(text: &str) -> Option<Self> {
        match text {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

/// Who put an item on the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoSource {
    #[default]
    Agent,
    /// A request the person sent while a goal ran; its details are their words.
    Person,
    /// Added by the goal check because the goal or a request asked for it.
    Check,
}

impl TodoSource {
    fn is_agent(&self) -> bool {
        *self == Self::Agent
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub title: String,
    /// What "done" means and the specifics; for the person's request, their words.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub details: String,
    #[serde(default)]
    pub status: TodoStatus,
    /// One line on how it ended, once completed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub outcome: String,
    #[serde(default, skip_serializing_if = "TodoSource::is_agent")]
    pub source: TodoSource,
    /// Why the goal check reopened it, until the agent completes it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reopened: Option<String>,
    /// A goal check saw it completed and let it stand.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub accepted: bool,
    /// For a request of the person's: the list's revision when it arrived. A write from an
    /// agent that hadn't seen that revision yet can't take it off by leaving it out.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub since: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl TodoItem {
    pub fn is_open(&self) -> bool {
        self.status != TodoStatus::Completed
    }
}

/// A chat's list as the runtime keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TodoList {
    /// Bumped on every change, by the change.
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub items: Vec<TodoItem>,
    /// The next id to hand out.
    #[serde(default)]
    pub next_id: u64,
    /// The person's requests the agent took off the list before finishing them, in their words,
    /// until a goal check has judged the removal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_requests: Vec<String>,
}

/// One item of a `todo_write` call, validated and bounded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenItem {
    pub id: Option<String>,
    pub title: String,
    /// `None` keeps the details an item with this id already has.
    pub details: Option<String>,
    pub status: TodoStatus,
    pub outcome: String,
}

/// What a write changed, for diagnostics (counts only) and hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WriteSummary {
    pub items: usize,
    pub open: usize,
    pub completed: usize,
    /// The person's unfinished requests the agent took off the list in this write.
    pub requests_removed: usize,
    /// Removed requests forgotten because too many were waiting for a check.
    pub removals_forgotten: usize,
    /// The person's requests the write left out because the agent hadn't seen them yet.
    pub unseen_kept: usize,
}

/// Collapses whitespace and control characters and keeps at most `max` characters.
fn one_line(text: &str, max: usize) -> String {
    crate::goal::one_line(text, max)
}

/// Keeps line breaks (details are prose) but drops other control characters and bounds it.
fn bounded_text(text: &str, max: usize) -> String {
    text.trim()
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .take(max)
        .collect()
}

/// Reads a `todo_write` call: `{"todos":[{"id"?, "title", "details"?, "status", "outcome"?}]}`.
/// `limit` is the most items it may hold. Anything malformed is an error the agent sees.
pub fn parse_write(args: &Value, limit: usize) -> Result<Vec<WrittenItem>, String> {
    let items = args["todos"]
        .as_array()
        .ok_or("todo_write needs `todos`: the whole list, each item with a title and a status")?;
    if items.len() > limit {
        return Err(format!("todo_write keeps at most {limit} items"));
    }
    items
        .iter()
        .map(|item| {
            let title = one_line(item["title"].as_str().unwrap_or_default(), MAX_TITLE_CHARS);
            if title.is_empty() {
                return Err("each item needs a `title`".to_owned());
            }
            let status = item["status"].as_str().unwrap_or("pending");
            let status = TodoStatus::parse(status)
                .ok_or_else(|| format!("unknown status '{}'", one_line(status, 40)))?;
            Ok(WrittenItem {
                id: item["id"]
                    .as_str()
                    .map(|id| one_line(id, 40))
                    .filter(|id| !id.is_empty()),
                title,
                details: item["details"]
                    .as_str()
                    .map(|details| bounded_text(details, MAX_DETAILS_CHARS)),
                status,
                outcome: one_line(
                    item["outcome"].as_str().unwrap_or_default(),
                    MAX_OUTCOME_CHARS,
                ),
            })
        })
        .collect()
}

impl TodoList {
    /// The most items the next write may hold.
    pub fn write_limit(&self) -> usize {
        MAX_ITEMS.max(self.items.len())
    }

    fn fresh_id(&mut self) -> String {
        self.next_id = self.next_id.max(1);
        let id = format!("t{}", self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    fn bump(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    /// Replaces the list with what the agent wrote, having last seen the list at revision
    /// `seen`. An item sent with a known id keeps its source, the person's words and (when the
    /// write leaves them out) its details; any other is new. A request of the person's that
    /// arrived after `seen` stays, at the end; one taken off unfinished is remembered for the
    /// goal check.
    pub fn apply_write(&mut self, written: Vec<WrittenItem>, seen: u64) -> WriteSummary {
        self.bump();
        let mut old = std::mem::take(&mut self.items);
        let mut items = Vec::with_capacity(written.len());
        for item in written {
            let known = item
                .id
                .as_ref()
                .and_then(|id| old.iter().position(|known| &known.id == id))
                .map(|index| old.remove(index));
            let mut next = match known {
                Some(known) => known,
                None => TodoItem {
                    id: self.fresh_id(),
                    ..Default::default()
                },
            };
            next.title = item.title;
            if next.source != TodoSource::Person {
                if let Some(details) = item.details {
                    next.details = details;
                }
            }
            if next.status != item.status {
                next.accepted = false;
            }
            next.status = item.status;
            next.outcome = if item.status == TodoStatus::Completed {
                item.outcome
            } else {
                String::new()
            };
            if item.status == TodoStatus::Completed {
                next.reopened = None;
            }
            items.push(next);
        }
        let mut summary = WriteSummary::default();
        for gone in old {
            if gone.source == TodoSource::Person && gone.since > seen {
                summary.unseen_kept += 1;
                items.push(gone);
            } else if gone.source == TodoSource::Person && gone.is_open() {
                summary.requests_removed += 1;
                self.removed_requests.push(gone.details);
            }
        }
        let excess = self.removed_requests.len().saturating_sub(MAX_ITEMS);
        self.removed_requests.drain(..excess);
        summary.removals_forgotten = excess;
        self.items = items;
        summary.items = self.items.len();
        summary.open = self.items.iter().filter(|item| item.is_open()).count();
        summary.completed = summary.items - summary.open;
        summary
    }

    /// Adds a request the person sent while a goal ran, in their words. Sending the same words
    /// again adds nothing. Nothing of theirs is ever dropped: a full list makes room by letting
    /// go of its oldest finished item of the agent's, and grows past the limit when there is none.
    pub fn add_request(&mut self, words: &str) -> bool {
        let details = bounded_text(words, MAX_REQUEST_CHARS);
        if details.is_empty()
            || self
                .items
                .iter()
                .any(|item| item.source == TodoSource::Person && item.details == details)
        {
            return false;
        }
        if self.items.len() >= MAX_ITEMS {
            if let Some(index) = self
                .items
                .iter()
                .position(|item| !item.is_open() && item.source != TodoSource::Person)
            {
                self.items.remove(index);
            }
        }
        self.bump();
        let id = self.fresh_id();
        self.items.push(TodoItem {
            id,
            title: one_line(&details, REQUEST_TITLE_CHARS),
            details,
            source: TodoSource::Person,
            since: self.revision,
            ..Default::default()
        });
        true
    }

    /// The goal check reopens an item it doesn't accept; false when there is no such item.
    pub fn reopen(&mut self, id: &str, reason: &str) -> bool {
        let Some(index) = self.items.iter().position(|item| item.id == id) else {
            return false;
        };
        self.bump();
        let item = &mut self.items[index];
        item.status = TodoStatus::Pending;
        item.accepted = false;
        item.outcome.clear();
        item.reopened = Some(one_line(reason, MAX_REOPEN_CHARS)).filter(|r| !r.is_empty());
        if item.reopened.is_none() {
            item.reopened = Some("not shown done".to_owned());
        }
        true
    }

    /// The goal check adds what the goal or a request asks for and the list lacks.
    pub fn add_from_check(&mut self, title: &str, details: &str) {
        let title = one_line(title, MAX_TITLE_CHARS);
        if title.is_empty()
            || self
                .items
                .iter()
                .any(|item| item.is_open() && item.title.eq_ignore_ascii_case(&title))
        {
            return;
        }
        self.bump();
        let id = self.fresh_id();
        self.items.push(TodoItem {
            id,
            title,
            details: bounded_text(details, MAX_DETAILS_CHARS),
            source: TodoSource::Check,
            ..Default::default()
        });
    }

    /// Completed items a check let stand count as accepted from then on.
    pub fn accept_completed(&mut self) {
        if self
            .items
            .iter()
            .any(|item| !item.is_open() && !item.accepted)
        {
            self.bump();
        }
        for item in &mut self.items {
            if item.status == TodoStatus::Completed {
                item.accepted = true;
            }
        }
    }

    /// Forgets the first `count` removed requests once a goal check has judged them.
    pub fn judged_removals(&mut self, count: usize) {
        let count = count.min(self.removed_requests.len());
        if count > 0 {
            self.bump();
            self.removed_requests.drain(..count);
        }
    }

    pub fn open(&self) -> impl Iterator<Item = &TodoItem> {
        self.items.iter().filter(|item| item.is_open())
    }

    /// The short note the model reads at the start of a turn and after a checkpoint; `None`
    /// for an empty list. Open items come whole, the latest finished ones by title, the rest
    /// counted; past [`MAX_NOTE_CHARS`] the details are shortened, never the titles.
    pub fn note(&self) -> Option<String> {
        if self.items.is_empty() {
            return None;
        }
        let done: Vec<&TodoItem> = self.items.iter().filter(|item| !item.is_open()).collect();
        let shown_done = done.len().saturating_sub(NOTE_DONE_SHOWN);
        let mut lines = Vec::new();
        let mut done_seen = 0;
        for item in &self.items {
            if !item.is_open() {
                done_seen += 1;
                if done_seen <= shown_done {
                    continue;
                }
            }
            lines.push(Line {
                head: head(item, false),
                details: if item.is_open() {
                    item.details.clone()
                } else {
                    String::new()
                },
            });
        }
        let mut text = render(lines, MAX_NOTE_CHARS);
        if shown_done > 0 {
            text.push_str(&format!("\n({shown_done} earlier items done)"));
        }
        Some(crate::prompts::fill(
            crate::prompts::RUNTIME_TODO_NOTE,
            &[("items", &text)],
        ))
    }

    /// The list as the goal check reads it: every item with its details, bounded to
    /// [`MAX_CHECK_CHARS`] by shortening details (finished items' first).
    pub fn for_check(&self) -> String {
        if self.items.is_empty() {
            return "(empty)".to_owned();
        }
        let lines = self
            .items
            .iter()
            .map(|item| Line {
                head: head(item, true),
                details: item.details.clone(),
            })
            .collect();
        render(lines, MAX_CHECK_CHARS)
    }
}

struct Line {
    head: String,
    details: String,
}

fn head(item: &TodoItem, for_check: bool) -> String {
    let mut tags = vec![match item.status {
        TodoStatus::Pending => "pending",
        TodoStatus::InProgress => "in progress",
        TodoStatus::Completed if for_check && item.accepted => "done, accepted by an earlier check",
        TodoStatus::Completed => "done",
    }
    .to_owned()];
    match item.source {
        TodoSource::Person => tags.push("the person asked".to_owned()),
        TodoSource::Check => tags.push("added by the check".to_owned()),
        TodoSource::Agent => {}
    }
    if let Some(reason) = &item.reopened {
        tags.push(format!("reopened by the check: {reason}"));
    }
    let mut line = format!("- {} [{}] {}", item.id, tags.join("; "), item.title);
    if !item.outcome.is_empty() {
        line.push_str(&format!(" — outcome: {}", item.outcome));
    }
    line
}

/// The lines with their details, shortened evenly when they don't all fit in `max` characters.
fn render(lines: Vec<Line>, max: usize) -> String {
    let heads: usize = lines.iter().map(|line| line.head.chars().count() + 1).sum();
    let wanted: Vec<usize> = lines
        .iter()
        .map(|line| match line.details.is_empty() {
            true => 0,
            // "  details: " and a line break.
            false => line.details.chars().count() + 12,
        })
        .collect();
    let mut room = max.saturating_sub(heads);
    let mut allowance = vec![0usize; lines.len()];
    // Water-fill: the shortest details fit whole, the rest share what is left evenly.
    let mut order: Vec<usize> = (0..lines.len()).filter(|&i| wanted[i] > 0).collect();
    order.sort_by_key(|&i| wanted[i]);
    let mut left = order.len();
    for i in order {
        let share = room / left.max(1);
        let given = wanted[i].min(share);
        allowance[i] = given;
        room -= given;
        left -= 1;
    }
    let mut text = String::new();
    for (index, line) in lines.into_iter().enumerate() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&line.head);
        let budget = allowance[index].saturating_sub(12);
        if budget > 0 {
            let count = line.details.chars().count();
            let details: String = if count <= budget {
                line.details.replace('\n', "\n    ")
            } else {
                let cut: String = line
                    .details
                    .chars()
                    .take(budget.saturating_sub(1))
                    .collect();
                format!("{}…", cut.replace('\n', "\n    "))
            };
            text.push_str("\n  details: ");
            text.push_str(&details);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    fn write(list: &mut TodoList, todos: Value) -> WriteSummary {
        let items = parse_write(&json!({ "todos": todos }), list.write_limit()).unwrap();
        list.apply_write(items, u64::MAX)
    }

    #[test]
    fn a_write_replaces_the_list_and_ids_keep_items() {
        let mut list = TodoList::default();
        write(
            &mut list,
            json!([{"title":"Read the docs","status":"in_progress","details":"AGENTS.md and docs/"},
                   {"title":"Build the board","status":"pending"}]),
        );
        assert_eq!(list.items[0].id, "t1");
        assert_eq!(list.items[1].id, "t2");
        write(
            &mut list,
            json!([{"id":"t1","title":"Read the docs","status":"completed","outcome":"two rules found"},
                   {"id":"t2","title":"Build the board","status":"in_progress"}]),
        );
        assert_eq!(list.items[0].details, "AGENTS.md and docs/", "details kept");
        assert_eq!(list.items[0].outcome, "two rules found");
        assert_eq!(list.items[1].status, TodoStatus::InProgress);
        let summary = write(
            &mut list,
            json!([{"title":"Something else","status":"pending"}]),
        );
        assert_eq!(summary.items, 1);
        assert_eq!(list.items[0].id, "t3", "a new item never reuses an id");
    }

    #[test]
    fn the_persons_words_stay_theirs_and_an_unfinished_request_taken_off_is_remembered() {
        let mut list = TodoList::default();
        assert!(list.add_request("Can you also add a CSV export?"));
        assert!(
            !list.add_request("Can you also add a CSV export?"),
            "counted once"
        );
        let id = list.items[0].id.clone();
        write(
            &mut list,
            json!([{"id": id, "title":"CSV export","details":"something else","status":"in_progress"}]),
        );
        assert_eq!(list.items[0].details, "Can you also add a CSV export?");
        assert_eq!(list.items[0].source, TodoSource::Person);
        let summary = write(&mut list, json!([]));
        assert_eq!(summary.requests_removed, 1);
        assert_eq!(list.removed_requests, ["Can you also add a CSV export?"]);
        list.judged_removals(5);
        assert!(list.removed_requests.is_empty());
    }

    #[test]
    fn a_request_the_agent_hadnt_seen_survives_its_next_write() {
        let mut list = TodoList::default();
        write(&mut list, json!([{"title":"Board","status":"in_progress"}]));
        let seen = list.revision;
        list.add_request("Also add dark mode");
        let items = parse_write(
            &json!({"todos":[{"id":"t1","title":"Board","status":"completed"}]}),
            50,
        )
        .unwrap();
        let summary = list.apply_write(items, seen);
        assert_eq!(summary.unseen_kept, 1);
        assert_eq!(summary.requests_removed, 0);
        assert_eq!(list.items[1].details, "Also add dark mode");
        assert!(list.revision > seen);
    }

    #[test]
    fn a_full_list_lets_go_of_a_finished_item_never_a_request() {
        let mut list = TodoList::default();
        let todos: Vec<Value> = (0..MAX_ITEMS)
            .map(|n| json!({"title": format!("step {n}"), "status": if n == 0 { "completed" } else { "pending" }}))
            .collect();
        write(&mut list, Value::Array(todos));
        list.add_request("first");
        assert_eq!(list.items.len(), MAX_ITEMS);
        assert!(!list.items.iter().any(|item| item.title == "step 0"));
        list.add_request("second");
        assert_eq!(
            list.items.len(),
            MAX_ITEMS + 1,
            "nothing finished to let go of"
        );
        assert_eq!(list.write_limit(), MAX_ITEMS + 1);
    }

    #[test]
    fn the_check_reopens_and_adds_and_completing_again_clears_the_reason() {
        let mut list = TodoList::default();
        write(
            &mut list,
            json!([{"title":"Verify in the browser","status":"completed"}]),
        );
        assert!(list.reopen("t1", "the import with a wrong file was never tried"));
        assert!(!list.reopen("t9", "x"));
        assert!(list.items[0].is_open());
        list.add_from_check("Add CSV export", "the person asked for it");
        list.add_from_check("add csv export", "");
        assert_eq!(list.items.len(), 2);
        assert_eq!(list.items[1].source, TodoSource::Check);
        write(
            &mut list,
            json!([{"id":"t1","title":"Verify in the browser","status":"completed"},{"id":"t2","title":"Add CSV export","status":"pending"}]),
        );
        assert_eq!(list.items[0].reopened, None);
        assert_eq!(list.items[1].source, TodoSource::Check);
    }

    #[test]
    fn bad_writes_are_refused_with_a_reason() {
        assert!(parse_write(&json!({}), 10).is_err());
        assert!(parse_write(&json!({"todos":[{"status":"pending"}]}), 10).is_err());
        assert!(parse_write(&json!({"todos":[{"title":"x","status":"done"}]}), 10).is_err());
        assert!(parse_write(&json!({"todos":[{"title":"x"},{"title":"y"}]}), 1).is_err());
    }

    #[test]
    fn the_note_shows_open_items_whole_and_folds_old_finished_ones() {
        let mut list = TodoList::default();
        assert_eq!(list.note(), None);
        let todos: Vec<Value> = (0..12)
            .map(|n| json!({"title": format!("step {n}"), "status": "completed"}))
            .chain([json!({"title":"Now","status":"in_progress","details":"what done means"})])
            .collect();
        write(&mut list, Value::Array(todos));
        let note = list.note().unwrap();
        assert!(
            note.starts_with("[To-do — runtime, not from the user]"),
            "{note}"
        );
        assert!(
            note.contains("[in progress] Now\n  details: what done means"),
            "{note}"
        );
        assert!(!note.contains("step 3"), "{note}");
        assert!(note.contains("step 11"), "{note}");
        assert!(note.contains("(4 earlier items done)"), "{note}");
    }

    #[test]
    fn long_lists_stay_within_the_bounds_and_keep_every_open_title() {
        let mut list = TodoList::default();
        for n in 0..MAX_ITEMS {
            list.add_request(&format!("request {n} {}", "words ".repeat(300)));
        }
        let note = list.note().unwrap();
        assert!(
            note.chars().count() <= MAX_NOTE_CHARS + 200,
            "{}",
            note.len()
        );
        let check = list.for_check();
        assert!(check.chars().count() <= MAX_CHECK_CHARS, "{}", check.len());
        for n in 0..MAX_ITEMS {
            assert!(check.contains(&format!("request {n} ")));
        }
    }

    proptest! {
        /// Tool arguments come from the model: any of them is refused or bounded.
        #[test]
        fn any_write_is_refused_or_bounded(raw in ".{0,400}", count in 0usize..8, title in ".{0,300}", details in ".{0,900}", status in "[a-z_]{0,12}") {
            let _ = parse_write(&serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null), 5);
            let item = json!({"title": title, "details": details, "status": status, "outcome": details, "id": title});
            let args = json!({"todos": vec![item; count]});
            if let Ok(items) = parse_write(&args, 5) {
                prop_assert!(items.len() <= 5);
                let mut list = TodoList::default();
                list.apply_write(items, u64::MAX);
                for item in &list.items {
                    prop_assert!(!item.title.is_empty() && item.title.chars().count() <= MAX_TITLE_CHARS);
                    prop_assert!(item.details.chars().count() <= MAX_DETAILS_CHARS);
                    prop_assert!(item.outcome.chars().count() <= MAX_OUTCOME_CHARS);
                }
                if let Some(note) = list.note() {
                    prop_assert!(note.chars().count() <= MAX_NOTE_CHARS + 200);
                }
            }
        }
    }
}
