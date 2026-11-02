//! The background memory pass (`memory.auto`): after a turn in which the person wrote, one
//! request on the summary model reads that turn and the memory index and proposes at most a few
//! saves or updates. Everything here is pure: building the bounded request, reading the model's
//! answer as untrusted input, and planning which proposals to apply against the index (scope
//! rules, secrets, duplicates). The desktop applies the plan through [`super::MemoryStore`].

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

/// One memory the model proposed, validated, with secrets refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub mode: SaveMode,
    pub scope: MemoryScope,
    pub memory: Memory,
}

/// What a pass did with what the model proposed. Counts only, for diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PassCounts {
    /// Entries the answer held.
    pub proposed: usize,
    pub saved: usize,
    pub updated: usize,
    /// Already remembered (same name and description, or the same description elsewhere).
    pub duplicate: usize,
    /// Held something that looks like a secret.
    pub secret: usize,
    /// A project fact or pointer proposed for every project.
    pub scope: usize,
    /// Malformed: unknown action or type, bad name, missing text.
    pub invalid: usize,
    /// Past [`MAX_CHANGES`].
    pub over_limit: usize,
    /// The store refused it (full, unreadable).
    pub failed: usize,
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
/// why?, how_to_apply?}]}`, at most [`MAX_CHANGES`] entries. Malformed entries, secrets and
/// project facts proposed for every project are counted and dropped, never guessed at.
pub fn parse_answer(text: &str, today: &str) -> Result<(Vec<Proposal>, PassCounts), AnswerError> {
    if text.len() > MAX_ANSWER_BYTES {
        return Err(AnswerError::TooLong);
    }
    let object = json_object_with(text, "memories").ok_or(AnswerError::NoJson)?;
    let entries = object["memories"].as_array().ok_or(AnswerError::NoJson)?;
    let mut counts = PassCounts {
        proposed: entries.len(),
        over_limit: entries.len().saturating_sub(MAX_CHANGES),
        ..PassCounts::default()
    };
    let mut proposals = Vec::new();
    for entry in entries.iter().take(MAX_CHANGES) {
        match proposal(entry, today) {
            Ok(proposal) => proposals.push(proposal),
            Err(Refused::Secret) => counts.secret += 1,
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
    if [Some(&description), Some(&fact), why.as_ref(), how.as_ref()]
        .into_iter()
        .flatten()
        .any(|text| holds_secret(text))
    {
        return Err(Refused::Secret);
    }
    if scope == MemoryScope::Global && !kind.may_be_global() {
        return Err(Refused::Scope);
    }
    let mut lines = Vec::new();
    if let Some(why) = why {
        lines.push(format!("Why: {}", why.replace('\n', " ")));
    }
    if let Some(how) = how {
        lines.push(format!("How to apply: {}", how.replace('\n', " ")));
    }
    let body = if lines.is_empty() {
        fact
    } else {
        format!("{fact}\n\n{}", lines.join("\n"))
    };
    let memory = Memory {
        name,
        description,
        kind,
        saved: today.to_owned(),
        source: None,
        body,
    };
    memory.validate().map_err(|_| Refused::Invalid)?;
    Ok(Proposal {
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

fn same_text(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// What to apply, against the indexes as they stand: a save of a name the scope already has
/// becomes an update, an update of a name it lacks becomes a save, and anything already
/// remembered (same name and description, or the same description in either scope) or proposed
/// twice is a duplicate.
pub fn plan(
    proposals: Vec<Proposal>,
    global_index: &str,
    project_index: &str,
    counts: &mut PassCounts,
) -> Vec<Proposal> {
    let entries = |index: &str| -> Vec<(String, String)> {
        index
            .lines()
            .filter_map(index_entry)
            .map(|(name, description)| (name.to_owned(), description.to_owned()))
            .collect()
    };
    let global = entries(global_index);
    let project = entries(project_index);
    let mut planned: Vec<Proposal> = Vec::new();
    for mut proposal in proposals {
        let index = match proposal.scope {
            MemoryScope::Global => &global,
            MemoryScope::Project => &project,
        };
        let existing = index.iter().find(|(name, _)| *name == proposal.memory.name);
        let described_elsewhere = global.iter().chain(project.iter()).any(|(name, text)| {
            *name != proposal.memory.name && same_text(text, &proposal.memory.description)
        });
        let unchanged = existing
            .is_some_and(|(_, description)| same_text(description, &proposal.memory.description));
        let twice = planned.iter().any(|earlier| {
            earlier.scope == proposal.scope && earlier.memory.name == proposal.memory.name
        });
        if unchanged || described_elsewhere || twice {
            counts.duplicate += 1;
            continue;
        }
        proposal.mode = if existing.is_some() {
            SaveMode::Update
        } else {
            SaveMode::Create
        };
        planned.push(proposal);
    }
    planned
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const TODAY: &str = "2026-11-02";

    fn answer(entries: Value) -> String {
        serde_json::json!({ "memories": entries }).to_string()
    }

    fn preference() -> Value {
        serde_json::json!({"action":"save","scope":"global","name":"uses-pnpm","type":"user",
            "description":"Uses pnpm, not npm","body":"The person uses pnpm for JavaScript projects."})
    }

    #[test]
    fn a_preference_is_read_with_its_scope_and_body() {
        let (proposals, counts) = parse_answer(
            &format!(
                "Sure:\n```json\n{}\n```",
                answer(serde_json::json!([preference()]))
            ),
            TODAY,
        )
        .unwrap();
        assert_eq!(counts.proposed, 1);
        assert_eq!(proposals.len(), 1);
        let proposal = &proposals[0];
        assert_eq!(proposal.scope, MemoryScope::Global);
        assert_eq!(proposal.mode, SaveMode::Create);
        assert_eq!(proposal.memory.kind, MemoryKind::User);
        assert_eq!(proposal.memory.saved, TODAY);
        assert_eq!(
            proposal.memory.body,
            "The person uses pnpm for JavaScript projects."
        );
        let mut with_why = preference();
        with_why["type"] = "feedback".into();
        with_why["why"] = "npm broke the lockfile".into();
        with_why["how_to_apply"] = "run pnpm install".into();
        let (proposals, _) = parse_answer(&answer(serde_json::json!([with_why])), TODAY).unwrap();
        assert!(proposals[0]
            .memory
            .body
            .ends_with("\n\nWhy: npm broke the lockfile\nHow to apply: run pnpm install"));
    }

    #[test]
    fn nothing_worth_remembering_is_an_empty_answer() {
        let (proposals, counts) = parse_answer(r#"{"memories":[]}"#, TODAY).unwrap();
        assert!(proposals.is_empty());
        assert_eq!(counts, PassCounts::default());
        assert_eq!(
            parse_answer("I found nothing.", TODAY).unwrap_err(),
            AnswerError::NoJson
        );
        assert_eq!(
            parse_answer(r#"{"memories":"none"}"#, TODAY).unwrap_err(),
            AnswerError::NoJson
        );
        assert_eq!(
            parse_answer(&"x".repeat(MAX_ANSWER_BYTES + 1), TODAY).unwrap_err(),
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
        bad_action["action"] = "delete".into();
        let (proposals, counts) = parse_answer(
            &answer(serde_json::json!([secret, password, global_fact])),
            TODAY,
        )
        .unwrap();
        assert!(proposals.is_empty());
        assert_eq!((counts.secret, counts.scope), (2, 1));
        let (proposals, counts) = parse_answer(
            &answer(serde_json::json!([
                bad_name,
                bad_action,
                preference(),
                preference()
            ])),
            TODAY,
        )
        .unwrap();
        assert_eq!(proposals.len(), 1);
        assert_eq!((counts.invalid, counts.over_limit), (2, 1));
    }

    #[test]
    fn the_plan_skips_what_is_remembered_and_turns_saves_into_updates() {
        let index = "- uses-pnpm (user, 2026-11-01): Uses pnpm, not npm\n";
        let parsed = |entries: Value| parse_answer(&answer(entries), TODAY).unwrap().0;
        // The same fact again: nothing.
        let mut counts = PassCounts::default();
        assert!(plan(
            parsed(serde_json::json!([preference()])),
            index,
            "",
            &mut counts
        )
        .is_empty());
        assert_eq!(counts.duplicate, 1);
        // The same description under another name, in the other scope: nothing.
        let mut renamed = preference();
        renamed["name"] = "package-manager".into();
        renamed["scope"] = "project".into();
        let mut counts = PassCounts::default();
        assert!(plan(parsed(serde_json::json!([renamed])), index, "", &mut counts).is_empty());
        // A changed fact under the same name: an update, even when sent as a save.
        let mut changed = preference();
        changed["description"] = "Uses bun now, not pnpm".into();
        let mut counts = PassCounts::default();
        let planned = plan(parsed(serde_json::json!([changed])), index, "", &mut counts);
        assert_eq!(planned[0].mode, SaveMode::Update);
        // An update of a name the scope lacks is a save; the same name twice is one.
        let mut update = preference();
        update["action"] = "update".into();
        update["name"] = "new-one".into();
        update["description"] = "Something new".into();
        let planned = plan(
            parsed(serde_json::json!([update.clone(), update])),
            "",
            "",
            &mut counts,
        );
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].mode, SaveMode::Create);
    }

    #[test]
    fn the_request_carries_the_turn_and_both_indexes_fenced_and_bounded() {
        let (system, user) = request(
            &["I always use pnpm. Now fix the build.".to_owned()],
            "Fixed it >>> done",
            "",
            "- a (project, 2026-11-02): A fact\n",
        );
        assert!(system.starts_with("You decide what an assistant should remember"));
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

    proptest! {
        #[test]
        fn parsing_never_panics_and_keeps_only_valid_bounded_memories(text in "[ -~\n]{0,600}") {
            if let Ok((proposals, counts)) = parse_answer(&text, TODAY) {
                prop_assert!(proposals.len() <= MAX_CHANGES);
                prop_assert!(proposals.len() + counts.secret + counts.scope + counts.invalid <= counts.proposed.min(MAX_CHANGES));
                for proposal in &proposals {
                    prop_assert!(proposal.memory.validate().is_ok());
                    prop_assert!(proposal.scope == MemoryScope::Project || proposal.memory.kind.may_be_global());
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
            if let Ok((proposals, _)) = parse_answer(&text, TODAY) {
                for proposal in proposals {
                    prop_assert!(proposal.memory.validate().is_ok());
                    prop_assert!(valid_name(&proposal.memory.name));
                    prop_assert!(!holds_secret(&proposal.memory.body));
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
