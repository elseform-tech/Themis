//! Plain-text views of usage and effective permissions for the terminal.

use super::*;

/// Token counts the way the Usage settings show them: 1234, 12.3k, 4.56M.
pub(super) fn tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{:.1}k", count as f64 / 1e3),
        _ => format!("{:.2}M", count as f64 / 1e6),
    }
}

/// `themis usage`: the same totals as Settings › Usage, as plain text.
pub fn render_usage(summary: &Value) -> String {
    let mut out = String::from("Tokens from Themis only (provider dashboards may count more)\n");
    if summary["available"] == false {
        out.push_str("Usage records are not available (see `themis doctor`).\n");
        return out;
    }
    let number = |value: &Value, key: &str| value[key].as_u64().unwrap_or(0);
    let line = |label: &str, totals: &Value| {
        let cost = match (
            totals["estimated_cost_usd"].as_f64(),
            number(totals, "unpriced_requests"),
        ) {
            (Some(cost), 0) => format!("  ≈${cost:.2}"),
            (Some(cost), _) => format!("  ≈${cost:.2}+"),
            (None, _) => String::new(),
        };
        // One column of names: long model or chat names are cut to fit it.
        let label = if label.chars().count() > 24 {
            format!("{}…", label.chars().take(23).collect::<String>())
        } else {
            label.to_owned()
        };
        format!(
            "{label:<24} {:>5} requests  {:>7} in ({} cached)  {:>7} out ({} reasoning){cost}\n",
            number(totals, "requests"),
            tokens(number(totals, "input_tokens")),
            tokens(number(totals, "cached_input_tokens")),
            tokens(number(totals, "output_tokens")),
            tokens(number(totals, "reasoning_tokens")),
        )
    };
    let windows = &summary["windows"];
    for (label, key) in [
        ("5 hours", "five_hours"),
        ("Today", "today"),
        ("This week", "week"),
        ("This month", "month"),
    ] {
        out.push_str(&line(label, &windows[key]));
    }
    let models = summary["by_model"].as_array().cloned().unwrap_or_default();
    if !models.is_empty() {
        out.push_str("Models, last 30 days\n");
        for model in models {
            let name = model["key"].as_str().unwrap_or("unknown");
            let free = if model["free"] == true { " (free)" } else { "" };
            out.push_str(&line(&format!("  {name}{free}"), &model["totals"]));
        }
    }
    let chats = summary["by_chat"].as_array().cloned().unwrap_or_default();
    if !chats.is_empty() {
        out.push_str("Chats, last 30 days\n");
        for chat in chats {
            let name = chat["title"].as_str().unwrap_or("Deleted chats");
            out.push_str(&line(&format!("  {name}"), &chat["totals"]));
        }
    }
    if windows["month"]["estimated_cost_usd"].is_number() {
        out.push_str(
            "≈ costs are estimates at public list prices (models.dev); plans may bill differently.\n",
        );
    }
    out
}

/// `themis permissions [PROJECT] [--thread ID] [--json]`: the same read-only
/// view as Settings → Permissions, from the shared app server.
pub(super) async fn permissions(client: &Client, words: &[String]) -> Result<(), String> {
    let mut project = None;
    let mut thread = None;
    let mut as_json = false;
    let mut rest = words.iter();
    while let Some(word) = rest.next() {
        match word.as_str() {
            "--json" => as_json = true,
            "--thread" => thread = Some(rest.next().ok_or_else(|| usage().to_owned())?.clone()),
            other if !other.starts_with("--") && project.is_none() => {
                project = Some(other.to_owned())
            }
            _ => return Err(usage().to_owned()),
        }
    }
    if project.is_none() && thread.is_none() {
        project = std::env::current_dir()
            .ok()
            .map(|dir| dir.to_string_lossy().into_owned());
    }
    let view = client
        .call(
            "get_effective_permissions",
            json!({"projectRoot": project, "threadId": thread}),
        )
        .await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&view).map_err(|error| error.to_string())?
        );
    } else {
        print!("{}", render_permissions(&view));
    }
    Ok(())
}

