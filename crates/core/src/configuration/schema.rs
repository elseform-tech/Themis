use super::*;

/// Editor schema for the same configuration accepted by the runtime.
pub fn schema() -> Value {
    let mut schema = base_schema();
    let (providers, models) = providers::schema();
    schema["properties"]["providers"] = providers;
    schema["properties"]["models"] = models;
    // Here because one more key in the base schema's macro reaches the compiler's recursion limit.
    schema["properties"]["project_guidance"] = serde_json::json!({"type":"boolean","default":true});
    schema["properties"]["memory"] = serde_json::json!({"type":"object","additionalProperties":false,"properties":{
        "enabled":{"type":"boolean","default":true,"description":"Keep memories across chats (Settings › Memory): saved, updated and removed in the background after each turn. A project or run layer may only set it to false."},
        "models":{"type":"array","minItems":MIN_MEMORY_MODELS,"maxItems":MAX_MEMORY_MODELS,"uniqueItems":true,"items":{"type":"string","minLength":1,"maxLength":200},"default":DEFAULT_MEMORY_MODELS,"description":"Models that run memory jobs, tried in order (Settings › Models). User file only."},
        "daily_budget_usd":{"type":"number","minimum":0,"maximum":MAX_MEMORY_BUDGET_USD,"default":0.5,"description":"Estimated dollars the memory models may spend in a day, together. User file only."},
        "consolidate_every_hours":{"type":"integer","minimum":1,"maximum":MAX_CONSOLIDATE_HOURS,"default":24,"description":"At most one idle consolidation (merge duplicates, drop stale memories) this often. User file only."},
        "thinking":{"enum":["auto","least","model"],"default":"auto","description":"How much the memory models think before answering: auto (their own thinking for a pass that carries lessons, the least otherwise), least (always the least, fastest) or model (always their own). User file only."}
    }});
    schema
}

