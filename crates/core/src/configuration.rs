//! Versioned runtime configuration and immutable per-run approval policy.
use crate::tools::{is_destructive_variant, Approval, ApprovalHook, RiskLevel, ToolAction};
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
    /// Argv prefix (program, then arguments) a `shell` call must start with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
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
                command: None,
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
                    && rule.command.as_ref().is_none_or(|prefix| {
                        // A broad allow (`["git","push"]`) must not silently cover
                        // a destructive variant (`git push --force`) unless the
                        // rule itself names that variant.
                        let covers_variant = rule.action != PolicyAction::Allow
                            || !is_destructive_variant(&action.command)
                            || is_destructive_variant(prefix);
                        covers_variant
                            && (command_has_prefix(&action.command, prefix)
                                || (rule.action == PolicyAction::Deny
                                    && command_has_prefix_normalized(&action.command, prefix)))
                    })
                    && rule
                        .risk
                        .as_ref()
                        .is_none_or(|risk| risk == risk_name(action.risk))
            })
            .map_or(self.default, |rule| rule.action)
    }
}
/// Exact token-prefix match: `["cargo","test"]` matches `cargo test --lib` but
/// not `cargo testing`, `/usr/bin/cargo test` or an empty command.
pub fn command_has_prefix(command: &[String], prefix: &[String]) -> bool {
    !prefix.is_empty() && command.len() >= prefix.len() && command[..prefix.len()] == *prefix
}
/// Normalized program name: basename, lowercased on macOS/Windows
/// (case-insensitive file systems), `.exe`/`.cmd`/`.bat` stripped on Windows.
fn normalize_program(program: &str) -> String {
    let base = program
        .rsplit(|c| c == '/' || (cfg!(windows) && c == '\\'))
        .next()
        .unwrap_or(program);
    let mut name = if cfg!(any(target_os = "macos", windows)) {
        base.to_lowercase()
    } else {
        base.to_owned()
    };
    if cfg!(windows) {
        for ext in [".exe", ".cmd", ".bat"] {
            if let Some(stem) = name.strip_suffix(ext) {
                name = stem.to_owned();
                break;
            }
        }
    }
    name
}
/// Prefix match with the program token normalized on both sides. Used only for
/// deny rules so `/bin/rm`, `./rm` and `RM` (on case-insensitive systems) are
/// still denied by `["rm"]`; allow/ask rules stay exact so they never widen.
fn command_has_prefix_normalized(command: &[String], prefix: &[String]) -> bool {
    match (command.split_first(), prefix.split_first()) {
        (Some((program, rest)), Some((first, wanted))) => {
            normalize_program(program) == normalize_program(first) && rest.starts_with(wanted)
        }
        _ => false,
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
    allowed: std::sync::Mutex<std::collections::HashSet<(String, &'static str)>>, // (grant key, risk)
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
                let key = (action.grant_key(), risk_name(action.risk));
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

/// Only provider models whose reasoning-off controls are verified by wire tests.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SummaryModel {
    #[serde(rename = "longcat-2.5-preview-free")]
    LongCat,
    #[serde(rename = "gpt-5.6-luna")]
    Gpt56Luna,
    #[serde(rename = "gpt-6-luna")]
    Gpt6Luna,
}
impl SummaryModel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LongCat => "longcat-2.5-preview-free",
            Self::Gpt56Luna => "gpt-5.6-luna",
            Self::Gpt6Luna => "gpt-6-luna",
        }
    }
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
    /// Explicit internal routing only; absent retains ordinary-model-based routing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_model: Option<SummaryModel>,
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
            summary_model: None,
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
        if let Some(command) = &rule.command {
            ensure!(
                rule.tool == "shell",
                "approval rule command is only valid with tool \"shell\""
            );
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
                    "risk":{"enum":["read","write","execute","network","destructive"]},
                    "command":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"string","minLength":1,"maxLength":256}}
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
                "summary_model":{"enum":[null,"longcat-2.5-preview-free","gpt-5.6-luna","gpt-6-luna"],"default":null},
                "prompt_file":{"type":"string"}
            }}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_model_override_follows_precedence_and_can_be_cleared() {
        let user = r#"{"compaction":{"summary_model":"gpt-5.6-luna"}}"#;
        let project = r#"{"compaction":{"summary_model":"gpt-6-luna"}}"#;
        let resolved = resolve(Some(user), Some(project), None).unwrap();
        assert_eq!(
            serde_json::to_value(&resolved.config).unwrap()["compaction"]["summary_model"],
            "gpt-6-luna"
        );
        assert_eq!(resolved.provenance["compaction.summary_model"], "project");
        let cleared = resolve(
            Some(user),
            Some(project),
            Some(r#"{"compaction":{"summary_model":null}}"#),
        )
        .unwrap();
        assert!(
            serde_json::to_value(cleared.config).unwrap()["compaction"]["summary_model"].is_null()
        );
        assert_eq!(cleared.provenance["compaction.summary_model"], "run");
        for model in ["step-5-preview-free", "unknown-model", "", "longcat-2.0"] {
            assert!(resolve(
                Some(&format!(
                    r#"{{"compaction":{{"summary_model":"{model}"}}}}"#
                )),
                None,
                None
            )
            .is_err());
        }
    }
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
                command: None,
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
            command: Vec::new(),
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

    fn shell_action(argv: &[&str]) -> ToolAction {
        ToolAction {
            command: argv.iter().map(|t| t.to_string()).collect(),
            tool: "shell".into(),
            summary: String::new(),
            risk: RiskLevel::Execute,
        }
    }

    #[test]
    fn command_rules_match_token_prefixes_only() {
        let config = resolve(
            Some(
                r#"{"approval":{"rules":[
                {"tool":"shell","command":["cargo","test"],"action":"allow"},
                {"tool":"shell","command":["echo"],"action":"allow"}]}}"#,
            ),
            Some(r#"{"approval":{"rules":[{"tool":"shell","command":["rm"],"action":"deny"}]}}"#),
            None,
        )
        .unwrap()
        .config;
        let decide = |argv: &[&str]| config.approval.decision(&shell_action(argv));
        assert_eq!(decide(&["cargo", "test", "--lib"]), PolicyAction::Allow);
        assert_eq!(decide(&["echo"]), PolicyAction::Allow);
        assert_eq!(decide(&["cargo", "build"]), PolicyAction::Ask);
        assert_eq!(decide(&["cargo", "testing"]), PolicyAction::Ask);
        assert_eq!(decide(&["cargo"]), PolicyAction::Ask);
        assert_eq!(decide(&["/bin/echo", "x"]), PolicyAction::Ask);
        assert_eq!(decide(&["rm", "-rf", "x"]), PolicyAction::Deny);
        // Deny rules also catch path-qualified spellings of the program...
        assert_eq!(decide(&["/bin/rm", "x"]), PolicyAction::Deny);
        assert_eq!(decide(&["./rm"]), PolicyAction::Deny);
        // ...but allow rules never widen: a local ./echo is not `echo`.
        assert_eq!(decide(&["./echo", "hi"]), PolicyAction::Ask);
        assert_eq!(decide(&["/bin/echo"]), PolicyAction::Ask);
        // Wrappers still bypass deny rules (tool policy, not a sandbox).
        assert_eq!(decide(&["env", "rm", "x"]), PolicyAction::Ask);
        #[cfg(any(target_os = "macos", windows))]
        assert_eq!(decide(&["RM", "x"]), PolicyAction::Deny);
        #[cfg(windows)]
        assert_eq!(decide(&["rm.exe", "x"]), PolicyAction::Deny);
        // A command rule never matches a tool that carries no argv.
        let mut other = shell_action(&[]);
        other.tool = "git".into();
        assert_eq!(config.approval.decision(&other), PolicyAction::Ask);
    }

    #[test]
    fn command_rule_validation() {
        let bad = |rule: &str| {
            resolve(
                Some(&format!(r#"{{"approval":{{"rules":[{rule}]}}}}"#)),
                None,
                None,
            )
            .is_err()
        };
        assert!(!bad(
            r#"{"tool":"shell","command":["ls"],"action":"allow"}"#
        ));
        assert!(bad(r#"{"tool":"shell","command":[],"action":"allow"}"#));
        assert!(bad(r#"{"tool":"shell","command":[""],"action":"allow"}"#));
        assert!(bad(
            r#"{"tool":"git","command":["status"],"action":"allow"}"#
        ));
        assert!(bad(
            r#"{"tool":"shell*","command":["ls"],"action":"allow"}"#
        ));
        assert!(bad(
            r#"{"tool":"shell","command":["a"],"action":"allow","x":1}"#
        ));
        let seventeen = vec!["a"; 17].join("\",\"");
        assert!(bad(&format!(
            r#"{{"tool":"shell","command":["{seventeen}"],"action":"allow"}}"#
        )));
        let long = "a".repeat(257);
        assert!(bad(&format!(
            r#"{{"tool":"shell","command":["{long}"],"action":"allow"}}"#
        )));
        // Project layer may add command denies but not allows.
        assert!(resolve(
            None,
            Some(r#"{"approval":{"rules":[{"tool":"shell","command":["ls"],"action":"allow"}]}}"#),
            None
        )
        .is_err());
    }

    #[test]
    fn grant_key_scopes_shell_to_program_and_subcommand() {
        let key = |argv: &[&str]| shell_action(argv).grant_key();
        assert_eq!(key(&["cargo", "test", "--lib"]), key(&["cargo", "test"]));
        assert_ne!(key(&["cargo", "test"]), key(&["cargo", "publish"]));
        assert_ne!(key(&["cargo", "test"]), key(&["rm", "-rf", "x"]));
        // Flag-first calls are keyed by their full argv.
        assert_ne!(key(&["bash", "-c", "ls"]), key(&["bash", "-c", "rm x"]));
        assert_eq!(key(&["ls"]), key(&["ls"]));
        // Only subcommand-style CLIs share a key across their arguments.
        assert_eq!(key(&["cargo", "test", "--lib"]), key(&["cargo", "test"]));
        assert_ne!(
            key(&["find", ".", "-name", "x"]),
            key(&["find", ".", "-delete"])
        );
        assert_ne!(
            key(&["sudo", "rm", "foo"]),
            key(&["sudo", "rm", "-rf", "/"])
        );
        assert_ne!(key(&["env", "X=1", "rm"]), key(&["env", "X=1", "ls"]));
        assert_ne!(
            key(&["cargo", "run", "--", "a"]),
            key(&["cargo", "run", "--", "b"])
        );
        assert_ne!(key(&["npm", "exec", "a"]), key(&["npm", "exec", "b"]));
        assert_ne!(key(&["ssh", "host", "ls"]), key(&["ssh", "host", "rm"]));
        let mut git = shell_action(&[]);
        git.tool = "git".into();
        assert_eq!(git.grant_key(), "git");
        // The git tool carries `["git", args...]`: subcommand-scoped, but
        // destructive checkout/branch variants get their own key.
        let git_key = |argv: &[&str]| {
            let mut action = shell_action(argv);
            action.tool = "git".into();
            action.grant_key()
        };
        assert_eq!(git_key(&["git", "add", "a"]), git_key(&["git", "add", "b"]));
        assert_eq!(
            git_key(&["git", "checkout", "main"]),
            git_key(&["git", "checkout", "dev"])
        );
        assert_ne!(
            git_key(&["git", "checkout", "main"]),
            git_key(&["git", "checkout", "-f", "main"])
        );
        assert_ne!(
            git_key(&["git", "branch"]),
            git_key(&["git", "branch", "-D", "x"])
        );
        assert_ne!(
            git_key(&["git", "add", "a"]),
            shell_action(&["git", "add", "a"]).grant_key()
        );
    }

    #[test]
    fn destructive_variants_get_their_own_grant_key() {
        let key = |argv: &[&str]| shell_action(argv).grant_key();
        // Harmless siblings still share a key.
        assert_eq!(
            key(&["git", "push"]),
            key(&["git", "push", "origin", "main"])
        );
        assert_eq!(
            key(&["git", "clean", "-n"]),
            key(&["git", "clean", "--dry-run"])
        );
        assert_eq!(key(&["git", "reset"]), key(&["git", "reset", "HEAD", "f"]));
        assert_eq!(key(&["git", "branch", "-d", "x"]), key(&["git", "branch"]));
        assert!(key(&["cargo", "publish", "--dry-run"]).contains("--dry-run"));
        for (safe, danger) in [
            (vec!["git", "push"], vec!["git", "push", "--force"]),
            (
                vec!["git", "push"],
                vec!["git", "push", "-f", "origin", "main"],
            ),
            (
                vec!["git", "push"],
                vec!["git", "push", "--force-with-lease"],
            ),
            (vec!["git", "push"], vec!["git", "push", "--mirror"]),
            (
                vec!["git", "push"],
                vec!["git", "push", "--delete", "origin", "x"],
            ),
            (vec!["git", "push"], vec!["git", "push", "origin", ":x"]),
            (vec!["git", "push"], vec!["git", "push", "origin", "+main"]),
            (vec!["git", "push"], vec!["git", "push", "-fu"]),
            (vec!["git", "clean", "-n"], vec!["git", "clean", "-fdx"]),
            (vec!["git", "clean", "-n"], vec!["git", "clean", "-nd"]),
            (vec!["git", "reset"], vec!["git", "reset", "--hard"]),
            (
                vec!["git", "checkout", "main"],
                vec!["git", "checkout", "-f", "main"],
            ),
            (
                vec!["git", "checkout", "main"],
                vec!["git", "checkout", "--force", "main"],
            ),
            (
                vec!["git", "switch", "main"],
                vec!["git", "switch", "--discard-changes", "main"],
            ),
            (
                vec!["git", "restore", "--staged", "f"],
                vec!["git", "restore", "f"],
            ),
            (vec!["git", "branch"], vec!["git", "branch", "-D", "x"]),
            (
                vec!["git", "branch"],
                vec!["git", "branch", "--delete", "--force", "x"],
            ),
            (vec!["git", "log"], vec!["git", "rebase", "main"]),
            (vec!["npm", "test"], vec!["npm", "publish"]),
            (vec!["pnpm", "test"], vec!["pnpm", "publish", "--tag", "x"]),
            (vec!["yarn", "test"], vec!["yarn", "publish"]),
            (vec!["cargo", "build"], vec!["cargo", "publish"]),
            (vec!["cargo", "build"], vec!["cargo", "yank", "x"]),
        ] {
            assert_ne!(key(&safe), key(&danger), "{safe:?} vs {danger:?}");
            assert!(
                is_destructive_variant(&danger.iter().map(|t| t.to_string()).collect::<Vec<_>>()),
                "{danger:?}"
            );
        }
        // Different destructive calls do not share a key with each other either.
        assert_ne!(
            key(&["git", "push", "--force", "origin", "a"]),
            key(&["git", "push", "--force", "origin", "b"])
        );
        // Global options before the subcommand do not hide the variant.
        assert!(is_destructive_variant(&[
            "git".into(),
            "-C".into(),
            "d".into(),
            "push".into(),
            "-f".into()
        ]));
        assert!(is_destructive_variant(&[
            "/usr/bin/GIT.exe".into(),
            "reset".into(),
            "--hard".into()
        ]));
        assert!(!is_destructive_variant(&[
            "git".into(),
            "status".into(),
            "-f".into()
        ]));
    }

    #[test]
    fn allow_rule_for_git_push_does_not_cover_forced_push() {
        let config = resolve(
            Some(r#"{"approval":{"rules":[{"tool":"shell","command":["git","push"],"action":"allow"}]}}"#),
            None,
            None,
        )
        .unwrap();
        let policy = config.config.approval.clone();
        let decide = |argv: &[&str]| policy.decision(&shell_action(argv));
        assert_eq!(decide(&["git", "push"]), PolicyAction::Allow);
        assert_eq!(
            decide(&["git", "push", "origin", "main"]),
            PolicyAction::Allow
        );
        for argv in [
            ["git", "push", "--force"].as_slice(),
            &["git", "push", "origin", "main", "-f"],
            &["git", "push", "origin", ":old"],
        ] {
            assert_eq!(decide(argv), PolicyAction::Ask, "{argv:?}");
        }
        // A rule that names the variant explicitly is honored.
        let explicit = resolve(
            Some(r#"{"approval":{"rules":[{"tool":"shell","command":["git","push","--force"],"action":"allow"}]}}"#),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            explicit
                .config
                .approval
                .decision(&shell_action(&["git", "push", "--force", "origin"])),
            PolicyAction::Allow
        );
        // Deny rules are unaffected.
        let deny = resolve(
            Some(r#"{"approval":{"rules":[{"tool":"shell","command":["git","push"],"action":"deny"}]}}"#),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            deny.config
                .approval
                .decision(&shell_action(&["git", "push", "--force"])),
            PolicyAction::Deny
        );
    }

    #[test]
    fn approving_git_push_once_always_still_asks_for_force_push() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let hook = configured_hook(
            ApprovalMode::Custom,
            ApprovalPolicy::default(),
            Arc::new(crate::tools::FnHook::new(move |_: &ToolAction| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Approval::AllowAlways
            })),
        );
        hook.approve(&shell_action(&["git", "push"]));
        hook.approve(&shell_action(&["git", "push", "origin", "main"]));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        hook.approve(&shell_action(&["git", "push", "--force"]));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn cached_shell_grant_does_not_extend_to_another_program() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let hook = configured_hook(
            ApprovalMode::Custom,
            ApprovalPolicy::default(),
            Arc::new(crate::tools::FnHook::new(move |_: &ToolAction| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Approval::AllowAlways
            })),
        );
        assert_eq!(
            hook.approve(&shell_action(&["cargo", "test"])),
            Approval::AllowOnce
        );
        assert_eq!(
            hook.approve(&shell_action(&["cargo", "test", "-q"])),
            Approval::AllowOnce
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        hook.approve(&shell_action(&["rm", "-rf", "x"]));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    mod prop {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn prefix_match_agrees_with_slice_prefix(
                command in proptest::collection::vec(".{0,8}", 0..6),
                prefix in proptest::collection::vec(".{0,8}", 0..6),
            ) {
                let expected = !prefix.is_empty() && command.starts_with(&prefix);
                prop_assert_eq!(command_has_prefix(&command, &prefix), expected);
                // Any non-empty prefix of the command itself always matches.
                for n in 1..=command.len() {
                    prop_assert!(command_has_prefix(&command, &command[..n]));
                }
            }

            #[test]
            fn destructive_flag_never_shares_the_harmless_key(
                sub in proptest::sample::select(vec!["push", "clean", "reset", "checkout", "branch"]),
                tail in proptest::collection::vec("[a-z/:+.-]{0,6}", 0..4),
            ) {
                let argv = |extra: &[String]| {
                    let mut v = vec!["git".to_string(), sub.to_string()];
                    v.extend(extra.iter().cloned());
                    v
                };
                let mk = |command: Vec<String>| ToolAction {
                    command, tool: "shell".into(), summary: String::new(), risk: RiskLevel::Execute,
                };
                let danger = mk(argv(&tail));
                let harmless = mk(argv(&[]));
                if is_destructive_variant(&danger.command) {
                    prop_assert_ne!(danger.grant_key(), harmless.grant_key());
                    // Full argv: never shorter than the whole command.
                    prop_assert_eq!(danger.grant_key().split('\0').count(), 1 + danger.command.len());
                } else {
                    prop_assert_eq!(danger.grant_key(), harmless.grant_key());
                }
            }

            #[test]
            fn is_destructive_variant_never_panics(command in proptest::collection::vec(".{0,8}", 0..6)) {
                let _ = is_destructive_variant(&command);
            }

            #[test]
            fn grant_key_never_panics_and_is_stable(command in proptest::collection::vec(".{0,8}", 0..6)) {
                let action = ToolAction {
                    command,
                    tool: "shell".into(),
                    summary: String::new(),
                    risk: RiskLevel::Execute,
                };
                prop_assert_eq!(action.grant_key(), action.grant_key());
            }
        }
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
            command: Vec::new(),
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
