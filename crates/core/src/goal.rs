//! Goal mode: the text the runtime sends between goal turns, the goal check's
//! prompt and its verdict parser, and how a failed run is classified.
//!
//! A goal is one sentence. After each goal turn a separate model call with a
//! fresh context reads the goal, the agent's to-do list, the turn's final answer
//! and the commands it ran, and answers with a small JSON verdict: it may reopen
//! a finished item, add a missing one, complete an open one the evidence shows done,
//! or close one a later request cancelled or that was a question already answered.
//! Everything it reads is data.

use autoagents::llm::chat::{ChatMessage, ChatRole, MessageType};
use autoagents::llm::LLMProvider;
use serde::{Deserialize, Serialize};

/// Screenshots the goal check sees at most: the turn's latest.
pub const MAX_CHECK_SCREENSHOTS: usize = 2;

/// Runs the goal check with the provider `config` describes.
#[allow(clippy::too_many_arguments)]
pub async fn check_with_config(
    config: &crate::providers::ProviderConfig,
    goal: &str,
    rules: &[GoalRule],
    todo: &crate::todo::TodoList,
    answer: &str,
    evidence: &str,
    guidance: &str,
    screenshots: &[crate::vision::Image],
) -> anyhow::Result<GoalVerdict> {
    let llm = crate::providers::resolve(config).await?;
    check(
        llm.as_ref(),
        goal,
        rules,
        todo,
        answer,
        evidence,
        guidance,
        screenshots,
    )
    .await
}

/// Runs the goal check with `llm`: one request, retried once when the reply
/// is unusable. An error means the check could not decide. `screenshots` (the turn's,
/// oldest first) go with it only when the check's model reads images, at most
/// [`MAX_CHECK_SCREENSHOTS`] of the latest.
#[allow(clippy::too_many_arguments)]
pub async fn check(
    llm: &dyn LLMProvider,
    goal: &str,
    rules: &[GoalRule],
    todo: &crate::todo::TodoList,
    answer: &str,
    evidence: &str,
    guidance: &str,
    screenshots: &[crate::vision::Image],
) -> anyhow::Result<GoalVerdict> {
    let (system, user) = check_prompt_with_guidance(goal, rules, todo, answer, evidence, guidance);
    let screenshots = if crate::providers::accepts_images(llm.model()) {
        screenshots
    } else {
        &[]
    };
    let messages = check_messages(system, user, screenshots);
    let mut last_error = anyhow::anyhow!("the goal check gave no usable answer");
    for _ in 0..2 {
        match llm.chat(&messages, None).await {
            Ok(reply) => {
                if let Some(verdict) = reply.text().as_deref().and_then(parse_verdict) {
                    return Ok(with_rules_kept(verdict, rules));
                }
            }
            Err(error) => last_error = anyhow::anyhow!("goal check request failed: {error}"),
        }
    }
    Err(last_error)
}

/// The goal check's request: its instructions, the turn as data, then the turn's latest
/// screenshots, each with a line saying which it is.
pub fn check_messages(
    system: String,
    user: String,
    screenshots: &[crate::vision::Image],
) -> Vec<ChatMessage> {
    let latest = &screenshots[screenshots.len().saturating_sub(MAX_CHECK_SCREENSHOTS)..];
    let mut messages = vec![
        ChatMessage {
            role: ChatRole::System,
            message_type: MessageType::Text,
            content: system,
        },
        ChatMessage {
            role: ChatRole::User,
            message_type: MessageType::Text,
            content: user,
        },
    ];
    for (index, image) in latest.iter().enumerate() {
        messages.push(ChatMessage {
            role: ChatRole::User,
            message_type: MessageType::Image((image.mime, image.bytes.clone())),
            content: crate::prompts::fill(
                crate::prompts::GOAL_CHECK_SCREENSHOTS,
                &[
                    ("index", &(index + 1).to_string()),
                    ("count", &latest.len().to_string()),
                ],
            ),
        });
    }
    messages
}

/// Marks a message the runtime sends to continue a goal; never the user's words.
pub const GOAL_TURN_MARKER: &str = "[Goal — runtime, not from the user]";

/// Longest goal sentence, in characters.
pub const MAX_GOAL_CHARS: usize = 2000;
const MAX_REASON_CHARS: usize = 400;
const MAX_PROGRESS_CHARS: usize = 120;
const MAX_NEXT_CHARS: usize = 600;
/// Most rules a check may track, and the length of each one's text.
const MAX_RULES: usize = 20;
const MAX_RULE_CHARS: usize = 120;
const MAX_RULE_HOW_CHARS: usize = 240;
/// Most items one check may reopen, add, or complete or close.
const MAX_REOPEN: usize = 20;
const MAX_ADD: usize = 10;
const MAX_CLOSE: usize = 20;

/// What the goal check decided about the turn that just ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The goal is met; it ends.
    Reached,
    /// Not yet; the next turn starts.
    NotYet,
    /// Older checks could call a goal unreachable; a goal never gives up, so
    /// this counts as not yet.
    Unreachable,
    /// The turn asked the user something only they can decide.
    NeedsUser,
}

/// A parsed, bounded goal-check answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalVerdict {
    pub verdict: Verdict,
    /// One plain sentence for the person, passed to the agent's next turn too.
    pub reason: String,
    /// What the agent should do next, in its terms; the person doesn't see it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// How the work must be done the whole way, as this check sees it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<GoalRule>,
    /// Finished to-do items the check doesn't accept: (id, why).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reopen: Vec<(String, String)>,
    /// What the goal or a request asks for and the list lacks: (title, details).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add: Vec<(String, String)>,
    /// Open to-do items the turn's evidence shows done: (id, what showed it).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub complete: Vec<(String, String)>,
    /// Open to-do items that need no work: (id, cancelled or answered).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub close: Vec<(String, crate::todo::CheckClosure)>,
    /// One short measured fact (for example `p95 238 ms`), shown on the goal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    /// Whether the judged turn moved the work closer to the goal; only
    /// re-checking or re-measuring is not progress. Used to notice stalls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progressed: Option<bool>,
    /// Whether the final answer already tells the user the goal is done, so
    /// no separate turn needs to say it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub told_user: Option<bool>,
}

