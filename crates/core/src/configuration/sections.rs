use super::*;

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
    /// Consecutive checkpoints, each within two model requests of the last, after which a run
    /// stops as unable to make progress in its budget; 0 never stops.
    pub thrash_checkpoints: usize,
    pub max_stages: usize,
    pub retry_limit: u8,
    /// Legacy: accepted so older files still load; has no effect on stage
    /// output and is not written or documented.
    #[serde(skip_serializing)]
    pub section_max_tokens: u32,
    pub summary_max_tokens: u32,
    /// Explicit internal routing only; absent retains ordinary-model-based routing. Any model
    /// a connected provider offers (`<provider>/<id>` for one from `providers`); a run checks
    /// it against the captured catalogs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_model: Option<String>,
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
            thrash_checkpoints: 6,
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
/// Memory across chats (`memory`, Settings › Memory); see [`crate::memory`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryConfig {
    /// Keep memories. On, a background pass on the memory models saves, updates and removes
    /// them after each turn, and every run starts with them; off, nothing is saved or read and
    /// saved memories are kept. A project or run layer may only set it to false.
    pub enabled: bool,
    /// The models that run memory jobs, tried in order (Settings › Models › Memory models): a
    /// job moves to the next on a provider error, a time-out, a rate limit, an answer it can't
    /// read, or the daily cap. At least [`MIN_MEMORY_MODELS`]; user file only.
    pub models: Vec<String>,
    /// Estimated dollars (list prices) the memory models may spend in a day, all together;
    /// past it a job waits for tomorrow. Free models are never held back. User file only.
    pub daily_budget_usd: f64,
    /// At most one consolidation (merge duplicates, drop stale memories) this often, when the
    /// app is idle. User file only.
    pub consolidate_every_hours: u32,
    /// How much the memory models think before answering: `auto` (their own thinking for a pass
    /// that carries lessons, the least otherwise), `least` (always the least each model allows,
    /// fastest) or `model` (always each model's own default). User file only.
    pub thinking: MemoryThinking,
}

/// How much the memory models think (`memory.thinking`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryThinking {
    /// The model's own thinking for a pass that carries lessons (the turn ran into failed
    /// steps, a goal completed), the least otherwise. Measured with
    /// `evals/memory`: thinking writes fuller lessons (cause, how to check) at about four times
    /// the tokens, and adds nothing elsewhere.
    #[default]
    Auto,
    /// The least thinking each model allows.
    Least,
    /// Each model's own default thinking.
    Model,
}

/// Fewest models `memory.models` may list.
pub const MIN_MEMORY_MODELS: usize = 3;
/// Most models `memory.models` may list.
pub const MAX_MEMORY_MODELS: usize = 8;
/// Highest `memory.daily_budget_usd`.
pub const MAX_MEMORY_BUDGET_USD: f64 = 100.0;
/// Longest `memory.consolidate_every_hours`: a month.
pub const MAX_CONSOLIDATE_HOURS: u32 = 720;

/// The default memory models: cheap OpenCode Go models on three different wire protocols
/// (Responses, Messages, Chat Completions), so one protocol's outage leaves two.
pub const DEFAULT_MEMORY_MODELS: [&str; 3] = [
    "muse-spark-1.3-contributor",
    "claude-haiku-5-5",
    "deepseek-v4.1-flash",
];

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            models: DEFAULT_MEMORY_MODELS.map(str::to_owned).to_vec(),
            daily_budget_usd: 0.5,
            consolidate_every_hours: 24,
            thinking: MemoryThinking::Auto,
        }
    }
}

/// A project or run layer may only turn memory off; which models run it and what they may
/// spend are the person's own.
pub(super) fn check_memory_restriction(layer: &Value, origin: &str) -> Result<()> {
    if let Some(value) = layer.pointer("/memory/enabled") {
        ensure!(
            value == &Value::Bool(false),
            "the {origin} configuration may only set memory.enabled to false"
        );
    }
    for field in [
        "models",
        "daily_budget_usd",
        "consolidate_every_hours",
        "thinking",
    ] {
        ensure!(
            layer.pointer(&format!("/memory/{field}")).is_none(),
            "the {origin} configuration can't set memory.{field}"
        );
    }
    Ok(())
}

pub(super) fn validate_memory(config: &RuntimeConfig) -> Result<()> {
    let memory = &config.memory;
    ensure!(
        (MIN_MEMORY_MODELS..=MAX_MEMORY_MODELS).contains(&memory.models.len()),
        "memory.models lists {MIN_MEMORY_MODELS} to {MAX_MEMORY_MODELS} models"
    );
    for (index, model) in memory.models.iter().enumerate() {
        ensure!(
            valid_model_id(model),
            "memory.models must list model ids (<provider>/<id> for a model from providers)"
        );
        ensure!(
            !memory.models[..index].contains(model),
            "memory.models lists {model} twice"
        );
        providers::check_model_prefix(&config.providers, "memory.models", model, false)?;
    }
    ensure!(
        memory.daily_budget_usd.is_finite()
            && (0.0..=MAX_MEMORY_BUDGET_USD).contains(&memory.daily_budget_usd),
        "memory.daily_budget_usd must be 0..{MAX_MEMORY_BUDGET_USD}"
    );
    ensure!(
        (1..=MAX_CONSOLIDATE_HOURS).contains(&memory.consolidate_every_hours),
        "memory.consolidate_every_hours must be 1..{MAX_CONSOLIDATE_HOURS}"
    );
    Ok(())
}

