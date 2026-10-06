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
        // Active runs retain their immutable snapshot; changes apply to the next run.
        let outcome = async {
            Ok::<_, anyhow::Error>(match action {
                "list" => serde_json::to_value(store.list()?)?,
                "import_path" => serde_json::to_value(store.import_path(
                    scope,
                    Path::new(args["path"].as_str().unwrap_or_default()),
                    args["name"].as_str(),
                )?)?,
                "import_json" => {
                    let content = args["content"].as_str().unwrap_or_default();
                    if content.len() > 8 * 1024 * 1024 {
                        return Err(anyhow::anyhow!("Import exceeds 8 MiB"));
                    }
                    let value: Value = serde_json::from_str(content)?;
                    if value.get("spec").is_some()
                        || value.get("name").is_some_and(Value::is_string)
                    {
                        let mut spec: PluginSpec =
                            serde_json::from_value(value.get("spec").cloned().unwrap_or(value))?;
                        if !name.is_empty() {
                            spec.name = name.into();
                        }
                        serde_json::to_value(store.save(scope, spec, None)?)?
                    } else {
                        serde_json::to_value(store.import_mcp_json(
                            scope,
                            if name.is_empty() {
                                "imported-mcp"
                            } else {
                                name
                            },
                            content,
                        )?)?
                    }
                }
                "preview_repository" => serde_json::to_value(
                    store
                        .preview_repository(
                            name,
                            args["url"].as_str().unwrap_or_default(),
                            args["reference"].as_str(),
                            args["subdirectory"].as_str(),
                        )
                        .await?,
                )?,
                "import_repository" => serde_json::to_value(
                    store
                        .import_repository(
                            scope,
                            name,
                            args["url"].as_str().unwrap_or_default(),
                            args["reference"].as_str(),
                            args["subdirectory"].as_str(),
                        )
                        .await?,
                )?,
                "remove_component" => {
                    store.remove_component(
                        scope,
                        name,
                        args["kind"].as_str().unwrap_or_default(),
                        args["id"].as_str().unwrap_or_default(),
                    )?;
                    Value::Null
                }
                "set_component_enabled" => {
                    store.set_component_enabled(
                        scope,
                        name,
                        args["kind"].as_str().unwrap_or_default(),
                        args["id"].as_str().unwrap_or_default(),
                        args["enabled"]
                            .as_bool()
                            .ok_or_else(|| anyhow::anyhow!("enabled must be a boolean"))?,
                    )?;
                    Value::Null
                }
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
                "preview" => serde_json::to_value(
                    store
                        .preview(args["marketplace"].as_str().unwrap_or_default(), name)
                        .await?,
                )?,
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
