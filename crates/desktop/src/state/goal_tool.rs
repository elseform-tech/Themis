//! The agent's goal tools. `create_goal` creates the chat's goal when the user asks for one;
//! `update_goal` ends it as complete (with evidence for each requirement) or blocked, or pauses
//! it when the person asked. Like the own-chat automation lifecycle they need no approval
//! prompt, but a deny rule still wins.

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

pub(super) struct UpdateGoalTool {
    pub(super) state: AppState,
    pub(super) sink: Arc<dyn EventSink>,
    pub(super) thread_id: String,
    pub(super) run: RunApproval,
}

impl std::fmt::Debug for UpdateGoalTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateGoalTool").finish_non_exhaustive()
    }
}

fn error(message: impl ToString) -> ToolCallError {
    ToolCallError::RuntimeError(message.to_string().into())
}

/// Refuses `tool` when an approval rule denies it; otherwise it approves itself.
fn denied(
    state: &AppState,
    run: &RunApproval,
    thread_id: &str,
    tool: &str,
) -> Option<ToolCallError> {
    let action = ToolAction {
        display: None,
        command: Vec::new(),
        tool: tool.to_owned(),
        summary: tool.to_owned(),
        risk: RiskLevel::Write,
    };
    let decision = if run.mode == ApprovalMode::Yolo {
        run.policy.yolo_decision(&action)
    } else {
        run.policy.decision(&action)
    };
    let _ = state.inner.diagnostics.append(
        "approval",
        "auto_decided",
        "info",
        json!({"thread_id": thread_id, "tool": tool, "decision": format!("{decision:?}"), "reason": "auto: own-chat goal"}),
    );
    (decision == PolicyAction::Deny).then(|| {
        error(format!(
            "approval denied for '{tool}': the approval rules deny changing goals"
        ))
    })
}

#[async_trait]
impl ToolRuntime for GoalTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        if let Some(refused) = denied(&self.state, &self.run, &self.thread_id, "create_goal") {
            return Err(refused);
        }
        let objective = args["objective"].as_str().unwrap_or_default().to_owned();
        let checklist = themis_core::goal::parse_checklist(&args["checklist"]).map_err(error)?;
        let goal = self
            .state
            .create_goal(
                Arc::clone(&self.sink),
                self.thread_id.clone(),
                objective,
                checklist,
            )
            .await
            .map_err(error)?;
        Ok(json!({
            "objective": goal.text,
            "status": "running",
            "note": themis_core::prompts::GOAL_CREATED_NOTE
        }))
    }
}

impl ToolT for GoalTool {
    fn name(&self) -> &str {
        "create_goal"
    }
    fn description(&self) -> &str {
        themis_core::prompts::CREATE_GOAL_TOOL
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{
            "objective":{"type":"string","description":"The objective in the user's words, with every condition they gave."},
            "checklist":{"type":"array","minItems":1,"maxItems":themis_core::goal::MAX_CHECKLIST,"items":{"type":"string","maxLength":themis_core::goal::MAX_ITEM_CHARS},"description":"One item per concrete thing the user asked for or approved, each a short statement someone could check by using the result."}
        },"required":["objective","checklist"],"additionalProperties":false})
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
}

#[async_trait]
impl ToolRuntime for UpdateGoalTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let update = themis_core::goal::parse_update(&args).map_err(error)?;
        if let Some(refused) = denied(&self.state, &self.run, &self.thread_id, "update_goal") {
            return Err(refused);
        }
        // A message the person queued meanwhile is part of the goal and not read yet.
        if matches!(update, themis_core::goal::GoalUpdate::Complete(_))
            && !self.state.queue_for(&self.thread_id).is_empty()
        {
            return Err(error(themis_core::prompts::GOAL_COMPLETE_UNREAD));
        }
        let goal = self
            .state
            .update_goal_status(Arc::clone(&self.sink), self.thread_id.clone(), update)
            .await
            .map_err(error)?;
        Ok(json!({"status": goal.status}))
    }
}

impl ToolT for UpdateGoalTool {
    fn name(&self) -> &str {
        "update_goal"
    }
    fn description(&self) -> &str {
        themis_core::prompts::UPDATE_GOAL_TOOL
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{
            "status":{"type":"string","enum":["complete","blocked","paused"],"description":"`complete` only when current evidence proves every requirement; `blocked` only after the same blocker held for three goal turns in a row; `paused` only when the user asked."},
            "evidence":{"type":"array","description":"With complete: one entry per requirement of the objective.","items":{"type":"object","properties":{"requirement":{"type":"string","description":"One requirement, in the objective's terms."},"proof":{"type":"string","description":"What proves it now: a command and its result, a file, a test, a page check."}},"required":["requirement","proof"],"additionalProperties":false}},
            "reason":{"type":"string","description":"With blocked: the blocker, in one sentence the user can act on."}
        },"required":["status"],"additionalProperties":false})
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
}