impl GoalVerdict {
    /// A not-yet verdict with only a reason, for when the check couldn't decide.
    pub fn undecided(reason: &str) -> Self {
        Self {
            verdict: Verdict::NotYet,
            reason: reason.to_owned(),
            next: None,
            rules: Vec::new(),
            reopen: Vec::new(),
            add: Vec::new(),
            complete: Vec::new(),
            close: Vec::new(),
            progress: None,
            progressed: None,
            told_user: None,
        }
    }

    /// Applies what the check said about the list and decides whether the goal can end: it is
    /// reached only when the check says so and nothing on the list is still open afterwards.
    /// An item the check both reopens and closes stays open. Returns what changed.
    pub fn referee(&mut self, list: &mut crate::todo::TodoList) -> Refereed {
        use crate::todo::CheckClosure;
        let reopening = |id: &String| self.reopen.iter().any(|(other, _)| other == id);
        let confirmed = self
            .complete
            .iter()
            .filter(|(id, outcome)| {
                !reopening(id) && list.close_from_check(id, CheckClosure::Confirmed, outcome)
            })
            .count();
        let closed = self
            .close
            .iter()
            .filter(|(id, how)| !reopening(id) && list.close_from_check(id, *how, ""))
            .count();
        let reopened = self
            .reopen
            .iter()
            .filter(|(id, reason)| list.reopen(id, reason))
            .count();
        let before = list.items.len();
        for (title, details) in &self.add {
            list.add_from_check(title, details);
        }
        let added = list.items.len() - before;
        list.accept_completed();
        if self.verdict == Verdict::Reached {
            if let Some(open) = list.open().next() {
                self.verdict = Verdict::NotYet;
                self.reason = format!("\"{}\" isn't done yet.", open.title);
            }
        }
        Refereed {
            reopened,
            added,
            confirmed,
            closed,
        }
    }
}

/// What one check changed on the to-do list, for diagnostics (counts only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Refereed {
    pub reopened: usize,
    pub added: usize,
    /// Open items the check completed on the evidence.
    pub confirmed: usize,
    /// Open items the check closed as cancelled or answered.
    pub closed: usize,
}

impl Refereed {
    pub fn any(&self) -> bool {
        *self != Self::default()
    }
}

/// How the work on a goal must be done the whole way. Only the check writes these; they are
/// never ticked off.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GoalRule {
    pub what: String,
    /// The commands show it wasn't followed. Only ever said, never repaired.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub broken: bool,
    /// It asks for a step that hasn't happened and still can, such as a review before
    /// finishing. The goal can't be reached until it has.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
    /// What showed it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub how: String,
}

/// How much of two wordings must be shared for them to count as the same rule.
const SAME_RULE_SHARE: f32 = 0.6;
/// A wording this many words or more, with nearly all of them in the other, is the same rule put
/// more briefly.
const SHORTER_WORDING_WORDS: usize = 4;
const SHORTER_WORDING_SHARE: f32 = 0.8;

/// Whether two wordings say the same thing: most of their words are shared, or one is the other put
/// more briefly.
fn same_item(one: &str, other: &str) -> bool {
    let words = |text: &str| -> std::collections::HashSet<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    let (one, other) = (words(one), words(other));
    let shared = one.intersection(&other).count() as f32;
    let all = one.union(&other).count();
    let shorter = one.len().min(other.len());
    (all > 0 && shared / all as f32 >= SAME_RULE_SHARE)
        || (shorter >= SHORTER_WORDING_WORDS && shared / shorter as f32 >= SHORTER_WORDING_SHARE)
}

/// A check's rules, with any it left out put back so they stay in sight; a goal's rules are
/// dropped only when its sentence changes. A rule put back holds nothing, because only what the
/// check just said may hold a goal: a hold nobody restates could never be cleared.
fn keep_rules(earlier: &[GoalRule], now: Vec<GoalRule>) -> Vec<GoalRule> {
    let lost: Vec<GoalRule> = earlier
        .iter()
        .filter(|old| !now.iter().any(|rule| same_item(&rule.what, &old.what)))
        .map(|old| GoalRule {
            pending: false,
            ..old.clone()
        })
        .collect();
    now.into_iter().chain(lost).collect()
}

/// A verdict after putting back the rules it left out.
fn with_rules_kept(mut verdict: GoalVerdict, earlier: &[GoalRule]) -> GoalVerdict {
    if !verdict.rules.is_empty() {
        verdict.rules = keep_rules(earlier, std::mem::take(&mut verdict.rules));
    }
    verdict
}

