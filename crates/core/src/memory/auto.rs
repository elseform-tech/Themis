//! The memory jobs' requests and answers: the pass after a turn in which the person wrote (one
//! request on the memory models reads that turn and the memory indexes and proposes at most a
//! few saves, updates or deletes) and the idle consolidation of one scope (merge duplicates,
//! drop stale memories). Everything here is pure: building the bounded requests, reading the
//! model's answer as untrusted input, and planning which proposals to apply against what is
//! remembered (scope rules, secrets, duplicates). The desktop applies the plan through
//! [`super::MemoryStore`].

use serde::Serialize;
use serde_json::Value;

use super::{strip_secrets, valid_name, Memory, MemoryKind, MemoryScope, SaveMode};

/// Most bytes of the person's messages one pass reads; past it the middle is cut.
pub const MAX_PERSON_BYTES: usize = 8 * 1024;
/// Most bytes of the agent's final reply one pass reads; past it the middle is cut.
pub const MAX_REPLY_BYTES: usize = 4 * 1024;
/// Most bytes of each index one pass reads (an index is already capped at this).
pub const MAX_INDEX_BYTES: usize = super::MAX_INDEX_BYTES;
/// Most bytes of the model's answer that are read; a longer one is refused.
pub const MAX_ANSWER_BYTES: usize = 16 * 1024;
/// Most proposals one pass applies; the rest are counted and dropped.
pub const MAX_CHANGES: usize = 3;
/// Most proposals one consolidation applies.
pub const MAX_CONSOLIDATE_CHANGES: usize = 10;
/// Most bytes of one memory's text a consolidation request carries.
pub const MAX_CONSOLIDATE_BODY_BYTES: usize = 1024;
/// Most bytes of memories one consolidation request carries; the oldest past it are left out.
pub const MAX_CONSOLIDATE_BYTES: usize = 48 * 1024;
/// Most `{` positions tried when looking for the answer's JSON object.
const MAX_JSON_STARTS: usize = 64;

/// `text` within `max` bytes: whole when it fits, else its start and end around a cut mark,
/// split on character boundaries.
pub fn bounded(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    const MARK: &str = "\n[…]\n";
    let room = max.saturating_sub(MARK.len());
    let mut head = room / 2;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - (room - head);
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}{MARK}{}", &text[..head], &text[tail..])
}

/// Breaks up the fence marks the request uses, so quoted text can't close its fence.
fn fenced(text: &str) -> String {
    text.replace("<<<", "< < <").replace(">>>", "> > >")
}

fn index_text(index: &str) -> String {
    let index = bounded(index.trim_end(), MAX_INDEX_BYTES);
    if index.is_empty() {
        "(none)".to_owned()
    } else {
        index
    }
}

/// The pass's system and user messages: the person's messages and the final reply (each
/// bounded), and both indexes. Its size is bounded whatever the turn or memory count.
pub fn request(
    person: &[String],
    reply: &str,
    global_index: &str,
    project_index: &str,
) -> (String, String) {
    let person = person
        .iter()
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n---\n");
    let user = crate::prompts::fill(
        crate::prompts::MEMORY_AUTO_USER,
        &[
            ("global", &index_text(global_index)),
            ("project", &index_text(project_index)),
            ("person", &fenced(&bounded(&person, MAX_PERSON_BYTES))),
            ("reply", &fenced(&bounded(reply.trim(), MAX_REPLY_BYTES))),
        ],
    );
    (crate::prompts::MEMORY_AUTO_SYSTEM.to_owned(), user)
}

/// The most bytes [`request`]'s user message can have.
pub fn max_request_bytes() -> usize {
    crate::prompts::MEMORY_AUTO_USER.len()
        + 2 * MAX_INDEX_BYTES
        + MAX_PERSON_BYTES
        + MAX_REPLY_BYTES
}

