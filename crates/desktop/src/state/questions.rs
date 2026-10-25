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
const MAX_CHOICES: usize = 6;
const MAX_CHOICE_CHARS: usize = 200;
const MAX_TOPIC_CHARS: usize = 80;
const MAX_ANSWER_CHARS: usize = 10_000;

/// Starts the message that delivers answers; the goal supervisor reads it back
/// to tell an answer from an ordinary reply. Never change it.
pub(crate) const ANSWER_MARKER: &str = "[Answer to your question — from the user]";

/// What the agent asked, before it is stored.
#[derive(Debug, Clone, Default)]
pub(super) struct NewQuestion {
    pub question: String,
    pub choices: Vec<String>,
    pub assumption: Option<String>,
    pub blocking: bool,
    pub topic: Option<String>,
}

/// Why a question was not asked.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum AskRefused {
    Empty,
    TooMany,
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
            .take(MAX_CHOICES)
            .collect();
        let assumption = asked
            .assumption
            .map(|text| bounded(&text, MAX_ASSUMPTION_CHARS))
            .filter(|text| !text.is_empty());
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
                question.blocking = asked.blocking;
                question.topic = topic.or(question.topic.take());
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
                    blocking: asked.blocking,
                    topic,
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
                (
                    "assumption",
                    question.assumption.as_deref().unwrap_or("nothing stated"),
                ),
                ("answer", answer),
            ],
        );
        // Under the queue lock, like a steer: closing the inbox takes the same
        // lock, so the answer lands in a live inbox or in the queue.
        self.edit_queue(sink, thread_id, |queue| {
            let inboxes = self
                .inner
                .steer_inboxes
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some((inbox, _)) = inboxes.get(thread_id) {
                inbox
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
        if open.is_empty() {
            return None;
        }
        let list = open
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
            .join("; ");
        Some(themis_core::prompts::fill(
            themis_core::prompts::GOAL_OPEN_QUESTIONS,
            &[("questions", &list)],
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
