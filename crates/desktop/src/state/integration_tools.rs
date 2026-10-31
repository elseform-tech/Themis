//! Agent-facing management uses the same validated state APIs as the UI and CLI.
use super::automation::{
    schedule_summary, shell_policy_for, CheckGrant, CheckPolicy, CALENDAR_EXPIRY_MINS,
};
use super::{AppState, Automation, AutomationInput, PathBuf};
use crate::types::AutomationOrigin;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use themis_core::configuration::{ApprovalMode, ApprovalPolicy, PolicyAction};
use themis_core::plugins::connections::McpServer;
use themis_core::tools::{
    async_trait, ApprovalHook, ToolAction, ToolCallError, ToolRuntime, ToolT,
};

/// The approval mode and policy a run captured when it started, plus the
/// interactive hook used to ask about a check command, and the chat's grants.
#[derive(Clone)]
pub(super) struct RunApproval {
    pub(super) mode: ApprovalMode,
    pub(super) policy: ApprovalPolicy,
    pub(super) interactive: Arc<dyn ApprovalHook>,
    pub(super) grants: Arc<themis_core::chat_grants::ChatGrants>,
}

impl AppState {
    /// Management tools outside a run (tests): capture the live mode and policy.
    #[cfg(test)]
    pub(super) fn integration_management_tools(
        &self,
        approvals: Arc<dyn ApprovalHook>,
        project: PathBuf,
        thread_id: String,
    ) -> Vec<Box<dyn ToolT>> {
        let mode = self
            .inner
            .threads
            .try_read()
            .ok()
            .and_then(|threads| threads.get(&thread_id).map(|thread| thread.approval_mode))
            .unwrap_or_default();
        let policy = self
            .runtime_configuration(Some(&project), None)
            .map(|resolved| resolved.config.approval)
            .unwrap_or_default();
        let run = RunApproval {
            mode,
            policy,
            interactive: approvals.clone(),
            grants: self.chat_grants_for(&thread_id),
        };
        self.integration_management_tools_for_run(approvals, project, thread_id, run)
    }

    pub(super) fn integration_management_tools_for_run(
        &self,
        approvals: Arc<dyn ApprovalHook>,
        project: PathBuf,
        thread_id: String,
        run: RunApproval,
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
                run: run.clone(),
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
    run: RunApproval,
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
        // An agent manages the automations of its own chat without asking:
        // the user stops them from Workspace Tasks. Anything else (another
        // chat's automation) follows the normal approval rules.
        let own_chat_automation =
            self.name == "manage_automations" && self.targets_own_chat(&args).await;
        if own_chat_automation {
            self.own_chat_policy(&args)?;
        } else {
            let action = match themis_core::tools::sole_mcp_command_save(self.name, &args) {
                // One command-based server and nothing else executable: a single
                // dialog with the exact argv, never remembered for the chat.
                Some((_, argv)) => mcp_command_action(argv),
                None => ToolAction {
                    display: None,
                    command: Vec::new(),
                    tool: themis_core::tools::integration_approval_identity(self.name, &args),
                    summary: format!(
                        "{} {}",
                        self.name,
                        args["action"].as_str().unwrap_or("list")
                    ),
                    risk: themis_core::tools::integration_risk(self.name, &args)
                        .expect("known management tool"),
                },
            };
            themis_core::tools::check_approval_permitted(self.approvals.as_ref(), &action)?;
        }
        if self.name == "manage_automations" {
            return self.automation_action(args).await.map_err(error);
        }
        if args.get("projectRoot").is_none() {
            args["projectRoot"] = self.project.to_string_lossy().as_ref().into();
        }
        if args.get("scope").is_none() {
            args["scope"] = "global".into();
        }
        if args["action"].as_str().unwrap_or("list") == "list" {
            args["action"] = "inventory".into();
        }
        // One management call per plugin at a time: the definition a dialog
        // shows is then the one that is saved.
        let plugin_name = if args["action"] == "save" {
            args["spec"]["name"].as_str()
        } else {
            args["name"].as_str()
        }
        .unwrap_or_default()
        .to_owned();
        let lock = self
            .state
            .plugin_call_lock(args["scope"].as_str().unwrap_or("global"), &plugin_name);
        let _serialized = lock.lock().await;
        let effects = self.mcp_command_effects(&args)?;
        let sole = themis_core::tools::sole_mcp_command_save(self.name, &args).is_some();
        let approved = self.approve_mcp_command(&effects, sole)?;
        self.state
            .plugin_action_for_agent(args, approved)
            .await
            .map_err(error)
    }
}

/// The dialog for launching a new or changed MCP command: exact argv as data.
fn mcp_command_action(argv: Vec<String>) -> ToolAction {
    ToolAction {
        display: Some(themis_core::tools::CommandDisplay {
            argv,
            cwd: None,
            background: false,
            sandbox_blocked: None,
        }),
        command: Vec::new(),
        tool: themis_core::tools::MCP_COMMAND_APPROVAL_TOOL.to_owned(),
        summary: "Enable a command-based MCP connection".to_owned(),
        risk: themis_core::tools::RiskLevel::Execute,
    }
}

/// (server name, server, resource files, executable files) of a plugin.
type StoredMcp = (
    String,
    McpServer,
    std::collections::BTreeMap<String, String>,
    Vec<String>,
);

