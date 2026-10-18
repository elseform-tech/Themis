//! Runs one tool-use task with the real chat toolset in a scratch project and prints a JSON
//! report of every tool call; the Python harness in `evals/tools` sets up the project and
//! grades the end state.
//!
//! ```text
//! cargo run --release -p themis-core --features eval --example tool_eval -- \
//!   --project /tmp/x/project --task /tmp/x/task.json --model longcat-2.5-preview-free
//! ```
//!
//! `task.json` holds `prompt`, and optionally `checkpoint` (a summary standing in for
//! compacted history) and `evidence` (project-relative saved-originals directory, which also
//! offers `recall` and `notes`), `offer_notes` (false withholds `notes`) and `followups`
//! (later messages in the same chat). The key comes from `OPENCODE_KEY`; `--base-url` points at a mock.

use std::path::PathBuf;

use serde_json::Value;
use themis_core::providers::{resolve, ProviderConfig, ProviderKind};
use themis_core::runtime::tool_eval::{run_tool_task, ToolTaskOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut project = PathBuf::new();
    let mut task_file = PathBuf::new();
    let mut model = None;
    let mut base_url = None;
    let mut max_turns = 12;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| anyhow::anyhow!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--project" => project = PathBuf::from(value()?),
            "--task" => task_file = PathBuf::from(value()?),
            "--model" => model = Some(value()?),
            "--base-url" => base_url = Some(value()?),
            "--max-turns" => max_turns = value()?.parse()?,
            other => anyhow::bail!("unknown flag {other}"),
        }
    }
    let task: Value = serde_json::from_str(&std::fs::read_to_string(&task_file)?)?;
    let prompt = task["prompt"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("task needs a prompt"))?;
    let key = match (&base_url, std::env::var("OPENCODE_KEY")) {
        (_, Ok(key)) => key,
        (Some(_), Err(_)) => "mock-key".to_owned(),
        (None, Err(_)) => anyhow::bail!("OPENCODE_KEY is not set"),
    };
    let mut config = ProviderConfig::new(ProviderKind::Go, key.clone());
    if let Some(model) = &model {
        config = config.with_model(model);
    }
    if let Some(url) = &base_url {
        config = config.with_base_url(url);
    }
    let llm = resolve(&config).await?;
    let options = ToolTaskOptions {
        checkpoint: task["checkpoint"].as_str().map(str::to_owned),
        evidence: task["evidence"].as_str().map(|dir| project.join(dir)),
        max_turns: task["max_turns"]
            .as_u64()
            .map_or(max_turns, |turns| turns as usize),
        budget: task["budget"]
            .as_u64()
            .map_or(64_000, |budget| budget as usize),
        recent_messages: task["recent_messages"]
            .as_u64()
            .map_or(20, |recent| recent as usize),
        offer_notes: task["offer_notes"].as_bool().unwrap_or(true),
        followups: task["followups"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|turn| turn.as_str().map(str::to_owned))
            .collect(),
        snapshots: task["snapshots"].as_bool().unwrap_or(false),
        allow_all: task["allow_all"].as_bool().unwrap_or(false),
        nudges_off: strings(&task["nudges_off"]),
        experiments: strings(&task["experiments"]),
        hide_tools: strings(&task["hide_tools"]),
        replay_reasoning: task["replay_reasoning"].as_bool().unwrap_or(true),
    };
    // As the app does: the live catalog's limits and thinking switches apply to the run
    // (none without a real key, or when the catalog cannot be loaded).
    let catalog = if std::env::var("OPENCODE_KEY").is_ok() {
        themis_core::providers::refresh_go_catalog(themis_core::providers::GO_BASE_URL, &key)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let report = themis_core::providers::with_catalog_limits(
        &catalog,
        run_tool_task(&llm, &project, prompt, options),
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect()
}
