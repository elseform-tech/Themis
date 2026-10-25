//! The agent's `set_goal` tool: after the user agrees to make a request a
//! goal, the agent sets it for its own chat. Like the own-chat automation
//! lifecycle it needs no approval prompt, but a deny rule still wins.

use std::sync::Arc;

use serde_json::{json, Value};
use themis_core::configuration::{ApprovalMode, PolicyAction};
use themis_core::tools::{async_trait, RiskLevel, ToolAction, ToolCallError, ToolRuntime, ToolT};

use super::integration_tools::RunApproval;
use super::AppState;
use crate::sink::EventSink;

pub(super) struct GoalTool {
    pub(super) state: AppState,
    pub(super) sink: Arc<dyn EventSink>,
    pub(super) thread_id: String,
    pub(super) run: RunApproval,
}

impl std::fmt::Debug for GoalTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoalTool").finish_non_exhaustive()
    }
}

fn error(message: impl ToString) -> ToolCallError {
    ToolCallError::RuntimeError(message.to_string().into())
}

#[async_trait]
impl ToolRuntime for GoalTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let goal = args["goal"]
            .as_str()
            .ok_or_else(|| error("set_goal needs `goal`: the goal in one sentence"))?;
        let action = ToolAction {
            display: None,
            command: Vec::new(),
            tool: "set_goal".to_owned(),
            summary: "set_goal".to_owned(),
            risk: RiskLevel::Write,
        };
        let decision = if self.run.mode == ApprovalMode::Yolo {
            self.run.policy.yolo_decision(&action)
        } else {
            self.run.policy.decision(&action)
        };
        let _ = self.state.inner.diagnostics.append(
            "approval",
            "auto_decided",
            "info",
            json!({"thread_id": self.thread_id, "tool": "set_goal", "decision": format!("{decision:?}"), "reason": "auto: own-chat goal"}),
        );
        if decision == PolicyAction::Deny {
            return Err(error(
                "approval denied for 'set_goal': the approval rules deny setting goals",
            ));
        }
        let goal = self
            .state
            .set_goal(
                Arc::clone(&self.sink),
                self.thread_id.clone(),
                goal.to_owned(),
            )
            .await
            .map_err(error)?;
        Ok(json!({
            "goal": goal.text,
            "status": "running",
            "note": themis_core::prompts::GOAL_SET_NOTE
        }))
    }
}

impl ToolT for GoalTool {
    fn name(&self) -> &str {
        "set_goal"
    }
    fn description(&self) -> &str {
        "Set this chat's goal: one sentence the runtime keeps working toward, turn after turn, until a separate check finds it reached. Call it only after the user agreed to make their request a goal (when a request clearly needs many rounds of work toward a measurable or judgeable outcome, first ask once, in one line: \"Want me to make this a goal and keep working until it's reached?\"). Include any conditions the user added. Never call it for questions or one-step changes. Replaces an existing goal."
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{"goal":{"type":"string","description":"The goal in one sentence, in the user's terms."}},"required":["goal"],"additionalProperties":false})
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
}
