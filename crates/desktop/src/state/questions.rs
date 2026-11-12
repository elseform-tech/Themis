//! Questions the agent asks the user without stopping its work.
//!
//! The agent asks with `ask_user` (see `question_tool`), saying what it will
//! assume meanwhile, and carries on. A chat's questions live in
//! `questions/<chat id>.json` beside `threads.json`; the transcript records
//! each one and its answer so the chat shows them. An answer is delivered like
//! a steer when a run is going (a steer the run never reads returns to the
//! queue), otherwise it goes out first as the next message, so it is never
//! lost. Only a blocking question ends the run that asked it. Questions asked
//! together share a group: one box shows them as pages and sends their answers
//! in one message.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use super::chat_grants::safe_chat_id;
use super::AppState;
use crate::sink::EventSink;
use crate::types::{Question, QueuedMessage, ThreadEvent, ThreadEventEnvelope};

/// Directory name under the app data directory.
pub(super) const QUESTIONS_DIR: &str = "questions";
/// Most questions one chat can have open at once.
pub(super) const MAX_OPEN: usize = 8;
/// Most questions kept per chat, answered ones included (the oldest answered go first).
const MAX_KEPT: usize = 100;
const MAX_QUESTION_CHARS: usize = 1_000;
const MAX_ASSUMPTION_CHARS: usize = 1_000;
/// A question offers 2 to 4 choices, or none (then its recommendation is a suggestion).
const MAX_CHOICES: usize = 4;
const MAX_CHOICE_CHARS: usize = 200;
const MAX_TOPIC_CHARS: usize = 80;
/// A choice's hint (one paragraph: what happens, its advantages, disadvantages and
/// consequences) or a reason: long enough to say it, short enough to read at a glance.
const MAX_HINT_CHARS: usize = 600;
const MAX_ANSWER_CHARS: usize = 10_000;

/// Starts the message that delivers answers; the goal supervisor reads it back
/// to tell an answer from an ordinary reply. Never change it.
pub(crate) const ANSWER_MARKER: &str = "[Answer to your question — from the user]";

/// What the agent asked, before it is stored.
#[derive(Debug, Clone, Default)]
pub(super) struct NewQuestion {
    pub question: String,
    pub choices: Vec<String>,
    /// The agent's recommended answer (`recommended`), which it works on until the
    /// answer comes: one of the choices, or a suggestion when there are none.
    pub assumption: Option<String>,
    /// Each answer's hint: one paragraph on what picking it means (`hint` on a choice).
    pub reasons: Vec<(String, String)>,
    /// Why the recommendation is recommended (the top-level `why`): the recommendation's
    /// hint when it has none of its own.
    pub why: Option<String>,
    pub blocking: bool,
    pub topic: Option<String>,
    /// The subagent asking, by id: its answer goes to that subagent.
    pub subagent: Option<String>,
    /// Asks for a permission (Auto mode's block box): only the person may answer it.
    pub permission: bool,
}

/// Why a question was not asked.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum AskRefused {
    Empty,
    TooMany,
}

/// The choices as shown: the recommended one first, distinct, at most
/// [`MAX_CHOICES`]; a lone choice is no choice but the suggestion.
fn shape_choices(
    choices: Vec<String>,
    recommended: Option<String>,
) -> (Vec<String>, Option<String>) {
    let mut shaped: Vec<String> = Vec::new();
    for choice in recommended
        .iter()
        .filter(|_| !choices.is_empty())
        .chain(&choices)
    {
        if !shaped.contains(choice) {
            shaped.push(choice.clone());
        }
    }
    shaped.truncate(MAX_CHOICES);
    if shaped.len() < 2 {
        let suggestion = recommended.or_else(|| shaped.pop());
        return (Vec::new(), suggestion);
    }
    (shaped, recommended)
}

fn bounded(text: &str, max: usize) -> String {
    themis_core::goal::one_line(text, max)
}

/// A question ready to store: bounded, its choices shaped, and hints only for the answers
/// on offer.
struct Shaped {
    text: String,
    topic: Option<String>,
    choices: Vec<String>,
    assumption: Option<String>,
    reasons: BTreeMap<String, String>,
    blocking: bool,
    subagent: Option<String>,
    permission: bool,
}

