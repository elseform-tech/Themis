//! The agent's `ask_user` tool: a question to the user that does not stop the
//! work. Like `create_goal` it needs no approval prompt, but a deny rule still
//! wins. A blocking question ends the run after this tool batch. One call may
//! carry up to [`MAX_PAGES`] questions, shown as pages of one box.

use std::sync::Arc;

use serde_json::{json, Value};
use themis_core::configuration::{ApprovalMode, PolicyAction};
use themis_core::tools::{async_trait, RiskLevel, ToolAction, ToolCallError, ToolRuntime, ToolT};

use super::integration_tools::RunApproval;
use super::questions::{AskRefused, NewQuestion, MAX_OPEN};
use super::AppState;
use crate::sink::EventSink;
use crate::types::Question;

/// Most questions one `ask_user` call may carry: the pages of one box.
pub(super) const MAX_PAGES: usize = 4;
/// The shortest hint taken, in characters: shorter ones rarely say what happens, what it costs
/// and what follows.
pub(super) const MIN_HINT_CHARS: usize = 200;

pub(super) struct QuestionTool {
    pub(super) state: AppState,
    pub(super) sink: Arc<dyn EventSink>,
    pub(super) thread_id: String,
    pub(super) run_id: String,
    pub(super) run: RunApproval,
    /// Set for a blocking question: the run ends after this tool batch.
    pub(super) end: themis_core::runtime::EndRequest,
    /// The subagent asking, by id and name. It keeps working: only the chat's own run can end on
    /// a question.
    pub(super) subagent: Option<(String, String)>,
}

impl std::fmt::Debug for QuestionTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuestionTool").finish_non_exhaustive()
    }
}

fn error(message: impl ToString) -> ToolCallError {
    ToolCallError::RuntimeError(message.to_string().into())
}

/// The questions of one call: `questions`, or the single question at the top level.
fn pages(args: &Value) -> Result<Vec<&Value>, String> {
    let refused = || {
        themis_core::prompts::fill(
            themis_core::prompts::ASK_USER_PAGES,
            &[("max", &MAX_PAGES.to_string())],
        )
    };
    match &args["questions"] {
        Value::Null => Ok(vec![args]),
        Value::Array(list)
            if !list.is_empty() && list.len() <= MAX_PAGES && args.get("question").is_none() =>
        {
            Ok(list.iter().collect())
        }
        _ => Err(refused()),
    }
}

/// One choice: its answer and its hint (one paragraph on what picking it means, at least
/// [`MIN_HINT_CHARS`] characters), or why it is refused (naming each missing field, or the
/// hint's length). A bare string is a choice without a hint.
fn parse_choice(index: usize, choice: &Value) -> Result<(String, String), String> {
    let field = |key: &str| {
        choice
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
    };
    let answer = choice.as_str().map(str::trim).or_else(|| field("answer"));
    let missing: Vec<String> = answer
        .filter(|answer| !answer.is_empty())
        .map_or(&["answer"][..], |_| &[])
        .iter()
        .chain(["hint"].iter().filter(|key| field(key).is_none()))
        .map(|key| format!("`{key}`"))
        .collect();
    if !missing.is_empty() {
        let label = answer.map_or_else(String::new, |answer| format!(" (\"{answer}\")"));
        return Err(themis_core::prompts::fill(
            themis_core::prompts::ASK_USER_CHOICE_INCOMPLETE,
            &[
                ("choice", &format!("{}{label}", index + 1)),
                ("missing", &missing.join(", ")),
            ],
        ));
    }
    let answer = answer.unwrap_or_default();
    let hint = field("hint").unwrap_or_default();
    let chars = hint.chars().count();
    if chars < MIN_HINT_CHARS {
        return Err(themis_core::prompts::fill(
            themis_core::prompts::ASK_USER_HINT_THIN,
            &[
                ("choice", &format!("{} (\"{answer}\")", index + 1)),
                ("chars", &chars.to_string()),
                ("min", &MIN_HINT_CHARS.to_string()),
            ],
        ));
    }
    Ok((answer.to_owned(), hint.to_owned()))
}

/// One question as the model sent it, or why it is refused.
fn parse(args: &Value) -> Result<NewQuestion, String> {
    let text = |key: &str| args[key].as_str().map(str::to_owned);
    let hints = args["choices"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, choice)| parse_choice(index, choice))
        .collect::<Result<Vec<_>, _>>()?;
    // `assumption` was the name before `recommended`; either is read.
    let recommended = text("recommended").or_else(|| text("assumption"));
    Ok(NewQuestion {
        question: text("question").unwrap_or_default(),
        choices: hints.iter().map(|(answer, _)| answer.clone()).collect(),
        reasons: hints,
        why: text("why"),
        // A question the agent can recommend an answer to never stops the work: it works
        // on that answer meanwhile. Only one it cannot answer at all may block.
        blocking: args["blocking"].as_bool().unwrap_or(false)
            && recommended
                .as_deref()
                .is_none_or(|answer| answer.trim().is_empty()),
        assumption: recommended,
        topic: text("topic"),
        subagent: None,
    })
}

