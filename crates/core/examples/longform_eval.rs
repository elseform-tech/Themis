//! Runs one long-form recall case in agent mode: replays `transcript.jsonl`, forces a
//! compaction after each chunk in `case.json` (saving originals under
//! `<workdir>/project/.themis/context/eval-thread`), writes the post-compaction working
//! messages to `<workdir>/context.json`, then answers each `probes.json` question with a
//! real agent turn that has the default toolset under an allow-read policy. Prints a JSON
//! report; the Python harness in `evals/longform` grades it and validates citations
//! against the saved snapshots.
//!
//! ```text
//! cargo run --release -p themis-core --features eval --example longform_eval -- \
//!   --case evals/longform/cases/<name> --workdir /tmp/x --model longcat-2.5-preview-free
//! ```
//!
//! The key comes from `OPENCODE_KEY`; `--base-url` points at a mock and needs none.

use std::path::PathBuf;

use serde_json::{json, Value};
use themis_core::providers::{resolve, ProviderConfig, ProviderKind};
use themis_core::runtime::eval::{
    answer_probe_agent, compact_chunks, parse_transcript, record_count, EvalPolicy,
};

struct Args {
    case: PathBuf,
    workdir: PathBuf,
    model: Option<String>,
    budget: usize,
    recent: usize,
    max_calls: usize,
    base_url: Option<String>,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args {
        case: PathBuf::new(),
        workdir: PathBuf::new(),
        model: None,
        budget: 24_000,
        recent: 20,
        max_calls: 8,
        base_url: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(flag) = iter.next() {
        let mut value = || {
            iter.next()
                .ok_or_else(|| anyhow::anyhow!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--case" => args.case = value()?.into(),
            "--workdir" => args.workdir = value()?.into(),
            "--model" => args.model = Some(value()?),
            "--budget" => args.budget = value()?.parse()?,
            "--recent" => args.recent = value()?.parse()?,
            "--max-calls" => args.max_calls = value()?.parse()?,
            "--base-url" => args.base_url = Some(value()?),
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    anyhow::ensure!(!args.case.as_os_str().is_empty(), "--case is required");
    anyhow::ensure!(
        !args.workdir.as_os_str().is_empty(),
        "--workdir is required"
    );
    Ok(args)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let case: Value = serde_json::from_str(&std::fs::read_to_string(args.case.join("case.json"))?)?;
    let probes: Value =
        serde_json::from_str(&std::fs::read_to_string(args.case.join("probes.json"))?)?;
    let transcript = parse_transcript(&std::fs::read_to_string(
        args.case.join("transcript.jsonl"),
    )?)?;
    let ends: Vec<usize> = case["chunks"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("case.json needs a chunks array"))?
        .iter()
        .filter_map(|end| end.as_u64().map(|end| end as usize))
        .collect();
    anyhow::ensure!(
        ends.windows(2).all(|pair| pair[0] < pair[1])
            && ends
                .last()
                .is_some_and(|end| *end <= record_count(&transcript)),
        "chunks must ascend and stay within the transcript"
    );
    let key = match (&args.base_url, std::env::var("OPENCODE_KEY")) {
        (_, Ok(key)) => key,
        (Some(_), Err(_)) => "mock-key".to_owned(),
        (None, Err(_)) => anyhow::bail!("OPENCODE_KEY is not set"),
    };
    let mut config = ProviderConfig::new(ProviderKind::Go, key);
    if let Some(model) = &args.model {
        config = config.with_model(model);
    }
    if let Some(url) = &args.base_url {
        config = config.with_base_url(url);
    }
    let llm = resolve(&config).await?;
    let project = args.workdir.join("project");
    let evidence = project.join(".themis/context/eval-thread");
    std::fs::create_dir_all(&evidence)?;
    let policy = EvalPolicy {
        context_token_budget: args.budget,
        recent_messages: args.recent,
    };
    let outcome = compact_chunks(&llm, &transcript, &ends, policy, &evidence).await?;
    std::fs::write(
        args.workdir.join("context.json"),
        serde_json::to_string(&outcome.messages)?,
    )?;
    let mut answers = Vec::new();
    if outcome.valid {
        for probe in probes.as_array().into_iter().flatten() {
            let question = probe["question"].as_str().unwrap_or_default();
            let answered = answer_probe_agent(
                &llm,
                &outcome.messages,
                question,
                &project,
                &evidence,
                policy,
                args.max_calls,
            )
            .await;
            let mut entry = match answered {
                Ok(answer) => serde_json::to_value(answer)?,
                Err(error) => json!({"answer": "", "finished": false, "error": error.to_string()}),
            };
            entry["id"] = probe["id"].clone();
            answers.push(entry);
        }
    }
    let report = json!({
        "case": case["name"],
        "model": args.model,
        "budget": args.budget,
        "recent_messages": args.recent,
        "max_tool_calls": args.max_calls,
        "transcript_records": record_count(&transcript),
        "chunks": outcome.chunks,
        "valid": outcome.valid,
        "verbatim_records": outcome.verbatim_records,
        "answers": answers,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
