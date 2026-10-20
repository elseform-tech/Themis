//! Runs one mid-turn re-read eval: copies the case's `project/` into a temporary folder,
//! runs `task.json`'s task from an empty conversation with a small budget, so the run
//! compacts while it works, and prints every tool call and checkpoint as JSON.
//!
//!   cargo run --release -p themis-core --features eval --example redo_eval -- \
//!     --case evals/compaction/cases/midturn-ephemeris --model M --budget 16000 \
//!     [--turns 40] [--effort max] [--base-url URL] [--trace steps.jsonl]
//!
//! `shell` and `git` are always denied; file tools may write inside the temporary project.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use themis_core::providers::{resolve, ProviderConfig, ProviderKind};
use themis_core::runtime::eval::{task_run, EvalPolicy};

fn arg(flag: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let case = PathBuf::from(arg("--case").ok_or_else(|| anyhow::anyhow!("--case is required"))?);
    let model = arg("--model");
    let budget: usize = arg("--budget").map_or(Ok(16_000), |v| v.parse())?;
    let recent: usize = arg("--recent").map_or(Ok(20), |v| v.parse())?;
    let turns: usize = arg("--turns").map_or(Ok(40), |v| v.parse())?;
    let spec: Value = serde_json::from_str(&std::fs::read_to_string(case.join("task.json"))?)?;
    let task = spec["task"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("task.json needs task"))?;
    let key = match (arg("--base-url"), std::env::var("OPENCODE_KEY")) {
        (_, Ok(key)) => key,
        (Some(_), Err(_)) => "mock-key".to_owned(),
        (None, Err(_)) => anyhow::bail!("OPENCODE_KEY is not set"),
    };
    let mut config = ProviderConfig::new(ProviderKind::Go, key.clone());
    if let Some(model) = &model {
        config = config.with_model(model);
    }
    config.reasoning_effort = arg("--effort");
    if let Some(url) = arg("--base-url") {
        config = config.with_base_url(url);
    }
    let llm = resolve(&config).await?;
    let project = tempfile::tempdir()?;
    copy_tree(&case.join("project"), project.path())?;
    let policy = EvalPolicy {
        context_token_budget: budget,
        recent_messages: recent,
    };
    // As the app does: the live catalog's output limits and thinking switches apply (without
    // them a 24k budget caps each reply at 3k tokens and long file writes are cut off).
    let catalog = if arg("--base-url").is_none() {
        themis_core::providers::refresh_go_catalog(themis_core::providers::GO_BASE_URL, &key)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let run = themis_core::providers::with_catalog_limits(
        &catalog,
        task_run(
            &llm,
            task,
            project.path(),
            policy,
            turns,
            arg("--trace").map(PathBuf::from),
        ),
    )
    .await?;
    let written: Vec<String> = spec["outputs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|path| project.path().join(path).is_file())
        .map(str::to_owned)
        .collect();
    let report = json!({
        "case": spec["name"], "model": model, "budget": budget, "turns": turns,
        "steps": run.steps, "error": run.error, "outputs_written": written,
        "repeat_reminders": run.repeat_reminders, "loop_warnings": run.loop_warnings,
        "request_failures": run.request_failures,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