pub(super) fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Human-readable form of `get_effective_permissions`.
pub fn render_permissions(view: &Value) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let source = |name: &str| match name {
        "default" => "Built-in",
        "user" => "User file",
        "project" => "Project file",
        "run" => "Run override",
        _ => "Unknown",
    };
    if let Some(root) = view["project_root"].as_str() {
        let _ = writeln!(out, "Project: {root}");
    }
    match view["mode"].as_str() {
        Some("yolo") => {
            let _ = writeln!(
                out,
                "Mode: YOLO (this chat): no approval prompts; the sandbox still applies"
            );
        }
        Some(_) => {
            let _ = writeln!(
                out,
                "Mode: Custom (this chat): rules below decide, otherwise Themis asks"
            );
        }
        None => {
            let _ = writeln!(out, "Mode: Custom by default; YOLO means no approval prompts, the sandbox still applies");
        }
    }
    if let Some(error) = view["validation_error"].as_str() {
        let _ = writeln!(
            out,
            "\nConfiguration is invalid; runs are refused until it is fixed:\n  {error}"
        );
    }
    let approval = &view["approval"];
    if let Some(rules) = approval["rules"].as_array() {
        let _ = writeln!(out, "\nRules (first match wins):");
        for (index, rule) in rules.iter().enumerate() {
            let mut line = format!(
                "  {}. {} {}",
                index + 1,
                text(&rule["action"]),
                text(&rule["tool"])
            );
            if let Some(risk) = rule["risk"].as_str() {
                let _ = write!(line, " risk={risk}");
            }
            if let Some(command) = rule["command"].as_array() {
                let argv: Vec<String> = command.iter().map(text).collect();
                let _ = write!(
                    line,
                    " command={}",
                    serde_json::to_string(&argv).unwrap_or_default()
                );
            }
            let _ = writeln!(
                out,
                "{line}  [{}]",
                source(rule["source"].as_str().unwrap_or(""))
            );
        }
        let _ = writeln!(
            out,
            "  Otherwise: {}  [{}]",
            text(&approval["default"]),
            source(approval["default_source"].as_str().unwrap_or("default"))
        );
        let added = approval["project_rule_count"].as_u64().unwrap_or(0);
        if added > 0 {
            let _ = writeln!(out, "  +{added} deny rules from this project");
        }
    }
    if let Some(files) = approval["sensitive_files"].as_array() {
        let patterns: Vec<String> = files.iter().map(text).collect();
        let _ = writeln!(
            out,
            "\nSensitive files (ask first in Custom): {}",
            if patterns.is_empty() {
                "none".to_owned()
            } else {
                patterns.join(" ")
            }
        );
    }
    let sandbox = &view["sandbox"];
    if sandbox.is_object() {
        let _ = writeln!(out, "\nSandbox: {}", text(&sandbox["summary"]));
        let network = text(&sandbox["network"]);
        let note = sandbox["network_note"]
            .as_str()
            .map(|note| format!(" ({note})"))
            .unwrap_or_default();
        let _ = writeln!(out, "  Network: {network}{note}");
        if let Some(paths) = sandbox["writable_paths"].as_array() {
            let _ = writeln!(
                out,
                "  Writable: {}",
                paths.iter().map(text).collect::<Vec<_>>().join(", ")
            );
        }
        if let Some(paths) = sandbox["protected_paths"].as_array() {
            let _ = writeln!(
                out,
                "  Never writable: {}",
                paths.iter().map(text).collect::<Vec<_>>().join(", ")
            );
        }
        if let Some(paths) = sandbox["secret_paths"].as_array() {
            let _ = writeln!(
                out,
                "  Never readable: {}",
                paths.iter().map(text).collect::<Vec<_>>().join(", ")
            );
        }
        if let Some(backends) = sandbox["backends"].as_array() {
            for backend in backends {
                let state = if backend["available"] == true {
                    "available"
                } else {
                    "not available"
                };
                let detail = if backend["supported_here"] == true {
                    text(&backend["detail"])
                } else {
                    format!("{} only", text(&backend["os"]))
                };
                let _ = writeln!(out, "  {}: {state} ({detail})", text(&backend["label"]));
            }
        }
    }
    let _ = writeln!(out, "\nConfiguration files:");
    for file in view["files"].as_array().into_iter().flatten() {
        let scope = if file["scope"] == "user" {
            "User"
        } else {
            "Project"
        };
        let _ = writeln!(
            out,
            "  {scope}: {} ({})",
            text(&file["path"]),
            text(&file["status"])
        );
        if file["error"].is_object() {
            let error = &file["error"];
            let position = match (error["line"].as_u64(), error["column"].as_u64()) {
                (Some(line), Some(column)) => format!("line {line}, column {column}: "),
                (Some(line), None) => format!("line {line}: "),
                _ => String::new(),
            };
            let _ = writeln!(out, "    {position}{}", text(&error["message"]));
        }
    }
    let chat = &view["chat"];
    if chat.is_object() {
        let _ = writeln!(
            out,
            "\nAllowed in this chat ({}):",
            text(&chat["grants_path"])
        );
        let grants = chat["grants"].as_array().cloned().unwrap_or_default();
        if grants.is_empty() {
            let _ = writeln!(out, "  nothing yet");
        }
        for grant in grants {
            let what = if let Some(server) = grant["server"].as_str() {
                format!("every tool of MCP server {server}")
            } else if let Some(command) = grant["command"].as_array().filter(|c| !c.is_empty()) {
                format!(
                    "{} {}",
                    text(&grant["tool"]),
                    serde_json::to_string(&command.iter().map(text).collect::<Vec<_>>())
                        .unwrap_or_default()
                )
            } else {
                text(&grant["tool"])
            };
            let _ = writeln!(out, "  {what} ({})", text(&grant["risk"]));
        }
        if let Some(warning) = chat["warning"].as_str() {
            let _ = writeln!(out, "  The grant file was ignored: {warning}");
        }
    }
    out
}
