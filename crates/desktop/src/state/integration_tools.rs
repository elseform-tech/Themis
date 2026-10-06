//! Agent-facing management uses the same validated state APIs as the UI and CLI.
use super::{AppState, AutomationInput, PathBuf};
use serde_json::{json, Value};
use std::sync::Arc;
use themis_core::tools::{
    async_trait, ApprovalHook, ToolAction, ToolCallError, ToolRuntime, ToolT,
};

impl AppState {
    pub(super) fn integration_management_tools(
        &self,
        approvals: Arc<dyn ApprovalHook>,
        project: PathBuf,
        thread_id: String,
    ) -> Vec<Box<dyn ToolT>> {
        [
            "list_plugins",
            "save_skill",
            "manage_integrations",
            "manage_automations",
        ]
        .into_iter()
        .map(|name| {
            Box::new(ManagementTool {
                name,
                state: self.clone(),
                approvals: approvals.clone(),
                project: project.clone(),
                thread_id: thread_id.clone(),
            }) as Box<dyn ToolT>
        })
        .collect()
    }
}

struct ManagementTool {
    name: &'static str,
    state: AppState,
    approvals: Arc<dyn ApprovalHook>,
    project: PathBuf,
    thread_id: String,
}
impl std::fmt::Debug for ManagementTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ManagementTool").field(&self.name).finish()
    }
}
#[async_trait]
impl ToolRuntime for ManagementTool {
    async fn execute(&self, mut args: Value) -> Result<Value, ToolCallError> {
        if !args.is_object() {
            return Err(error("Management arguments must be an object"));
        }
        if self.name == "list_plugins" {
            args["action"] = "list".into();
        }
        if self.name == "save_skill" {
            args["action"] = "save_skill".into();
        }
        themis_core::tools::check_approval_permitted(
            self.approvals.as_ref(),
            &ToolAction {
                tool: themis_core::tools::integration_approval_identity(self.name, &args),
                summary: format!(
                    "{} {}",
                    self.name,
                    args["action"].as_str().unwrap_or("list")
                ),
                risk: themis_core::tools::integration_risk(self.name, &args)
                    .expect("known management tool"),
            },
        )?;
        if self.name == "manage_automations" {
            return self.automation_action(args).await.map_err(error);
        }
        if args.get("projectRoot").is_none() {
            args["projectRoot"] = self.project.to_string_lossy().as_ref().into();
        }
        if args.get("scope").is_none() {
            args["scope"] = "local".into();
        }
        self.state.plugin_action(args).await.map_err(error)
    }
}
impl ManagementTool {
    async fn automation_action(&self, args: Value) -> Result<Value, String> {
        let action = args["action"].as_str().unwrap_or("list");
        let all = self.state.list_automations().await;
        if action == "list" {
            return serde_json::to_value(all).map_err(|e| e.to_string());
        }
        let id = args["id"].as_str().unwrap_or_default().to_owned();
        match action {
            "enable" | "disable" => serde_json::to_value(
                self.state
                    .set_automation_enabled(id, action == "enable")
                    .await?,
            )
            .map_err(|e| e.to_string()),
            "delete" => {
                self.state.delete_automation(id).await?;
                Ok(Value::Null)
            }
            "create" | "update" => {
                let mut input = if action == "update" {
                    serde_json::to_value(
                        all.iter()
                            .find(|item| item.id == id)
                            .ok_or("Unknown automation")?,
                    )
                    .map_err(|e| e.to_string())?
                } else {
                    json!({"project_root":self.project,"target_thread_id":self.thread_id,"provider":"go","model":"","skill_ids":[],"interval_mins":60,"enabled":true})
                };
                let patch = args
                    .get("input")
                    .unwrap_or(&args)
                    .as_object()
                    .ok_or("Automation input must be an object")?;
                for (key, value) in patch {
                    input[key] = value.clone();
                }
                let input: AutomationInput =
                    serde_json::from_value(input).map_err(|e| e.to_string())?;
                let saved = if action == "create" {
                    self.state.create_automation(input).await?
                } else {
                    self.state.update_automation(id, input).await?
                };
                serde_json::to_value(saved).map_err(|e| e.to_string())
            }
            _ => Err("Unknown automation action".into()),
        }
    }
}
fn error(message: impl ToString) -> ToolCallError {
    ToolCallError::RuntimeError(message.to_string().into())
}
impl ToolT for ManagementTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        match self.name {
            "list_plugins" => "Inspect installed plugins, bundled skills, MCP connections and hooks in the current project and user scope.",
            "save_skill" => "Create or update a personal skill through the shared registry. Supply skill, plugin, scope and expectedRevision when updating.",
            "manage_integrations" => "Manage integrations through shared validated APIs. Actions: list, save, save_skill, set_component_enabled, remove_component, enable, disable, delete, marketplaces, catalog, add_marketplace, remove_marketplace, install, update, import_path, import_json, import_repository, test_mcp, test_hook. Imports accept path or JSON/config or source repository with ref and subdirectory; component actions take component type and id. Defaults to current project/local scope. Executable operations require approval.",
            _ => "Manage recurring tasks. Actions: list, create, update, enable, disable, delete. Supply id for existing tasks; input contains name/task and optional calendar schedule {repeat,time,timezone,weekday}. Creation defaults to current project/chat. Updates merge input with saved fields. Calendar timezone is IANA; time HH:MM; weekday 0=Monday. Uses normal runtime approvals.",
        }
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{"action":{"type":"string"},"scope":{"type":"string","enum":["local","global"]},"projectRoot":{"type":"string"},"name":{"type":"string"},"plugin":{"type":"string"},"expectedRevision":{"type":"string"},"skill":{"type":"object"},"spec":{"type":"object"},"id":{"type":"string"},"path":{"type":"string"},"source":{"type":"string"},"ref":{"type":"string"},"subdirectory":{"type":"string"},"json":{"type":["object","string"]},"component":{"type":"string"},"enabled":{"type":"boolean"},"input":{"type":"object"}},"additionalProperties":true})
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use themis_core::tools::{AllowAllHook, DenyAllHook};

    #[tokio::test]
    async fn agent_automation_creation_uses_current_chat_and_update_preserves_fields() {
        let data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let state = AppState::new_for_test(data.path().join("settings.json"));
        let thread = state
            .create_thread(
                project.path().to_string_lossy().into(),
                super::super::ProviderKind::Go,
                Some("model".into()),
            )
            .await
            .unwrap();
        let tools = state.integration_management_tools(
            Arc::new(AllowAllHook),
            project.path().into(),
            thread.id.clone(),
        );
        let tool = tools
            .iter()
            .find(|tool| tool.name() == "manage_automations")
            .unwrap();
        let created = tool.execute(json!({"action":"create","input":{"name":"Review daily","task":"Review recent changes","schedule":{"repeat":"daily","time":"19:00","timezone":"Europe/Berlin"}}})).await.unwrap();
        assert_eq!(created["target_thread_id"], thread.id);
        let updated = tool
            .execute(
                json!({"action":"update","id":created["id"],"input":{"name":"Evening review"}}),
            )
            .await
            .unwrap();
        assert_eq!(updated["task"], "Review recent changes");
        assert_eq!(updated["schedule"], created["schedule"]);
        let restarted = AppState::new_for_test(data.path().join("settings.json"));
        assert_eq!(restarted.list_automations().await[0].name, "Evening review");
    }

    #[tokio::test]
    async fn agent_management_is_approval_gated_and_uses_shared_storage() {
        let data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let state = AppState::new_for_test(data.path().join("settings.json"));
        let args = json!({"plugin":"personal","skill":{"id":"review","name":"Review","description":"Review changes","instructions":"Inspect source","allowedTools":[],"scripts":[]}});
        let denied = state.integration_management_tools(
            Arc::new(DenyAllHook),
            project.path().into(),
            "chat".into(),
        );
        assert!(denied
            .iter()
            .find(|t| t.name() == "save_skill")
            .unwrap()
            .execute(args.clone())
            .await
            .is_err());
        assert!(state
            .plugin_store(Some(project.path().into()))
            .list()
            .unwrap()
            .iter()
            .all(|plugin| plugin.spec.name != "personal"));
        let allowed = state.integration_management_tools(
            Arc::new(AllowAllHook),
            project.path().into(),
            "chat".into(),
        );
        allowed
            .iter()
            .find(|t| t.name() == "save_skill")
            .unwrap()
            .execute(args)
            .await
            .unwrap();
        let saved = state
            .plugin_action(json!({"action":"list","projectRoot":project.path()}))
            .await
            .unwrap();
        assert_eq!(saved[0]["spec"]["skills"][0]["id"], "review");
        let restarted = AppState::new_for_test(data.path().join("settings.json"));
        assert!(restarted
            .prompt_skills(Some(project.path().to_string_lossy().into()))
            .await
            .unwrap()
            .iter()
            .any(|s| s.name == "Review"));
    }
}