/// One change the model proposed, validated, with secrets refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proposal {
    /// Save a new memory or update one by name.
    Save {
        mode: SaveMode,
        scope: MemoryScope,
        memory: Memory,
    },
    /// Remove a memory that no longer holds.
    Delete { scope: MemoryScope, name: String },
}

impl Proposal {
    pub fn scope(&self) -> MemoryScope {
        match self {
            Self::Save { scope, .. } | Self::Delete { scope, .. } => *scope,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Save { memory, .. } => &memory.name,
            Self::Delete { name, .. } => name,
        }
    }
}

/// What a job did with what the model proposed. Counts only, for diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PassCounts {
    /// Entries the answer held.
    pub proposed: usize,
    pub saved: usize,
    pub updated: usize,
    pub deleted: usize,
    /// Already remembered (same name and text, or the same description elsewhere).
    pub duplicate: usize,
    /// A delete of a memory that isn't there.
    pub missing: usize,
    /// Held something that looks like a secret.
    pub sensitive: usize,
    /// A project fact or pointer proposed for every project, or (consolidating) another scope.
    pub scope: usize,
    /// Malformed: unknown action or type, bad name, missing text.
    pub invalid: usize,
    /// Past the job's limit.
    pub over_limit: usize,
    /// The store refused it (full, unreadable).
    pub failed: usize,
}

impl PassCounts {
    /// Whether the job changed memory.
    pub fn changed(&self) -> bool {
        self.saved + self.updated + self.deleted > 0
    }
}

/// Why an answer could not be read at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerError {
    TooLong,
    NoJson,
}

impl AnswerError {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TooLong => "too_long",
            Self::NoJson => "unreadable",
        }
    }
}

/// The first JSON object in `text` that has `key`, among the first few `{`. Untrusted input.
pub fn json_object_with(text: &str, key: &str) -> Option<serde_json::Map<String, Value>> {
    for (start, _) in text.match_indices('{').take(MAX_JSON_STARTS) {
        let mut values = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
        if let Some(Ok(Value::Object(object))) = values.next() {
            if object.contains_key(key) {
                return Some(object);
            }
        }
    }
    None
}

