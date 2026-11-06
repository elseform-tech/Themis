use super::*;

fn merge(
    target: &mut Value,
    source: Value,
    origin: &str,
    path: &str,
    provenance: &mut BTreeMap<String, String>,
) {
    if let Some(right) = source.as_object() {
        if !target.is_object() {
            *target = Value::Object(Default::default());
        }
        let left = target.as_object_mut().expect("object initialized above");
        for (key, value) in right {
            let path = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            merge(
                left.entry(key.clone()).or_insert(Value::Null),
                value.clone(),
                origin,
                &path,
                provenance,
            );
        }
    } else {
        *target = source;
        provenance.retain(|key, _| !key.starts_with(&format!("{path}.")));
        provenance.insert(path.into(), origin.into());
    }
}
pub fn resolve(
    user: Option<&str>,
    project: Option<&str>,
    explicit: Option<&str>,
) -> Result<ResolvedConfig> {
    let defaults = serde_json::to_value(RuntimeConfig::default())?;
    let mut value = Value::Object(Default::default());
    let mut provenance = BTreeMap::new();
    merge(&mut value, defaults, "default", "", &mut provenance);
    let mut rule_sources = vec!["default".to_owned(); ApprovalPolicy::default().rules.len()];
    for (origin, content) in [("user", user), ("project", project), ("run", explicit)] {
        if let Some(content) = content {
            let mut layer = parse(content)?;
            if origin != "user" {
                // Project and run layers may only tighten the sandbox.
                if let Some(sandbox) = layer.get("sandbox") {
                    let current: crate::sandbox::SandboxConfig =
                        serde_json::from_value(value["sandbox"].clone())
                            .context("invalid sandbox configuration")?;
                    crate::sandbox::check_restriction(&current, sandbox, origin)?;
                }
                check_web_restriction(&layer, origin)?;
                let subagents: SubagentsConfig = serde_json::from_value(value["subagents"].clone())
                    .context("invalid subagents configuration")?;
                check_subagents_restriction(&subagents, &layer, origin)?;
                let hooks: HooksConfig = serde_json::from_value(value["hooks"].clone())
                    .context("invalid hooks configuration")?;
                check_hooks_restriction(&hooks, &layer, origin)?;
                check_memory_restriction(&layer, origin)?;
                providers::check_provider_restriction(&layer, origin)?;
                let current: BrowserConfig = serde_json::from_value(value["browser"].clone())
                    .context("invalid browser configuration")?;
                tighten_browser_layer(&current, &mut layer, origin)?;
            }
            if origin == "project" {
                if let Some(policy) = layer.as_object_mut().and_then(|m| m.remove("approval")) {
                    let mut policy = policy;
                    ensure!(policy.is_object(), "project approval must be an object");
                    if policy.get("rules").is_none() {
                        policy["rules"] = serde_json::json!([]);
                    }
                    let added_files = take_project_sensitive_files(&mut policy)?;
                    let policy: ApprovalPolicy = serde_json::from_value(policy)
                        .context("invalid project approval policy")?;
                    ensure!(
                        policy.auto_model.is_none(),
                        "approval.auto_model is user file only: a project may not choose who reviews"
                    );
                    ensure!(
                        policy.default != PolicyAction::Allow
                            && policy.rules.iter().all(|r| r.action == PolicyAction::Deny),
                        "project approval policy may only add deny rules"
                    );
                    let mut current: ApprovalPolicy =
                        serde_json::from_value(value["approval"].clone())?;
                    let added =
                        policy.rules.len() + usize::from(policy.default == PolicyAction::Deny);
                    rule_sources.splice(0..0, std::iter::repeat_n("project".to_owned(), added));
                    let mut rules = policy.rules;
                    rules.extend(current.rules);
                    current.rules = rules;
                    if !added_files.is_empty() {
                        current.sensitive_files.extend(added_files);
                        provenance.insert(
                            "approval.sensitive_files".into(),
                            "user + project additions".into(),
                        );
                    }
                    if policy.default == PolicyAction::Deny {
                        current.rules.insert(
                            0,
                            ApprovalRule {
                                command: None,
                                tool: "*".into(),
                                action: PolicyAction::Deny,
                                risk: None,
                            },
                        );
                        current.default = PolicyAction::Deny;
                    }
                    value["approval"] = serde_json::to_value(current)?;
                    provenance.insert(
                        "approval.rules".into(),
                        "user + project restrictions".into(),
                    );
                    if policy.default == PolicyAction::Deny {
                        provenance.insert("approval.default".into(), "project".into());
                    }
                }
            }
            if let Some(rules) = layer
                .get("approval")
                .and_then(|approval| approval.get("rules"))
                .and_then(Value::as_array)
            {
                rule_sources = vec![origin.to_owned(); rules.len()];
            }
            merge(&mut value, layer, origin, "", &mut provenance);
        }
    }
    let config: RuntimeConfig =
        serde_json::from_value(value).context("invalid runtime configuration")?;
    validate(&config)?;
    rule_sources.resize(config.approval.rules.len(), "default".to_owned());
    Ok(ResolvedConfig {
        config,
        provenance,
        rule_sources,
    })
}
/// Takes a project's `approval.sensitive_files`: it adds to the list, and may not exempt.
fn take_project_sensitive_files(policy: &mut Value) -> Result<Vec<String>> {
    let Some(files) = policy
        .as_object_mut()
        .and_then(|fields| fields.remove("sensitive_files"))
    else {
        return Ok(Vec::new());
    };
    let files: Vec<String> = serde_json::from_value(files)
        .context("project approval.sensitive_files must be a list of file names")?;
    ensure!(
        files.iter().all(|file| !file.starts_with('!')),
        "project approval.sensitive_files may only add files, not exempt them"
    );
    Ok(files)
}