impl QuestionTool {
    /// One page checked for asking: a subagent never blocks, and a question that does not
    /// block says what the agent recommends and why.
    fn check(&self, page: &Value) -> Result<NewQuestion, String> {
        let mut asked = parse(page)?;
        if asked.question.trim().is_empty() {
            return Err("ask_user needs `question`: the question to the user".to_owned());
        }
        if let Some((id, name)) = &self.subagent {
            if asked.blocking || page["recommended"].as_str().is_none() {
                return Err("ask_user: a subagent can't stop to wait for an answer: give `recommended` and `why`, and carry on with that".to_owned());
            }
            asked.question = format!("{name}: {}", asked.question);
            asked.subagent = Some(id.clone());
        }
        if asked.blocking {
            return Ok(asked);
        }
        let Some(recommended) = asked.assumption.as_deref().filter(|a| !a.trim().is_empty()) else {
            return Err("ask_user needs `recommended`: your recommended answer (one of the choices, or your suggestion), which you work on until the user answers (or set `blocking` when nothing more can be done without the answer)".to_owned());
        };
        let explained = asked
            .why
            .as_deref()
            .is_some_and(|why| !why.trim().is_empty())
            || asked
                .reasons
                .iter()
                .any(|(answer, _)| answer == recommended);
        if !explained {
            return Err(
                "ask_user needs `why`: one short line on why you recommend that answer".to_owned(),
            );
        }
        Ok(asked)
    }

    /// Every page checked, or the first refusal (naming its page when there are several).
    fn check_all(&self, args: &Value) -> Result<Vec<NewQuestion>, String> {
        let pages = pages(args)?;
        let several = pages.len() > 1;
        pages
            .into_iter()
            .enumerate()
            .map(|(index, page)| {
                self.check(page).map_err(|refused| {
                    if several {
                        format!("question {}: {refused}", index + 1)
                    } else {
                        refused
                    }
                })
            })
            .collect()
    }

    fn denied_by_rule(&self) -> bool {
        let action = ToolAction {
            display: None,
            command: Vec::new(),
            tool: "ask_user".to_owned(),
            summary: "ask_user".to_owned(),
            risk: RiskLevel::Read,
        };
        // YOLO keeps a deny rule someone wrote, like every other tool.
        let decision = if self.run.mode == ApprovalMode::Yolo {
            self.run.policy.yolo_decision(&action)
        } else {
            self.run.policy.decision(&action)
        };
        decision == PolicyAction::Deny
    }

    /// The result's note: the run ends when any page blocks; otherwise the agent works on
    /// its recommendations.
    fn note(&self, questions: &[Question]) -> String {
        if questions.iter().any(|question| question.blocking) {
            *self.end.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(themis_core::prompts::ASK_USER_WAITING.to_owned());
            return themis_core::prompts::ASK_USER_BLOCKING.to_owned();
        }
        let assumption = match questions {
            [question] => question.assumption.clone().unwrap_or_default(),
            _ => questions
                .iter()
                .map(|question| {
                    format!(
                        "{}: {}",
                        question.question,
                        question.assumption.as_deref().unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("; "),
        };
        themis_core::prompts::fill(
            themis_core::prompts::ASK_USER_ASKED,
            &[("assumption", &assumption)],
        )
    }
}

#[async_trait]
impl ToolRuntime for QuestionTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let asked = self.check_all(&args).map_err(error)?;
        if self.denied_by_rule() {
            return Err(error(
                "approval denied for 'ask_user': the approval rules deny asking the user",
            ));
        }
        let questions =
            match self
                .state
                .ask_questions(self.sink.as_ref(), &self.thread_id, &self.run_id, asked)
            {
                Ok(questions) => questions,
                Err(AskRefused::Empty) => {
                    return Err(error("ask_user needs `question`: the question to the user"))
                }
                Err(AskRefused::TooMany) => {
                    return Ok(json!({
                        "asked": false,
                        "note": themis_core::prompts::fill(
                            themis_core::prompts::ASK_USER_LIMIT,
                            &[("max", &MAX_OPEN.to_string())],
                        )
                    }))
                }
            };
        let ids: Vec<&str> = questions
            .iter()
            .map(|question| question.id.as_str())
            .collect();
        let note = self.note(&questions);
        let note = self.state.with_still_open(&self.thread_id, &ids, note);
        let blocking = questions.iter().any(|question| question.blocking);
        Ok(match ids.as_slice() {
            [id] => json!({"asked": true, "id": id, "blocking": blocking, "note": note}),
            _ => json!({"asked": true, "ids": ids, "blocking": blocking, "note": note}),
        })
    }
}

/// One question's fields, at the top level or as an item of `questions`.
fn question_properties() -> Value {
    json!({
        "question": {"type": "string"},
        "choices": {"type": "array", "minItems": 2, "maxItems": 4, "items": {
            "type": "object",
            "properties": {
                "answer": {"type": "string"},
                "hint": {"type": "string"}
            },
            "required": ["answer", "hint"],
            "additionalProperties": false
        }},
        "recommended": {"type": "string"},
        "why": {"type": "string"},
        "blocking": {"type": "boolean"},
        "topic": {"type": "string"}
    })
}

impl ToolT for QuestionTool {
    fn name(&self) -> &str {
        "ask_user"
    }
    fn description(&self) -> &str {
        themis_core::prompts::ASK_USER_DESCRIPTION
    }
    fn args_schema(&self) -> Value {
        let mut properties = question_properties();
        properties["questions"] = json!({
            "type": "array",
            "minItems": 1,
            "maxItems": MAX_PAGES,
            "items": {
                "type": "object",
                "properties": question_properties(),
                "required": ["question"],
                "additionalProperties": false
            }
        });
        json!({"type": "object", "properties": properties, "additionalProperties": false})
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
}

#[cfg(test)]
mod tests;
