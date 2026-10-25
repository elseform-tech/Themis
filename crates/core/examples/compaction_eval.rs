//! Runs one compaction-eval case: replays `transcript.jsonl`, forces a compaction
//! after each chunk in `case.json`, then answers every `probes.json` question from
//! the post-compaction messages only. Prints a JSON report; the Python harness in
//! `evals/compaction` grades it.
//!
//! ```text
//! cargo run --release -p themis-core --features eval --example compaction_eval -- \
//!   --case evals/compaction/cases/<name> --model longcat-2.5-preview-free --budget 24000
//! ```
//!
//! The key comes from `OPENCODE_KEY`; `--base-url` points at a mock and needs none.

use std::path::PathBuf;

use serde_json::{json, Value};
use themis_core::providers::{resolve, ProviderConfig, ProviderKind};
use themis_core::runtime::eval::{
    answer_probe, compact_chunks, first_user_request, parse_transcript, record_count, EvalPolicy,
};

struct Args {
    case: PathBuf,
    model: Option<String>,
    budget: usize,
    recent: usize,
    base_url: Option<String>,
    out: Option<PathBuf>,
    first_request: bool,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args {
        case: PathBuf::new(),
        model: None,
        budget: 24_000,
        recent: 20,
        base_url: None,
        out: None,
        first_request: true,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(flag) = iter.next() {
        let mut value = || {
            iter.next()
                .ok_or_else(|| anyhow::anyhow!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--case" => args.case = value()?.into(),
            "--model" => args.model = Some(value()?),
            "--budget" => args.budget = value()?.parse()?,
            "--recent" => args.recent = value()?.parse()?,
            "--base-url" => args.base_url = Some(value()?),
            "--out" => args.out = Some(value()?.into()),
            // `off` compacts as before the chat's first message reached every checkpoint.
            "--first-request" => args.first_request = value()? != "off",
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    anyhow::ensure!(!args.case.as_os_str().is_empty(), "--case is required");
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
    let evidence = tempfile::tempdir()?;
    let first = if args.first_request {
        first_user_request(&transcript)
    } else {
        String::new()
    };
    let outcome = themis_core::runtime::with_first_request(
        first,
        compact_chunks(
            &llm,
            &transcript,
            &ends,
            EvalPolicy {
                context_token_budget: args.budget,
                recent_messages: args.recent,
            },
            evidence.path(),
        ),
    )
    .await?;
    let mut answers = Vec::new();
    for probe in probes.as_array().into_iter().flatten() {
        let question = probe["question"].as_str().unwrap_or_default();
        let (answer, error) = match answer_probe(&llm, &outcome.messages, question).await {
            Ok(answer) => (Some(answer), None),
            Err(error) => (None, Some(error.to_string())),
        };
        answers.push(json!({"id": probe["id"], "answer": answer, "error": error}));
    }
    let report = json!({
        "case": case["name"],
        "model": args.model,
        "budget": args.budget,
        "recent_messages": args.recent,
        "first_request": args.first_request,
        "transcript_records": record_count(&transcript),
        "chunks": outcome.chunks,
        "valid": outcome.valid,
        "verbatim_records": outcome.verbatim_records,
        "answers": answers,
    });
    let text = serde_json::to_string_pretty(&report)?;
    match args.out {
        Some(path) => std::fs::write(path, text)?,
        None => println!("{text}"),
    }
    Ok(())
}