fn base_schema() -> Value {
    serde_json::json!({
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "title":"Themis runtime configuration","type":"object","additionalProperties":false,
        "properties":{
            "skills":{"type":"object","additionalProperties":false,"properties":{"paths":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":1024},"default":[".agents/skills",".codex/skills",".claude/skills"]}}},
            "instructions":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":1024}},
            "logging":{"type":"object","additionalProperties":false,"properties":{"level":{"enum":["debug","info","warn","error"],"default":"info"},"context_metrics":{"enum":["off","summary","detailed"],"default":"summary"}}},
        "$schema":{"type":"string"},"schema_version":{"const":1,"default":1},
            "context_token_budget":{"oneOf":[{"const":"auto"},{"type":"integer","minimum":2000,"maximum":1000000}],"default":"auto"},
            "recent_messages":{"type":"integer","minimum":2,"maximum":200},
            "total_turns":{"type":"integer","minimum":1,"maximum":2000},
            "approval":{"type":"object","additionalProperties":false,"properties":{
                "default":{"enum":["allow","ask","deny"],"default":"ask"},
                "sensitive_files":{"type":"array","maxItems":128,"items":{"type":"string","minLength":1,"maxLength":129}},
                "auto_model":{"type":"string","minLength":1,"maxLength":128},
                "rules":{"type":"array","maxItems":256,"items":{"type":"object","additionalProperties":false,"required":["tool","action"],"properties":{
                    "tool":{"type":"string","pattern":"^([A-Za-z0-9_.-]+\\*?|\\*)$"},
                    "action":{"enum":["allow","ask","deny"]},
                    "risk":{"enum":["read","write","execute","network","destructive"]},
                    "command":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"string","minLength":1,"maxLength":256}}
                }}}
            }},
            "sandbox":{"type":"object","additionalProperties":false,"properties":{
                "enabled":{"type":"boolean","default":true},
                "backend":{"enum":["auto","seatbelt","bubblewrap","landlock"],"default":"auto"},
                "network":{"enum":["allow","block"],"default":"allow"},
                "writable_paths":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":1024},"default":[]},
                "allow_keychain":{"type":"boolean","default":false,"description":"macOS: let sandboxed commands use the Keychain; projects may only set false"}
            }},
            "web":{"type":"object","additionalProperties":false,"properties":{
                "search_url":{"type":"string","maxLength":1024,"description":"Your SearXNG instance (JSON output enabled); user file only"}
            }},
            "browser":{"type":"object","additionalProperties":false,"properties":{
                "enabled":{"type":"boolean","default":true},
                "executable":{"type":"string","maxLength":1024,"description":"Chrome, Chromium or Edge binary; user file only"},
                "developer_mode":{"type":"boolean","default":true,"description":"Enable full CDP access: the agent's debugging tools"},
                "permissions":{"type":"object","additionalProperties":false,"properties":{
                    "browsing":{"enum":["allow","ask","block"],"default":"allow"},
                    "downloads":{"enum":["allow","ask","block"],"default":"allow"},
                    "debugging":{"enum":["allow","ask","block"],"default":"block"}
                }},
                "site_exceptions":{"type":"array","maxItems":500,"items":{"type":"object","additionalProperties":false,"required":["site"],"properties":{
                    "site":{"type":"string","minLength":1,"maxLength":253},
                    "browsing":{"enum":["allow","ask","block"]},
                    "downloads":{"enum":["allow","ask","block"]},
                    "debugging":{"enum":["allow","ask","block"]}
                }}},
                "show_secrets":{"type":"boolean","default":false},
                "downloads_directory":{"type":"string","maxLength":1024,"description":"Absolute folder for downloads; user file only"},
                "open_workspace":{"type":"boolean","default":false},
                "screenshots_to_model":{"type":"boolean","default":true,"description":"Screenshots the agent takes also go to its model when the model reads images; projects may only set false"}
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
                "thrash_checkpoints":{"type":"integer","minimum":0,"maximum":50,"default":6},
                "max_stages":{"type":"integer","minimum":1,"maximum":128,"default":32},
                "retry_limit":{"type":"integer","minimum":0,"maximum":2,"default":2},
                "section_max_tokens":{"type":"integer","minimum":256,"maximum":16384,"deprecated":true},
                "summary_max_tokens":{"type":"integer","minimum":256,"maximum":16384,"default":8192},
                "summary_model":{"type":["string","null"],"minLength":1,"maxLength":256,"default":null,"description":"Any model a connected provider offers; <provider>/<id> for one from providers"},
                "prompt_file":{"type":"string"}
            }},
            "subagents":{"type":"object","additionalProperties":false,"properties":{
                "enabled":{"type":"boolean","default":true,"description":"Offer the subagent tools to the agent"},
                "max_parallel":{"type":"integer","minimum":1,"maximum":6,"default":3,"description":"Subagents running at once per chat"},
                "max_turns":{"type":"integer","minimum":1,"maximum":1000,"default":40,"description":"Model requests per subagent"},
                "model":{"type":["string","null"],"minLength":1,"maxLength":256,"default":null,"description":"Model for subagents; absent, the chat's model. <provider>/<id> for one from providers"},
                "models":{"type":"array","maxItems":3,"uniqueItems":true,"items":{"type":"string","minLength":1,"maxLength":256},"default":[],"description":"Up to three more models the agent may give a subagent; any other is refused"},
                "allow_writes":{"type":"boolean","default":true,"description":"false keeps every subagent read-only"},
                "allow_mcp":{"type":"boolean","default":true,"description":"false gives no subagent MCP tools"},
                "allow_web":{"type":"boolean","default":true,"description":"false gives no subagent the web tools"},
                "allow_nested":{"type":"boolean","default":true,"description":"false stops subagents starting subagents, whatever a plugin's profile names. A project or run layer may only set it to false."}
            }},
            "hooks":{"type":"object","additionalProperties":false,"properties":{
                "max_wakes":{"type":"integer","minimum":0,"maximum":10,"default":3,"description":"Turns in a row a finished background plugin hook may start in an idle chat before you write again; 0 never starts one, and what a hook reports waits for the next turn"},
                "session_idle_minutes":{"type":"integer","minimum":0,"maximum":10080,"default":60,"description":"Minutes a chat may sit idle before its session ends for plugin hooks; writing again starts the next one. 0 never ends one for being idle"},
                "prompt_hook_calls":{"type":"integer","minimum":0,"maximum":1000,"default":50,"description":"Model requests prompt hooks may make in a run, each on the chat's model. A guard that finds none left refuses. 0 turns prompt hooks off; only your own runtime.jsonc can set this"}
            }}
        }
    })
}