/// A command-based server this call would newly enable or change, with the
/// digest of the definition (and resources) the dialog is about.
struct McpEffect {
    name: String,
    server: McpServer,
    digest: Option<String>,
}
impl ManagementTool {
    /// Command-based MCP connections this call would newly enable or change:
    /// the app launches their command on the next start, so the person must
    /// approve that exact command. Imports, installs and updates leave
    /// connections disabled or unapproved and need no such prompt.
    fn mcp_command_effects(&self, args: &Value) -> Result<Vec<McpEffect>, ToolCallError> {
        let scope = args["scope"].as_str().unwrap_or("global");
        // (server name, server, resource files, executable files) after the call.
        let wanted: Vec<StoredMcp> = match args["action"].as_str().unwrap_or("list") {
            "save" => {
                serde_json::from_value::<themis_core::plugins::PluginSpec>(args["spec"].clone())
                    .map(|spec| {
                        spec.mcp
                            .into_iter()
                            .filter(|(_, server)| server.enabled)
                            .map(|(name, server)| {
                                (
                                    name,
                                    server,
                                    spec.files.clone(),
                                    spec.executable_files.clone(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }
            "set_component_enabled" if args["kind"] == "mcp" && args["enabled"] == true => {
                self.stored_mcp(scope, args["name"].as_str(), args["id"].as_str())
            }
            "enable" => self.stored_mcp(scope, args["name"].as_str(), None),
            _ => Vec::new(),
        };
        let root = self
            .project
            .canonicalize()
            .unwrap_or_else(|_| self.project.clone());
        let plugin = if args["action"] == "save" {
            args["spec"]["name"].as_str()
        } else {
            args["name"].as_str()
        }
        .unwrap_or_default();
        let effects: Vec<McpEffect> = wanted
            .into_iter()
            .filter(|(_, server, _, _)| server.command.is_some() && server.enabled)
            .map(|(name, server, files, executable)| {
                let digest = super::mcp_consent::definition_digest(&server, &files, &executable);
                McpEffect {
                    name,
                    server,
                    digest,
                }
            })
            .filter(|effect| {
                !self.state.mcp_definition_consented(
                    &root,
                    scope,
                    plugin,
                    &effect.name,
                    effect.digest.as_deref(),
                )
            })
            .collect();
        if effects.len() > 1 {
            return Err(error(
                "Change one command-based MCP connection per call so its exact command can be approved",
            ));
        }
        Ok(effects)
    }

    #[allow(clippy::type_complexity)]
    fn stored_mcp(
        &self,
        scope: &str,
        plugin: Option<&str>,
        id: Option<&str>,
    ) -> Vec<(
        String,
        McpServer,
        std::collections::BTreeMap<String, String>,
        Vec<String>,
    )> {
        self.state
            .plugin_store(Some(self.project.clone()))
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.scope == scope && Some(p.spec.name.as_str()) == plugin)
            .flat_map(|p| {
                let (files, executable) = (p.spec.files.clone(), p.spec.executable_files.clone());
                p.spec.mcp.into_iter().map(move |(name, mut server)| {
                    server.enabled = true;
                    (name, server, files.clone(), executable.clone())
                })
            })
            .filter(|(name, ..)| id.is_none_or(|id| id == name))
            .collect()
    }

    /// Asks about the exact command, every time and never remembered for the
    /// chat; a deny rule refuses. Returns the connections a person approved,
    /// with the digest the dialog was about. YOLO and allow rules apply the
    /// change but record no consent: the connection then waits for a person to
    /// enable it (`sole`: the runtime's dialog already asked).
    fn approve_mcp_command(
        &self,
        effects: &[McpEffect],
        sole: bool,
    ) -> Result<Vec<(String, Option<String>)>, ToolCallError> {
        let Some(effect) = effects.first() else {
            return Ok(Vec::new());
        };
        let command = effect.server.command.clone().unwrap_or_default();
        let action = mcp_command_action(
            std::iter::once(command)
                .chain(effect.server.args.iter().cloned())
                .collect(),
        );
        let (allowed, answered) = if self.run.mode == ApprovalMode::Yolo {
            (
                self.run.policy.yolo_decision(&action) != PolicyAction::Deny,
                false,
            )
        } else {
            match self.run.policy.decision(&action) {
                PolicyAction::Allow => (true, false),
                PolicyAction::Deny => (false, false),
                PolicyAction::Ask if sole => (true, true),
                PolicyAction::Ask => (!self.run.interactive.approve(&action).is_denied(), true),
            }
        };
        if !allowed {
            return Err(error(
                "approval denied for 'manage_integrations': the MCP connection command was not approved",
            ));
        }
        Ok(if answered {
            vec![(effect.name.clone(), effect.digest.clone())]
        } else {
            Vec::new()
        })
    }

    /// Own-chat lifecycle never prompts, but the captured policy still applies:
    /// a deny rule rejects, ask proceeds silently (the user's decision), YOLO
    /// is unchanged. Every automatic decision is logged.
    fn own_chat_policy(&self, args: &Value) -> Result<(), ToolCallError> {
        let action = ToolAction {
            display: None,
            command: Vec::new(),
            tool: themis_core::tools::integration_approval_identity(self.name, args),
            summary: format!(
                "{} {}",
                self.name,
                args["action"].as_str().unwrap_or("list")
            ),
            risk: themis_core::tools::integration_risk(self.name, args)
                .expect("known management tool"),
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
            json!({"thread_id": self.thread_id, "tool": action.tool, "decision": format!("{decision:?}"), "reason": "auto: own-chat automation lifecycle"}),
        );
        if decision == PolicyAction::Deny {
            return Err(error(format!(
                "approval denied for '{}': the approval rules deny this automation action",
                self.name
            )));
        }
        Ok(())
    }

    /// True when an automation action only touches this chat's own automations.
    async fn targets_own_chat(&self, args: &Value) -> bool {
        match args["action"].as_str().unwrap_or("list") {
            "create" => true,
            "update" | "enable" | "disable" | "delete" | "complete" => {
                let own = self.own_automations().await;
                match args["id"].as_str().filter(|id| !id.is_empty()) {
                    Some(id) => {
                        own.iter().any(|item| item.id == id)
                            || !self
                                .state
                                .list_automations()
                                .await
                                .iter()
                                .any(|item| item.id == id)
                    }
                    None => own.len() == 1,
                }
            }
            _ => false,
        }
    }

    async fn own_automations(&self) -> Vec<Automation> {
        self.state
            .list_automations()
            .await
            .into_iter()
            .filter(|item| item.target_thread_id.as_deref() == Some(self.thread_id.as_str()))
            .collect()
    }

    /// The `id` argument, or the chat's only automation when it is omitted.
    async fn resolve_id(&self, args: &Value) -> Result<String, String> {
        if let Some(id) = args["id"].as_str().filter(|id| !id.is_empty()) {
            return Ok(id.to_owned());
        }
        let own = self.own_automations().await;
        match own.as_slice() {
            [only] => Ok(only.id.clone()),
            [] => Err("This chat has no automations.".to_owned()),
            _ => Err(format!(
                "Several automations belong to this chat; pass id (one of: {}).",
                own.iter()
                    .map(|item| format!("{} \"{}\"", item.id, item.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    async fn automation_action(&self, args: Value) -> Result<Value, String> {
        let action = args["action"].as_str().unwrap_or("list");
        if action == "list" {
            let all = self.state.list_automations().await;
            return Ok(Value::Array(all.iter().map(describe).collect()));
        }
        let id = self.resolve_id(&args).await.or_else(|error| {
            if action == "create" {
                Ok(String::new())
            } else {
                Err(error)
            }
        })?;
        match action {
            "enable" | "disable" => Ok(describe(
                &self
                    .state
                    .set_automation_enabled(id, action == "enable")
                    .await?,
            )),
            "delete" => {
                let removed = self.state.remove_automation(&id, "deleted by agent").await?;
                Ok(json!({"deleted": true, "id": removed.id, "name": removed.name, "reason": "deleted by agent"}))
            }
            "complete" => {
                let removed = self
                    .state
                    .remove_automation(&id, "completed by agent")
                    .await?;
                Ok(json!({"completed": true, "id": removed.id, "name": removed.name, "reason": "completed by agent", "runs": removed.run_count}))
            }
            "create" | "update" => {
                let stored = if action == "update" {
                    Some(
                        self.state
                            .list_automations()
                            .await
                            .into_iter()
                            .find(|item| item.id == id)
                            .ok_or("Unknown automation")?,
                    )
                } else {
                    None
                };
                let mut input = match &stored {
                    Some(stored) => serde_json::to_value(stored).map_err(|e| e.to_string())?,
                    None => json!({"project_root":self.project,"target_thread_id":self.thread_id,"provider":"go","model":"","skill_ids":[],"interval_mins":0,"enabled":true}),
                };
                let patch = automation_patch(&args)?;
                self.merge_patch(&mut input, patch, stored.is_some())?;
                let input: AutomationInput =
                    serde_json::from_value(input).map_err(|e| e.to_string())?;
                let approved = self.approve_check_command(&input, stored.as_ref()).await?;
                let saved = if action == "create" {
                    self.state
                        .create_automation_checked(input, AutomationOrigin::Agent, approved)
                        .await?
                } else {
                    self.state
                        .update_automation_checked(id, input, approved, AutomationOrigin::Agent)
                        .await?
                };
                Ok(describe(&saved))
            }
            _ => Err("Unknown automation action. Use list, create, update, enable, disable, delete or complete.".into()),
        }
    }

    /// A check command runs like the `shell` tool. Lifecycle actions are free,
    /// but an argv the captured policy would ask about needs the user's answer
    /// now, in this interactive run, for exactly this argv, asked as a `shell`
    /// action (command-scoped rules, non-git denial) without any grant cache.
    /// Returns what authorized it. Allow by rule or by the
    /// run's captured YOLO mode stores nothing: the scheduler re-evaluates the
    /// current mode and policy before every execution.
    async fn approve_check_command(
        &self,
        input: &AutomationInput,
        stored: Option<&Automation>,
    ) -> Result<CheckGrant, String> {
        let Some(argv) = &input.watch.check_command else {
            return Ok(CheckGrant::None);
        };
        if stored.is_some_and(|item| item.watch_state.approved_check.as_ref() == Some(argv)) {
            return Ok(CheckGrant::None);
        }
        match shell_policy_for(self.run.mode, &self.run.policy, argv) {
            CheckPolicy::Deny => Err("check_command is denied by the approval rules".to_owned()),
            CheckPolicy::Allow => Ok(CheckGrant::Allowed),
            CheckPolicy::Ask => {
                let mut action = super::automation::check_action(argv);
                if self.run.grants.covers(&action) {
                    return Ok(CheckGrant::Allowed);
                }
                action.summary = format!(
                    "watch check command (runs every {} min until the watch ends)",
                    input.interval_mins
                );
                let decision = self.run.interactive.approve(&action);
                if decision.is_denied() {
                    Err("The user did not approve the check command, so the watch was not saved. Retry without check_command (read the file yourself on each run), or ask the user to allow this exact command.".to_owned())
                } else {
                    if decision == themis_core::tools::Approval::AllowAlways {
                        self.run.grants.remember(&action);
                    }
                    Ok(CheckGrant::Approved)
                }
            }
        }
    }

    /// Applies the agent's fields onto the stored (or default) input. Chat and
    /// project are fixed to the current chat; interval and calendar are mutually
    /// exclusive and fully specified.
    fn merge_patch(
        &self,
        input: &mut Value,
        mut patch: Map<String, Value>,
        updating: bool,
    ) -> Result<(), String> {
        for fixed in ["target_thread_id", "project_root"] {
            if let Some(value) = patch.remove(fixed) {
                if value.as_str() != input[fixed].as_str() && !value.is_null() {
                    return Err(format!(
                        "{fixed} cannot be changed: automations belong to this chat"
                    ));
                }
            }
        }
        for ignored in [
            "provider",
            "model",
            "skill_ids",
            "reasoning_effort",
            "origin",
            "id",
            "action",
        ] {
            patch.remove(ignored);
        }
        let repeat = patch.remove("repeat");
        let repeat = repeat.as_ref().and_then(Value::as_str);
        let interval = patch.remove("interval_mins").filter(|v| !v.is_null());
        let calendar = ["time", "timezone", "weekday"]
            .iter()
            .any(|key| patch.contains_key(*key))
            || matches!(repeat, Some("daily" | "weekdays" | "weekly"));
        let schedule = patch.remove("schedule").filter(|v| !v.is_null());
        let mut schedule = schedule;
        if let Some(repeat) = repeat {
            if !matches!(repeat, "interval" | "daily" | "weekdays" | "weekly") {
                return Err(format!(
                    "repeat must be interval, daily, weekdays or weekly (got \"{repeat}\")"
                ));
            }
        }
        if calendar {
            if schedule.is_some() {
                return Err("Give the calendar schedule either as schedule {repeat,time,timezone,weekday} or as repeat/time/timezone/weekday, not both.".into());
            }
            let mut built = Map::new();
            built.insert("repeat".into(), repeat.unwrap_or("daily").into());
            for key in ["time", "timezone", "weekday"] {
                if let Some(value) = patch.remove(key) {
                    built.insert(key.into(), value);
                }
            }
            built.entry("time").or_insert_with(|| "09:00".into());
            built.entry("timezone").or_insert_with(|| "UTC".into());
            schedule = Some(Value::Object(built));
        }
        if interval.is_some() && (schedule.is_some()) {
            return Err("interval_mins cannot be combined with a calendar schedule (repeat daily/weekdays/weekly). Use interval_mins alone for \"every N minutes\" (2 to 60), or the calendar schedule alone for fixed times.".into());
        }
        if repeat == Some("interval") && interval.is_none() && !updating {
            return Err("repeat \"interval\" needs interval_mins (2 to 60).".into());
        }
        match (interval, schedule) {
            (Some(minutes), None) => {
                input["interval_mins"] = minutes;
                input["schedule"] = Value::Null;
            }
            (None, Some(schedule)) => {
                input["schedule"] = schedule;
                input["interval_mins"] = 60.into();
            }
            (None, None) if !updating && input["schedule"].is_null() => {
                return Err("Say when it runs: interval_mins (2 to 60) for \"every N minutes\", or a calendar schedule such as repeat \"weekdays\" with time \"09:00\" and timezone.".into());
            }
            _ => {}
        }
        if let Some(minutes) = patch.remove("expires_in_mins") {
            if patch.contains_key("expires_at") {
                return Err("Give expires_in_mins or expires_at, not both.".into());
            }
            let minutes = minutes
                .as_i64()
                .filter(|minutes| (1..=CALENDAR_EXPIRY_MINS).contains(minutes))
                .ok_or_else(|| {
                    format!("expires_in_mins must be between 1 and {CALENDAR_EXPIRY_MINS} (7 days; interval watches 1440)")
                })?;
            patch.insert(
                "expires_at".into(),
                (chrono::Utc::now() + chrono::Duration::minutes(minutes))
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                    .into(),
            );
        }
        for (key, value) in patch {
            input[key] = value;
        }
        Ok(())
    }
}

/// Documented `input` fields of `manage_automations` (interval vs calendar, limits, check).
fn automation_input_schema() -> Value {
    json!({
        "type": "object",
        "description": "manage_automations fields. Timing is ONE of: interval_mins (2-60, every N minutes) OR repeat daily|weekdays|weekly with time/timezone (weekdays = Monday to Friday).",
        "properties": {
            "name": {"type": "string"},
            "task": {"type": "string", "description": "What to do on each run."},
            "interval_mins": {"type": "integer", "minimum": 2, "maximum": 60},
            "repeat": {"type": "string", "enum": ["interval", "daily", "weekdays", "weekly"]},
            "time": {"type": "string", "description": "HH:MM local time for calendar repeats."},
            "timezone": {"type": "string", "description": "IANA zone, e.g. Europe/Berlin."},
            "weekday": {"type": "integer", "minimum": 0, "maximum": 6, "description": "0 = Monday; weekly only."},
            "expires_in_mins": {"type": "integer", "minimum": 1, "maximum": 10080, "description": "Interval watches expire after 120 minutes by default (at most 1440); calendar automations after 7 days (at most 10080). Only the user can extend or remove it."},
            "expires_at": {"type": "string", "description": "RFC3339, at most 24 hours ahead."},
            "max_runs": {"type": "integer", "minimum": 1, "maximum": 1000, "description": "Defaults to 60 for interval watches."},
            "stop_when": {"type": "string", "description": "Short plain-language goal shown to the user, e.g. status.txt says DONE."},
            "check_command": {"type": "array", "items": {"type": "string"}, "description": "argv run without the model each interval; the model starts only when its output changes."},
            "enabled": {"type": "boolean"}
        }
    })
}

/// The fields of a create/update call, from `input` or the top level.
fn automation_patch(args: &Value) -> Result<Map<String, Value>, String> {
    match args.get("input") {
        Some(input) => input
            .as_object()
            .cloned()
            .ok_or_else(|| "Automation input must be an object".to_owned()),
        None => Ok(args.as_object().cloned().unwrap_or_default()),
    }
}

/// A stored automation plus the facts the agent should report back.
fn describe(automation: &Automation) -> Value {
    let mut value = serde_json::to_value(automation).unwrap_or(Value::Null);
    value["schedule_summary"] = schedule_summary(automation).into();
    value
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
            "list_plugins" => "Read paginated integration metadata, each plugin's hook definitions and configured/enabled MCP counts in the current project and user scope, without bundled contents. Use kind, offset and limit (default 25, maximum 100); follow next_offset. Enabled does not imply connected or healthy.",
            "save_skill" => "Create or update a personal skill through the shared registry. Supply skill, plugin, scope and expectedRevision when updating.",
            "manage_integrations" => "Manage integrations through shared validated APIs. Actions: list (paginated metadata and MCP counts; kind/offset/limit, next_offset), save, save_skill, save_hook, set_component_enabled, remove_component, enable, disable, delete, marketplaces, sources, updates, check_source, preview_source, scan_marketplace, catalog, preview, add_marketplace, set_marketplace_enabled, remove_marketplace, install, update, import_path, import_json, import_repository, preview_repository, test_mcp, test_hook. import_path takes path; import_json takes content as a JSON string; import_repository and preview_repository take url with optional reference and subdirectory; preview_repository reads package contents without installing. save_hook takes name (the plugin) and hook, and keeps the rest of the plugin; the hook is saved turned off. Component actions take name, kind (skill/mcp/hook), id and enabled for set_component_enabled. Marketplace actions take source or marketplace as appropriate; source is owner/repo, an HTTPS Git URL or a local folder. preview_source reads a source's name, publisher and plugin count before adding it, and add_marketplace takes its name from the source. updates lists installed plugins whose upstream changed, and check_source (marketplace as name) looks for changes now; update installs the newly checked package, and new or changed connections and hooks arrive turned off. preview takes marketplace and name to inspect supported package contents without installing; it reuses the completed marketplace scan. scan_marketplace returns saved status and starts or resumes background work; refresh requests a new scan. test_mcp takes server configuration; test_hook takes hook configuration. New integrations default to global scope, available across projects; preserve the existing scope when editing. Executable operations require approval.",
            _ => "Manage this chat's recurring automations and short-interval watches. No approval is needed for automations of this chat. Actions: list, create, update, enable, disable, delete, complete. Results echo the STORED schedule_summary, interval_mins, schedule, expires_at, max_runs and next_run_at; report those exact facts to the user, never what you intended. Put fields in `input` (or at the top level). create needs name, task and exactly ONE timing: (a) interval watch: interval_mins 2-60 (every N minutes; optionally repeat \"interval\"), or (b) calendar: repeat daily|weekdays|weekly (weekdays = Monday to Friday) with time HH:MM, timezone (IANA, e.g. Europe/Berlin) and weekday 0=Monday for weekly. interval_mins and a calendar schedule cannot be mixed. A watch is temporary: it expires in 2 hours by default (set expires_in_mins up to 1440); calendar automations expire after 7 days by default (up to 10080); only the user can extend or remove an expiry. A watch also stops after max_runs (default 60) runs; set stop_when to a short plain-language goal (shown to the user), and when that goal is met call action complete (with id, optional when this chat has one automation) to end it. Optional check_command (an argv array such as [\"cat\",\"status.txt\"]) polls cheaply: it runs without the model each interval and the model starts only when its output (or exit code) changes. It is a shell command, so it follows the shell approval policy, unlike the lifecycle actions: in Custom mode the user is asked to approve that exact argv when you create or update the watch (declined means the watch is not saved; then create it without check_command and read the file yourself on each run), and it stops if the policy later denies it. Three failures in a row stop an automation. Updates merge input into the saved fields; update with interval_mins switches to an interval, with a calendar schedule switches to the calendar. Automations always continue this chat; delete removes one that is no longer wanted.",
        }
    }
    fn args_schema(&self) -> Value {
        let mut schema = json!({"type":"object","properties":{"action":{"type":"string"},"scope":{"type":"string","enum":["local","global"]},"projectRoot":{"type":"string"},"name":{"type":"string"},"plugin":{"type":"string"},"expectedRevision":{"type":"string"},"skill":{"type":"object"},"spec":{"type":"object"},"id":{"type":"string"},"path":{"type":"string"},"source":{"type":"string"},"url":{"type":"string"},"reference":{"type":"string"},"subdirectory":{"type":"string"},"content":{"type":"string"},"kind":{"type":"string","enum":["plugin","skill","mcp","hook"]},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100},"marketplace":{"type":"string"},"refresh":{"type":"boolean"},"server":{"type":"object"},"hook":{"type":"object"},"enabled":{"type":"boolean"}},"additionalProperties":true});
        schema["properties"]["input"] = automation_input_schema();
        schema
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
    async fn advertised_import_and_component_arguments_use_shared_api() {
        let data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let state = AppState::new_for_test(data.path().join("settings.json"));
        let tools = state.integration_management_tools(
            Arc::new(AllowAllHook),
            project.path().into(),
            "chat".into(),
        );
        let manager = tools
            .iter()
            .find(|tool| tool.name() == "manage_integrations")
            .unwrap();
        for field in [
            "content",
            "url",
            "reference",
            "subdirectory",
            "kind",
            "server",
            "hook",
        ] {
            assert!(manager.args_schema()["properties"].get(field).is_some());
        }
        let content = json!({"name":"imported","mcp":{"example":{"command":"missing-server","enabled":false}}}).to_string();
        manager
            .execute(json!({"action":"import_json","content":content}))
            .await
            .unwrap();
        manager.execute(json!({"action":"set_component_enabled","name":"imported","kind":"mcp","id":"example","enabled":true})).await.unwrap();
        let listed = manager.execute(json!({"action":"list"})).await.unwrap();
        let imported = listed["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|plugin| plugin["spec"]["name"] == "imported")
            .unwrap();
        assert_eq!(imported["spec"]["mcp"]["example"]["enabled"], true);
    }

    #[tokio::test]
    async fn hooks_can_be_created_changed_tested_and_deleted_without_touching_the_plugin() {
        let data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let state = AppState::new_for_test(data.path().join("settings.json"));
        let tools = state.integration_management_tools(
            Arc::new(AllowAllHook),
            project.path().into(),
            "chat".into(),
        );
        let manager = tools
            .iter()
            .find(|tool| tool.name() == "manage_integrations")
            .unwrap();
        state
            .plugin_action(json!({"action":"save","scope":"global","spec":{"name":"guards","files":{"notes.md":"keep me"}}}))
            .await
            .unwrap();
        let hook = |command: &str| json!({"name":"guard","event":"BeforeTool","command":command,"matcher":"shell","enabled":true,"timeout_seconds":5});
        let current = || async {
            let plugins = state.plugin_action(json!({"action":"list"})).await.unwrap();
            plugins
                .as_array()
                .unwrap()
                .iter()
                .find(|plugin| plugin["spec"]["name"] == "guards")
                .unwrap()
                .clone()
        };
        let save = |command: &str, revision: Value| json!({"action":"save_hook","name":"guards","hook":hook(command),"expectedRevision":revision});
        // Create: saved off, even when asked for on; the plugin's files stay.
        let revision = current().await["revision"].clone();
        manager.execute(save("echo one", revision)).await.unwrap();
        let plugin = current().await;
        // The agent reads hooks through `list_plugins`, which has no plugin files.
        let lister = tools
            .iter()
            .find(|tool| tool.name() == "list_plugins")
            .unwrap();
        let listed = lister.execute(json!({"kind":"hook"})).await.unwrap();
        let spec = &listed["items"][0]["spec"];
        assert_eq!(spec["hooks"][0]["command"], "echo one");
        assert_eq!(spec["hooks"][0]["matcher"], "shell");
        assert!(spec.get("files").is_none());
        assert_eq!(plugin["spec"]["hooks"][0]["name"], "guard");
        assert_eq!(plugin["spec"]["hooks"][0]["enabled"], false);
        assert_eq!(plugin["spec"]["files"]["notes.md"], "keep me");
        // Saving it unchanged keeps it on; a revision from before that save is refused.
        let revision = plugin["revision"].clone();
        manager.execute(json!({"action":"set_component_enabled","name":"guards","kind":"hook","id":"guard","enabled":true})).await.unwrap();
        manager
            .execute(save("echo one", revision.clone()))
            .await
            .unwrap();
        assert_eq!(current().await["spec"]["hooks"][0]["enabled"], true);
        assert!(manager.execute(save("echo stale", revision)).await.is_err());
        // Changing the command turns it off.
        let revision = current().await["revision"].clone();
        manager.execute(save("echo two", revision)).await.unwrap();
        let plugin = current().await;
        assert_eq!(plugin["spec"]["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(plugin["spec"]["hooks"][0]["command"], "echo two");
        assert_eq!(plugin["spec"]["hooks"][0]["enabled"], false);
        // Test, then delete.
        manager
            .execute(json!({"action":"test_hook","hook":hook("exit 0")}))
            .await
            .unwrap();
        manager
            .execute(
                json!({"action":"remove_component","name":"guards","kind":"hook","id":"guard"}),
            )
            .await
            .unwrap();
        let plugin = current().await;
        assert!(plugin["spec"]["hooks"].as_array().unwrap().is_empty());
        assert_eq!(plugin["spec"]["files"]["notes.md"], "keep me");
    }

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
        assert_eq!(created["origin"], "agent");
        let updated = tool
            .execute(
                json!({"action":"update","id":created["id"],"input":{"name":"Evening review"}}),
            )
            .await
            .unwrap();
        assert_eq!(updated["task"], "Review recent changes");
        assert_eq!(updated["origin"], "agent");
        assert_eq!(updated["schedule"], created["schedule"]);
        let restarted = AppState::new_for_test(data.path().join("settings.json"));
        assert_eq!(restarted.list_automations().await[0].name, "Evening review");
    }

    async fn watch_tool(
        hook: Arc<dyn ApprovalHook>,
    ) -> (
        AppState,
        Box<dyn ToolT>,
        String,
        tempfile::TempDir,
        tempfile::TempDir,
    ) {
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
        let tool = state
            .integration_management_tools(hook, project.path().into(), thread.id.clone())
            .into_iter()
            .find(|tool| tool.name() == "manage_automations")
            .unwrap();
        (state, tool, thread.id, data, project)
    }

    #[tokio::test]
    async fn interval_and_calendar_are_explicit_exclusive_and_echoed() {
        let (_state, tool, _thread, _data, _project) = watch_tool(Arc::new(AllowAllHook)).await;
        let base = json!({"name":"w","task":"look"});
        let with = |extra: Value| {
            let mut input = base.clone();
            for (key, value) in extra.as_object().unwrap() {
                input[key] = value.clone();
            }
            json!({"action":"create","input":input})
        };
        // Nothing to say when it runs.
        let error = tool.execute(with(json!({}))).await.unwrap_err().to_string();
        assert!(error.contains("interval_mins (2 to 60)"), "{error}");
        // Mixing is rejected with a clear error instead of silently ignoring one.
        let mixed = tool
            .execute(with(json!({"interval_mins":60,"schedule":{"repeat":"daily","time":"00:00","timezone":"UTC"}})))
            .await
            .unwrap_err()
            .to_string();
        assert!(mixed.contains("cannot be combined"), "{mixed}");
        let mixed = tool
            .execute(with(
                json!({"interval_mins":5,"repeat":"weekdays","time":"09:00"}),
            ))
            .await
            .unwrap_err()
            .to_string();
        assert!(mixed.contains("cannot be combined"), "{mixed}");
        // Range is 2..=60 for the agent.
        for bad in [1, 61] {
            let error = tool
                .execute(with(json!({"interval_mins":bad})))
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("between 2 and 60"), "{error}");
        }
        // Interval watch: stored facts are echoed, expiry and run limit default.
        let interval = tool
            .execute(with(
                json!({"repeat":"interval","interval_mins":2,"stop_when":"status.txt says DONE"}),
            ))
            .await
            .unwrap();
        assert_eq!(interval["interval_mins"], 2);
        assert_eq!(interval["schedule_summary"], "every 2 min");
        assert_eq!(interval["max_runs"], 60);
        assert!(interval["schedule"].is_null());
        let expires =
            chrono::DateTime::parse_from_rfc3339(interval["expires_at"].as_str().unwrap()).unwrap();
        let hours = (expires.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_minutes();
        assert!(
            (118..=121).contains(&hours),
            "default expiry is 2 hours, got {hours} min"
        );
        let next = chrono::DateTime::parse_from_rfc3339(interval["next_run_at"].as_str().unwrap())
            .unwrap();
        assert!((next.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds() <= 120);
        // Weekdays is a real option and is echoed as such.
        let weekdays = tool
            .execute(with(
                json!({"repeat":"weekdays","time":"09:00","timezone":"Europe/Berlin"}),
            ))
            .await
            .unwrap();
        assert_eq!(weekdays["schedule"]["repeat"], "weekdays");
        assert_eq!(
            weekdays["schedule_summary"],
            "weekdays (Monday to Friday) at 09:00 Europe/Berlin"
        );
        let hours =
            (chrono::DateTime::parse_from_rfc3339(weekdays["expires_at"].as_str().unwrap())
                .unwrap()
                .with_timezone(&chrono::Utc)
                - chrono::Utc::now())
            .num_hours();
        assert!(
            (166..=168).contains(&hours),
            "calendar default is 7 days, got {hours} h"
        );
        // Switching an interval watch to the calendar clears the interval and back.
        let switched = tool
            .execute(json!({"action":"update","id":weekdays["id"],"input":{"interval_mins":10,"expires_in_mins":30}}))
            .await
            .unwrap();
        assert_eq!(switched["schedule_summary"], "every 10 min");
        assert!(switched["schedule"].is_null());
        // An interval watch is capped at 24 hours.
        let error = tool
            .execute(
                json!({"action":"update","id":weekdays["id"],"input":{"expires_in_mins":1500}}),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("at most 1440 minutes"), "{error}");
        // Complete ends the watch and says so.
        let done = tool
            .execute(json!({"action":"complete","id":interval["id"]}))
            .await
            .unwrap();
        assert_eq!(done["reason"], "completed by agent");
        let left = tool.execute(json!({"action":"list"})).await.unwrap();
        assert_eq!(left.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn agent_calendar_expiry_cannot_be_extended_or_cleared_but_the_user_can() {
        let (state, tool, _thread, _data, _project) = watch_tool(Arc::new(AllowAllHook)).await;
        let created = tool
            .execute(json!({"action":"create","input":{"name":"w","task":"look","repeat":"daily","time":"09:00","timezone":"UTC","expires_in_mins":60}}))
            .await
            .unwrap();
        let id = created["id"].as_str().unwrap();
        let first = created["expires_at"].as_str().unwrap().to_owned();
        // Shorter is fine; longer than 7 days, or removing it, is not.
        tool.execute(json!({"action":"update","id":id,"input":{"expires_in_mins":10080}}))
            .await
            .unwrap();
        for bad in [
            json!({"expires_at": (chrono::Utc::now() + chrono::Duration::days(8)).to_rfc3339()}),
            json!({"expires_at": null}),
            json!({"expires_in_mins": 10081}),
        ] {
            let error = tool
                .execute(json!({"action":"update","id":id,"input":bad}))
                .await
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("the user") || error.contains("between 1 and"),
                "{error}"
            );
        }
        let kept = state.list_automations().await.remove(0);
        assert_ne!(kept.watch.expires_at.as_deref(), Some(first.as_str()));
        // The user's edit (UI or CLI path) can extend it and remove it.
        let mut input = crate::types::AutomationInput {
            name: kept.name.clone(),
            project_root: kept.project_root.clone(),
            target_thread_id: kept.target_thread_id.clone(),
            provider: kept.provider,
            model: String::new(),
            reasoning_effort: None,
            skill_ids: vec![],
            interval_mins: kept.interval_mins.into(),
            schedule: kept.schedule.clone(),
            task: kept.task.clone(),
            enabled: true,
            watch: kept.watch.clone(),
        };
        input.watch.expires_at =
            Some((chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339());
        let extended = state
            .update_automation(id.to_owned(), input.clone())
            .await
            .unwrap();
        assert!(extended.watch.expires_at.is_some());
        input.watch.expires_at = None;
        let cleared = state.update_automation(id.to_owned(), input).await.unwrap();
        assert!(cleared.watch.expires_at.is_none());
        // A user-created calendar automation is unlimited by default.
        let user = state
            .create_automation(crate::types::AutomationInput {
                name: "mine".into(),
                project_root: kept.project_root,
                target_thread_id: kept.target_thread_id,
                provider: kept.provider,
                model: String::new(),
                reasoning_effort: None,
                skill_ids: vec![],
                interval_mins: 60,
                schedule: kept.schedule,
                task: "t".into(),
                enabled: true,
                watch: Default::default(),
            })
            .await
            .unwrap();
        assert!(user.watch.expires_at.is_none());
    }

    #[tokio::test]
    async fn own_chat_needs_no_approval_but_other_chats_still_do() {
        // Denying every approval does not block this chat's own automations.
        let (state, tool, _thread, _data, project) = watch_tool(Arc::new(DenyAllHook)).await;
        let created = tool
            .execute(
                json!({"action":"create","input":{"name":"w","task":"look","interval_mins":5}}),
            )
            .await
            .unwrap();
        tool.execute(json!({"action":"disable","id":created["id"]}))
            .await
            .unwrap();
        tool.execute(json!({"action":"delete","id":created["id"]}))
            .await
            .unwrap();
        // Another chat's automation stays approval-gated for a different chat.
        let other_thread = state
            .create_thread(
                project.path().to_string_lossy().into(),
                super::super::ProviderKind::Go,
                Some("model".into()),
            )
            .await
            .unwrap();
        let foreign = state
            .create_automation(AutomationInput {
                name: "theirs".into(),
                project_root: project.path().to_string_lossy().into(),
                target_thread_id: Some(other_thread.id),
                provider: super::super::ProviderKind::Go,
                model: String::new(),
                reasoning_effort: None,
                skill_ids: vec![],
                interval_mins: 30,
                schedule: None,
                task: "other".into(),
                enabled: true,
                watch: Default::default(),
            })
            .await
            .unwrap();
        for action in ["delete", "complete", "disable"] {
            let error = tool
                .execute(json!({"action":action,"id":foreign.id}))
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("approval denied"), "{action}: {error}");
        }
        assert_eq!(state.list_automations().await.len(), 1);
    }

    fn marker_check(project: &std::path::Path, name: &str) -> (Vec<String>, std::path::PathBuf) {
        (vec!["touch".into(), name.into()], project.join(name))
    }

    async fn tick(state: &AppState, id: &str) {
        let sink: Arc<dyn crate::sink::EventSink> = Arc::new(crate::sink::ChannelSink::channel().0);
        state
            .set_automation_next_run_for_test(id, "2001-01-01T00:00:00Z".into())
            .await
            .unwrap();
        state.tick_automations_once(&sink).await;
    }

    fn create_args(argv: &[String]) -> Value {
        json!({"action":"create","input":{"name":"w","task":"look","interval_mins":2,"check_command":argv}})
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unapproved_check_commands_never_execute() {
        // Custom, no rules, the user answers no: nothing is saved, nothing runs.
        let (state, tool, _thread, data, project) = watch_tool(Arc::new(DenyAllHook)).await;
        let (argv, marker) = marker_check(project.path(), "ran.marker");
        let error = tool
            .execute(create_args(&argv))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("did not approve"), "{error}");
        assert!(state.list_automations().await.is_empty());
        // The lifecycle itself stays free: the same watch without a check works.
        let plain = tool
            .execute(
                json!({"action":"create","input":{"name":"w","task":"look","interval_mins":2}}),
            )
            .await
            .unwrap();
        tick(&state, plain["id"].as_str().unwrap()).await;
        assert!(!marker.exists());
        // A client that writes the record without an approval cannot run it either.
        let forged = state
            .create_automation_checked(
                crate::types::AutomationInput {
                    name: "forged".into(),
                    project_root: project.path().to_string_lossy().into(),
                    target_thread_id: None,
                    provider: super::super::ProviderKind::Go,
                    model: "m".into(),
                    reasoning_effort: None,
                    skill_ids: vec![],
                    interval_mins: 2,
                    schedule: None,
                    task: "t".into(),
                    enabled: true,
                    watch: crate::types::WatchSettings {
                        check_command: Some(argv.clone()),
                        ..Default::default()
                    },
                },
                AutomationOrigin::Agent,
                CheckGrant::None,
            )
            .await;
        assert!(forged.unwrap_err().contains("needs approval"));
        drop(data);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn approval_is_bound_to_the_exact_argv_and_policy_is_rechecked() {
        let (state, tool, thread, data, project) = watch_tool(Arc::new(AllowAllHook)).await;
        let (argv, marker) = marker_check(project.path(), "ran.marker");
        let created = tool.execute(create_args(&argv)).await.unwrap();
        let id = created["id"].as_str().unwrap().to_owned();
        assert_eq!(created["check_command"], json!(argv));
        tick(&state, &id).await;
        assert!(marker.exists(), "approved exact argv runs");

        // The scheduler never asks: a different argv has no approval of its own.
        let stored = state.list_automations().await.remove(0);
        assert_eq!(stored.watch_state.approved_check, Some(argv.clone()));
        let denying = state.integration_management_tools(
            Arc::new(DenyAllHook),
            project.path().into(),
            thread.clone(),
        );
        let manage = denying
            .iter()
            .find(|t| t.name() == "manage_automations")
            .unwrap();
        let (other, other_marker) = marker_check(project.path(), "other.marker");
        let error = manage
            .execute(json!({"action":"update","id":id,"input":{"check_command":other}}))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("did not approve"), "{error}");
        // Changing the argv through an approving run re-binds the approval to the new argv only.
        tool.execute(json!({"action":"update","id":id,"input":{"check_command":other}}))
            .await
            .unwrap();
        let rebound = state.list_automations().await.remove(0);
        assert_eq!(rebound.watch_state.approved_check, Some(other.clone()));
        std::fs::remove_file(&marker).unwrap();
        tick(&state, &id).await;
        assert!(other_marker.exists() && !marker.exists());

        // A deny rule added later stops it and says why.
        std::fs::write(
            data.path().join("runtime.jsonc"),
            r#"{"approval":{"rules":[{"tool":"shell","action":"deny","command":["touch"]}]}}"#,
        )
        .unwrap();
        std::fs::remove_file(&other_marker).unwrap();
        tick(&state, &id).await;
        assert!(!other_marker.exists());
        let paused = state.list_automations().await.remove(0);
        assert!(!paused.enabled);
        assert_eq!(
            paused.watch_state.stopped_reason.as_deref(),
            Some("check command denied by approval rules")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn allow_rules_and_yolo_run_without_a_prompt_until_the_policy_changes() {
        // An allow rule: created and run with an approver that would refuse everything.
        let (state, tool, thread, data, project) = watch_tool(Arc::new(DenyAllHook)).await;
        let _ = tool;
        let (argv, marker) = marker_check(project.path(), "ran.marker");
        std::fs::write(
            data.path().join("runtime.jsonc"),
            r#"{"approval":{"rules":[{"tool":"shell","command":["touch"],"action":"allow"}]}}"#,
        )
        .unwrap();
        // A new run captures the new policy.
        let tool = rebuild(&state, Arc::new(DenyAllHook), project.path(), &thread);
        let allowed = tool.execute(create_args(&argv)).await.unwrap();
        assert!(state.list_automations().await[0]
            .watch_state
            .approved_check
            .is_none());
        tick(&state, allowed["id"].as_str().unwrap()).await;
        assert!(marker.exists());
        // Rules gone, so Ask again: the allow-rule watch stops (no stored approval).
        std::fs::remove_file(data.path().join("runtime.jsonc")).unwrap();
        std::fs::remove_file(&marker).unwrap();
        tick(&state, allowed["id"].as_str().unwrap()).await;
        assert!(!marker.exists());
        assert_eq!(
            state.list_automations().await[0]
                .watch_state
                .stopped_reason
                .as_deref(),
            Some("check command needs approval")
        );
        state
            .delete_automation(allowed["id"].as_str().unwrap().into())
            .await
            .unwrap();

        // YOLO chat: created and run unprompted, but switching to Custom stops it.
        state
            .set_thread_approval_mode(
                thread.clone(),
                themis_core::configuration::ApprovalMode::Yolo,
            )
            .await
            .unwrap();
        let tool = rebuild(&state, Arc::new(DenyAllHook), project.path(), &thread);
        let yolo = tool.execute(create_args(&argv)).await.unwrap();
        let id = yolo["id"].as_str().unwrap();
        tick(&state, id).await;
        assert!(marker.exists());
        state
            .set_thread_approval_mode(
                state.list_automations().await[0]
                    .target_thread_id
                    .clone()
                    .unwrap(),
                themis_core::configuration::ApprovalMode::Custom,
            )
            .await
            .unwrap();
        std::fs::remove_file(&marker).unwrap();
        tick(&state, id).await;
        assert!(!marker.exists());
        assert!(!state.list_automations().await[0].enabled);
    }

    fn rebuild(
        state: &AppState,
        hook: Arc<dyn ApprovalHook>,
        project: &std::path::Path,
        thread: &str,
    ) -> Box<dyn ToolT> {
        state
            .integration_management_tools(hook, project.into(), thread.to_owned())
            .into_iter()
            .find(|tool| tool.name() == "manage_automations")
            .unwrap()
    }

    type Asked = Arc<std::sync::Mutex<Vec<(String, Vec<String>)>>>;
    fn always_hook() -> (Arc<dyn ApprovalHook>, Asked) {
        let asked: Asked = Arc::default();
        let seen = asked.clone();
        let hook = themis_core::tools::FnHook::new(move |action: &ToolAction| {
            seen.lock()
                .unwrap()
                .push((action.tool.clone(), action.command.clone()));
            themis_core::tools::Approval::AllowAlways
        });
        (Arc::new(hook), asked)
    }

    #[tokio::test]
    async fn check_prompts_are_exact_shell_actions_and_grants_never_widen() {
        let (hook, asked) = always_hook();
        let (state, _tool, thread, data, project) = watch_tool(hook.clone()).await;
        let tool = rebuild(&state, hook.clone(), project.path(), &thread);
        let a: Vec<String> = vec!["cat".into(), "a.txt".into()];
        let b: Vec<String> = vec!["curl".into(), "https://example.invalid".into()];
        tool.execute(create_args(&a)).await.unwrap();
        // "Always allow" for argv A does not cover argv B.
        tool.execute(create_args(&b)).await.unwrap();
        let log = asked.lock().unwrap().clone();
        assert_eq!(log, vec![("shell".to_owned(), a), ("shell".to_owned(), b)]);
        // default allow + a shell ask rule still prompts (policy sees a shell action).
        std::fs::write(
            data.path().join("runtime.jsonc"),
            r#"{"approval":{"default":"allow","rules":[{"tool":"shell","action":"ask"}]}}"#,
        )
        .unwrap();
        let tool = rebuild(&state, hook.clone(), project.path(), &thread);
        tool.execute(create_args(&["cat".into(), "c.txt".into()]))
            .await
            .unwrap();
        assert_eq!(asked.lock().unwrap().len(), 3);
        // Without the ask rule, default allow runs it with no prompt.
        std::fs::write(
            data.path().join("runtime.jsonc"),
            r#"{"approval":{"default":"allow","rules":[]}}"#,
        )
        .unwrap();
        let tool = rebuild(&state, hook, project.path(), &thread);
        tool.execute(create_args(&["cat".into(), "d.txt".into()]))
            .await
            .unwrap();
        assert_eq!(asked.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn deny_rules_still_apply_to_own_chat_lifecycle() {
        let (state, _tool, thread, data, project) = watch_tool(Arc::new(AllowAllHook)).await;
        let plain = json!({"action":"create","input":{"name":"w","task":"look","interval_mins":5}});
        for config in [
            r#"{"approval":{"rules":[{"tool":"manage_automations*","action":"deny"}]}}"#,
            r#"{"approval":{"default":"deny","rules":[]}}"#,
        ] {
            std::fs::write(data.path().join("runtime.jsonc"), config).unwrap();
            let tool = rebuild(&state, Arc::new(AllowAllHook), project.path(), &thread);
            let error = tool.execute(plain.clone()).await.unwrap_err().to_string();
            assert!(error.contains("approval denied"), "{config}: {error}");
        }
        assert!(state.list_automations().await.is_empty());
        // Ask (the default) proceeds silently, as decided.
        std::fs::remove_file(data.path().join("runtime.jsonc")).unwrap();
        let tool = rebuild(&state, Arc::new(DenyAllHook), project.path(), &thread);
        tool.execute(plain).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn captured_yolo_is_not_a_stored_approval_and_a_paused_watch_can_be_reapproved() {
        let (state, _tool, thread, _data, project) = watch_tool(Arc::new(DenyAllHook)).await;
        let (argv, marker) = marker_check(project.path(), "ran.marker");
        state
            .set_thread_approval_mode(
                thread.clone(),
                themis_core::configuration::ApprovalMode::Yolo,
            )
            .await
            .unwrap();
        // The run captured YOLO ...
        let tool = rebuild(&state, Arc::new(DenyAllHook), project.path(), &thread);
        // ... and the user flips the chat to Custom mid-run.
        state
            .set_thread_approval_mode(
                thread.clone(),
                themis_core::configuration::ApprovalMode::Custom,
            )
            .await
            .unwrap();
        let created = tool.execute(create_args(&argv)).await.unwrap();
        let id = created["id"].as_str().unwrap().to_owned();
        assert!(state.list_automations().await[0]
            .watch_state
            .approved_check
            .is_none());
        tick(&state, &id).await;
        assert!(!marker.exists(), "Custom policy applies at execution");
        let paused = state.list_automations().await.remove(0);
        assert_eq!(
            paused.watch_state.stopped_reason.as_deref(),
            Some("check command needs approval")
        );
        // A later interactive run may approve the SAME argv again.
        let (hook, asked) = always_hook();
        let tool = rebuild(&state, hook, project.path(), &thread);
        tool.execute(json!({"action":"update","id":id,"input":{"check_command":argv}}))
            .await
            .unwrap();
        assert_eq!(asked.lock().unwrap().len(), 1);
        assert_eq!(
            state.list_automations().await[0].watch_state.approved_check,
            Some(argv)
        );
        state
            .set_automation_enabled(id.clone(), true)
            .await
            .unwrap();
        tick(&state, &id).await;
        assert!(marker.exists());
    }

    #[tokio::test]
    async fn inventory_returns_counts_without_bundled_content() {
        let data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let state = AppState::new_for_test(data.path().join("settings.json"));
        state.plugin_action(json!({"action":"import_json","name":"large","content":json!({"name":"large","files":{"README.md":"bulk".repeat(60_000)},"mcp":{"example":{"command":"missing-server","enabled":false}}}).to_string()})).await.unwrap();
        let tools = state.integration_management_tools(
            Arc::new(AllowAllHook),
            project.path().into(),
            "chat".into(),
        );
        for name in ["list_plugins", "manage_integrations"] {
            let listed = tools
                .iter()
                .find(|t| t.name() == name)
                .unwrap()
                .execute(json!({"action":"list","kind":"mcp"}))
                .await
                .unwrap();
            assert_eq!(listed["counts"]["mcp"]["total"], 1);
            assert_eq!(listed["counts"]["mcp"]["enabled"], 0);
            assert_eq!(listed["items"][0]["spec"]["name"], "large");
            assert!(listed["items"][0]["spec"].get("files").is_none());
            assert!(listed.to_string().len() < 4096);
        }
        let default = tools
            .iter()
            .find(|t| t.name() == "manage_integrations")
            .unwrap()
            .execute(json!({}))
            .await
            .unwrap();
        assert_eq!(default["counts"]["mcp"]["total"], 1);
        assert!(default["items"][0]["spec"].get("files").is_none());
        for args in [
            json!({"limit":0}),
            json!({"limit":101}),
            json!({"offset":2,"kind":"mcp"}),
            json!({"kind":"unknown"}),
        ] {
            let mut args = args;
            args["action"] = "inventory".into();
            assert!(state.plugin_action(args).await.is_err());
        }
        // The UI/CLI's complete inspection contract and durable contents are unchanged.
        let full = state.plugin_action(json!({"action":"list"})).await.unwrap();
        assert_eq!(
            full[0]["spec"]["files"]["README.md"]
                .as_str()
                .unwrap()
                .len(),
            240_000
        );
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
        assert_eq!(saved[0]["scope"], "global");
        assert!(state
            .plugin_store(None)
            .available_skills()
            .unwrap()
            .iter()
            .any(|skill| skill.name == "Review"));
        let restarted = AppState::new_for_test(data.path().join("settings.json"));
        assert!(restarted
            .prompt_skills(Some(project.path().to_string_lossy().into()))
            .await
            .unwrap()
            .iter()
            .any(|s| s.name == "Review"));
    }
}
