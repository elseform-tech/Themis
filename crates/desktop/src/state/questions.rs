//! Questions the agent asks the user without stopping its work.
//!
//! The agent asks with `ask_user` (see `question_tool`), saying what it will
//! assume meanwhile, and carries on. A chat's questions live in
//! `questions/<chat id>.json` beside `threads.json`; the transcript records
//! each one and its answer so the chat shows them. An answer is delivered like
//! a steer when a run is going (a steer the run never reads returns to the
//! queue), otherwise it goes out first as the next message, so it is never
//! lost. Only a blocking question ends the run that asked it.

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
/// A reason says what picking that answer does (steps, commands, files) and its tradeoff:
/// long enough for that, short enough to scan.
const MAX_REASON_CHARS: usize = 400;
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
    /// One short line per answer on its tradeoff (`why` on a choice), and why the
    /// recommendation is recommended (the top-level `why`).
    pub reasons: Vec<(String, String)>,
    pub why: Option<String>,
    pub blocking: bool,
    pub topic: Option<String>,
    /// The subagent asking, by id: its answer goes to that subagent.
    pub subagent: Option<String>,
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
        // Reasons only for answers on offer; the recommendation's own `why` wins.
        let offered = |answer: &str| {
            choices.iter().any(|c| c == answer) || assumption.as_deref() == Some(answer)
        };
        let mut reasons = std::collections::BTreeMap::new();
        for (answer, why) in asked.reasons {
            let (answer, why) = (
                bounded(&answer, MAX_CHOICE_CHARS),
                bounded(&why, MAX_REASON_CHARS),
            );
            if offered(&answer) && !why.is_empty() {
                reasons.insert(answer, why);
            }
        }
        if let (Some(answer), Some(why)) = (
            &assumption,
            asked.why.map(|why| bounded(&why, MAX_REASON_CHARS)),
        ) {
            if !why.is_empty() {
                reasons.insert(answer.clone(), why);
            }
        }
        let _guard = self
            .inner
            .question_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut questions = self.questions_for(thread_id);
        let same = questions.iter().position(|question| {
            question.answer.is_none()
                && (topic.is_some() && question.topic == topic || question.question == text)
        });
        let question = match same {
            Some(index) => {
                let question = &mut questions[index];
                question.question = text;
                question.choices = choices;
                question.assumption = assumption;
                question.reasons = reasons;
                question.blocking = asked.blocking;
                question.topic = topic.or(question.topic.take());
                question.subagent = asked.subagent;
                question.run_id = run_id.to_owned();
                question.clone()
            }
            None => {
                if questions.iter().filter(|q| q.answer.is_none()).count() >= MAX_OPEN {
                    return Err(AskRefused::TooMany);
                }
                let question = Question {
                    id: uuid::Uuid::new_v4().to_string(),
                    run_id: run_id.to_owned(),
                    question: text,
                    choices,
                    assumption,
                    reasons,
                    blocking: asked.blocking,
                    topic,
                    subagent: asked.subagent,
                    asked_at: now_ms(),
                    answer: None,
                };
                questions.push(question.clone());
                // Keep the newest; the oldest answered questions go first.
                while questions.len() > MAX_KEPT {
                    let Some(oldest) = questions.iter().position(|q| q.answer.is_some()) else {
                        break;
                    };
                    questions.remove(oldest);
                }
                question
            }
        };
        self.save_questions(thread_id, &questions);
        let _ = self.inner.diagnostics.append(
            "questions",
            if same.is_some() { "updated" } else { "asked" },
            "info",
            json!({"thread_id": thread_id, "question_id": question.id, "blocking": question.blocking}),
        );
        self.question_event(
            sink,
            thread_id,
            run_id,
            ThreadEvent::QuestionAsked {
                question: question.clone(),
            },
        );
        Ok(question)
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
        let running = self.require_thread(&thread_id).await?;
        let answer = answer.trim().to_owned();
        if answer.is_empty() {
            return Err("Write an answer".to_owned());
        }
        if answer.chars().count() > MAX_ANSWER_CHARS {
            return Err("This answer is too long".to_owned());
        }
        let question = {
            let _guard = self
                .inner
                .question_lock
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let mut questions = self.questions_for(&thread_id);
            let question = questions
                .iter_mut()
                .find(|question| question.id == question_id)
                .ok_or_else(|| "That question is no longer in this chat".to_owned())?;
            if question.answer.is_some() {
                return Err("That question is already answered".to_owned());
            }
            question.answer = Some(answer.clone());
            let question = question.clone();
            self.save_questions(&thread_id, &questions);
            question
        };
        self.question_event(
            sink.as_ref(),
            &thread_id,
            "",
            ThreadEvent::QuestionAnswered {
                id: question.id.clone(),
                answer: answer.clone(),
            },
        );
        // An "Allow once" to an Auto block lets that exact action run once.
        self.auto_question_answered(&question.id, &answer);
        let steered = self.deliver_answer(sink.as_ref(), &thread_id, &question, &answer)?;
        let _ = self.inner.diagnostics.append(
            "questions",
            "answered",
            "info",
            json!({"thread_id": thread_id, "question_id": question.id, "delivery": if steered { "run" } else { "queue" }}),
        );
        if !running || !steered {
            self.dispatch_queue(sink, &thread_id).await;
        }
        Ok(question)
    }

    /// Hands the answer to the running run, or puts it first in the queue
    /// (joining an answer already waiting there). Returns whether a run took it.
    fn deliver_answer(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        question: &Question,
        answer: &str,
    ) -> Result<bool, String> {
        let body = themis_core::prompts::fill(
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
        );
        // The subagent that asked gets it while it is running; once it has ended, the main
        // agent does, like any answer.
        if let Some(agent) = question
            .subagent
            .as_deref()
            .and_then(|id| self.running_subagent(thread_id, id))
        {
            agent
                .inbox
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_back(format!("{ANSWER_MARKER}\n{body}"));
            return Ok(true);
        }
        // Under the queue lock, like a steer: closing the inbox takes the same
        // lock, so the answer lands in a live inbox or in the queue.
        self.edit_queue(sink, thread_id, |queue| {
            let inboxes = self
                .inner
                .steer_inboxes
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = inboxes.get(thread_id) {
                entry
                    .inbox
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push_back(format!("{ANSWER_MARKER}\n{body}"));
                return Ok(true);
            }
            match queue.first_mut() {
                Some(first) if !first.held && first.text.starts_with(ANSWER_MARKER) => {
                    first.text.push_str("\n\n");
                    first.text.push_str(&body);
                }
                _ => queue.insert(
                    0,
                    QueuedMessage {
                        id: uuid::Uuid::new_v4().to_string(),
                        text: format!("{ANSWER_MARKER}\n{body}"),
                        attachments: Vec::new(),
                        reasoning_effort: None,
                        held: false,
                        note: None,
                    },
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
    pub(super) fn with_still_open(&self, thread_id: &str, asked_id: &str, note: String) -> String {
        let others: Vec<_> = self
            .open_questions(thread_id)
            .into_iter()
            .filter(|open| open.id != asked_id)
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
                (Some(reason), true) => format!("{choice} (recommended: {reason})"),
                (Some(reason), false) => format!("{choice} ({reason})"),
                (None, true) => format!("{choice} (recommended)"),
                (None, false) => choice.clone(),
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
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
}
