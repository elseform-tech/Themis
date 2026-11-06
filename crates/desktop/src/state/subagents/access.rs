//! What a subagent may use: models, connections, web, nesting and write paths.

use super::*;

impl SubagentKit {
    /// The kit `agent` starts its own subagents with, when it may: its profile's list names
    /// `Agent` or `Task`, the setting allows it, and the main agent started it, so nesting stops
    /// at one level. It has the tools in `tools`, and those are all its subagents can have.
    pub(super) fn nested_kit(
        &self,
        agent: &Subagent,
        tools: &[Box<dyn ToolT>],
    ) -> Option<Arc<SubagentKit>> {
        if !self.may_nest(agent) {
            return None;
        }
        let spawnable = agent
            .profile
            .as_ref()
            .map(|named| named.profile.spawn_profiles.as_slice())
            .unwrap_or_default();
        let mut names = HashMap::new();
        let mut labels = HashMap::new();
        for tool in &agent.mcp.tools {
            if let Some(found) = self.mcp.names.get(tool) {
                names.insert(tool.clone(), found.clone());
            }
            if let Some(found) = self.mcp.labels.get(tool) {
                labels.insert(tool.clone(), found.clone());
            }
        }
        Some(Arc::new(Self {
            state: self.state.clone(),
            sink: Arc::clone(&self.sink),
            thread_id: self.thread_id.clone(),
            run_id: self.run_id.clone(),
            work_root: self.work_root.clone(),
            is_git: self.is_git,
            mode: self.mode,
            approval_timeout: self.approval_timeout,
            unattended: self.unattended,
            config: self.config.clone(),
            catalog: self.catalog.clone(),
            endpoint_override: self.endpoint_override,
            hooks: self.hooks.clone(),
            mcp: SubagentMcp {
                names: Arc::new(names),
                labels: Arc::new(labels),
                startup_rules: self.mcp.startup_rules.clone(),
            },
            web: SubagentWeb {
                config: self.web.config.clone(),
                permissions: Arc::clone(&self.web.permissions),
            },
            skills: self.skills.clone(),
            project_guidance: self.project_guidance.clone(),
            skill_list: self.skill_list.clone(),
            plugin_bins: self.plugin_bins.clone(),
            plugins: self.plugins.clone(),
            llm: Arc::clone(&self.llm),
            model: self.model.clone(),
            policy: self.policy,
            sandbox: Arc::clone(&self.sandbox),
            grants: Arc::clone(&self.grants),
            // Stopping it stops them, and its result reaches only its own run.
            stopped: Arc::clone(&agent.stop),
            notes: RunNotes::new(),
            slots: Arc::clone(&self.nested_slots),
            nested_slots: Arc::clone(&self.nested_slots),
            prompt_model: self.prompt_model.clone(),
            nested: Some(NestedLimits {
                parent_id: agent.id.clone(),
                parent_name: agent.name.clone(),
                tools: tools.iter().map(|tool| tool.name().to_owned()).collect(),
                write_paths: agent.write_paths.clone(),
                shell_scopes: agent.shell_scopes().to_vec(),
                profile_required: !spawnable.is_empty(),
            }),
            profiles: self.profiles.only(spawnable),
            started: Mutex::new(Vec::new()),
            other_llms: tokio::sync::Mutex::new(HashMap::new()),
        }))
    }

    /// Whether this subagent gets to start subagents.
    pub(super) fn may_nest(&self, agent: &Subagent) -> bool {
        self.nested.is_none() && self.config.subagents.allow_nested && names(agent, SPAWN_TOOL)
    }

    /// Whether the agent must name a profile for each subagent it starts.
    pub(in crate::state) fn profile_required(&self) -> bool {
        self.nested
            .as_ref()
            .is_some_and(|nested| nested.profile_required)
    }

    /// What this subagent's shell is held to: its own list, else the one it inherited.
    pub(super) fn shell_limit(&self, agent: &Subagent) -> Vec<String> {
        match (agent.shell_scopes(), &self.nested) {
            ([], Some(nested)) => nested.shell_scopes.clone(),
            (own, _) => own.to_vec(),
        }
    }