fn shape(asked: NewQuestion) -> Result<Shaped, AskRefused> {
    let text = bounded(&asked.question, MAX_QUESTION_CHARS);
    if text.is_empty() {
        return Err(AskRefused::Empty);
    }
    let topic = asked
        .topic
        .map(|topic| bounded(&topic, MAX_TOPIC_CHARS))
        .filter(|topic| !topic.is_empty());
    let choices: Vec<String> = asked
        .choices
        .iter()
        .map(|choice| bounded(choice, MAX_CHOICE_CHARS))
        .filter(|choice| !choice.is_empty())
        .collect();
    let assumption = asked
        .assumption
        .map(|text| bounded(&text, MAX_ASSUMPTION_CHARS))
        .filter(|text| !text.is_empty());
    let (choices, assumption) = shape_choices(choices, assumption);
    let offered =
        |answer: &str| choices.iter().any(|c| c == answer) || assumption.as_deref() == Some(answer);
    // Hints only for answers on offer; the recommendation's `why` stands in for a hint it
    // lacks (a suggestion has none).
    let mut reasons = BTreeMap::new();
    for (answer, hint) in asked.reasons {
        let (answer, hint) = (
            bounded(&answer, MAX_CHOICE_CHARS),
            bounded(&hint, MAX_HINT_CHARS),
        );
        if offered(&answer) && !hint.is_empty() {
            reasons.insert(answer, hint);
        }
    }
    let why = asked.why.map(|why| bounded(&why, MAX_HINT_CHARS));
    if let (Some(answer), Some(why)) = (&assumption, why.filter(|why| !why.is_empty())) {
        reasons.entry(answer.clone()).or_insert(why);
    }
    Ok(Shaped {
        text,
        topic,
        choices,
        assumption,
        reasons,
        blocking: asked.blocking,
        subagent: asked.subagent,
        permission: asked.permission,
    })
}

/// The open question `asked` updates: the same topic, or the same wording. A permission
/// question and an ordinary one never merge, so an agent repeating an Auto block's wording
/// asks a question of its own and cannot rewrite the person's box.
fn same_open(questions: &[Question], asked: &Shaped) -> Option<usize> {
    questions.iter().position(|question| {
        question.answer.is_none()
            && question.permission == asked.permission
            && (asked.topic.is_some() && question.topic == asked.topic
                || question.question == asked.text)
    })
}

