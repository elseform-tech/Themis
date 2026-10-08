use super::*;
use serde_json::{json, Value};
use themis_core::plugins::{AvailableSkill, PluginSpec, PluginStore};

impl AppState {
    pub(super) fn plugin_store(&self, project: Option<PathBuf>) -> PluginStore {
        let config = self.runtime_configuration(project.as_deref(), None).ok();
        let store = PluginStore::new(
            self.inner
                .skills_path
                .parent()
                .unwrap_or(Path::new("."))
                .into(),
            project,
        );
        match config {
            Some(config) => store
                .clone()
                .with_skill_paths(config.config.skills.paths)
                .unwrap_or(store),
            None => store,
        }
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
        let started = std::time::Instant::now();
        let operation_id = uuid::Uuid::new_v4().to_string();
        let _ = self.inner.diagnostics.append(
            "integration",
            "operation_started",
            "info",
            json!({"operation_id":operation_id,"action":action,"plugin":name,"scope":scope}),
        );
        let context = themis_core::diagnostics::DiagnosticContext {
            log: self.inner.diagnostics.clone(),
            thread_id: String::new(),
            run_id: operation_id.clone(),
            level: self
                .runtime_configuration(project.as_deref(), None)
                .map(|r| r.config.logging.level)
                .unwrap_or_else(|_| "info".into()),
        };
        let outcome = themis_core::diagnostics::ACTIVE.scope(context,async {
            Ok::<_, anyhow::Error>(match action {
                "list" => serde_json::to_value(store.list()?)?,
                "inspect_path" => serde_json::to_value(store.inspect_path(Path::new(args["path"].as_str().unwrap_or_default()),args["name"].as_str())?)?,
                "inspect_json" => {
                    let content = args["content"].as_str().unwrap_or_default();
                    let value = themis_core::configuration::parse(content)?;
                    if value.get("spec").is_some() || value.get("name").is_some_and(Value::is_string) {
                        let mut spec: PluginSpec = serde_json::from_value(value.get("spec").cloned().unwrap_or(value))?;
                        if !name.is_empty() { spec.name=name.into(); }
                        serde_json::to_value(themis_core::plugins::ImportPreview::from_spec(spec)?)?
                    } else { serde_json::to_value(store.inspect_mcp_json(if name.is_empty(){"imported-mcp"}else{name}, &value.to_string())?)? }
                },
                "inspect_repository" => serde_json::to_value(store.inspect_repository(name,args["url"].as_str().unwrap_or_default(),args["reference"].as_str(),args["subdirectory"].as_str()).await?)?,
                "inspect_marketplace" => serde_json::to_value(store.inspect_marketplace(args["marketplace"].as_str().unwrap_or_default(),name).await?)?,
                "import_path" => serde_json::to_value(store.import_path_with_options(
                    scope,
                    Path::new(args["path"].as_str().unwrap_or_default()),
                    args["name"].as_str(),
                    args["allowPartial"].as_bool().unwrap_or(false),
                )?)?,
                "import_json" => {
                    let content = args["content"].as_str().unwrap_or_default();
                    if content.len() > 8 * 1024 * 1024 {
                        return Err(anyhow::anyhow!("Import exceeds 8 MiB"));
                    }
                    let value: Value = themis_core::configuration::parse(content)?;
                    if value.get("spec").is_some()
                        || value.get("name").is_some_and(Value::is_string)
                    {
                        let mut spec: PluginSpec =
                            serde_json::from_value(value.get("spec").cloned().unwrap_or(value))?;
                        if !name.is_empty() {
                            spec.name = name.into();
                        }
                        if !args["allowPartial"].as_bool().unwrap_or(false) && !spec.import_issues.is_empty() { return Err(anyhow::anyhow!("Plugin has unsupported components; inspect and explicitly choose partial installation")); }
                        serde_json::to_value(store.save(scope, spec, None)?)?
                    } else {
                        serde_json::to_value(store.import_mcp_json_with_options(
                            scope,
                            if name.is_empty() {
                                "imported-mcp"
                            } else {
                                name
                            },
                            &value.to_string(),
                            args["allowPartial"].as_bool().unwrap_or(false),
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
                        .import_repository_with_options(
                            scope,
                            name,
                            args["url"].as_str().unwrap_or_default(),
                            args["reference"].as_str(),
                            args["subdirectory"].as_str(),
                            args["allowPartial"].as_bool().unwrap_or(false),
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
                        .install_with_options(
                            scope,
                            args["marketplace"].as_str().unwrap_or_default(),
                            name,
                            args["allowPartial"].as_bool().unwrap_or(false),
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
        })
        .await;
        let mut details = json!({"operation_id":operation_id,"action":action,"plugin":name,"scope":scope,"elapsed_ms":started.elapsed().as_millis(),"success":outcome.is_ok()});
        if let Ok(value) = &outcome {
            if let Some(report) = value.get("report") {
                details["report"] = report.clone();
            }
        }
        if let Err(error) = &outcome {
            let message = format!("{error:#}");
            details["error_code"] = if message.contains("unsupported") {
                "unsupported_capability"
            } else if message.contains("credential") || message.contains("environment") {
                "invalid_environment_or_authentication"
            } else {
                "operation_failed"
            }
            .into();
            if message.starts_with("mcpServers.") || message.starts_with("mcp.") {
                details["field"] = message.split(':').next().unwrap_or_default().into();
            }
        }
        let _ = self.inner.diagnostics.append(
            "integration",
            if outcome.is_ok() {
                "operation_completed"
            } else {
                "operation_failed"
            },
            if outcome.is_ok() { "info" } else { "error" },
            details,
        );
        outcome.map_err(|e| format!("{e:#}\nDiagnostic operation: {operation_id}"))
    }
}