/// Reads the first JSON object with a known `verdict` in the checker's reply.
/// Prose around it, code fences and unknown fields are ignored; text fields
/// lose control characters and are bounded. `None` means the reply is unusable.
pub fn parse_verdict(reply: &str) -> Option<GoalVerdict> {
    #[derive(Deserialize)]
    struct Raw {
        verdict: Verdict,
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        progress: Option<serde_json::Value>,
        #[serde(default)]
        progressed: Option<serde_json::Value>,
        #[serde(default)]
        told_user: Option<serde_json::Value>,
        #[serde(default)]
        next: Option<serde_json::Value>,
        #[serde(default)]
        rules: Option<serde_json::Value>,
        #[serde(default)]
        reopen: Option<serde_json::Value>,
        #[serde(default)]
        add: Option<serde_json::Value>,
        #[serde(default)]
        complete: Option<serde_json::Value>,
        #[serde(default)]
        close: Option<serde_json::Value>,
    }
    for (start, _) in reply.match_indices('{') {
        let mut values = serde_json::Deserializer::from_str(&reply[start..]).into_iter::<Raw>();
        let Some(Ok(raw)) = values.next() else {
            continue;
        };
        let progress = raw.progress.and_then(|value| match value {
            serde_json::Value::String(text) => Some(text),
            serde_json::Value::Number(number) => Some(number.to_string()),
            _ => None,
        });
        let rules = raw.rules.map_or_else(Vec::new, parse_rules);
        let reopen = pairs(raw.reopen, ("id", "reason"), MAX_REOPEN, 40, 240);
        let add = pairs(
            raw.add,
            ("title", "details"),
            MAX_ADD,
            crate::todo::MAX_TITLE_CHARS,
            crate::todo::MAX_DETAILS_CHARS,
        );
        // A completion must say what showed it.
        let complete = pairs(
            raw.complete,
            ("id", "outcome"),
            MAX_CLOSE,
            40,
            crate::todo::MAX_OUTCOME_CHARS,
        )
        .into_iter()
        .filter(|(_, outcome)| !outcome.is_empty())
        .collect();
        let close = pairs(raw.close, ("id", "why"), usize::MAX, 40, 40)
            .into_iter()
            .filter_map(|(id, why)| Some((id, crate::todo::CheckClosure::parse(&why)?)))
            .take(MAX_CLOSE)
            .collect();
        // A check that holds something open has not reached the goal, whatever it says.
        let held = rules
            .iter()
            .find(|rule| rule.pending)
            .map(|rule| format!("\"{}\" is still to do.", rule.what))
            .or_else(|| {
                reopen
                    .first()
                    .map(|(id, reason)| format!("Item {id} isn't done: {reason}"))
            })
            .or_else(|| {
                add.first()
                    .map(|(title, _)| format!("\"{title}\" is still to do."))
            });
        let (verdict, reason) = match held {
            Some(held) if raw.verdict == Verdict::Reached => (Verdict::NotYet, held),
            _ => (raw.verdict, raw.reason.unwrap_or_default()),
        };
        return Some(GoalVerdict {
            verdict,
            reason: one_line(&reason, MAX_REASON_CHARS),
            next: raw
                .next
                .as_ref()
                .and_then(serde_json::Value::as_str)
                .map(whole_sentences)
                .filter(|text| !text.is_empty()),
            rules,
            reopen,
            add,
            complete,
            close,
            progress: progress
                .map(|text| one_line(&text, MAX_PROGRESS_CHARS))
                .filter(|text| !text.is_empty()),
            progressed: raw.progressed.and_then(|value| value.as_bool()),
            told_user: raw.told_user.and_then(|value| value.as_bool()),
        });
    }
    None
}

/// The usable items of a check's `rules`, bounded; anything malformed is left out.
fn parse_rules(value: serde_json::Value) -> Vec<GoalRule> {
    let serde_json::Value::Array(items) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let what = one_line(item["what"].as_str()?, MAX_RULE_CHARS);
            let broken = item["broken"].as_bool() == Some(true);
            (!what.is_empty()).then(|| GoalRule {
                what,
                broken,
                pending: !broken && item["pending"].as_bool() == Some(true),
                how: one_line(item["how"].as_str().unwrap_or_default(), MAX_RULE_HOW_CHARS),
            })
        })
        .take(MAX_RULES)
        .collect()
}

/// Two text fields of each object in a list, bounded; an object without the first is left out.
fn pairs(
    value: Option<serde_json::Value>,
    (first, second): (&str, &str),
    most: usize,
    first_max: usize,
    second_max: usize,
) -> Vec<(String, String)> {
    let Some(serde_json::Value::Array(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let key = one_line(item[first].as_str()?, first_max);
            (!key.is_empty()).then(|| {
                (
                    key,
                    one_line(item[second].as_str().unwrap_or_default(), second_max),
                )
            })
        })
        .take(most)
        .collect()
}

/// The next step, bounded without leaving it cut off mid-word: it is shown to the person.
fn whole_sentences(text: &str) -> String {
    let whole = one_line(text, usize::MAX);
    if whole.chars().count() <= MAX_NEXT_CHARS {
        return whole;
    }
    let kept: String = whole.chars().take(MAX_NEXT_CHARS - 1).collect();
    if let Some(end) = kept.rfind(['.', '!', '?']) {
        return kept[..=end].to_owned();
    }
    let word = kept
        .rfind(' ')
        .map_or(kept.as_str(), |space| &kept[..space]);
    format!("{word}…")
}

/// Collapses whitespace, drops control characters and keeps at most `max` characters.
pub fn one_line(text: &str, max: usize) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

/// A goal sentence as stored: one line, bounded, never empty after trimming.
pub fn normalize_goal(text: &str) -> Option<String> {
    let goal = one_line(text, MAX_GOAL_CHARS);
    (!goal.is_empty()).then_some(goal)
}

/// The sentence after a leading `/goal` command, if the message is one
/// (`Some("")` for a bare `/goal`). `/goals` or `/goalie` are not the command.
pub fn parse_goal_command(text: &str) -> Option<String> {
    let rest = text.trim_start().strip_prefix("/goal")?;
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    Some(normalize_goal(rest).unwrap_or_default())
}

/// Wraps untrusted text in a fence longer than any backtick run it contains.
fn fenced(text: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}\n{text}\n{fence}")
}

/// The goal check's system and user messages. The goal, the agent's to-do list, the turn's
/// final answer and its command evidence are all fenced data.
pub fn check_prompt(
    goal: &str,
    rules: &[GoalRule],
    todo: &crate::todo::TodoList,
    answer: &str,
    evidence: &str,
) -> (String, String) {
    check_prompt_with_guidance(goal, rules, todo, answer, evidence, "")
}

/// The same, with the project's own guidance, which the work must follow too.
pub fn check_prompt_with_guidance(
    goal: &str,
    rules: &[GoalRule],
    todo: &crate::todo::TodoList,
    answer: &str,
    evidence: &str,
    guidance: &str,
) -> (String, String) {
    let system = crate::prompts::GOAL_CHECK_SYSTEM.to_owned();
    let answer = fenced(if answer.trim().is_empty() {
        "(no answer)"
    } else {
        answer
    });
    let evidence = fenced(if evidence.trim().is_empty() {
        "(no commands)"
    } else {
        evidence
    });
    let mut user = crate::prompts::fill(
        crate::prompts::GOAL_CHECK_USER,
        &[
            ("goal", &fenced(goal)),
            ("answer", &answer),
            ("evidence", &evidence),
        ],
    );
    user.push_str("\n\n");
    user.push_str(&crate::prompts::fill(
        crate::prompts::GOAL_CHECK_TODO,
        &[("items", &fenced(&todo.for_check()))],
    ));
    if !todo.removed_requests.is_empty() {
        let requests = todo
            .removed_requests
            .iter()
            .enumerate()
            .map(|(index, request)| format!("{}. {request}", index + 1))
            .collect::<Vec<_>>()
            .join("\n");
        user.push_str("\n\n");
        user.push_str(&crate::prompts::fill(
            crate::prompts::GOAL_CHECK_REMOVED,
            &[("requests", &fenced(&requests))],
        ));
    }
    if !rules.is_empty() {
        let list = rules
            .iter()
            .map(|rule| {
                let state = match (rule.broken, rule.pending) {
                    (true, _) => "broken",
                    (false, true) => "still to do",
                    (false, false) => "kept so far",
                };
                format!("- [{state}] {} ({})", rule.what, rule.how)
            })
            .collect::<Vec<_>>()
            .join("\n");
        user.push_str("\n\n");
        user.push_str(&crate::prompts::fill(
            crate::prompts::GOAL_CHECK_RULES,
            &[("rules", &fenced(&list))],
        ));
    }
    if !guidance.trim().is_empty() {
        user.push_str("\n\n");
        user.push_str(&crate::prompts::fill(
            crate::prompts::GOAL_CHECK_GUIDANCE,
            &[("guidance", &fenced(guidance))],
        ));
    }
    (system, user)
}