    /// The write paths of this run's running subagents, with their owners.
    pub(super) fn running_paths(&self) -> Vec<(String, String)> {
        self.started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|agent| !agent.ended())
            .flat_map(|agent| {
                agent
                    .write_paths
                    .iter()
                    .map(|path| (path.clone(), agent.name.clone()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// The write paths a batch of briefs asks for, normalized and checked together: nothing
    /// outside the project or in protected folders, and no path another running subagent or
    /// another brief already owns.
    pub(in crate::state) fn check_write_paths(
        &self,
        asked: &[Vec<String>],
    ) -> Result<Vec<Vec<String>>, String> {
        let protected = self.sandbox.protected().to_vec();
        let mut taken = self.running_paths();
        let mut normalized = Vec::new();
        for (index, paths) in asked.iter().enumerate() {
            let mut own = Vec::new();
            for raw in paths {
                let path =
                    themis_core::sandbox::normalize_write_path(&self.work_root, raw, &protected)?;
                if let Some(nested) = &self.nested {
                    if !nested.write_paths.iter().any(|own| path_inside(own, &path)) {
                        return Err(format!(
                            "'{path}' isn't among the paths {} may change",
                            nested.parent_name
                        ));
                    }
                }
                if let Some((other, owner)) = taken
                    .iter()
                    .find(|(other, _)| themis_core::sandbox::write_paths_overlap(other, &path))
                {
                    return Err(format!("'{path}' overlaps '{other}', which {owner} owns"));
                }
                own.push(path);
            }
            taken.extend(
                own.iter()
                    .map(|path| (path.clone(), format!("subagent {}", index + 1))),
            );
            normalized.push(own);
        }
        Ok(normalized)
    }

    /// The model subagents use when none is given: `subagents.model`, else the chat's.
    pub(in crate::state) fn default_model(&self) -> String {
        self.config
            .subagents
            .model
            .clone()
            .unwrap_or_else(|| self.model.clone())
    }

    /// The further models the agent may give a subagent (`subagents.models`), without the default.
    pub(in crate::state) fn other_models(&self) -> Vec<String> {
        let default = self.default_model();
        let mut models: Vec<String> = Vec::new();
        for model in &self.config.subagents.models {
            if *model != default && !models.contains(model) {
                models.push(model.clone());
            }
        }
        models
    }

    /// The models a subagent tries in turn: its own, the default, the chat's.
    pub(super) fn candidates(&self, chosen: &str) -> Vec<String> {
        let mut models = vec![chosen.to_owned()];
        for model in [self.default_model(), self.model.clone()] {
            if !models.contains(&model) {
                models.push(model);
            }
        }
        models
    }

    pub(super) async fn llm_for(&self, model: &str) -> Result<Arc<dyn LLMProvider>, String> {
        if let Some(reason) = self
            .catalog
            .iter()
            .find(|entry| entry.id == model)
            .and_then(|entry| entry.unavailable_reason.clone())
        {
            return Err(format!("{model} is unavailable: {reason}"));
        }
        if model == self.model {
            return Ok(Arc::clone(&self.llm));
        }
        let mut cache = self.other_llms.lock().await;
        if let Some(found) = cache.get(model) {
            return found.clone();
        }
        let resolved = match Box::pin(self.state.provider_config_for(
            &self.config,
            model,
            &self.thread_id,
        ))
        .await
        {
            Ok(config) => themis_core::providers::resolve(&config)
                .await
                .map_err(|error| format!("{model} could not start: {error:#}")),
            Err(error) => Err(format!("{model} could not start: {error}")),
        };
        cache.insert(model.to_owned(), resolved.clone());
        resolved
    }

    /// Whether the agent can give subagents MCP tools.
    pub(in crate::state) fn offers_mcp(&self) -> bool {
        self.config.subagents.allow_mcp && !self.mcp.names.is_empty()
    }

    /// The MCP tools a subagent gets: all of the chat's, or what its profile's list allows,
    /// narrowed by what the main agent asked for. `asked` holds Themis names or `*`, and empty
    /// means none. Whatever was named and can't be had is left out, not fatal.
    pub(in crate::state) fn mcp_access(
        &self,
        profile: Option<&NamedProfile>,
        asked: Option<&[String]>,
    ) -> McpAccess {
        let mut access = McpAccess::default();
        let wanted_by_asking = |tool: &str| {
            asked.is_none_or(|asked| asked.iter().any(|name| name == "*" || name == tool))
        };
        for (tool, names) in self.mcp.names.iter() {
            if !wanted_by_asking(tool) {
                continue;
            }
            if profile.is_none_or(|named| named.profile.allows_mcp(names)) {
                access.tools.push(tool.clone());
            } else if asked.is_some_and(|asked| asked.iter().any(|name| name == tool)) {
                access.left_out.push(tool.clone());
            }
        }
        access.tools.sort();
        let all_names: Vec<&String> = self.mcp.names.values().flatten().collect();
        if let Some(named) = profile {
            let missing = named.profile.missing_mcp_tools(&all_names);
            access
                .left_out
                .extend(missing.into_iter().map(str::to_owned));
        }
        access.left_out.extend(
            asked
                .into_iter()
                .flatten()
                .filter(|name| *name != "*" && !self.mcp.names.contains_key(*name))
                .cloned(),
        );
        let off = !self.config.subagents.allow_mcp;
        if off {
            let tools = std::mem::take(&mut access.tools);
            access.left_out.extend(tools);
        }
        for tool in &access.tools {
            let label = self.mcp.labels.get(tool);
            let server = label.and_then(|label| label.split(" · ").next());
            if let Some(server) = server {
                if !access.connections.iter().any(|known| known == server) {
                    access.connections.push(server.to_owned());
                }
            }
        }
        access.connections.sort();
        if !access.left_out.is_empty() {
            self.log_left_out(
                if off { "setting" } else { "not_connected" },
                &access.left_out,
            );
        }
        access
    }

    /// Whether subagents may have the web tools at all: `subagents.allow_web` is on, and the
    /// run's sandbox doesn't block the network. These tools run in the app, outside the OS
    /// sandbox, so a subagent held to a blocked network must not be handed them.
    pub(in crate::state) fn offers_web(&self) -> bool {
        self.config.subagents.allow_web
            && self.sandbox.network() != themis_core::sandbox::NetworkPolicy::Block
    }

    /// The web tools this subagent may have.
    pub(super) fn web_access(&self, profile: Option<&NamedProfile>) -> WebAccess {
        let allowed = |tool: &&str| profile.is_none_or(|named| named.profile.allows(tool));
        let wanted: Vec<&str> = WEB_TOOLS.iter().copied().filter(allowed).collect();
        // A list that names them and can't have them is told so; no list asked for nothing.
        let named = profile.is_some_and(|named| named.profile.tools.is_some());
        if self.offers_web() {
            // No search address means no search, for the main agent as well.
            let tools: Vec<&str> = wanted
                .iter()
                .copied()
                .filter(|tool| *tool != "web_search" || self.web.config.search_url.is_some())
                .collect();
            return WebAccess {
                left_out: wanted
                    .iter()
                    .filter(|tool| named && !tools.contains(tool))
                    .map(|tool| (*tool).to_owned())
                    .collect(),
                tools: tools.into_iter().map(str::to_owned).collect(),
            };
        }
        let left_out: Vec<String> = if named {
            wanted.iter().map(|tool| (*tool).to_owned()).collect()
        } else {
            Vec::new()
        };
        if !left_out.is_empty() {
            let _ = self.state.inner.diagnostics.append_with_level(
                "subagents",
                "web_left_out",
                "warn",
                json!({"thread_id": self.thread_id, "run_id": self.run_id,
                    "reason": if self.config.subagents.allow_web { "network_blocked" } else { "setting" },
                    "count": left_out.len()}),
                Some(&self.config.logging.level),
            );
        }
        WebAccess {
            tools: Vec::new(),
            left_out,
        }
    }

    /// Metadata only: why, how many and which tools, never a call's contents.
    pub(super) fn log_left_out(&self, reason: &str, names: &[String]) {
        let listed: Vec<String> = names
            .iter()
            .take(LOGGED_NAMES)
            .map(|name| name.chars().take(LOGGED_NAME_CHARS).collect())
            .collect();
        let _ = self.state.inner.diagnostics.append_with_level(
            "subagents",
            "mcp_left_out",
            "warn",
            json!({"thread_id": self.thread_id, "run_id": self.run_id, "reason": reason,
                "count": names.len(), "tools": listed}),
            Some(&self.config.logging.level),
        );
    }

    /// This subagent's MCP tools, asking `approvals` on every call: the tools themselves, what
    /// they are called, and what could not be had after all.
    pub(super) async fn mcp_tools_for(
        &self,
        agent: &Arc<Subagent>,
        approvals: &Arc<dyn ApprovalHook>,
    ) -> (Vec<Box<dyn ToolT>>, Vec<String>) {
        let mut tools: Vec<Box<dyn ToolT>> = Vec::new();
        if agent.mcp.tools.is_empty() {
            return (tools, Vec::new());
        }
        for plugin in &self.plugins {
            for (name, server) in &plugin.spec.mcp {
                if !server.enabled {
                    continue;
                }
                let connected = themis_core::diagnostics::ACTIVE
                    .scope(
                        self.diagnostic_context(agent),
                        self.state.mcp_tools(
                            &self.work_root,
                            plugin,
                            name,
                            Arc::clone(approvals),
                            Some(&self.mcp.startup_rules),
                        ),
                    )
                    .await;
                if let Ok((found, _, _)) = connected {
                    tools.extend(
                        found.into_iter().filter(|tool| {
                            agent.mcp.tools.iter().any(|wanted| wanted == tool.name())
                        }),
                    );
                }
            }
        }
        let missing: Vec<String> = agent
            .mcp
            .tools
            .iter()
            .filter(|wanted| !tools.iter().any(|tool| tool.name() == wanted.as_str()))
            .cloned()
            .collect();
        if !missing.is_empty() {
            self.log_left_out("not_connected", &missing);
        }
        (tools, missing)
    }
}