/// Plain text of an answer field: control characters removed, trimmed.
fn field(entry: &Value, key: &str) -> Option<String> {
    let text: String = entry[key]
        .as_str()?
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// Whether hiding secrets would change `text`: it holds one.
fn holds_secret(text: &str) -> bool {
    strip_secrets(text) != text
}

/// Reads the model's answer: `{"memories":[{action, scope?, name, type, description, body,
/// why?, how_to_apply?, source?}]}` (a delete needs only `name` and `scope`), at most `limit`
/// entries. Malformed entries, secrets and project facts proposed for every project are counted
/// and dropped, never guessed at.
pub fn parse_answer(
    text: &str,
    today: &str,
    limit: usize,
) -> Result<(Vec<Proposal>, PassCounts), AnswerError> {
    if text.len() > MAX_ANSWER_BYTES {
        return Err(AnswerError::TooLong);
    }
    let object = json_object_with(text, "memories").ok_or(AnswerError::NoJson)?;
    let entries = object["memories"].as_array().ok_or(AnswerError::NoJson)?;
    let mut counts = PassCounts {
        proposed: entries.len(),
        over_limit: entries.len().saturating_sub(limit),
        ..PassCounts::default()
    };
    let mut proposals = Vec::new();
    for entry in entries.iter().take(limit) {
        match proposal(entry, today) {
            Ok(proposal) => proposals.push(proposal),
            Err(Refused::Secret) => counts.sensitive += 1,
            Err(Refused::Scope) => counts.scope += 1,
            Err(Refused::Invalid) => counts.invalid += 1,
        }
    }
    Ok((proposals, counts))
}

enum Refused {
    Secret,
    Scope,
    Invalid,
}

fn proposal(entry: &Value, today: &str) -> Result<Proposal, Refused> {
    let mode = match entry["action"].as_str() {
        Some("save") => SaveMode::Create,
        Some("update") => SaveMode::Update,
        Some("delete") => SaveMode::Update,
        _ => return Err(Refused::Invalid),
    };
    let scope = match entry["scope"].as_str() {
        None | Some("") => MemoryScope::Project,
        Some(text) => MemoryScope::parse(text).ok_or(Refused::Invalid)?,
    };
    let name = entry["name"]
        .as_str()
        .map(str::trim)
        .filter(|name| valid_name(name))
        .ok_or(Refused::Invalid)?
        .to_owned();
    if entry["action"] == "delete" {
        return Ok(Proposal::Delete { scope, name });
    }
    let kind = entry["type"]
        .as_str()
        .and_then(MemoryKind::parse)
        .ok_or(Refused::Invalid)?;
    let description = field(entry, "description")
        .map(|text| crate::goal::one_line(&text, super::MAX_DESCRIPTION_CHARS))
        .filter(|text| !text.is_empty())
        .ok_or(Refused::Invalid)?;
    let fact = field(entry, "body").ok_or(Refused::Invalid)?;
    let why = field(entry, "why");
    let how = field(entry, "how_to_apply");
    let source = field(entry, "source")
        .map(|text| crate::goal::one_line(&text, super::MAX_SOURCE_CHARS))
        .filter(|text| !text.is_empty());
    if [
        Some(&description),
        Some(&fact),
        why.as_ref(),
        how.as_ref(),
        source.as_ref(),
    ]
    .into_iter()
    .flatten()
    .any(|text| holds_secret(text))
    {
        return Err(Refused::Secret);
    }
    if scope == MemoryScope::Global && !kind.may_be_global() {
        return Err(Refused::Scope);
    }
    let body = super::compose_body(&fact, why.as_deref(), how.as_deref());
    let memory = Memory {
        name,
        description,
        kind,
        saved: today.to_owned(),
        source,
        body,
    };
    memory.validate().map_err(|_| Refused::Invalid)?;
    Ok(Proposal::Save {
        mode,
        scope,
        memory,
    })
}

/// The name and description of an index line (`- name (type, date…): description`).
pub fn index_entry(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("- ")?;
    let (name, rest) = rest.split_once(" (")?;
    let (_, description) = rest.split_once("): ")?;
    Some((name, description.trim_end()))
}

/// A memory already remembered, as a job sees it: from an index line (no text) or a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    pub scope: MemoryScope,
    pub name: String,
    pub description: String,
    /// The memory's text when the job read it (consolidation).
    pub body: Option<String>,
}

/// What `index` (one scope's index file) says is remembered.
pub fn known_from_index(scope: MemoryScope, index: &str) -> Vec<Known> {
    index
        .lines()
        .filter_map(index_entry)
        .map(|(name, description)| Known {
            scope,
            name: name.to_owned(),
            description: description.to_owned(),
            body: None,
        })
        .collect()
}

/// What `memories` (one scope's listing) hold.
pub fn known_from_listing(scope: MemoryScope, memories: &[Memory]) -> Vec<Known> {
    memories
        .iter()
        .map(|memory| Known {
            scope,
            name: memory.name.clone(),
            description: memory.description.clone(),
            body: Some(memory.body.clone()),
        })
        .collect()
}