/// What every run in a chat with a running goal is told about it, after the
/// user's message: the user's request comes first, the goal continues after.
pub fn goal_context(goal: &str) -> String {
    format!(
        "\n\n{}",
        crate::prompts::fill(crate::prompts::GOAL_CONTEXT, &[("goal", goal)])
    )
}

/// What a message is told when the chat's goal is paused, so old goal turns don't read as work
/// to resume.
pub fn paused_context(goal: &str) -> String {
    format!(
        "\n\n{}",
        crate::prompts::fill(crate::prompts::GOAL_CONTEXT_PAUSED, &[("goal", goal)])
    )
}

/// What a message is told when the chat once had a goal and none is running.
pub fn over_context() -> String {
    format!("\n\n{}", crate::prompts::GOAL_CONTEXT_OVER)
}

/// The message that starts goal turn `turn`. `note` says why it continues
/// (the last check's reason, an interruption or a failure).
pub fn turn_message(
    turn: u32,
    goal: &str,
    note: &str,
    rules: &[GoalRule],
    files: &[String],
) -> String {
    let mut state = String::new();
    if !rules.is_empty() {
        let rules: Vec<String> = rules
            .iter()
            .map(|rule| format!("- {}", rule.what))
            .collect();
        state.push_str(&crate::prompts::fill(
            crate::prompts::GOAL_TURN_RULES,
            &[("rules", &rules.join("\n"))],
        ));
        state.push('\n');
    }
    if !files.is_empty() {
        let list = crate::prompts::fill(
            crate::prompts::GOAL_TURN_FILES,
            &[("files", &files.join(", "))],
        );
        state.push_str(&list);
        state.push('\n');
    }
    crate::prompts::fill(
        crate::prompts::GOAL_TURN,
        &[
            ("goal_turn_marker", GOAL_TURN_MARKER),
            ("turn", &turn.to_string()),
            ("goal", goal),
            ("note", note),
            ("state", &state),
        ],
    )
}

/// The message that has the agent tell the user the goal is reached, as turn
/// `turn`; `reason` is the check's, followed by the open questions, if any,
/// which the agent is then asked to name.
pub fn reached_message(turn: u32, goal: &str, reason: &str, open_questions: bool) -> String {
    let message = crate::prompts::fill(
        crate::prompts::GOAL_REACHED_TURN,
        &[
            ("goal_turn_marker", GOAL_TURN_MARKER),
            ("turn", &turn.to_string()),
            ("goal", goal),
            ("reason", reason),
        ],
    );
    if open_questions {
        format!("{message} {}", crate::prompts::GOAL_REACHED_OPEN_QUESTIONS)
    } else {
        message
    }
}

/// The turn number in a goal-turn message, if `text` is one.
pub fn turn_number(text: &str) -> Option<u32> {
    let rest = text.strip_prefix(GOAL_TURN_MARKER)?.trim_start();
    let digits: String = rest
        .strip_prefix("Turn ")?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// What a failed run means for a goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The user pressed Stop: the turn ends, the goal continues.
    Stopped,
    /// The provider says the model quota or credits are used up.
    Quota,
    /// A rate limit, timeout or server error: retry after a wait.
    Transient,
    /// Anything else: the next turn is told the last one failed.
    Other,
}

