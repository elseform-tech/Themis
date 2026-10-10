//! Versioned runtime configuration and immutable per-run approval policy.
use crate::tools::{Approval, ApprovalHook, RiskLevel, ToolAction};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    Yolo,
    #[default]
    Custom,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Allow,
    #[default]
    Ask,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRule {
    pub tool: String,
    pub action: PolicyAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApprovalPolicy {
    pub default: PolicyAction,
    pub rules: Vec<ApprovalRule>,
}
impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            default: PolicyAction::Ask,
            rules: vec![ApprovalRule {
                tool: "*".into(),
                action: PolicyAction::Allow,
                risk: Some("read".into()),
            }],
        }
    }
}
impl ApprovalPolicy {
    pub fn decision(&self, action: &ToolAction) -> PolicyAction {
        self.rules
            .iter()
            .find(|rule| {
                let matches = rule
                    .tool
                    .strip_suffix('*')
                    .map_or(rule.tool == action.tool, |prefix| {
                        action.tool.starts_with(prefix)
                    });
                matches
                    && rule
                        .risk
                        .as_ref()
                        .is_none_or(|risk| risk == risk_name(action.risk))
            })
            .map_or(self.default, |rule| rule.action)
    }
}
fn risk_name(risk: RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Read => "read",
        RiskLevel::Write => "write",
        RiskLevel::Execute => "execute",
        RiskLevel::Network => "network",
        RiskLevel::Destructive => "destructive",
    }
}

