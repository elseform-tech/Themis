use super::*;
use serde_json::{json, Value};
use themis_core::plugins::{AvailableSkill, PluginSpec, PluginStore};

impl AppState {
    pub(super) fn plugin_store(&self, project: Option<PathBuf>) -> PluginStore {
        PluginStore::new(
            self.inner
                .skills_path
                .parent()
                .unwrap_or(Path::new("."))
                .into(),
            project,
        )
    }
    pub async fn prompt_skills(
        &self,
        project_root: Option<String>,
    ) -> Result<Vec<AvailableSkill>, String> {
        let project = project_root
            .as_deref()
            .map(canonical_project_dir)
            .transpose()?;
        let mut skills = self
            .plugin_store(project)
            .available_skills()
            .map_err(|e| e.to_string())?;
        skills.extend(
            self.list_skills()
                .await
                .into_iter()
                .map(|s| AvailableSkill {
                    id: s.id,
                    name: s.name,
                    description: s.description,
                    plugin: "Personal (legacy)".into(),
                    scope: "global".into(),
                    retired: false,
                }),
        );
        Ok(skills)
    }
    pub async fn plugin_action(&self, args: Value) -> Result<Value, String> {
        let project = args["projectRoot"]
            .as_str()
            .map(canonical_project_dir)
            .transpose()?;
        let store = self.plugin_store(project.clone());
        let scope = args["scope"].as_str().unwrap_or("global");
        let name = args["name"].as_str().unwrap_or_default();
        let action = args["action"].as_str().unwrap_or("list");
        // No mutation while a project is running: its capability snapshot is immutable.
        if !matches!(
            action,
            "list" | "marketplaces" | "catalog" | "test_mcp" | "test_hook"
        ) && self.inner.running_count.load(Ordering::SeqCst) > 0
        {
            return Err("Wait for active runs to finish before changing plugins".into());
        }
        let outcome = async {
            Ok::<_, anyhow::Error>(match action {
                "list" => serde_json::to_value(store.list()?)?,
                "save" => serde_json::to_value(store.save(
                    scope,
                    serde_json::from_value::<PluginSpec>(args["spec"].clone())?,
                    args["expectedRevision"].as_str(),
                )?)?,
                "save_skill" => serde_json::to_value(store.save_skill(
                    scope,
                    args["plugin"].as_str().unwrap_or("personal"),
                    serde_json::from_value(args["skill"].clone())?,
                    args["expectedRevision"].as_str(),
                )?)?,
                "delete" => {
                    store.delete(scope, name)?;
                    Value::Null
                }
                "enable" | "disable" => {
                    store.set_enabled(scope, name, action == "enable")?;
                    Value::Null
                }
                "marketplaces" => serde_json::to_value(store.marketplaces()?)?,
                "add_marketplace" => {
                    store.add_marketplace(name, args["source"].as_str().unwrap_or_default())?;
                    Value::Null
                }
                "remove_marketplace" => {
                    store.remove_marketplace(name)?;
                    Value::Null
                }
                "catalog" => {
                    store
                        .catalog(name, args["refresh"].as_bool().unwrap_or(false))
                        .await?
                }
                "install" | "update" => serde_json::to_value(
                    store
                        .install(
                            scope,
                            args["marketplace"].as_str().unwrap_or_default(),
                            name,
                        )
                        .await?,
                )?,
                "test_mcp" => {
                    let spec = serde_json::from_value(args["server"].clone())?;
                    let root = project
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("Choose a project to test a connection"))?;
                    let mut connection =
                        themis_core::plugins::connections::Connection::connect(&spec, root).await?;
                    json!({"connected":true,"tools":connection.tools().await?})
                }
                "test_hook" => {
                    let hook: themis_core::plugins::Hook =
                        serde_json::from_value(args["hook"].clone())?;
                    let root = project
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("Choose a project to test a hook"))?;
                    // This explicitly requested test runs its displayed command; ordinary runs use approvals.
                    let approvals: Arc<dyn themis_core::tools::ApprovalHook> =
                        Arc::new(themis_core::tools::AllowAllHook);
                    themis_core::plugins::hooks::run(
                        &hook,
                        root,
                        &json!({"event":hook.event,"test":true}),
                        &approvals,
                    )
                    .await?
                }
                _ => return Err(anyhow::anyhow!("Unknown plugin action")),
            })
        }
        .await;
        outcome.map_err(|e| e.to_string())
    }
}