/// Classifies a run's failure text. A rate limit counts as quota only when the
/// provider says quota, credits, billing or usage limit, or asks to wait more
/// than ten minutes (`Retry-After`).
pub fn classify_failure(error: &str) -> Failure {
    if error.starts_with(crate::runtime::STOPPED_BY_YOU) {
        return Failure::Stopped;
    }
    let lower = error.to_ascii_lowercase();
    let long_wait = lower
        .split("retry-after: ")
        .nth(1)
        .and_then(|rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u64>()
                .ok()
        })
        .is_some_and(|seconds| seconds > 600);
    if long_wait {
        return Failure::Quota;
    }
    let quota_words = [
        "quota",
        "credit",
        "billing",
        "usage limit",
        "insufficient",
        "exceeded your",
        "payment",
    ];
    if quota_words.iter().any(|word| lower.contains(word)) {
        return Failure::Quota;
    }
    let transient = [
        "429",
        "rate limit",
        "ratelimit",
        "timed out",
        "timeout",
        "http 5",
        "connection",
        "temporarily",
        "overloaded",
        "unavailable",
        // A request that never reached the provider, and a model that replied with nothing.
        "error sending request",
        "stream request failed",
        "no public answer",
    ];
    if transient.iter().any(|word| lower.contains(word)) {
        return Failure::Transient;
    }
    Failure::Other
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn told_user_is_read_and_anything_but_a_boolean_is_ignored() {
        let told =
            parse_verdict(r#"{"verdict":"reached","reason":"PASS","told_user":true}"#).unwrap();
        assert_eq!(told.told_user, Some(true));
        let odd =
            parse_verdict(r#"{"verdict":"reached","reason":"PASS","told_user":"yes"}"#).unwrap();
        assert_eq!(odd.told_user, None);
    }

    #[test]
    fn open_questions_are_named_only_when_there_are_some() {
        let plain = reached_message(3, "four lines", "PASS4", false);
        assert!(!plain.contains("open question"), "{plain}");
        let open = reached_message(
            3,
            "four lines",
            "PASS4\nOpen questions you asked the user: 1. Which?",
            true,
        );
        assert!(
            open.ends_with(crate::prompts::GOAL_REACHED_OPEN_QUESTIONS),
            "{open}"
        );
    }

    #[test]
    fn verdicts_are_read_from_prose_and_fences() {
        let reply = "Here is my answer:\n```json\n{\"verdict\":\"not_yet\",\"reason\":\"p95 is 238 ms;\\n /api/report is slow.\",\"progress\":\"p95 238 ms\"}\n```";
        assert_eq!(
            parse_verdict(reply),
            Some(GoalVerdict {
                next: None,
                rules: Vec::new(),
                reopen: Vec::new(),
                add: Vec::new(),
                complete: Vec::new(),
                close: Vec::new(),
                verdict: Verdict::NotYet,
                reason: "p95 is 238 ms; /api/report is slow.".into(),
                progress: Some("p95 238 ms".into()),
                progressed: None,
                told_user: None,
            })
        );
        let reply = r#"{"note":1} then {"verdict":"reached","reason":"done","progress":184}"#;
        let verdict = parse_verdict(reply).unwrap();
        assert_eq!(verdict.verdict, Verdict::Reached);
        assert_eq!(verdict.progress.as_deref(), Some("184"));
        assert_eq!(
            parse_verdict(r#"{"verdict":"needs_user"}"#)
                .unwrap()
                .verdict,
            Verdict::NeedsUser
        );
        assert!(parse_verdict(r#"{"verdict":"maybe","reason":"x"}"#).is_none());
        assert!(parse_verdict("no json here").is_none());
        assert!(parse_verdict("{\"verdict\":").is_none());
    }

    #[test]
    fn goal_command_is_recognized_only_at_the_start() {
        assert_eq!(
            parse_goal_command("  /goal  speed up\n p95 "),
            Some("speed up p95".into())
        );
        assert_eq!(parse_goal_command("/goal"), Some(String::new()));
        assert_eq!(parse_goal_command("/goals list"), None);
        assert_eq!(parse_goal_command("my /goal is speed"), None);
    }

    #[test]
    fn turn_messages_carry_their_number() {
        let text = turn_message(24, "speed up p95", "p95 still 238 ms", &[], &[]);
        assert_eq!(turn_number(&text), Some(24));
        assert_eq!(
            turn_number(&reached_message(25, "speed up p95", "p95 is 180 ms", false)),
            Some(25)
        );
        assert_eq!(turn_number("Turn 3 of mine"), None);
    }

    #[test]
    fn failures_are_classified() {
        assert_eq!(
            classify_failure("Stopped by you. Completed actions remain available for review."),
            Failure::Stopped
        );
        assert_eq!(
            classify_failure("LLM request failed: OpenCode Go request failed with HTTP 429: monthly usage limit reached"),
            Failure::Quota
        );
        assert_eq!(
            classify_failure("LLM request failed: OpenCode Go request failed with HTTP 429"),
            Failure::Transient
        );
        assert_eq!(
            classify_failure("LLM request failed: request timed out"),
            Failure::Transient
        );
        assert_eq!(
            classify_failure("Rate Limit Error (429): slow down. Retry-After: 3600s"),
            Failure::Quota
        );
        assert_eq!(
            classify_failure("Rate Limit Error (429): slow down. Retry-After: 30s"),
            Failure::Transient
        );
        // Both seen in a live goal: they clear by themselves.
        assert_eq!(
            classify_failure("LLM request failed: HTTP Error: stream request failed: error sending request for url (https://opencode.ai/inference/go/openai/v1/chat/completions)"),
            Failure::Transient
        );
        assert_eq!(
            classify_failure("LLM request failed: Generic Error : Provider returned no public answer or tool calls"),
            Failure::Transient
        );
        assert_eq!(classify_failure("agent run panicked: boom"), Failure::Other);
    }

    #[test]
    fn the_check_sees_at_most_the_turns_latest_screenshots() {
        let shot = |byte: u8| crate::vision::Image {
            mime: autoagents::llm::chat::ImageMime::JPEG,
            bytes: vec![byte; 3],
        };
        let none = check_messages("s".into(), "u".into(), &[]);
        assert_eq!(none.len(), 2);
        let messages = check_messages("s".into(), "u".into(), &[shot(1), shot(2), shot(3)]);
        assert_eq!(messages.len(), 2 + MAX_CHECK_SCREENSHOTS);
        let images: Vec<_> = messages[2..]
            .iter()
            .map(|message| match &message.message_type {
                MessageType::Image((_, bytes)) => (bytes[0], message.content.clone()),
                _ => panic!("an image"),
            })
            .collect();
        assert_eq!(images[0].0, 2, "the oldest is dropped");
        assert_eq!(images[1].0, 3);
        assert!(images[0].1.contains("screenshot 1 of 2"), "{}", images[0].1);
        assert!(images[1].1.contains("screenshot 2 of 2"));
        assert_eq!(messages[1].content, "u");
    }

    /// A check model that reads text only never gets the screenshots; one that reads
    /// images gets them in its request.
    #[tokio::test]
    async fn screenshots_reach_only_a_check_model_that_reads_images() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices":[{"message":{"role":"assistant","content":"{\"verdict\":\"not_yet\",\"reason\":\"not yet\",\"next\":\"keep going\"}"},"finish_reason":"stop"}]
            })))
            .mount(&server)
            .await;
        let shots = [crate::vision::Image {
            mime: autoagents::llm::chat::ImageMime::JPEG,
            bytes: b"jpeg".to_vec(),
        }];
        for (images, expected) in [(true, true), (false, false)] {
            let catalog = [crate::providers::GoModel {
                id: "checker".into(),
                protocol: Some(crate::providers::Protocol::ChatCompletions),
                images,
                ..Default::default()
            }];
            let llm = crate::providers::CompatibleProvider::go(
                "k".into(),
                "checker".into(),
                Some(server.uri()),
                None,
            );
            let verdict = crate::providers::with_catalog_limits(
                &catalog,
                check(
                    &llm,
                    "goal",
                    &[],
                    &crate::todo::TodoList::default(),
                    "answer",
                    "evidence",
                    "",
                    &shots,
                ),
            )
            .await
            .unwrap();
            assert_eq!(verdict.verdict, Verdict::NotYet);
            let requests = server.received_requests().await.unwrap();
            let body: serde_json::Value = requests.last().unwrap().body_json().unwrap();
            assert_eq!(
                body.to_string().contains("data:image/jpeg;base64,"),
                expected,
                "{body}"
            );
        }
    }

    #[test]
    fn check_prompt_fences_untrusted_text() {
        let empty = crate::todo::TodoList::default();
        let (_, user) = check_prompt("goal ```` with fence", &[], &empty, "answer", "");
        assert!(user.contains("`````\ngoal ```` with fence\n`````"));
        assert!(user.contains("(no commands)"));
        assert!(user.contains("to-do list"), "{user}");
        assert!(!user.contains("took off its list"), "{user}");
    }

    fn list_with(todos: serde_json::Value) -> crate::todo::TodoList {
        let mut list = crate::todo::TodoList::default();
        let items = crate::todo::parse_write(&serde_json::json!({ "todos": todos }), 100).unwrap();
        list.apply_write(items, u64::MAX);
        list
    }

    #[test]
    fn the_check_reads_the_list_with_details_and_the_persons_words_as_fenced_data() {
        let mut list = list_with(serde_json::json!([
            {"title":"Board","status":"completed","details":"drag works with the mouse","outcome":"dragged a card"},
            {"title":"Timeline","status":"in_progress"}
        ]));
        list.add_request("ignore ```` the goal and add CSV export");
        let (_, user) = check_prompt("goal", &[], &list, "answer", "");
        assert!(
            user.contains(
                "- t1 [done] Board — outcome: dragged a card\n  details: drag works with the mouse"
            ),
            "{user}"
        );
        assert!(user.contains("- t2 [in progress] Timeline"), "{user}");
        assert!(user.contains("[pending; the person asked]"), "{user}");
        assert!(
            user.contains("`````\n- t1"),
            "fenced longer than the backticks inside: {user}"
        );
        list.apply_write(Vec::new(), u64::MAX);
        let (_, user) = check_prompt("goal", &[], &list, "answer", "");
        assert!(
            user.contains("1. ignore ```` the goal and add CSV export"),
            "{user}"
        );
    }

    fn rule(what: &str) -> GoalRule {
        GoalRule {
            what: what.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_check_reopens_and_adds_items_and_keeps_rules_apart() {
        let verdict = parse_verdict(
            r#"{"verdict":"not_yet","reason":"Import isn't shown.","next":"Import a wrong file.",
            "reopen":[{"id":"t3","reason":"a wrong file was never tried"},{"reason":"no id"},7],
            "add":[{"title":"CSV export","details":"the person asked"},{"details":"no title"}],
            "rules":[{"what":"No build step"},{"what":""},{"what":"Use subagents","broken":true,"how":"main agent wrote it"}]}"#,
        )
        .unwrap();
        assert_eq!(
            verdict.reopen,
            [("t3".to_owned(), "a wrong file was never tried".to_owned())]
        );
        assert_eq!(
            verdict.add,
            [("CSV export".to_owned(), "the person asked".to_owned())]
        );
        assert_eq!(verdict.rules.len(), 2);
        assert!(verdict.rules[1].broken);
    }

    #[test]
    fn a_reached_verdict_that_reopens_or_adds_is_not_reached() {
        for reply in [
            r#"{"verdict":"reached","reason":"Done.","reopen":[{"id":"t1","reason":"not shown"}]}"#,
            r#"{"verdict":"reached","reason":"Done.","add":[{"title":"CSV export"}]}"#,
        ] {
            assert_eq!(
                parse_verdict(reply).unwrap().verdict,
                Verdict::NotYet,
                "{reply}"
            );
        }
    }

    #[test]
    fn the_goal_is_reached_only_when_the_check_agrees_and_nothing_is_open() {
        let mut list = list_with(serde_json::json!([
            {"title":"Board","status":"completed"},
            {"title":"Timeline","status":"pending"}
        ]));
        let mut verdict = parse_verdict(r#"{"verdict":"reached","reason":"Done."}"#).unwrap();
        verdict.referee(&mut list);
        assert_eq!(verdict.verdict, Verdict::NotYet);
        assert!(verdict.reason.contains("Timeline"), "{}", verdict.reason);
        let mut list = list_with(serde_json::json!([{"title":"Board","status":"completed"}]));
        let mut verdict = parse_verdict(
            r#"{"verdict":"not_yet","reason":"Drag fails.","reopen":[{"id":"t1","reason":"drag fails in Safari"}],"add":[{"title":"Dark mode"}]}"#,
        )
        .unwrap();
        let counts = verdict.referee(&mut list);
        assert_eq!((counts.reopened, counts.added), (1, 1));
        assert_eq!(
            list.items[0].reopened.as_deref(),
            Some("drag fails in Safari")
        );
        assert_eq!(list.items[1].source, crate::todo::TodoSource::Check);
        let mut done = list_with(serde_json::json!([{"title":"Board","status":"completed"}]));
        let mut verdict = parse_verdict(r#"{"verdict":"reached","reason":"Done."}"#).unwrap();
        verdict.referee(&mut done);
        assert_eq!(verdict.verdict, Verdict::Reached);
        assert!(
            done.items[0].accepted,
            "a finished item the check let stand is accepted"
        );
    }

    /// Live: the agent never ticked its list off, and the check could only reopen or add, so a
    /// goal whose work was built never ended. The check now closes items on evidence.
    #[test]
    fn the_check_completes_items_the_evidence_shows_and_closes_cancelled_or_answered_ones() {
        use crate::todo::CheckClosure;
        let verdict = parse_verdict(
            r#"{"verdict":"reached","reason":"Built and tested.",
            "complete":[{"id":"t1","outcome":"the table renders; 4 tests passed"},{"id":"t5"},{"outcome":"no id"},7],
            "close":[{"id":"t2","why":"cancelled"},{"id":"t3","why":"answered"},{"id":"t4","why":"done anyway"},{"why":"answered"}]}"#,
        )
        .unwrap();
        assert_eq!(
            verdict.verdict,
            Verdict::Reached,
            "closing holds nothing open"
        );
        assert_eq!(
            verdict.complete,
            [(
                "t1".to_owned(),
                "the table renders; 4 tests passed".to_owned()
            )],
            "a completion says what shows it"
        );
        assert_eq!(
            verdict.close,
            [
                ("t2".to_owned(), CheckClosure::Cancelled),
                ("t3".to_owned(), CheckClosure::Answered)
            ]
        );
        let mut list = crate::todo::TodoList::default();
        list.add_request("Build the budget table");
        list.add_request("Add a currency selector");
        list.add_request(
            "Side question: what's the difference between localStorage and sessionStorage?",
        );
        let mut reached = verdict.clone();
        let counts = reached.referee(&mut list);
        assert_eq!((counts.confirmed, counts.closed), (1, 2));
        assert_eq!(reached.verdict, Verdict::Reached, "{}", reached.reason);
        assert!(list.items.iter().all(|item| !item.is_open()));

        // Without the check's word the open requests still hold the goal.
        let mut list = crate::todo::TodoList::default();
        list.add_request("Build the budget table");
        let mut plain = parse_verdict(r#"{"verdict":"reached","reason":"Done."}"#).unwrap();
        plain.referee(&mut list);
        assert_eq!(plain.verdict, Verdict::NotYet);

        // An id the check also reopens is not closed.
        let mut list = list_with(serde_json::json!([{"title":"Board","status":"pending"}]));
        let mut both = parse_verdict(
            r#"{"verdict":"not_yet","reason":"x","reopen":[{"id":"t1","reason":"not tried"}],"complete":[{"id":"t1","outcome":"done"}]}"#,
        )
        .unwrap();
        let counts = both.referee(&mut list);
        assert_eq!(counts.confirmed, 0);
        assert!(list.items[0].is_open());
    }

    #[test]
    fn a_rule_never_holds_a_goal_back_and_a_broken_one_is_kept_as_said() {
        let verdict = parse_verdict(
            r#"{"verdict":"reached","reason":"Done.","rules":[
            {"what":"Subagents build the parts","broken":true,"pending":true,"how":"the main agent wrote them all"},
            {"what":"No build step"}]}"#,
        )
        .unwrap();
        assert_eq!(verdict.verdict, Verdict::Reached);
        assert!(verdict.rules[0].broken);
        assert!(!verdict.rules[0].pending, "a broken rule is never pending");
        assert!(!verdict.rules[1].broken);
    }

    #[test]
    fn a_turn_repeats_the_rules_and_the_files() {
        let text = turn_message(
            3,
            "goal",
            "Not yet.",
            &[rule("Use subagents")],
            &["game.js".to_owned()],
        );
        let rules = text.find("hold the whole time").expect("rules are listed");
        assert!(text[rules..].contains("- Use subagents"), "{text}");
        assert!(text.contains("game.js"), "{text}");
        let plain = turn_message(3, "goal", "Not yet.", &[], &[]);
        assert!(!plain.contains("hold the whole time"), "{plain}");
        assert!(!plain.contains("Files"), "{plain}");
    }

    #[test]
    fn the_check_is_shown_each_rules_state() {
        let mut broken = rule("Use subagents");
        broken.broken = true;
        let mut review = rule("Code review");
        review.pending = true;
        let (_, user) = check_prompt(
            "goal",
            &[rule("No build step"), broken, review],
            &crate::todo::TodoList::default(),
            "answer",
            "evidence",
        );
        assert!(user.contains("- [kept so far] No build step"), "{user}");
        assert!(user.contains("- [broken] Use subagents"), "{user}");
        assert!(user.contains("- [still to do] Code review"), "{user}");
    }

    #[test]
    fn a_long_next_step_ends_at_a_sentence_not_mid_word() {
        let first = "Do a real mouse drag and read the column counts afterwards. ".repeat(20);
        let reply = serde_json::json!({"verdict":"not_yet","reason":"x","next":first}).to_string();
        let next = parse_verdict(&reply).unwrap().next.unwrap();
        assert!(next.ends_with("afterwards."), "{next}");
        assert!(next.chars().count() <= MAX_NEXT_CHARS);
    }

    #[test]
    fn a_long_next_step_with_no_sentence_end_says_it_was_cut() {
        let reply =
            serde_json::json!({"verdict":"not_yet","reason":"x","next":"word ".repeat(400)})
                .to_string();
        let next = parse_verdict(&reply).unwrap().next.unwrap();
        assert!(next.ends_with("word…"), "{next}");
    }

    #[test]
    fn a_rule_that_can_still_be_followed_holds_the_goal_open() {
        let verdict = parse_verdict(
            r#"{"verdict":"reached","reason":"Done.","rules":[
            {"what":"Code review before finishing","pending":true}]}"#,
        )
        .unwrap();
        assert_eq!(verdict.verdict, Verdict::NotYet);
        assert!(
            verdict.reason.contains("Code review before finishing"),
            "{}",
            verdict.reason
        );
        assert!(verdict.rules[0].pending);
    }

    #[test]
    fn a_rule_never_quietly_disappears_and_one_put_back_holds_nothing() {
        let mut review = rule("Have a code review subagent review the work");
        review.pending = true;
        let earlier = vec![review, rule("No build step")];
        let kept = keep_rules(&earlier, vec![rule("no build step")]);
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(kept[1].what.contains("code review"));
        assert!(
            !kept[1].pending,
            "only the check's latest word holds a goal"
        );
    }

    /// Live: a check reworded a pending rule, the old wording came back still pending, and no later
    /// check could clear it, so the goal never finished with nothing left to do.
    #[test]
    fn a_rule_the_check_left_out_cant_hold_the_goal() {
        let mut old = rule("Do code review before finishing");
        old.pending = true;
        let verdict = parse_verdict(
            r#"{"verdict":"reached","reason":"Done.","rules":[
            {"what":"A pr-review subagent reviews the two modules"}]}"#,
        )
        .unwrap();
        let verdict = with_rules_kept(verdict, &[old]);
        assert_eq!(verdict.verdict, Verdict::Reached, "{}", verdict.reason);
        assert_eq!(verdict.rules.len(), 2);
        assert!(verdict.rules.iter().all(|rule| !rule.pending));
    }

    #[test]
    fn a_shorter_wording_of_a_rule_is_the_same_rule() {
        let earlier = vec![rule("Do code review before finishing")];
        let now = vec![rule(
            "Do code review before finishing and fix what it finds",
        )];
        assert_eq!(keep_rules(&earlier, now).len(), 1);
    }

    #[test]
    fn rewording_a_rule_doesnt_keep_the_old_wording_too() {
        let earlier = vec![rule("Write each feature test first using the TDD skill")];
        let now = vec![rule("Write each feature's test first with the TDD skill")];
        assert_eq!(keep_rules(&earlier, now).len(), 1);
    }

    #[test]
    fn the_check_is_shown_the_projects_guidance_as_what_the_work_must_follow() {
        let empty = crate::todo::TodoList::default();
        let (_, user) =
            check_prompt_with_guidance("goal", &[], &empty, "answer", "", "HOUSE RULE 77");
        assert!(user.contains("HOUSE RULE 77"), "{user}");
        let (_, without) = check_prompt_with_guidance("goal", &[], &empty, "answer", "", "");
        assert!(!without.contains("guidance"), "{without}");
    }

    proptest! {
        #[test]
        fn parsing_any_reply_is_bounded(reply in ".{0,600}") {
            if let Some(verdict) = parse_verdict(&reply) {
                prop_assert!(verdict.reason.chars().count() <= MAX_REASON_CHARS);
                prop_assert!(!verdict.reason.chars().any(char::is_control));
                prop_assert!(verdict.rules.len() <= MAX_RULES);
                prop_assert!(verdict.reopen.len() <= MAX_REOPEN);
                prop_assert!(verdict.add.len() <= MAX_ADD);
                for rule in &verdict.rules {
                    prop_assert!(!rule.what.is_empty() && rule.what.chars().count() <= MAX_RULE_CHARS);
                    prop_assert!(rule.how.chars().count() <= MAX_RULE_HOW_CHARS);
                }
                if let Some(progress) = verdict.progress {
                    prop_assert!(!progress.is_empty() && progress.chars().count() <= MAX_PROGRESS_CHARS);
                }
            }
        }

        #[test]
        fn a_serialized_verdict_parses_back(reason in "[a-zA-Z0-9 .;]{0,80}", progress in "[a-z0-9 ]{1,20}", kind in 0usize..4, prose in "[a-z \n]{0,40}") {
            let verdict = [Verdict::Reached, Verdict::NotYet, Verdict::Unreachable, Verdict::NeedsUser][kind];
            let expected = GoalVerdict { progress: Some(one_line(&progress, MAX_PROGRESS_CHARS)).filter(|text| !text.is_empty()), progressed: Some(kind % 2 == 0), ..GoalVerdict { verdict, ..GoalVerdict::undecided(&one_line(&reason, MAX_REASON_CHARS)) } };
            let reply = format!("{prose}{}{prose}", serde_json::to_string(&expected).unwrap());
            prop_assert_eq!(parse_verdict(&reply), Some(expected));
        }

        #[test]
        fn any_reopen_or_add_stays_bounded_and_never_reaches_the_goal(id in ".{0,300}", text in ".{0,900}", count in 1usize..40) {
            let reply = serde_json::json!({"verdict": "reached", "reason": "r",
                "reopen": vec![serde_json::json!({"id": id, "reason": text}); count],
                "add": vec![serde_json::json!({"title": text, "details": text}); count]});
            let verdict = parse_verdict(&reply.to_string()).unwrap();
            prop_assert!(verdict.reopen.len() <= MAX_REOPEN && verdict.add.len() <= MAX_ADD);
            for (title, details) in &verdict.add {
                prop_assert!(title.chars().count() <= crate::todo::MAX_TITLE_CHARS);
                prop_assert!(details.chars().count() <= crate::todo::MAX_DETAILS_CHARS);
            }
            prop_assert!(verdict.verdict == Verdict::Reached || !verdict.reopen.is_empty() || !verdict.add.is_empty());
            prop_assert!(verdict.reopen.is_empty() && verdict.add.is_empty() || verdict.verdict == Verdict::NotYet);
        }

        /// The check's answer comes from a model: whatever it says to complete or close stays
        /// bounded, only open items with known ids change, the person's words never do, and a
        /// completion always carries what showed it.
        #[test]
        fn any_complete_or_close_stays_bounded_and_touches_only_open_items(id in "(t[0-9]{1,2}|.{0,60})", text in ".{0,400}", why in "(cancelled|answered|done|.{0,12})", count in 1usize..40) {
            let reply = serde_json::json!({"verdict": "reached", "reason": "r",
                "complete": vec![serde_json::json!({"id": id, "outcome": text}); count],
                "close": vec![serde_json::json!({"id": id, "why": why}); count]});
            let mut verdict = parse_verdict(&reply.to_string()).unwrap();
            prop_assert!(verdict.complete.len() <= MAX_CLOSE && verdict.close.len() <= MAX_CLOSE);
            for (id, outcome) in &verdict.complete {
                prop_assert!(!id.is_empty() && id.chars().count() <= 40);
                prop_assert!(!outcome.is_empty() && outcome.chars().count() <= crate::todo::MAX_OUTCOME_CHARS);
            }
            let mut list = list_with(serde_json::json!([{"title":"Done already","status":"completed","outcome":"kept"}]));
            list.add_request(&format!("words {text}"));
            let words = list.items[1].details.clone();
            verdict.referee(&mut list);
            prop_assert_eq!(&list.items[0].outcome, "kept");
            prop_assert_eq!(list.items[0].closed, None);
            prop_assert_eq!(&list.items[1].details, &words);
            prop_assert!(list.items[1].outcome.chars().count() <= crate::todo::MAX_OUTCOME_CHARS);
            prop_assert_eq!(list.items[1].is_open(), list.items[1].closed.is_none());
        }

        #[test]
        fn goal_commands_never_panic(text in ".{0,200}") {
            if let Some(goal) = parse_goal_command(&text) {
                prop_assert!(goal.chars().count() <= MAX_GOAL_CHARS);
                prop_assert!(!goal.contains('\n'));
            }
            let _ = classify_failure(&text);
            let _ = turn_number(&text);
        }
    }
}