struct ConfiguredHook {
    mode: ApprovalMode,
    policy: ApprovalPolicy,
    interactive: Arc<dyn ApprovalHook>,
    allowed: std::sync::Mutex<std::collections::HashSet<(String, &'static str)>>,
}
impl ApprovalHook for ConfiguredHook {
    fn approve(&self, action: &ToolAction) -> Approval {
        if self.mode == ApprovalMode::Yolo {
            return Approval::AllowOnce;
        }
        match self.policy.decision(action) {
            PolicyAction::Allow => Approval::AllowOnce,
            PolicyAction::Deny => Approval::Deny,
            PolicyAction::Ask => {
                let key = (action.tool.clone(), risk_name(action.risk));
                if self
                    .allowed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains(&key)
                {
                    return Approval::AllowOnce;
                }
                match self.interactive.approve(action) {
                    Approval::AllowAlways => {
                        self.allowed
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(key);
                        Approval::AllowOnce
                    }
                    decision => decision,
                }
            }
        }
    }
}
/// Wrap the existing UI approval hook; automatic decisions are never cached by tool name.
pub fn configured_hook(
    mode: ApprovalMode,
    policy: ApprovalPolicy,
    interactive: Arc<dyn ApprovalHook>,
) -> Arc<dyn ApprovalHook> {
    Arc::new(ConfiguredHook {
        mode,
        policy,
        interactive,
        allowed: Default::default(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CompactionConfig {
    pub timeout_seconds: u64,
    pub request_timeout_seconds: u64,
    /// Overall attachment preparation bound, independent of each document's job.
    pub document_batch_timeout_seconds: u64,
    /// Initial summary HTTP 429 backoff; subsequent retries double this floor.
    pub rate_limit_backoff_seconds: u64,
    /// Internal full-document preparation input capacity; ordinary work keeps its own budget.
    pub document_context_token_budget: usize,
    pub maintenance_threshold_ratio: f64,
    pub semantic_threshold_ratio: f64,
    pub target_headroom_interactions: usize,
    pub max_stages: usize,
    pub retry_limit: u8,
    pub section_max_tokens: u32,
    pub summary_max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_file: Option<String>,
    /// Resolved before a run starts; never accepted from JSON.
    #[serde(skip)]
    pub prompt: Option<String>,
}
impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: 900,
            request_timeout_seconds: 300,
            document_batch_timeout_seconds: 3600,
            rate_limit_backoff_seconds: 10,
            document_context_token_budget: 200000,
            maintenance_threshold_ratio: 0.83,
            semantic_threshold_ratio: 0.93,
            target_headroom_interactions: 6,
            max_stages: 32,
            retry_limit: 2,
            section_max_tokens: 4096,
            summary_max_tokens: 8192,
            prompt_file: None,
            prompt: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    pub level: String,
    pub context_metrics: ContextMetricsLevel,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextMetricsLevel {
    Off,
    #[default]
    Summary,
    Detailed,
}
impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            context_metrics: ContextMetricsLevel::Summary,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkillConfig {
    pub paths: Vec<String>,
}
impl Default for SkillConfig {
    fn default() -> Self {
        Self {
            paths: [".agents/skills", ".codex/skills", ".claude/skills"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeConfig {
    pub skills: SkillConfig,
    pub instructions: Vec<String>,
    pub logging: LoggingConfig,
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub schema_version: u32,
    pub approval: ApprovalPolicy,
    pub compaction: CompactionConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_token_budget: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_messages: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_turns: Option<usize>,
}
impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            skills: SkillConfig::default(),
            instructions: Vec::new(),
            logging: LoggingConfig::default(),
            schema: None,
            schema_version: 1,
            approval: ApprovalPolicy::default(),
            compaction: CompactionConfig::default(),
            context_token_budget: None,
            recent_messages: None,
            total_turns: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedConfig {
    pub config: RuntimeConfig,
    pub provenance: BTreeMap<String, String>,
}

pub fn parse(content: &str) -> Result<Value> {
    ensure!(content.len() <= 1024 * 1024, "configuration exceeds 1 MiB");
    let mut bytes = content.as_bytes().to_vec();
    let mut i = 0;
    let mut quoted = false;
    let mut escaped = false;
    while i < bytes.len() {
        if quoted {
            if escaped {
                escaped = false;
            } else if bytes[i] == b'\\' {
                escaped = true;
            } else if bytes[i] == b'"' {
                quoted = false;
            }
            i += 1;
            continue;
        }
        if bytes[i] == b'"' {
            quoted = true;
            i += 1;
            continue;
        }
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                bytes[i] = b' ';
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            bytes[i] = b' ';
            bytes[i + 1] = b' ';
            i += 2;
            let mut closed = false;
            while i < bytes.len() {
                if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    bytes[i] = b' ';
                    bytes[i + 1] = b' ';
                    i += 2;
                    closed = true;
                    break;
                }
                if bytes[i] != b'\n' {
                    bytes[i] = b' ';
                }
                i += 1;
            }
            ensure!(closed, "unterminated configuration comment");
            continue;
        }
        i += 1;
    }
    quoted = false;
    escaped = false;
    for i in 0..bytes.len() {
        if quoted {
            if escaped {
                escaped = false;
            } else if bytes[i] == b'\\' {
                escaped = true;
            } else if bytes[i] == b'"' {
                quoted = false;
            }
            continue;
        }
        if bytes[i] == b'"' {
            quoted = true;
            continue;
        }
        if bytes[i] == b','
            && bytes[i + 1..]
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| *b == b'}' || *b == b']')
        {
            bytes[i] = b' ';
        }
    }
    let value: Value = serde_json::from_slice(&bytes).context("invalid configuration JSONC")?;
    ensure!(value.is_object(), "configuration must be an object");
    Ok(value)
}
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
    for (origin, content) in [("user", user), ("project", project), ("run", explicit)] {
        if let Some(content) = content {
            let mut layer = parse(content)?;
            if origin == "project" {
                if let Some(policy) = layer.as_object_mut().and_then(|m| m.remove("approval")) {
                    let mut policy = policy;
                    ensure!(policy.is_object(), "project approval must be an object");
                    if policy.get("rules").is_none() {
                        policy["rules"] = serde_json::json!([]);
                    }
                    let policy: ApprovalPolicy = serde_json::from_value(policy)
                        .context("invalid project approval policy")?;
                    ensure!(
                        policy.default != PolicyAction::Allow
                            && policy.rules.iter().all(|r| r.action == PolicyAction::Deny),
                        "project approval policy may only add deny rules"
                    );
                    let mut current: ApprovalPolicy =
                        serde_json::from_value(value["approval"].clone())?;
                    let mut rules = policy.rules;
                    rules.extend(current.rules);
                    current.rules = rules;
                    if policy.default == PolicyAction::Deny {
                        current.rules.insert(
                            0,
                            ApprovalRule {
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
            merge(&mut value, layer, origin, "", &mut provenance);
        }
    }
    let config: RuntimeConfig =
        serde_json::from_value(value).context("invalid runtime configuration")?;
    validate(&config)?;
    Ok(ResolvedConfig { config, provenance })
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
        (1..=128).contains(&compaction.max_stages),
        "compaction.max_stages must be 1..128"
    );
    ensure!(
        config.compaction.retry_limit <= 2,
        "compaction.retry_limit must be 0..2"
    );
    for (name, tokens) in [
        ("section_max_tokens", config.compaction.section_max_tokens),
        ("summary_max_tokens", config.compaction.summary_max_tokens),
    ] {
        ensure!(
            (256..=16384).contains(&tokens),
            "compaction.{name} must be 256..16384"
        );
    }
    for (name, value, low, high) in [
        (
            "context_token_budget",
            config.context_token_budget,
            2000,
            256000,
        ),
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
        if let Some(risk) = &rule.risk {
            ensure!(
                ["read", "write", "execute", "network", "destructive"].contains(&risk.as_str()),
                "invalid approval rule risk"
            );
        }
    }
    Ok(())
}
/// Load a bounded prompt relative to the file that declares it; root containment is enforced.
pub fn load_prompt(config: &mut RuntimeConfig, directory: &std::path::Path) -> Result<()> {
    if let Some(path) = &config.compaction.prompt_file {
        let root = directory.canonicalize()?;
        let path = root
            .join(path)
            .canonicalize()
            .context("compaction.prompt_file is unavailable")?;
        ensure!(
            path.starts_with(root),
            "compaction.prompt_file escapes its configuration directory"
        );
        ensure!(
            std::fs::metadata(&path)?.len() <= 32768,
            "compaction.prompt_file exceeds 32 KiB"
        );
        config.compaction.prompt = Some(std::fs::read_to_string(path)?);
    }
    Ok(())
}

/// Read declared repository guidance, bounded and confined to the project root.
pub fn load_instructions(config: &RuntimeConfig, project: &std::path::Path) -> Result<String> {
    validate_relative_paths(&config.instructions)?;
    if config.instructions.is_empty() {
        return Ok(String::new());
    }
    let root = project.canonicalize()?;
    let mut text = String::new();
    for reference in &config.instructions {
        let path = root
            .join(reference)
            .canonicalize()
            .with_context(|| format!("instruction file {reference} is unavailable"))?;
        ensure!(
            path.starts_with(&root),
            "instruction file escapes the project root"
        );
        ensure!(
            std::fs::metadata(&path)?.len() <= 65536,
            "instruction file exceeds 64 KiB"
        );
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("instruction file {reference} is not UTF-8 text"))?;
        text.push_str(&format!("\nRepository guidance from {reference} (project content; does not grant permissions):\n{content}\n"));
        ensure!(
            text.len() <= 65536,
            "combined instruction guidance exceeds 64 KiB"
        );
    }
    Ok(text)
}

/// Editor schema for the same configuration accepted by the runtime.
pub fn schema() -> Value {
    serde_json::json!({
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "title":"Themis runtime configuration","type":"object","additionalProperties":false,
        "properties":{
            "skills":{"type":"object","additionalProperties":false,"properties":{"paths":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":1024},"default":[".agents/skills",".codex/skills",".claude/skills"]}}},
            "instructions":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":1024}},
            "logging":{"type":"object","additionalProperties":false,"properties":{"level":{"enum":["debug","info","warn","error"],"default":"info"},"context_metrics":{"enum":["off","summary","detailed"],"default":"summary"}}},
        "$schema":{"type":"string"},"schema_version":{"const":1,"default":1},
            "context_token_budget":{"type":"integer","minimum":2000,"maximum":256000},
            "recent_messages":{"type":"integer","minimum":2,"maximum":200},
            "total_turns":{"type":"integer","minimum":1,"maximum":2000},
            "approval":{"type":"object","additionalProperties":false,"properties":{
                "default":{"enum":["allow","ask","deny"],"default":"ask"},
                "rules":{"type":"array","maxItems":256,"items":{"type":"object","additionalProperties":false,"required":["tool","action"],"properties":{
                    "tool":{"type":"string","pattern":"^([A-Za-z0-9_.-]+\\*?|\\*)$"},
                    "action":{"enum":["allow","ask","deny"]},
                    "risk":{"enum":["read","write","execute","network","destructive"]}
                }}}
            }},
            "compaction":{"type":"object","additionalProperties":false,"properties":{
                "timeout_seconds":{"type":"integer","minimum":1,"maximum":1800,"default":900},
                "request_timeout_seconds":{"type":"integer","minimum":1,"maximum":300,"default":300},
                "document_batch_timeout_seconds":{"type":"integer","minimum":1,"maximum":21600,"default":3600},
                "rate_limit_backoff_seconds":{"type":"integer","minimum":1,"maximum":300,"default":10},
                "document_context_token_budget":{"type":"integer","minimum":24000,"maximum":1000000,"default":200000},
                "maintenance_threshold_ratio":{"type":"number","minimum":0.5,"maximum":0.95,"default":0.83},
                "semantic_threshold_ratio":{"type":"number","exclusiveMinimum":0.5,"maximum":0.97,"default":0.93},
                "target_headroom_interactions":{"type":"integer","minimum":1,"maximum":20,"default":6},
                "max_stages":{"type":"integer","minimum":1,"maximum":128,"default":32},
                "retry_limit":{"type":"integer","minimum":0,"maximum":2,"default":2},
                "section_max_tokens":{"type":"integer","minimum":256,"maximum":16384,"default":4096},
                "summary_max_tokens":{"type":"integer","minimum":256,"maximum":16384,"default":8192},
                "prompt_file":{"type":"string"}
            }}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_batch_and_summary_backoff_have_bounded_independent_configuration() {
        let mut config = RuntimeConfig::default();
        assert_eq!(config.compaction.document_batch_timeout_seconds, 3600);
        assert_eq!(config.compaction.rate_limit_backoff_seconds, 10);
        for invalid in [0, 21601] {
            config.compaction.document_batch_timeout_seconds = invalid;
            assert!(validate(&config)
                .unwrap_err()
                .to_string()
                .contains("document_batch_timeout_seconds"));
        }
        config.compaction.document_batch_timeout_seconds = 21600;
        for invalid in [0, 301] {
            config.compaction.rate_limit_backoff_seconds = invalid;
            assert!(validate(&config)
                .unwrap_err()
                .to_string()
                .contains("rate_limit_backoff_seconds"));
        }
        config.compaction.rate_limit_backoff_seconds = 300;
        validate(&config).unwrap();
        let schema = schema();
        assert_eq!(
            schema["properties"]["compaction"]["properties"]["document_batch_timeout_seconds"]
                ["maximum"],
            21600
        );
        assert_eq!(
            schema["properties"]["compaction"]["properties"]["rate_limit_backoff_seconds"]
                ["default"],
            10
        );
    }
    #[test]
    fn document_input_capacity_is_validated_independently_of_ordinary_context() {
        let mut config = RuntimeConfig {
            context_token_budget: Some(24000),
            compaction: CompactionConfig {
                document_context_token_budget: 1000000,
                ..Default::default()
            },
            ..Default::default()
        };
        validate(&config).unwrap();
        assert_eq!(config.context_token_budget, Some(24000));
        config.compaction.document_context_token_budget = 1000001;
        assert!(validate(&config)
            .unwrap_err()
            .to_string()
            .contains("document_context_token_budget"));
        config.compaction.document_context_token_budget = 23999;
        assert!(validate(&config).is_err());
    }

    #[test]
    fn adaptive_context_configuration_accepts_256k_and_validates_shared_controls() {
        let source = r#"{"context_token_budget":256000,"compaction":{"maintenance_threshold_ratio":0.83,"semantic_threshold_ratio":0.93,"target_headroom_interactions":6,"timeout_seconds":600,"request_timeout_seconds":300},"logging":{"context_metrics":"detailed"}}"#;
        let resolved = resolve(Some(source), None, None).unwrap();
        assert_eq!(resolved.config.context_token_budget, Some(256000));
        assert_eq!(
            resolved.provenance["compaction.request_timeout_seconds"],
            "user"
        );
        assert!(resolve(Some(r#"{"context_token_budget":256001}"#), None, None).is_err());
        for source in [
            r#"{"compaction":{"maintenance_threshold_ratio":0.96,"semantic_threshold_ratio":0.93}}"#,
            r#"{"compaction":{"semantic_threshold_ratio":1.0}}"#,
            r#"{"compaction":{"target_headroom_interactions":0}}"#,
            r#"{"compaction":{"request_timeout_seconds":0}}"#,
            r#"{"logging":{"context_metrics":"payloads"}}"#,
        ] {
            assert!(resolve(Some(source), None, None).is_err(), "{source}");
        }
    }
    #[test]
    fn longer_compaction_defaults_preserve_explicit_legacy_timeouts_and_match_schema() {
        assert_eq!(
            resolve(None, None, None)
                .unwrap()
                .config
                .compaction
                .timeout_seconds,
            900
        );
        assert!(resolve(
            Some(r#"{"compaction":{"timeout_seconds":300}}"#),
            None,
            None
        )
        .is_ok());
        assert!(resolve(
            Some(r#"{"compaction":{"timeout_seconds":1801}}"#),
            None,
            None
        )
        .is_err());
        assert!(resolve(Some(r#"{"compaction":{"timeout_seconds":0}}"#), None, None).is_err());
        let defaults = resolve(None, None, None).unwrap().config.compaction;
        assert_eq!(defaults.retry_limit, 2);
        assert_eq!(defaults.request_timeout_seconds, 300);
        for limit in 0..=2 {
            assert!(resolve(
                Some(&format!(r#"{{"compaction":{{"retry_limit":{limit}}}}}"#)),
                None,
                None
            )
            .is_ok());
        }
        assert!(resolve(Some(r#"{"compaction":{"retry_limit":3}}"#), None, None).is_err());
        assert_eq!(
            resolve(
                Some(r#"{"compaction":{"timeout_seconds":300,"retry_limit":1}}"#),
                None,
                None
            )
            .unwrap()
            .config
            .compaction
            .timeout_seconds,
            300
        );
        assert!(resolve(
            Some(r#"{"compaction":{"timeout_seconds":1800}}"#),
            None,
            None
        )
        .is_ok());
        let retry = &schema()["properties"]["compaction"]["properties"]["retry_limit"];
        assert_eq!(retry["default"], 2);
        assert_eq!(retry["maximum"], 2);
        let field = &schema()["properties"]["compaction"]["properties"]["timeout_seconds"];
        assert_eq!(field["default"], 900);
        assert_eq!(field["maximum"], 1800);
    }
    #[test]
    fn jsonc_layers_and_validation() {
        let c=resolve(Some("{\"compaction\":{\"timeout_seconds\":90,},//comment\n\"context_token_budget\":10000}"),Some("{\"context_token_budget\":12000}"),None).unwrap();
        assert_eq!(c.config.compaction.timeout_seconds, 90);
        assert_eq!(c.config.context_token_budget, Some(12000));
        assert_eq!(c.provenance["context_token_budget"], "project");
        assert!(resolve(Some("{\"unknown\":1}"), None, None).is_err());
        assert!(parse("{/*").is_err());
        assert!(resolve(None, Some("{\"approval\":7}"), None).is_err());
        assert!(resolve(Some("{\"logging\":{\"level\":\"verbose\"}}"), None, None).is_err());
        assert_eq!(c.provenance["compaction.retry_limit"], "default");
        assert_eq!(
            parse("{\"url\":\"https://x/a/*b\"}").unwrap()["url"],
            "https://x/a/*b"
        );
    }
    #[test]
    fn instructions_are_bounded_project_references() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("AGENTS.md"), "Preserve tests.").unwrap();
        let config = resolve(
            Some(r#"{"instructions":["AGENTS.md"],"skills":{"paths":["local-skills"]}}"#),
            None,
            None,
        )
        .unwrap()
        .config;
        assert!(load_instructions(&config, root.path())
            .unwrap()
            .contains("Preserve tests."));
        assert!(resolve(Some(r#"{"instructions":["../outside.md"]}"#), None, None).is_err());
        assert!(resolve(Some(r#"{"skills":{"paths":["/tmp/skills"]}}"#), None, None).is_err());
        std::fs::write(root.path().join("AGENTS.md"), "x".repeat(65537)).unwrap();
        assert!(load_instructions(&config, root.path()).is_err());
    }

    #[test]
    fn cached_user_grant_cannot_bypass_risk_deny() {
        let policy = ApprovalPolicy {
            default: PolicyAction::Ask,
            rules: vec![ApprovalRule {
                tool: "git".into(),
                action: PolicyAction::Deny,
                risk: Some("destructive".into()),
            }],
        };
        let hook = configured_hook(
            ApprovalMode::Custom,
            policy,
            Arc::new(crate::tools::AllowAllHook),
        );
        let action = |risk| ToolAction {
            tool: "git".into(),
            summary: String::new(),
            risk,
        };
        assert_eq!(hook.approve(&action(RiskLevel::Read)), Approval::AllowOnce);
        assert_eq!(hook.approve(&action(RiskLevel::Read)), Approval::AllowOnce);
        assert_eq!(
            hook.approve(&action(RiskLevel::Destructive)),
            Approval::Deny
        );
    }

    #[test]
    fn policy_order_and_project_restrictions() {
        let config = resolve(
            Some(r#"{"approval":{"default":"deny","rules":[{"tool":"read*","action":"allow"}]}}"#),
            Some(r#"{"approval":{"rules":[{"tool":"read_secret","action":"deny"}]}}"#),
            None,
        )
        .unwrap()
        .config;
        let action = |tool: &str| ToolAction {
            tool: tool.into(),
            summary: String::new(),
            risk: RiskLevel::Read,
        };
        assert_eq!(
            config.approval.decision(&action("read_secret")),
            PolicyAction::Deny
        );
        assert_eq!(
            config.approval.decision(&action("read_file")),
            PolicyAction::Allow
        );
        assert!(resolve(None, Some(r#"{"approval":{"default":"allow"}}"#), None).is_err());
        let hook = configured_hook(
            ApprovalMode::Yolo,
            config.approval,
            Arc::new(crate::tools::DenyAllHook),
        );
        assert_eq!(hook.approve(&action("shell")), Approval::AllowOnce);
    }
}
