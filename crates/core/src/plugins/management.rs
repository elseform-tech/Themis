//! The agent-facing plugin tools: the inventory listing and saving a skill.

use super::*;

struct ManagementTool {
    name: &'static str,
    store: PluginStore,
    approvals: std::sync::Arc<dyn crate::tools::ApprovalHook>,
}

impl std::fmt::Debug for ManagementTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("PluginTool").field(&self.name).finish()
    }
}

#[autoagents::async_trait]
impl crate::tools::ToolRuntime for ManagementTool {
    async fn execute(
        &self,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, autoagents::core::tool::ToolCallError> {
        use crate::tools::{RiskLevel, ToolAction};
        crate::tools::check_approval_permitted(
            self.approvals.as_ref(),
            &ToolAction {
                display: None,
                command: Vec::new(),
                tool: self.name.into(),
                summary: format!(
                    "{} {}",
                    self.name,
                    args.get("scope")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                ),
                risk: if self.name == "list_plugins" {
                    RiskLevel::Read
                } else {
                    RiskLevel::Write
                },
            },
        )?;
        let result = (|| -> anyhow::Result<serde_json::Value> {
            if self.name == "list_plugins" {
                return self.store.list_metadata(&args);
            }
            let skill: Skill = serde_json::from_value(args["skill"].clone())?;
            Ok(serde_json::to_value(self.store.save_skill(
                args["scope"].as_str().unwrap_or("local"),
                args["plugin"].as_str().unwrap_or("personal"),
                skill,
                args["expectedRevision"].as_str(),
            )?)?)
        })();
        result
            .map_err(|e| autoagents::core::tool::ToolCallError::RuntimeError(e.to_string().into()))
    }
}

impl crate::tools::ToolT for ManagementTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        if self.name == "list_plugins" {
            "Read paginated integration metadata and configured/enabled MCP counts, without bundled file or instruction contents. Enabled does not imply connected or healthy."
        } else {
            "Validate and save a generated skill in a personal local or global plugin"
        }
    }
    fn args_schema(&self) -> serde_json::Value {
        if self.name == "list_plugins" {
            serde_json::json!({"type":"object","properties":{"kind":{"type":"string","enum":["plugin","skill","mcp","hook","subagent"]},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}},"additionalProperties":false})
        } else {
            serde_json::json!({"type":"object","properties":{"scope":{"type":"string","enum":["local","global"]},"plugin":{"type":"string"},"expectedRevision":{"type":["string","null"]},"skill":{"type":"object","properties":{"id":{"type":"string"},"name":{"type":"string"},"description":{"type":"string"},"instructions":{"type":"string"},"allowedTools":{"type":"array","items":{"type":"string"}},"scripts":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"content":{"type":"string"}},"required":["name","content"]}}},"required":["id","name","description","instructions","allowedTools","scripts"]}},"required":["scope","plugin","skill"]})
        }
    }
    fn output_schema(&self) -> Option<serde_json::Value> {
        None
    }
}

/// One plugin as the agent's inventory shows it: metadata and component summaries, no contents.
fn inventory_item(p: &Plugin) -> serde_json::Value {
    let mcp = p
        .spec
        .mcp
        .iter()
        .map(|(id, s)| (id.clone(), serde_json::json!({"enabled":s.enabled})))
        .collect::<serde_json::Map<_, _>>();
    let skills = p
        .spec
        .skills
        .iter()
        .map(|s| serde_json::json!({"id":s.id,"name":s.name,"description":s.description}))
        .collect::<Vec<_>>();
    // A profile's instructions stay out: what it may do and run on is enough to choose it.
    let subagents = p.spec.subagents.iter().map(|a| serde_json::json!({"name":a.name,"description":a.description,"model":a.model,"tools":a.tools,"read_only":!a.allows_writes()})).collect::<Vec<_>>();
    serde_json::json!({"scope":p.scope,"source":p.source,"revision":p.revision,"enabled":p.enabled,
        "spec":{"name":p.spec.name,"description":p.spec.description,"package_kind":p.spec.package_kind,
            "mcp":mcp,"skills":skills,"skill_paths":p.spec.skill_paths,"hook_count":p.spec.hooks.len(),"hooks":p.spec.hooks,"subagents":subagents,"file_count":p.spec.files.len()}})
}

impl PluginStore {
    /// Agent-facing inventory only; UI/CLI inspection retains complete packages.
    pub fn list_metadata(&self, args: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let number = |key: &str, default: usize| -> anyhow::Result<usize> {
            args.get(key).map_or(Ok(default), |v| {
                v.as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| anyhow::anyhow!("{key} must be a nonnegative integer"))
            })
        };
        let offset = number("offset", 0)?;
        let limit = number("limit", 25)?;
        anyhow::ensure!((1..=100).contains(&limit), "limit must be 1..100");
        let kind = args
            .get("kind")
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| anyhow::anyhow!("kind must be a string"))
            })
            .transpose()?;
        anyhow::ensure!(
            kind.is_none_or(|k| matches!(k, "plugin" | "skill" | "mcp" | "hook" | "subagent")),
            "unknown inventory kind"
        );
        let plugins = self
            .list()?
            .into_iter()
            .filter(|p| match kind {
                Some("mcp") => !p.spec.mcp.is_empty(),
                Some("hook") => !p.spec.hooks.is_empty(),
                Some("skill") => !p.spec.skills.is_empty(),
                Some("subagent") => !p.spec.subagents.is_empty(),
                Some("plugin") => p.spec.package_kind.as_deref() == Some("plugin"),
                _ => true,
            })
            .collect::<Vec<_>>();
        anyhow::ensure!(offset <= plugins.len(), "inventory offset exceeds total");
        let mcp_total: usize = plugins.iter().map(|p| p.spec.mcp.len()).sum();
        let mcp_enabled: usize = plugins
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.spec.mcp.values().filter(|s| s.enabled).count())
            .sum();
        let items = plugins
            .iter()
            .skip(offset)
            .take(limit)
            .map(inventory_item)
            .collect::<Vec<_>>();
        let next = offset + items.len();
        Ok(
            serde_json::json!({"total":plugins.len(),"counts":{"mcp":{"total":mcp_total,"enabled":mcp_enabled}},"items":items,"next_offset":(next < plugins.len()).then_some(next)}),
        )
    }

    pub fn management_tools(
        &self,
        approvals: std::sync::Arc<dyn crate::tools::ApprovalHook>,
    ) -> Vec<Box<dyn crate::tools::ToolT>> {
        ["list_plugins", "save_skill"]
            .into_iter()
            .map(|name| {
                Box::new(ManagementTool {
                    name,
                    store: self.clone(),
                    approvals: approvals.clone(),
                }) as Box<dyn crate::tools::ToolT>
            })
            .collect()
    }
}
