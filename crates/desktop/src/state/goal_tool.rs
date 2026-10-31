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
        let pause = args["pause"].as_bool() == Some(true);
        let goal = args["goal"].as_str().filter(|goal| !goal.trim().is_empty());
        let remove = args["remove"].as_bool() == Some(true);
        if goal.is_none() && !pause && !remove {
            return Err(error("set_goal needs `goal`: the goal in one sentence"));
        }
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
        if (pause || remove) && !self.state.person_spoke_since_goal_check(&self.thread_id) {
            let verb = if remove { "removing" } else { "pausing" };
            return Err(error(format!(
                "set_goal: {verb} is refused because the person has not asked; keep working toward the goal"
            )));
        }
        if remove {
            self.state
                .remove_goal(Arc::clone(&self.sink), self.thread_id.clone())
                .await
                .map_err(error)?;
            return Ok(json!({"status": "removed"}));
        }
        if pause {
            self.state
                .pause_goal(Arc::clone(&self.sink), self.thread_id.clone())
                .await
                .map_err(error)?;
            return Ok(json!({"status": "paused"}));
        }
        let goal = self
            .state
            .set_goal(
                Arc::clone(&self.sink),
                self.thread_id.clone(),
                goal.unwrap_or_default().to_owned(),
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
        "Set this chat's goal: one sentence the runtime keeps working toward, turn after turn, until a separate check finds it reached. Call it only after the user agreed to make their request a goal (when a request clearly needs many rounds of work toward a measurable or judgeable outcome, first ask once, in one line: \"Want me to make this a goal and keep working until it's reached?\"). Include any conditions the user added. Never call it for questions or one-step changes. Replaces an existing goal. When the person asks to stop or pause the goal, pass pause: true instead; when they ask to remove, drop or cancel it, pass remove: true. Say it is done only after that succeeds; both are refused unless they asked."
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{"goal":{"type":"string","description":"The goal in one sentence, in the user's terms."},"pause":{"type":"boolean","description":"True to pause this chat's goal because the person asked."},"remove":{"type":"boolean","description":"True to remove this chat's goal because the person asked."}},"additionalProperties":false})
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
}