/// Stores `asked` in `questions`, updating the open question it repeats. Returns the stored
/// question and whether it was an update.
fn store_question(
    questions: &mut Vec<Question>,
    asked: Shaped,
    run_id: &str,
    group: Option<String>,
) -> (Question, bool) {
    if let Some(index) = same_open(questions, &asked) {
        let question = &mut questions[index];
        question.question = asked.text;
        question.choices = asked.choices;
        question.assumption = asked.assumption;
        question.reasons = asked.reasons;
        question.group = group;
        question.blocking = asked.blocking;
        question.topic = asked.topic.or(question.topic.take());
        question.subagent = asked.subagent;
        // Only questions of the same kind merge (see `same_open`); kept as a second guard.
        question.permission |= asked.permission;
        question.run_id = run_id.to_owned();
        return (question.clone(), true);
    }
    let question = Question {
        id: uuid::Uuid::new_v4().to_string(),
        run_id: run_id.to_owned(),
        question: asked.text,
        choices: asked.choices,
        assumption: asked.assumption,
        reasons: asked.reasons,
        group,
        blocking: asked.blocking,
        topic: asked.topic,
        subagent: asked.subagent,
        asked_at: now_ms(),
        answer: None,
        permission: asked.permission,
    };
    questions.push(question.clone());
    (question, false)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl AppState {
    fn questions_path(&self, thread_id: &str) -> Option<PathBuf> {
        safe_chat_id(thread_id).then(|| self.inner.questions_dir.join(format!("{thread_id}.json")))
    }

    /// The chat's questions, oldest first. A damaged file is ignored (no
    /// questions) and noted in diagnostics with a fixed reason.
    pub(super) fn questions_for(&self, thread_id: &str) -> Vec<Question> {
        let mut all = self
            .inner
            .questions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(questions) = all.get(thread_id) {
            return questions.clone();
        }
        let questions = match self.questions_path(thread_id).map(std::fs::read_to_string) {
            Some(Ok(text)) => serde_json::from_str(&text).unwrap_or_else(|_| {
                let _ = self.inner.diagnostics.append(
                    "questions",
                    "questions_ignored",
                    "warn",
                    json!({"thread_id": thread_id, "reason": "unreadable questions file"}),
                );
                Vec::new()
            }),
            _ => Vec::new(),
        };
        all.insert(thread_id.to_owned(), questions.clone());
        questions
    }

    fn save_questions(&self, thread_id: &str, questions: &[Question]) {
        self.inner
            .questions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(thread_id.to_owned(), questions.to_vec());
        let Some(path) = self.questions_path(thread_id) else {
            return;
        };
        if let Err(error) = super::write_json_file(&path, &questions) {
            self.record_error("save_questions", error);
        }
    }

    /// Records a question event in the chat's history and sends it to clients.
    fn question_event(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        run_id: &str,
        event: ThreadEvent,
    ) {
        let envelope = ThreadEventEnvelope {
            thread_id: thread_id.to_owned(),
            run_id: run_id.to_owned(),
            event,
        };
        if let Err(error) = self.inner.transcript.append_event(&envelope) {
            self.record_error("save_thread_event", error);
        }
        sink.emit_thread_event(&envelope);
    }

    /// The chat's unanswered questions, oldest first.
    pub(super) fn open_questions(&self, thread_id: &str) -> Vec<Question> {
        self.questions_for(thread_id)
            .into_iter()
            .filter(|question| question.answer.is_none())
            .collect()
    }

    /// Stores a question from `run_id`. An open question on the same topic, or
    /// with the same wording, is updated instead of adding another.
    pub(super) fn ask_question(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        run_id: &str,
        asked: NewQuestion,
    ) -> Result<Question, AskRefused> {
        let mut asked = self.ask_questions(sink, thread_id, run_id, vec![asked])?;
        asked.pop().ok_or(AskRefused::Empty)
    }

    /// Stores the questions of one `ask_user` call from `run_id`, all or none. Several share a
    /// group: one box shows them as pages. Each is still its own question, so an open one on
    /// the same topic, or with the same wording, is updated instead of adding another.
    pub(super) fn ask_questions(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        run_id: &str,
        asked: Vec<NewQuestion>,
    ) -> Result<Vec<Question>, AskRefused> {
        let shaped = asked
            .into_iter()
            .map(shape)
            .collect::<Result<Vec<_>, _>>()?;
        let group = (shaped.len() > 1).then(|| uuid::Uuid::new_v4().to_string());
        let _guard = self
            .inner
            .question_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut questions = self.questions_for(thread_id);
        let open = questions.iter().filter(|q| q.answer.is_none()).count();
        let new = shaped
            .iter()
            .filter(|asked| same_open(&questions, asked).is_none())
            .count();
        if new > 0 && open + new > MAX_OPEN {
            return Err(AskRefused::TooMany);
        }
        let stored: Vec<(Question, bool)> = shaped
            .into_iter()
            .map(|asked| store_question(&mut questions, asked, run_id, group.clone()))
            .collect();
        // Keep the newest; the oldest answered questions go first.
        while questions.len() > MAX_KEPT {
            let Some(oldest) = questions.iter().position(|q| q.answer.is_some()) else {
                break;
            };
            questions.remove(oldest);
        }
        self.save_questions(thread_id, &questions);
        let pages = stored.len();
        let new_ids: Vec<String> = stored
            .iter()
            .filter(|(_, updated)| !updated)
            .map(|(question, _)| question.id.clone())
            .collect();
        let asked = stored
            .into_iter()
            .map(|(question, updated)| {
                let _ = self.inner.diagnostics.append(
                    "questions",
                    if updated { "updated" } else { "asked" },
                    "info",
                    json!({"thread_id": thread_id, "question_id": question.id, "blocking": question.blocking, "pages": pages}),
                );
                self.question_event(
                    sink,
                    thread_id,
                    run_id,
                    ThreadEvent::QuestionAsked {
                        question: question.clone(),
                    },
                );
                question
            })
            .collect();
        drop(_guard);
        // A Vision's work chat asked: its Vision wakes to decide or pass the questions on.
        self.visions_question_asked(thread_id, &new_ids);
        Ok(asked)
    }

    /// The chat's questions, oldest first.
    pub async fn list_questions(&self, thread_id: String) -> Result<Vec<Question>, String> {
        self.require_thread(&thread_id).await?;
        Ok(self.questions_for(&thread_id))
    }

    /// Answers an open question and delivers the answer: into the running run
    /// like a steer, or first in the queue as the next message.
    pub async fn answer_question(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        question_id: String,
        answer: String,
    ) -> Result<Question, String> {
        let mut answered = self
            .answer_questions(sink, thread_id, vec![(question_id, answer)])
            .await?;
        answered.pop().ok_or_else(|| "Write an answer".to_owned())
    }

    /// Answers several open questions at once (the pages of one box) and delivers the answers
    /// together, in one message. All are answered, or none (an unknown, answered, empty or
    /// too long answer refuses the lot).
    pub async fn answer_questions(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        answers: Vec<(String, String)>,
    ) -> Result<Vec<Question>, String> {
        self.answer_questions_by(sink, thread_id, answers, true)
            .await
    }

    /// Answers questions as the person (`person`) or as a Vision. Only the person's "Allow once"
    /// lets an action Auto blocked run; a Vision's answer never does.
    pub(super) async fn answer_questions_by(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        answers: Vec<(String, String)>,
        person: bool,
    ) -> Result<Vec<Question>, String> {
        let running = self.require_thread(&thread_id).await?;
        let answered = self.record_answers(&thread_id, answers, person)?;
        for (question, answer) in &answered {
            self.question_event(
                sink.as_ref(),
                &thread_id,
                "",
                ThreadEvent::QuestionAnswered {
                    id: question.id.clone(),
                    answer: answer.clone(),
                },
            );
            // The person's "Allow once" to an Auto block lets that exact action run once.
            if person {
                self.auto_question_answered(&question.id, answer);
            }
        }
        let steered = self.deliver_answers(sink.as_ref(), &thread_id, &answered)?;
        for (question, _) in &answered {
            let _ = self.inner.diagnostics.append(
                "questions",
                "answered",
                "info",
                json!({"thread_id": thread_id, "question_id": question.id, "delivery": if steered { "run" } else { "queue" }, "pages": answered.len()}),
            );
        }
        if !running || !steered {
            self.dispatch_queue(sink, &thread_id).await;
        }
        Ok(answered.into_iter().map(|(question, _)| question).collect())
    }

    /// Stores the answers, all or none, returning each answered question with its answer.
    fn record_answers(
        &self,
        thread_id: &str,
        answers: Vec<(String, String)>,
        person: bool,
    ) -> Result<Vec<(Question, String)>, String> {
        if answers.is_empty() {
            return Err("Write an answer".to_owned());
        }
        let _guard = self
            .inner
            .question_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut questions = self.questions_for(thread_id);
        let mut answered = Vec::new();
        for (question_id, answer) in answers {
            let answer = answer.trim().to_owned();
            if answer.is_empty() {
                return Err("Write an answer".to_owned());
            }
            if answer.chars().count() > MAX_ANSWER_CHARS {
                return Err("This answer is too long".to_owned());
            }
            let question = questions
                .iter_mut()
                .find(|question| question.id == question_id)
                .ok_or_else(|| "That question is no longer in this chat".to_owned())?;
            if question.answer.is_some() {
                return Err("That question is already answered".to_owned());
            }
            // Checked under the lock the write takes: a permission question is the person's.
            if question.permission && !person {
                return Err(themis_core::prompts::AGENT_CLI_PERMISSION_QUESTION.to_owned());
            }
            question.answer = Some(answer.clone());
            answered.push((question.clone(), answer));
        }
        self.save_questions(thread_id, &questions);
        Ok(answered)
    }

    /// Hands the answers to the running run, or puts them first in the queue
    /// (joining an answer already waiting there), as one message. A running subagent that
    /// asked gets its own. Returns whether a run took the main agent's answers.
    fn deliver_answers(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        answered: &[(Question, String)],
    ) -> Result<bool, String> {
        // The subagent that asked gets it while it is running; once it has ended, the main
        // agent does, like any answer.
        let mut main = Vec::new();
        let mut subagents: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (question, answer) in answered {
            let body = answer_body(question, answer);
            match question.subagent.as_deref() {
                Some(id) if self.running_subagent(thread_id, id).is_some() => {
                    subagents.entry(id.to_owned()).or_default().push(body);
                }
                _ => main.push(body),
            }
        }
        for (id, bodies) in subagents {
            if let Some(agent) = self.running_subagent(thread_id, &id) {
                agent
                    .inbox
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push_back(format!("{ANSWER_MARKER}\n{}", bodies.join("\n\n")));
            }
        }
        if main.is_empty() {
            return Ok(true);
        }
        let body = main.join("\n\n");
        // Under the queue lock, like a steer: closing the inbox takes the same
        // lock, so the answer lands in a live inbox or in the queue.
        self.edit_queue(sink, thread_id, |queue| {
            let inboxes = self
                .inner
                .steer_inboxes
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = inboxes.get(thread_id) {
                entry.push(format!("{ANSWER_MARKER}\n{body}"), None);
                return Ok(true);
            }
            match queue.first_mut() {
                Some(first) if !first.held && first.text.starts_with(ANSWER_MARKER) => {
                    first.text.push_str("\n\n");
                    first.text.push_str(&body);
                }
                _ => queue.insert(
                    0,
                    QueuedMessage::new(format!("{ANSWER_MARKER}\n{body}"), Vec::new(), None, None),
                ),
            }
            Ok(false)
        })
    }

    /// The open questions as one line for a goal turn, or `None` without any.
    pub(super) fn open_questions_note(&self, thread_id: &str) -> Option<String> {
        let open = self.open_questions(thread_id);
        (!open.is_empty()).then(|| {
            themis_core::prompts::fill(
                themis_core::prompts::GOAL_OPEN_QUESTIONS,
                &[("questions", &numbered_questions(&open))],
            )
        })
    }

    /// A message the person sent (`person`; not a goal, automation or hook): hooks may start
    /// turns again, and it answers the questions still open. Returns what the agent's copy of
    /// the message ends with ("" for nothing).
    pub(super) fn person_message_arrived(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        text: &str,
        person: bool,
    ) -> String {
        if !person {
            return String::new();
        }
        self.reset_hook_wakes(thread_id);
        self.answer_open_in_chat(sink, thread_id, text)
            .map(|note| format!("\n\n{note}"))
            .unwrap_or_default()
    }

    /// The `ask_user` result's note, followed by the other questions still open, so a
    /// reworded question is not asked twice.
    pub(super) fn with_still_open(&self, thread_id: &str, asked: &[&str], note: String) -> String {
        let others: Vec<_> = self
            .open_questions(thread_id)
            .into_iter()
            .filter(|open| !asked.contains(&open.id.as_str()))
            .collect();
        if others.is_empty() {
            return note;
        }
        let still_open = themis_core::prompts::fill(
            themis_core::prompts::ASK_USER_STILL_OPEN,
            &[("questions", &numbered_questions(&others))],
        );
        format!("{note} {still_open}")
    }

    /// A message the person wrote while questions were open answers them: each is closed with
    /// that message as its answer (the boxes go away), and the agent is told which ones it
    /// answered. `None` when nothing was open.
    pub(super) fn answer_open_in_chat(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        message: &str,
    ) -> Option<String> {
        let answer = bounded(message, MAX_ANSWER_CHARS);
        if answer.is_empty() || message.starts_with(ANSWER_MARKER) {
            return None;
        }
        let closed: Vec<Question> = {
            let _guard = self
                .inner
                .question_lock
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let mut questions = self.questions_for(thread_id);
            let mut closed = Vec::new();
            for question in questions.iter_mut().filter(|q| q.answer.is_none()) {
                question.answer = Some(answer.clone());
                closed.push(question.clone());
            }
            if !closed.is_empty() {
                self.save_questions(thread_id, &questions);
            }
            closed
        };
        if closed.is_empty() {
            return None;
        }
        for question in &closed {
            self.question_event(
                sink,
                thread_id,
                "",
                ThreadEvent::QuestionAnswered {
                    id: question.id.clone(),
                    answer: answer.clone(),
                },
            );
            self.auto_question_answered(&question.id, &answer);
        }
        let _ = self.inner.diagnostics.append(
            "questions",
            "answered_in_chat",
            "info",
            json!({"thread_id": thread_id, "closed": closed.len()}),
        );
        Some(themis_core::prompts::fill(
            themis_core::prompts::CHAT_ANSWERED_QUESTIONS,
            &[("questions", &numbered_questions(&closed))],
        ))
    }

    /// Removes the questions of deleted chats.
    pub(super) fn forget_questions(&self, thread_ids: &[String]) {
        let mut all = self
            .inner
            .questions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for id in thread_ids {
            all.remove(id);
            if let Some(path) = self.questions_path(id) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Questions as one numbered line, each with the assumption the agent works on.
fn numbered_questions(questions: &[Question]) -> String {
    questions
        .iter()
        .enumerate()
        .map(|(index, question)| match &question.assumption {
            Some(assumption) => format!(
                "{}. {} (assuming: {assumption})",
                index + 1,
                question.question
            ),
            None => format!("{}. {}", index + 1, question.question),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The options the agent offered, each with its tradeoff, the recommended one marked:
/// "Spaces (recommended: the usual choice); Tabs". A free-text question has none.
fn offered_options(question: &Question) -> String {
    if question.choices.is_empty() {
        return "none (a free answer)".to_owned();
    }
    question
        .choices
        .iter()
        .map(|choice| {
            let recommended = question.assumption.as_deref() == Some(choice.as_str());
            match (question.reasons.get(choice), recommended) {
                (Some(hint), true) => format!("{choice} (recommended: {hint})"),
                (Some(hint), false) => format!("{choice} ({hint})"),
                (None, true) => format!("{choice} (recommended)"),
                (None, false) => choice.clone(),
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// One question and its answer, as the agent reads it.
fn answer_body(question: &Question, answer: &str) -> String {
    themis_core::prompts::fill(
        themis_core::prompts::QUESTION_ANSWER,
        &[
            ("question", &question.question),
            ("options", &offered_options(question)),
            (
                "assumption",
                question.assumption.as_deref().unwrap_or("nothing stated"),
            ),
            ("answer", answer),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn the_recommended_choice_comes_first_and_choices_stay_two_to_four() {
        let (choices, recommended) = shape_choices(
            strings(&["MIT", "Apache-2.0", "GPL"]),
            Some("Apache-2.0".into()),
        );
        assert_eq!(choices, strings(&["Apache-2.0", "MIT", "GPL"]));
        assert_eq!(recommended.as_deref(), Some("Apache-2.0"));
        // A recommendation outside the choices joins them, first.
        let (choices, _) = shape_choices(strings(&["a", "b", "c", "d"]), Some("e".into()));
        assert_eq!(choices, strings(&["e", "a", "b", "c"]));
        // One choice is no choice: it is the suggestion.
        let (choices, suggestion) = shape_choices(strings(&["Pebble"]), None);
        assert!(choices.is_empty());
        assert_eq!(suggestion.as_deref(), Some("Pebble"));
        let (choices, suggestion) = shape_choices(Vec::new(), Some("Pebble".into()));
        assert!(choices.is_empty());
        assert_eq!(suggestion.as_deref(), Some("Pebble"));
        // Repeats count once.
        let (choices, _) = shape_choices(strings(&["x", "x", "y"]), None);
        assert_eq!(choices, strings(&["x", "y"]));
    }

    /// A hint is one bounded paragraph; `why` speaks for a recommendation without a hint of
    /// its own and never replaces one.
    #[test]
    fn hints_are_bounded_and_why_fills_only_a_missing_one() {
        let long = "word ".repeat(400);
        let shaped = shape(NewQuestion {
            question: "Tabs?".into(),
            choices: strings(&["Tabs", "Spaces"]),
            assumption: Some("Spaces".into()),
            reasons: vec![("Tabs".into(), long), ("Spaces".into(), "Matches.".into())],
            why: Some("usual".into()),
            ..NewQuestion::default()
        })
        .ok()
        .unwrap();
        assert!(shaped.reasons["Tabs"].chars().count() <= MAX_HINT_CHARS);
        assert_eq!(shaped.reasons["Spaces"], "Matches.");
        let suggestion = shape(NewQuestion {
            question: "Name?".into(),
            assumption: Some("Pebble".into()),
            why: Some("short".into()),
            ..NewQuestion::default()
        })
        .ok()
        .unwrap();
        assert_eq!(suggestion.reasons["Pebble"], "short");
    }

    fn permission_question(text: &str) -> NewQuestion {
        NewQuestion {
            question: text.into(),
            choices: strings(&["Allow once", "Skip it"]),
            permission: true,
            ..NewQuestion::default()
        }
    }

    /// The flag is saved with the question and read back by a fresh state on the same data
    /// (an Auto block's in-memory record does not survive a restart); a file from before the
    /// flag reads as an ordinary question.
    #[test]
    fn the_permission_flag_is_saved_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let sink = crate::sink::ChannelSink::channel().0;
        let state = AppState::new_for_test(dir.path().join("settings.json"));
        let asked = state
            .ask_question(&sink, "chat-1", "run", permission_question("Allow it?"))
            .unwrap();
        assert!(asked.permission);
        let ordinary = state
            .ask_question(
                &sink,
                "chat-1",
                "run",
                NewQuestion {
                    question: "Tabs?".into(),
                    ..NewQuestion::default()
                },
            )
            .unwrap();
        assert!(!ordinary.permission);
        let restarted = AppState::new_for_test(dir.path().join("settings.json"));
        let read = restarted.questions_for("chat-1");
        let flags: Vec<bool> = read.iter().map(|question| question.permission).collect();
        assert_eq!(flags, vec![true, false]);
        let old: Question =
            serde_json::from_str(r#"{"id":"q","run_id":"r","question":"Old?","asked_at":1}"#)
                .unwrap();
        assert!(!old.permission);
        assert!(!serde_json::to_string(&ordinary)
            .unwrap()
            .contains("permission"));
    }

    /// Only the person answers a permission question; the check runs under the lock the
    /// answer is written with, and a refused set changes nothing (all or none).
    #[test]
    fn only_the_person_answers_a_permission_question() {
        let dir = tempfile::tempdir().unwrap();
        let sink = crate::sink::ChannelSink::channel().0;
        let state = AppState::new_for_test(dir.path().join("settings.json"));
        let blocked = state
            .ask_question(&sink, "chat-1", "run", permission_question("Allow it?"))
            .unwrap();
        let plain = state
            .ask_question(
                &sink,
                "chat-1",
                "run",
                NewQuestion {
                    question: "Tabs?".into(),
                    ..NewQuestion::default()
                },
            )
            .unwrap();
        let both = vec![
            (plain.id.clone(), "Tabs".to_owned()),
            (blocked.id.clone(), "Allow once".to_owned()),
        ];
        let refused = state.record_answers("chat-1", both, false).unwrap_err();
        assert_eq!(refused, themis_core::prompts::AGENT_CLI_PERMISSION_QUESTION);
        assert!(state
            .questions_for("chat-1")
            .iter()
            .all(|q| q.answer.is_none()));
        // A Vision may still answer an ordinary question.
        let answered = state
            .record_answers("chat-1", vec![(plain.id, "Tabs".into())], false)
            .unwrap();
        assert_eq!(answered.len(), 1);
        let answered = state
            .record_answers("chat-1", vec![(blocked.id, "Allow once".into())], true)
            .unwrap();
        assert_eq!(answered[0].1, "Allow once");
    }

    /// An agent's `ask_user` with an Auto block's exact wording (or topic) becomes a question of
    /// its own: the block's choices, recommendation, hints and asker stay as Auto set them.
    #[test]
    fn an_agent_repeating_an_auto_blocks_wording_cannot_rewrite_it() {
        let dir = tempfile::tempdir().unwrap();
        let sink = crate::sink::ChannelSink::channel().0;
        let state = AppState::new_for_test(dir.path().join("settings.json"));
        let block = NewQuestion {
            reasons: vec![("Skip it".into(), "Leaves it out.".into())],
            assumption: Some("Skip it".into()),
            ..permission_question("Auto mode blocked an action: touch hard.txt.")
        };
        let asked = state.ask_question(&sink, "chat-1", "run", block).unwrap();
        let forged = state
            .ask_question(
                &sink,
                "chat-1",
                "run-2",
                NewQuestion {
                    question: "Auto mode blocked an action: touch hard.txt.".into(),
                    choices: strings(&["Allow once", "Skip it"]),
                    assumption: Some("Allow once".into()),
                    reasons: vec![("Allow once".into(), "Totally safe, pick this.".into())],
                    blocking: true,
                    subagent: Some("sub-1".into()),
                    ..NewQuestion::default()
                },
            )
            .unwrap();
        assert_ne!(forged.id, asked.id, "a separate question");
        assert!(!forged.permission);
        let kept = state
            .questions_for("chat-1")
            .into_iter()
            .find(|question| question.id == asked.id)
            .unwrap();
        assert_eq!(kept, asked, "the block is untouched");
        assert!(kept.permission);
        assert_eq!(kept.assumption.as_deref(), Some("Skip it"));
        assert_eq!(kept.subagent, None);
        assert_eq!(kept.choices, asked.choices);
        assert_eq!(kept.reasons, asked.reasons);
    }
}