/// Subagents (`subagents`): child runs the agent may start; see the subagents design.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SubagentsConfig {
    /// Offer the subagent tools to the agent.
    pub enabled: bool,
    /// Subagents running at once per chat (1-6); more wait their turn.
    pub max_parallel: usize,
    /// Model requests per subagent (1-1000).
    pub max_turns: usize,
    /// Model for subagents; absent, the chat's model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Up to three more models the agent may give a subagent (for example when the person
    /// names one); any other is refused.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    /// `false` keeps every subagent read-only, whatever paths the agent asks for.
    pub allow_writes: bool,
    /// `false` gives no subagent MCP tools, whatever a profile names or the agent asks for.
    pub allow_mcp: bool,
    /// `false` gives no subagent the web tools, whatever a profile names.
    pub allow_web: bool,
    /// Whether a subagent whose profile names `Agent` or `Task` may start subagents of its own.
    pub allow_nested: bool,
}

/// Plugin hooks (`hooks`); the hooks themselves are in the plugin registry, this is how they may
/// act on a chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HooksConfig {
    /// How many turns in a row a finished background hook may start in an idle chat before the
    /// person writes again. 0 never starts one: what a hook reports waits for the next turn.
    pub max_wakes: u32,
    /// Minutes a chat may sit idle before its session ends for plugin hooks (`SessionEnd`, then
    /// `SessionStart` when the person writes again). 0 never ends one for being idle.
    pub session_idle_minutes: u32,
    /// Model requests `prompt` hooks may make in a run, which is what they cost. A guard that finds
    /// none left refuses. 0 turns them off, for the person only: nothing is asked or refused.
    pub prompt_hook_calls: u32,
}

/// Most `hooks.prompt_hook_calls` may say.
pub const MAX_PROMPT_HOOK_CALLS: u32 = 1000;

/// Most `hooks.max_wakes` may say.
pub const MAX_HOOK_WAKES: u32 = 10;
/// Most `hooks.session_idle_minutes` may say: a week.
pub const MAX_HOOK_IDLE_MINUTES: u32 = 7 * 24 * 60;

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            max_wakes: 3,
            session_idle_minutes: 60,
            prompt_hook_calls: 50,
        }
    }
}

/// A project or run layer may only lower how many turns hooks may start, and can't touch what a
/// `prompt` hook may spend: lowering it would switch off a guard the person turned on.
pub(super) fn check_hooks_restriction(
    current: &HooksConfig,
    layer: &Value,
    origin: &str,
) -> Result<()> {
    ensure!(
        layer.pointer("/hooks/prompt_hook_calls").is_none(),
        "the {origin} configuration can't set hooks.prompt_hook_calls"
    );
    if let Some(value) = layer.pointer("/hooks/max_wakes") {
        ensure!(
            value
                .as_u64()
                .is_some_and(|value| value <= u64::from(current.max_wakes)),
            "the {origin} configuration may only lower hooks.max_wakes (at most {})",
            current.max_wakes
        );
    }
    Ok(())
}

/// Most models `subagents.models` may list.
pub const MAX_SUBAGENT_MODELS: usize = 3;
impl Default for SubagentsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_parallel: 3,
            max_turns: 40,
            model: None,
            models: Vec::new(),
            allow_writes: true,
            allow_mcp: true,
            allow_web: true,
            allow_nested: true,
        }
    }
}

/// A project or run layer may only lower subagent limits or turn subagents (or their writes, MCP
/// tools or web tools) off; the model may be set anywhere.
pub(super) fn check_subagents_restriction(
    current: &SubagentsConfig,
    layer: &Value,
    origin: &str,
) -> Result<()> {
    for field in [
        "enabled",
        "allow_writes",
        "allow_mcp",
        "allow_web",
        "allow_nested",
    ] {
        if let Some(value) = layer.pointer(&format!("/subagents/{field}")) {
            ensure!(
                value == &Value::Bool(false),
                "the {origin} configuration may only set subagents.{field} to false"
            );
        }
    }
    if let Some(models) = layer.pointer("/subagents/models") {
        ensure!(
            models
                .as_array()
                .is_some_and(|models| models.iter().all(|model| model
                    .as_str()
                    .is_some_and(|model| current.models.iter().any(|known| known == model)))),
            "the {origin} configuration may only narrow subagents.models to models already listed"
        );
    }
    for (field, limit) in [
        ("max_parallel", current.max_parallel),
        ("max_turns", current.max_turns),
    ] {
        if let Some(value) = layer.pointer(&format!("/subagents/{field}")) {
            ensure!(
                value.as_u64().is_some_and(|value| value as usize <= limit),
                "the {origin} configuration may only lower subagents.{field} (at most {limit})"
            );
        }
    }
    Ok(())
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
