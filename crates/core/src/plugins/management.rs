//! The metadata-only plugin inventory (`plugin_action` `inventory`), which the agent's
//! `themis plugin list --kind subagent` reads.

use super::*;

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
    /// The metadata-only inventory; UI/CLI inspection retains complete packages.
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
}