fn same_text(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// What to apply, against what is remembered: a delete of a memory that isn't there is
/// dropped; a save of a name the scope already has becomes an update and an update of a name
/// it lacks a save; anything already remembered (same name, description and known text, or the
/// same description under a name not being deleted) or proposed twice is a duplicate.
pub fn plan(proposals: Vec<Proposal>, known: &[Known], counts: &mut PassCounts) -> Vec<Proposal> {
    let exists = |scope: MemoryScope, name: &str| {
        known
            .iter()
            .find(|entry| entry.scope == scope && entry.name == name)
    };
    let mut planned: Vec<Proposal> = Vec::new();
    // Deletes first: what they remove no longer counts as remembered elsewhere.
    let (deletes, saves): (Vec<_>, Vec<_>) = proposals
        .into_iter()
        .partition(|proposal| matches!(proposal, Proposal::Delete { .. }));
    for proposal in deletes {
        let twice = planned.iter().any(|earlier| {
            earlier.scope() == proposal.scope() && earlier.name() == proposal.name()
        });
        if twice {
            counts.duplicate += 1;
        } else if exists(proposal.scope(), proposal.name()).is_none() {
            counts.missing += 1;
        } else {
            planned.push(proposal);
        }
    }
    let deleted = |entry: &Known| {
        planned
            .iter()
            .any(|done| done.scope() == entry.scope && done.name() == entry.name)
    };
    let mut kept: Vec<Proposal> = Vec::new();
    for proposal in saves {
        let Proposal::Save {
            scope, mut memory, ..
        } = proposal
        else {
            continue;
        };
        let existing = exists(scope, &memory.name);
        let described_elsewhere = known.iter().any(|entry| {
            !(entry.scope == scope && entry.name == memory.name)
                && !deleted(entry)
                && same_text(&entry.description, &memory.description)
        });
        let unchanged = existing.is_some_and(|entry| {
            same_text(&entry.description, &memory.description)
                && entry
                    .body
                    .as_ref()
                    .is_none_or(|body| body.trim() == memory.body.trim())
        });
        let twice = kept
            .iter()
            .chain(planned.iter())
            .any(|earlier| earlier.scope() == scope && earlier.name() == memory.name);
        if unchanged || described_elsewhere || twice {
            counts.duplicate += 1;
            continue;
        }
        memory.source = memory.source.filter(|source| !source.is_empty());
        kept.push(Proposal::Save {
            mode: if existing.is_some() {
                SaveMode::Update
            } else {
                SaveMode::Create
            },
            scope,
            memory,
        });
    }
    planned.extend(kept);
    planned
}

/// The consolidation's system and user messages for one scope: every memory with its type,
/// date, source and text (each text at most [`MAX_CONSOLIDATE_BODY_BYTES`], the whole at most
/// [`MAX_CONSOLIDATE_BYTES`], newest first). Bounded whatever the memory count.
pub fn consolidation_request(scope: MemoryScope, memories: &[Memory]) -> (String, String) {
    let mut listed = String::new();
    for memory in memories {
        let entry = format!(
            "{}\n{}\n\n",
            memory.index_line(),
            bounded(memory.body.trim(), MAX_CONSOLIDATE_BODY_BYTES)
        );
        if listed.len() + entry.len() > MAX_CONSOLIDATE_BYTES {
            break;
        }
        listed.push_str(&entry);
    }
    let scope = scope.as_str();
    let system = crate::prompts::fill(
        crate::prompts::MEMORY_CONSOLIDATE_SYSTEM,
        &[("scope", scope)],
    );
    let user = crate::prompts::fill(
        crate::prompts::MEMORY_CONSOLIDATE_USER,
        &[("scope", scope), ("memories", &fenced(listed.trim_end()))],
    );
    (system, user)
}

/// A consolidation's proposals for its own scope only; another scope's are counted.
pub fn only_scope(
    proposals: Vec<Proposal>,
    scope: MemoryScope,
    counts: &mut PassCounts,
) -> Vec<Proposal> {
    let (kept, other): (Vec<_>, Vec<_>) = proposals
        .into_iter()
        .partition(|proposal| proposal.scope() == scope);
    counts.scope += other.len();
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const TODAY: &str = "2026-11-02";

    fn answer(entries: Value) -> String {
        serde_json::json!({ "memories": entries }).to_string()
    }

    fn parse(text: &str) -> Result<(Vec<Proposal>, PassCounts), AnswerError> {
        parse_answer(text, TODAY, MAX_CHANGES)
    }

    /// The saved memory of a save or update proposal.
    fn saved(proposal: &Proposal) -> (SaveMode, MemoryScope, &Memory) {
        match proposal {
            Proposal::Save {
                mode,
                scope,
                memory,
            } => (*mode, *scope, memory),
            Proposal::Delete { .. } => panic!("a delete: {proposal:?}"),
        }
    }

    fn known(global: &str, project: &str) -> Vec<Known> {
        let mut known = known_from_index(MemoryScope::Global, global);
        known.extend(known_from_index(MemoryScope::Project, project));
        known
    }

    fn preference() -> Value {
        serde_json::json!({"action":"save","scope":"global","name":"uses-pnpm","type":"user",
            "description":"Uses pnpm, not npm","body":"The person uses pnpm for JavaScript projects."})
    }

    #[test]
    fn a_preference_is_read_with_its_scope_and_body() {
        let (proposals, counts) = parse(&format!(
            "Sure:\n```json\n{}\n```",
            answer(serde_json::json!([preference()]))
        ))
        .unwrap();
        assert_eq!(counts.proposed, 1);
        assert_eq!(proposals.len(), 1);
        let (mode, scope, memory) = saved(&proposals[0]);
        assert_eq!(scope, MemoryScope::Global);
        assert_eq!(mode, SaveMode::Create);
        assert_eq!(memory.kind, MemoryKind::User);
        assert_eq!(memory.saved, TODAY);
        assert_eq!(memory.body, "The person uses pnpm for JavaScript projects.");
        let mut with_why = preference();
        with_why["type"] = "feedback".into();
        with_why["why"] = "npm broke the lockfile".into();
        with_why["how_to_apply"] = "run pnpm install".into();
        let (proposals, _) = parse(&answer(serde_json::json!([with_why]))).unwrap();
        assert!(saved(&proposals[0])
            .2
            .body
            .ends_with("\n\nWhy: npm broke the lockfile\nHow to apply: run pnpm install"));
    }

    #[test]
    fn nothing_worth_remembering_is_an_empty_answer() {
        let (proposals, counts) = parse(r#"{"memories":[]}"#).unwrap();
        assert!(proposals.is_empty());
        assert_eq!(counts, PassCounts::default());
        assert_eq!(parse("I found nothing.").unwrap_err(), AnswerError::NoJson);
        assert_eq!(
            parse(r#"{"memories":"none"}"#).unwrap_err(),
            AnswerError::NoJson
        );
        assert_eq!(
            parse(&"x".repeat(MAX_ANSWER_BYTES + 1)).unwrap_err(),
            AnswerError::TooLong
        );
    }

    #[test]
    fn secrets_scope_breaks_and_malformed_entries_are_refused_and_counted() {
        let mut secret = preference();
        secret["body"] = "Deploy with token ghp_abcdefghijklmnopqrstuvwxyz0123456789AB".into();
        let mut password = preference();
        password["name"] = "db-login".into();
        password["why"] = "password=hunter2".into();
        let mut global_fact = preference();
        global_fact["type"] = "project".into();
        let mut bad_name = preference();
        bad_name["name"] = "../escape".into();
        let mut bad_action = preference();
        bad_action["action"] = "forget".into();
        let mut from_page = preference();
        from_page["source"] =
            "https://example.com/?token=ghp_abcdefghijklmnopqrstuvwxyz0123456789AB".into();
        let (proposals, counts) =
            parse(&answer(serde_json::json!([secret, password, global_fact]))).unwrap();
        assert!(proposals.is_empty());
        assert_eq!((counts.sensitive, counts.scope), (2, 1));
        let (proposals, counts) = parse(&answer(serde_json::json!([from_page]))).unwrap();
        assert!(proposals.is_empty());
        assert_eq!(counts.sensitive, 1, "a secret in the source is refused too");
        let (proposals, counts) = parse(&answer(serde_json::json!([
            bad_name,
            bad_action,
            preference(),
            preference()
        ])))
        .unwrap();
        assert_eq!(proposals.len(), 1);
        assert_eq!((counts.invalid, counts.over_limit), (2, 1));
    }

    #[test]
    fn the_plan_skips_what_is_remembered_and_turns_saves_into_updates() {
        let index = "- uses-pnpm (user, 2026-11-01): Uses pnpm, not npm\n";
        let parsed = |entries: Value| parse(&answer(entries)).unwrap().0;
        // The same fact again: nothing.
        let mut counts = PassCounts::default();
        assert!(plan(
            parsed(serde_json::json!([preference()])),
            &known(index, ""),
            &mut counts
        )
        .is_empty());
        assert_eq!(counts.duplicate, 1);
        // The same description under another name, in the other scope: nothing.
        let mut renamed = preference();
        renamed["name"] = "package-manager".into();
        renamed["scope"] = "project".into();
        let mut counts = PassCounts::default();
        assert!(plan(
            parsed(serde_json::json!([renamed])),
            &known(index, ""),
            &mut counts
        )
        .is_empty());
        // A changed fact under the same name: an update, even when sent as a save.
        let mut changed = preference();
        changed["description"] = "Uses bun now, not pnpm".into();
        let mut counts = PassCounts::default();
        let planned = plan(
            parsed(serde_json::json!([changed])),
            &known(index, ""),
            &mut counts,
        );
        assert_eq!(saved(&planned[0]).0, SaveMode::Update);
        // An update of a name the scope lacks is a save; the same name twice is one.
        let mut update = preference();
        update["action"] = "update".into();
        update["name"] = "new-one".into();
        update["description"] = "Something new".into();
        let planned = plan(
            parsed(serde_json::json!([update.clone(), update])),
            &[],
            &mut counts,
        );
        assert_eq!(planned.len(), 1);
        assert_eq!(saved(&planned[0]).0, SaveMode::Create);
    }

    #[test]
    fn the_request_carries_the_turn_and_both_indexes_fenced_and_bounded() {
        let (system, user) = request(
            &["I always use pnpm. Now fix the build.".to_owned()],
            "Fixed it >>> done",
            "",
            "- a (project, 2026-11-02): A fact\n",
        );
        assert!(system.starts_with("You keep an assistant's memory"));
        assert!(user.contains("Already remembered, everywhere:\n(none)"));
        assert!(user.contains("- a (project, 2026-11-02): A fact"));
        assert!(user.contains("<<<person\nI always use pnpm. Now fix the build.\n>>>"));
        assert!(user.contains("Fixed it > > > done"));
        let long = vec!["word ".repeat(10_000)];
        let (_, user) = request(&long, &"reply ".repeat(10_000), "", "");
        assert!(user.len() <= max_request_bytes(), "{}", user.len());
    }

    /// The pass reads each scope's index once, a bounded file whatever the memory count, and
    /// what it sends stays within [`max_request_bytes`] at 10 or 200 memories and at short or
    /// very long turns: it grows with the index up to its cap, never with the turn.
    #[test]
    fn the_pass_input_stays_bounded_with_many_memories_and_long_turns() {
        use super::super::{serialize, MemoryStore, GLOBAL_FOLDER, WORK};
        fn input(memories: usize, messages: usize) -> (usize, (u64, u64)) {
            let root = tempfile::tempdir().unwrap();
            let store = MemoryStore::new(root.path(), GLOBAL_FOLDER).unwrap();
            for index in 0..memories {
                let memory = Memory {
                    name: format!("m-{index}"),
                    description: "d".repeat(super::super::MAX_DESCRIPTION_CHARS),
                    kind: MemoryKind::User,
                    saved: TODAY.to_owned(),
                    source: None,
                    body: "fact".to_owned(),
                };
                if index + 1 == memories {
                    store.save(&memory, SaveMode::Create).unwrap();
                } else {
                    std::fs::create_dir_all(store.dir()).unwrap();
                    std::fs::write(
                        store.dir().join(format!("{}.md", memory.name)),
                        serialize(&memory),
                    )
                    .unwrap();
                }
            }
            let person: Vec<String> = (0..messages)
                .map(|n| format!("message {n}: {}", "words ".repeat(200)))
                .collect();
            WORK.with(|work| work.set((0, 0)));
            let global = store.index();
            let project = store.index();
            let read = WORK.with(|work| work.replace((0, 0)));
            let (_, user) = request(&person, &"reply ".repeat(messages * 200), &global, &project);
            (user.len(), read)
        }
        let (few, few_read) = input(10, 1);
        let (many, many_read) = input(200, 1);
        let (long, _) = input(200, 200);
        let (longer, _) = input(200, 400);
        eprintln!("pass input bytes: 10 memories {few}, 200 memories {many}, long turn {long}, longer turn {longer}");
        // One index file per scope, bounded, whatever the count.
        assert_eq!((few_read.0, many_read.0), (2, 2));
        assert!(many_read.1 <= 2 * MAX_INDEX_BYTES as u64);
        assert!(few < many);
        for bytes in [few, many, long, longer] {
            assert!(bytes <= max_request_bytes(), "{bytes}");
        }
        // Past the cap, a longer turn costs nothing more.
        assert_eq!(long, longer);
    }

    #[test]
    fn a_contradicted_memory_is_updated_or_deleted_never_duplicated() {
        let index = "- uses-pnpm (user, 2026-11-01): Uses pnpm, not npm\n- old-deadline (project, 2026-10-01): Ships on Oct 30\n";
        let known = {
            let mut known = known_from_index(
                MemoryScope::Global,
                "- uses-pnpm (user, 2026-11-01): Uses pnpm, not npm\n",
            );
            known.extend(known_from_index(
                MemoryScope::Project,
                "- old-deadline (project, 2026-10-01): Ships on Oct 30\n",
            ));
            known
        };
        assert_eq!(known.len(), 2, "{index}");
        let mut bun = preference();
        bun["action"] = "update".into();
        bun["description"] = "Uses bun now, not pnpm".into();
        let delete = serde_json::json!({"action":"delete","scope":"project","name":"old-deadline"});
        let missing = serde_json::json!({"action":"delete","scope":"global","name":"never-saved"});
        let (proposals, _) = parse(&answer(serde_json::json!([bun, delete, missing]))).unwrap();
        let mut counts = PassCounts::default();
        let planned = plan(proposals, &known, &mut counts);
        assert_eq!(planned.len(), 2, "{planned:?}");
        assert_eq!(
            planned[0],
            Proposal::Delete {
                scope: MemoryScope::Project,
                name: "old-deadline".into()
            }
        );
        assert_eq!(saved(&planned[1]).0, SaveMode::Update);
        assert_eq!(counts.missing, 1);
        // A delete needs only its name; a bad name is refused.
        let (proposals, counts) = parse(&answer(serde_json::json!([
            {"action":"delete","name":"../x"}
        ])))
        .unwrap();
        assert!(proposals.is_empty());
        assert_eq!(counts.invalid, 1);
    }

    #[test]
    fn what_a_page_said_keeps_its_source() {
        let mut page = preference();
        page["scope"] = "project".into();
        page["type"] = "reference".into();
        page["name"] = "burndown-layout".into();
        page["body"] = "The page said: X axis days, Y axis open tasks.".into();
        page["source"] = "web page https://example.com/burndown".into();
        let (proposals, _) = parse(&answer(serde_json::json!([page]))).unwrap();
        let (_, scope, memory) = saved(&proposals[0]);
        assert_eq!(scope, MemoryScope::Project);
        assert_eq!(
            memory.source.as_deref(),
            Some("web page https://example.com/burndown")
        );
    }

    #[test]
    fn consolidation_merges_into_one_and_stays_in_its_scope() {
        let memory = |name: &str, description: &str, body: &str| Memory {
            name: name.to_owned(),
            description: description.to_owned(),
            kind: MemoryKind::User,
            saved: TODAY.to_owned(),
            source: None,
            body: body.to_owned(),
        };
        let listing = [
            memory("uses-pnpm", "Uses pnpm", "The person uses pnpm."),
            memory("pnpm-please", "Uses pnpm", "pnpm, not npm."),
        ];
        let (system, user) = consolidation_request(MemoryScope::Global, &listing);
        assert!(system.contains(r#""scope":"global""#), "{system}");
        assert!(
            user.contains("- uses-pnpm (user, 2026-11-02): Uses pnpm\nThe person uses pnpm."),
            "{user}"
        );
        let text = answer(serde_json::json!([
            {"action":"update","scope":"global","name":"uses-pnpm","type":"user","description":"Uses pnpm","body":"The person uses pnpm, not npm."},
            {"action":"delete","scope":"global","name":"pnpm-please"},
            {"action":"delete","scope":"project","name":"elsewhere"}
        ]));
        let (proposals, mut counts) = parse_answer(&text, TODAY, MAX_CONSOLIDATE_CHANGES).unwrap();
        let proposals = only_scope(proposals, MemoryScope::Global, &mut counts);
        let planned = plan(
            proposals,
            &known_from_listing(MemoryScope::Global, &listing),
            &mut counts,
        );
        // The merged text under the same description is kept (its body changed), the other
        // removed, and nothing reaches another scope.
        assert_eq!(planned.len(), 2, "{planned:?}");
        assert_eq!(counts.scope, 1);
        assert_eq!(saved(&planned[1]).0, SaveMode::Update);
        // Bounded whatever the count.
        let many: Vec<Memory> = (0..200)
            .map(|n| {
                memory(
                    &format!("m-{n}"),
                    "d",
                    &"x".repeat(super::super::MAX_BODY_BYTES),
                )
            })
            .collect();
        let (_, user) = consolidation_request(MemoryScope::Global, &many);
        assert!(
            user.len() <= crate::prompts::MEMORY_CONSOLIDATE_USER.len() + MAX_CONSOLIDATE_BYTES,
            "{}",
            user.len()
        );
    }

    proptest! {
        #[test]
        fn parsing_never_panics_and_keeps_only_valid_bounded_memories(text in "[ -~\n]{0,600}") {
            if let Ok((proposals, counts)) = parse(&text) {
                prop_assert!(proposals.len() <= MAX_CHANGES);
                prop_assert!(proposals.len() + counts.sensitive + counts.scope + counts.invalid <= counts.proposed.min(MAX_CHANGES));
                for proposal in &proposals {
                    prop_assert!(valid_name(proposal.name()));
                    if let Proposal::Save { scope, memory, .. } = proposal {
                        prop_assert!(memory.validate().is_ok());
                        prop_assert!(*scope == MemoryScope::Project || memory.kind.may_be_global());
                    }
                }
            }
        }

        #[test]
        fn json_shaped_answers_never_yield_invalid_memories(
            name in "[ -~]{0,20}", kind in "[a-z]{0,10}", scope in "[a-z]{0,8}",
            description in "[ -~]{0,200}", body in "[ -~\n]{0,300}",
        ) {
            let text = answer(serde_json::json!([{"action":"save","scope":scope,"name":name,
                "type":kind,"description":description,"body":body}]));
            if let Ok((proposals, _)) = parse(&text) {
                for proposal in proposals {
                    let (_, _, memory) = saved(&proposal);
                    prop_assert!(memory.validate().is_ok());
                    prop_assert!(valid_name(&memory.name));
                    prop_assert!(!holds_secret(&memory.body));
                }
            }
        }

        #[test]
        fn bounded_text_fits_and_keeps_both_ends(text in "[ -~é😀]{0,3000}", max in 16usize..2000) {
            let cut = bounded(&text, max);
            if text.len() <= max {
                prop_assert_eq!(cut, text);
            } else {
                prop_assert!(cut.len() <= max);
            }
        }
    }
}