fn validate_sensitive_files(files: &[String]) -> Result<()> {
    ensure!(
        files.len() <= 128,
        "approval.sensitive_files lists at most 128 patterns"
    );
    for file in files {
        let glob = file.strip_prefix('!').unwrap_or(file);
        ensure!(
            (1..=128).contains(&glob.len())
                && !glob.contains(['/', '\\'])
                && !glob.chars().any(char::is_control),
            "approval.sensitive_files holds file names of 1..128 bytes with * and ?, not paths: '{file}'"
        );
    }
    Ok(())
}

pub fn validate_relative_paths(paths: &[String]) -> Result<()> {
    ensure!(
        paths.len() <= 32,
        "at most 32 configured paths are supported"
    );
    for path in paths {
        ensure!(
            !path.is_empty() && path.len() <= 1024,
            "configured paths must be 1..1024 bytes"
        );
        ensure!(
            std::path::Path::new(path).components().all(|c| matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )),
            "configured paths must remain relative without parent traversal"
        );
        ensure!(
            path.split(['/', '\\']).all(|part| part != "..")
                && !path.contains(':')
                && !path.starts_with('\\'),
            "configured paths must remain relative without parent traversal"
        );
    }
    Ok(())
}

pub fn validate(config: &RuntimeConfig) -> Result<()> {
    crate::sandbox::validate(&config.sandbox)?;
    validate_web(config)?;
    validate_sensitive_files(&config.approval.sensitive_files)?;
    if let Some(model) = &config.approval.auto_model {
        ensure!(
            valid_model_id(model),
            "approval.auto_model must be a model id (<provider>/<id> for a model from providers)"
        );
    }
    providers::validate(&config.providers, &config.models)?;
    validate_relative_paths(&config.skills.paths)?;
    validate_relative_paths(&config.instructions)?;
    ensure!(
        ["debug", "info", "warn", "error"].contains(&config.logging.level.as_str()),
        "logging.level must be debug/info/warn/error"
    );
    ensure!(
        config.schema_version == 1,
        "unsupported schema_version; expected 1"
    );
    ensure!(
        (1..=1800).contains(&config.compaction.timeout_seconds),
        "compaction.timeout_seconds must be 1..1800"
    );
    let compaction = &config.compaction;
    ensure!(
        (1..=21600).contains(&compaction.document_batch_timeout_seconds),
        "compaction.document_batch_timeout_seconds must be 1..21600"
    );
    ensure!(
        (1..=300).contains(&compaction.rate_limit_backoff_seconds),
        "compaction.rate_limit_backoff_seconds must be 1..300"
    );
    ensure!(
        (24000..=1000000).contains(&compaction.document_context_token_budget),
        "compaction.document_context_token_budget must be 24000..1000000"
    );
    ensure!(
        (1..=300).contains(&compaction.request_timeout_seconds),
        "compaction.request_timeout_seconds must be 1..300"
    );
    ensure!(
        compaction.maintenance_threshold_ratio.is_finite()
            && compaction.semantic_threshold_ratio.is_finite()
            && (0.5..=0.95).contains(&compaction.maintenance_threshold_ratio)
            && compaction.semantic_threshold_ratio > compaction.maintenance_threshold_ratio
            && compaction.semantic_threshold_ratio <= 0.97,
        "compaction thresholds require 0.5 <= maintenance < semantic <= 0.97"
    );
    ensure!(
        (1..=20).contains(&compaction.target_headroom_interactions),
        "compaction.target_headroom_interactions must be 1..20"
    );
    ensure!(
        compaction.thrash_checkpoints <= 50,
        "compaction.thrash_checkpoints must be 0..50"
    );
    ensure!(
        (1..=128).contains(&compaction.max_stages),
        "compaction.max_stages must be 1..128"
    );
    ensure!(
        config.compaction.retry_limit <= 2,
        "compaction.retry_limit must be 0..2"
    );
    validate_memory(config)?;
    for (field, model) in [
        ("compaction.summary_model", &compaction.summary_model),
        ("subagents.model", &config.subagents.model),
    ] {
        if let Some(model) = model {
            ensure!(
                valid_model_id(model),
                "{field} must be a model id (<provider>/<id> for a model from providers)"
            );
            providers::check_model_prefix(&config.providers, field, model, false)?;
        }
    }
    ensure!(
        config.hooks.max_wakes <= MAX_HOOK_WAKES,
        "hooks.max_wakes must be 0..{MAX_HOOK_WAKES}"
    );
    ensure!(
        config.hooks.session_idle_minutes <= MAX_HOOK_IDLE_MINUTES,
        "hooks.session_idle_minutes must be 0..{MAX_HOOK_IDLE_MINUTES}"
    );
    ensure!(
        config.hooks.prompt_hook_calls <= MAX_PROMPT_HOOK_CALLS,
        "hooks.prompt_hook_calls must be 0..{MAX_PROMPT_HOOK_CALLS}"
    );
    ensure!(
        (1..=6).contains(&config.subagents.max_parallel),
        "subagents.max_parallel must be 1..6"
    );
    ensure!(
        (1..=1000).contains(&config.subagents.max_turns),
        "subagents.max_turns must be 1..1000"
    );
    ensure!(
        config.subagents.models.len() <= MAX_SUBAGENT_MODELS,
        "subagents.models lists at most {MAX_SUBAGENT_MODELS} models"
    );
    for model in &config.subagents.models {
        ensure!(
            valid_model_id(model),
            "subagents.models must list model ids (<provider>/<id> for a model from providers)"
        );
        providers::check_model_prefix(&config.providers, "subagents.models", model, false)?;
    }
    for (name, tokens) in [
        ("section_max_tokens", config.compaction.section_max_tokens),
        ("summary_max_tokens", config.compaction.summary_max_tokens),
    ] {
        ensure!(
            (256..=16384).contains(&tokens),
            "compaction.{name} must be 256..16384"
        );
    }
    if let Some(ContextTokenBudget::Tokens(tokens)) = config.context_token_budget {
        let high = crate::context_window::MAX_CONTEXT_TOKENS;
        ensure!(
            (MIN_CONTEXT_TOKENS..=high).contains(&tokens),
            "context_token_budget must be \"auto\" or {MIN_CONTEXT_TOKENS}..{high}"
        );
    }
    for (name, value, low, high) in [
        ("recent_messages", config.recent_messages, 2, 200),
        ("total_turns", config.total_turns, 1, 2000),
    ] {
        if let Some(value) = value {
            ensure!(
                (low..=high).contains(&value),
                "{name} must be {low}..{high}"
            );
        }
    }
    ensure!(
        config.approval.rules.len() <= 256,
        "approval.rules exceeds 256 rules"
    );
    for rule in &config.approval.rules {
        let stem = rule.tool.strip_suffix('*').unwrap_or(&rule.tool);
        ensure!(
            !rule.tool.is_empty()
                && stem
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'),
            "approval rule tool must be an exact name or trailing-star prefix"
        );
        if let Some(command) = &rule.command {
            if crate::tools::is_site_tool(&rule.tool) || rule.tool == "browser_*" {
                ensure!(
                    command.len() == 1 && valid_site(&command[0]),
                    "approval rule command for a web or browser tool must be one site, such as [\"example.com\"]"
                );
            } else {
                ensure!(
                    rule.tool == "shell"
                        || rule.tool == crate::tools::UNSANDBOXED_TOOL
                        || rule.tool == SENSITIVE_TOOL,
                    "approval rule command is only valid with tool \"shell\", \"shell_unsandboxed\" or \"sensitive_file\", or one site with a web or browser tool"
                );
            }
            ensure!(
                !command.is_empty() && command.len() <= 16,
                "approval rule command must have 1..16 tokens"
            );
            ensure!(
                command.iter().all(|t| !t.is_empty() && t.len() <= 256),
                "approval rule command tokens must be 1..256 bytes"
            );
        }
        if let Some(risk) = &rule.risk {
            ensure!(
                ["read", "write", "execute", "network", "destructive"].contains(&risk.as_str()),
                "invalid approval rule risk"
            );
        }
    }
    Ok(())
}
