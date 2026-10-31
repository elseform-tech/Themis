//! The agent's `ask_user` tool: a question to the user that does not stop the
//! work. Like `set_goal` it needs no approval prompt, but a deny rule still
//! wins. A blocking question ends the run after this tool batch.

use std::sync::Arc;

use serde_json::{json, Value};
use themis_core::configuration::{ApprovalMode, PolicyAction};
use themis_core::tools::{async_trait, RiskLevel, ToolAction, ToolCallError, ToolRuntime, ToolT};

use super::integration_tools::RunApproval;
use super::questions::{AskRefused, NewQuestion, MAX_OPEN};
use super::AppState;
use crate::sink::EventSink;

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

/// The question as the model sent it.
fn parse(args: &Value) -> NewQuestion {
    let text = |key: &str| args[key].as_str().map(str::to_owned);
    let choices: Vec<(String, Option<String>)> = args["choices"]
        .as_array()
        .map(|choices| {
            choices
                .iter()
                .filter_map(|choice| match choice {
                    Value::String(answer) => Some((answer.clone(), None)),
                    Value::Object(_) => Some((
                        choice["answer"].as_str()?.to_owned(),
                        choice["why"].as_str().map(str::to_owned),
                    )),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    NewQuestion {
        question: text("question").unwrap_or_default(),
        // A choice is its answer, or `{"answer", "why"}` with a one-line tradeoff.
        choices: choices.iter().map(|(answer, _)| answer.clone()).collect(),
        reasons: choices
            .into_iter()
            .filter_map(|(answer, why)| Some((answer, why?)))
            .collect(),
        why: text("why"),
        // `assumption` was the name before `recommended`; either is read.
        assumption: text("recommended").or_else(|| text("assumption")),
        // A question the agent can recommend an answer to never stops the work: it works
        // on that answer meanwhile. Only one it cannot answer at all may block.
        subagent: None,
        blocking: args["blocking"].as_bool().unwrap_or(false)
            && text("recommended")
                .or_else(|| text("assumption"))
                .is_none_or(|answer| answer.trim().is_empty()),
        topic: text("topic"),
    }
}

#[async_trait]
impl ToolRuntime for QuestionTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let mut asked = parse(&args);
        if let Some((id, name)) = &self.subagent {
            if asked.blocking || args["recommended"].as_str().is_none() {
                return Err(error(
                    "ask_user: a subagent can't stop to wait for an answer: give `recommended` and `why`, and carry on with that",
                ));
            }
            asked.question = format!("{name}: {}", asked.question);
            asked.subagent = Some(id.clone());
        }
        if !asked.blocking {
            let Some(recommended) = asked.assumption.as_deref().filter(|a| !a.trim().is_empty())
            else {
                return Err(error(
                    "ask_user needs `recommended`: your recommended answer (one of the choices, or your suggestion), which you work on until the user answers (or set `blocking` when nothing more can be done without the answer)",
                ));
            };
            let explained = asked
                .why
                .as_deref()
                .is_some_and(|why| !why.trim().is_empty())
                || asked
                    .reasons
                    .iter()
                    .any(|(answer, why)| answer == recommended && !why.trim().is_empty());
            if !explained {
                return Err(error(
                    "ask_user needs `why`: one short line on why you recommend that answer",
                ));
            }
        }
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
        if decision == PolicyAction::Deny {
            return Err(error(
                "approval denied for 'ask_user': the approval rules deny asking the user",
            ));
        }
        let question =
            match self
                .state
                .ask_question(self.sink.as_ref(), &self.thread_id, &self.run_id, asked)
            {
                Ok(question) => question,
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
        let note = if question.blocking {
            *self.end.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(themis_core::prompts::ASK_USER_WAITING.to_owned());
            themis_core::prompts::ASK_USER_BLOCKING.to_owned()
        } else {
            themis_core::prompts::fill(
                themis_core::prompts::ASK_USER_ASKED,
                &[(
                    "assumption",
                    question.assumption.as_deref().unwrap_or_default(),
                )],
            )
        };
        Ok(json!({"asked": true, "id": question.id, "blocking": question.blocking, "note": note}))
    }
}

impl ToolT for QuestionTool {
    fn name(&self) -> &str {
        "ask_user"
    }
    fn description(&self) -> &str {
        themis_core::prompts::ASK_USER_DESCRIPTION
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{
            "question":{"type":"string"},
            "choices":{"type":"array","minItems":2,"maxItems":4,"items":{"type":"object","properties":{"answer":{"type":"string"},"why":{"type":"string"}},"required":["answer"]}},
            "recommended":{"type":"string"},
            "why":{"type":"string"},
            "blocking":{"type":"boolean"},
            "topic":{"type":"string"}
        },"required":["question"],"additionalProperties":false})
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_read_leniently() {
        let asked = parse(
            &json!({"question": "Tabs?", "choices": ["Yes", 3, "No"], "recommended": "Yes", "topic": "indent"}),
        );
        assert_eq!(asked.question, "Tabs?");
        assert_eq!(asked.choices, ["Yes", "No"]);
        assert_eq!(asked.assumption.as_deref(), Some("Yes"));
        assert!(!asked.blocking);
        assert_eq!(asked.topic.as_deref(), Some("indent"));
        assert!(parse(&json!({"question": "Key?", "blocking": true})).blocking);
        // With a recommendation it does not block, whatever the model asked for.
        let recommended = parse(
            &json!({"question": "Which analytics?", "choices": ["Product", "Web"], "recommended": "Product", "blocking": true}),
        );
        assert!(!recommended.blocking);
        // A choice may carry a one-line tradeoff; the recommendation its own `why`.
        let explained = parse(
            &json!({"question": "Which analytics?", "choices": [{"answer": "Plausible", "why": "no cookies"}, "PostHog"], "recommended": "Plausible", "why": "simplest"}),
        );
        assert_eq!(explained.choices, ["Plausible", "PostHog"]);
        assert_eq!(
            explained.reasons,
            [("Plausible".to_owned(), "no cookies".to_owned())]
        );
        assert_eq!(explained.why.as_deref(), Some("simplest"));
        // The older name still reads.
        assert_eq!(
            parse(&json!({"question": "Q", "assumption": "A"}))
                .assumption
                .as_deref(),
            Some("A")
        );
    }
}
