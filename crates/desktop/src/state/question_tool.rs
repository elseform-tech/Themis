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
    NewQuestion {
        question: text("question").unwrap_or_default(),
        choices: args["choices"]
            .as_array()
            .map(|choices| {
                choices
                    .iter()
                    .filter_map(|choice| choice.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        assumption: text("assumption"),
        blocking: args["blocking"].as_bool().unwrap_or(false),
        topic: text("topic"),
    }
}

#[async_trait]
impl ToolRuntime for QuestionTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let asked = parse(&args);
        if !asked.blocking
            && asked
                .assumption
                .as_deref()
                .is_none_or(|a| a.trim().is_empty())
        {
            return Err(error(
                "ask_user needs `assumption`: what you will assume and work on until the user answers (or set `blocking` when nothing more can be done without the answer)",
            ));
        }
        let action = ToolAction {
            display: None,
            command: Vec::new(),
            tool: "ask_user".to_owned(),
            summary: "ask_user".to_owned(),
            risk: RiskLevel::Read,
        };
        let decision = if self.run.mode == ApprovalMode::Yolo {
            PolicyAction::Allow
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
            "question":{"type":"string","description":"One question to the user."},
            "choices":{"type":"array","items":{"type":"string"},"description":"Up to six short answers to pick from, when the answer is one of a few options."},
            "assumption":{"type":"string","description":"What you will assume and work on until the user answers."},
            "blocking":{"type":"boolean","description":"True only when nothing more can be done without the answer: the run ends and waits."},
            "topic":{"type":"string","description":"A short key; asking again with an open topic updates that question."}
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
            &json!({"question": "Tabs?", "choices": ["Yes", 3, "No"], "assumption": "Yes", "topic": "indent"}),
        );
        assert_eq!(asked.question, "Tabs?");
        assert_eq!(asked.choices, ["Yes", "No"]);
        assert_eq!(asked.assumption.as_deref(), Some("Yes"));
        assert!(!asked.blocking);
        assert_eq!(asked.topic.as_deref(), Some("indent"));
        assert!(parse(&json!({"question": "Key?", "blocking": true})).blocking);
    }
}
