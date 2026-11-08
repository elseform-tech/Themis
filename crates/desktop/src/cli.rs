//! CLI client and headless host for the same application state used by Tauri.

use crate::server::Client;
use serde_json::{json, Value};
use std::path::PathBuf;
mod integrations;
mod plugins;

mod app;
mod render;
use app::*;
use render::*;
pub use render::{render_permissions, render_usage};

fn data_dir() -> Result<PathBuf, String> {
    #[cfg(not(target_os = "windows"))]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    {
        Ok(home
            .ok_or("HOME is unset")?
            .join("Library/Application Support/ai.themis.desktop"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("ai.themis.desktop"))
            .ok_or_else(|| "APPDATA is unset".to_owned())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|path| path.join(".local/share")))
            .ok_or("HOME and XDG_DATA_HOME are unset")?;
        Ok(base.join("ai.themis.desktop"))
    }
}

/// `--help` or `-h` anywhere (or no command) prints help and does nothing else, so the agent's
/// shell treats it as a read: the integration usage for `plugin`/`skill`/`mcp`/`hook`, with the
/// exact definition of a skill, hook or MCP server.
fn help_for(words: &[String]) -> Option<String> {
    let asks = words.is_empty() || words.iter().any(|word| word == "--help" || word == "-h");
    let integration = words
        .first()
        .filter(|word| matches!(word.as_str(), "plugin" | "skill" | "mcp" | "hook"));
    asks.then(|| match integration {
        Some(command) => plugins::help(command),
        None => usage().to_owned(),
    })
}

fn usage() -> &'static str {
    "usage: themis [--data-dir DIR] COMMAND
Commands:
  serve                         Run the shared app server in the foreground
  server start|stop              Start or stop the background server
  status                        Report server version
  watch                         Stream all backend events as JSON
  models                        List OpenCode Go models
  plugin                        Manage plugins and marketplaces (plugin list)
  skill                         List, save, verify, delete, or ask agent to create skills
  mcp | hook                    Manage plugin connections and lifecycle hooks
  doctor [--json] [--deep]       Read-only local diagnostics
  doctor --integrations [--json] [--no-start] [--project PATH]
                                Check every plugin, skill, MCP server and hook
  permissions [PROJECT] [--thread ID] [--json]
                                Effective approval rules, sandbox and chat grants (read-only)
  usage [--json]                Tokens Themis used: 5 hours, today, week, month
  providers                     Select OpenCode Go and enter a hidden API key
  auth status                   Report key availability only
  thread create PROJECT [MODEL]  Create a shared desktop conversation
  thread list PROJECT           List conversations
  thread get ID                 Show conversation metadata
  thread history ID             Show persisted history
  thread send ID TEXT [--json|--detach]  Send and stream, or return a run handle
  thread stop ID                Stop a run
  chat ID                       Converse until /quit (TTY required)
  call METHOD [JSON_ARGS|-]     Call backend; - reads JSON from stdin (max 8 MiB)
  cli install [DIR]             Link the optional CLI into ~/.local/bin or DIR (Unix)
  app launch|quit               Open or close the desktop window
  --help | --version
Tool approvals require a terminal; unattended sends deny approval requests.
Never put keys in command arguments. Use providers or desktop Settings."
}

pub async fn run() -> Result<(), String> {
    let mut words = std::env::args().skip(1).collect::<Vec<_>>();
    if words.first().is_some_and(|word| word == "--themis-cli") {
        words.remove(0);
    }
    let directory = if words.first().is_some_and(|word| word == "--data-dir") {
        if words.len() < 2 {
            return Err(usage().to_owned());
        }
        let directory = PathBuf::from(words.remove(1));
        words.remove(0);
        directory
    } else {
        data_dir()?
    };
    let client = Client::new(directory.clone());
    if let Some(help) = help_for(&words) {
        println!("{help}");
        return Ok(());
    }
    if words == ["--version"] {
        println!("themis {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let starts_run = match words.as_slice() {
        [command, ..] if command == "chat" => true,
        [command, action, ..] => matches!(
            (command.as_str(), action.as_str()),
            ("thread", "send")
                | ("skill", "create")
                | ("call", "send_message" | "run_automation_now")
        ),
        _ => false,
    };
    if std::env::var_os("THEMIS_HOOK_ACTIVE").is_some() && starts_run {
        return Err("Hooks cannot start nested agent runs".into());
    }
    if words.first().is_some_and(|word| {
        matches!(
            word.as_str(),
            "thread"
                | "chat"
                | "models"
                | "call"
                | "auth"
                | "plugin"
                | "skill"
                | "mcp"
                | "hook"
                | "permissions"
                | "usage"
        )
    }) {
        ensure_server(&client, &directory).await?;
    }
    if words.first().is_some_and(|word| word == "doctor") {
        return integrations::doctor(&client, &directory, &words[1..]).await;
    }
    if words.first().is_some_and(|word| word == "permissions") {
        return permissions(&client, &words[1..]).await;
    }
    if words.first().is_some_and(|word| word == "usage") {
        let as_json = match &words[1..] {
            [] => false,
            [flag] if flag == "--json" => true,
            _ => return Err(usage().to_owned()),
        };
        let summary = client.call("get_usage_summary", Value::Null).await?;
        if as_json {
            println!(
                "{}",
                serde_json::to_string(&summary).map_err(|err| err.to_string())?
            );
        } else {
            print!("{}", render_usage(&summary));
        }
        return Ok(());
    }
    let result = match words.as_slice() {
        [kind, create, id, request] if kind == "skill" && create == "create" => {
            return send_message(&client, id, &format!("[[skill:skills]] {request}"), false).await;
        }
        [kind, ..] if matches!(kind.as_str(), "plugin" | "skill" | "mcp" | "hook") => {
            plugins::run(&client, &words, &directory).await?
        }
        [cli, install] if cli == "cli" && install == "install" => {
            let directory = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or("HOME is unset")?
                .join(".local/bin");
            install_cli(&directory)?;
            return Ok(());
        }
        [cli, install, path] if cli == "cli" && install == "install" => {
            install_cli(std::path::Path::new(path))?;
            return Ok(());
        }
        [command] if command == "serve" => {
            crate::run_headless_server_async(directory).await?;
            return Ok(());
        }
        [command] if command == "providers" => {
            configure_provider(&client).await?;
            return Ok(());
        }
        [command] if command == "status" => client.call("ping", Value::Null).await?,
        [server, start] if server == "server" && start == "start" => {
            ensure_server(&client, &directory).await?;
            client.call("ping", Value::Null).await?
        }
        [server, stop] if server == "server" && stop == "stop" => {
            client.call("shutdown", Value::Null).await?
        }
        [command] if command == "watch" => {
            let mut stream = client.subscribe().await?;
            loop {
                let event = Client::next_event(&mut stream).await?;
                println!(
                    "{}",
                    serde_json::to_string(&event).map_err(|err| err.to_string())?
                );
            }
        }
        [command] if command == "models" => client.call("list_go_models", Value::Null).await?,
        [auth, status] if auth == "auth" && status == "status" => {
            client.call("get_secret_status", Value::Null).await?
        }
        [thread, create, project] if thread == "thread" && create == "create" => {
            client
                .call(
                    "create_thread",
                    json!({"projectRoot": project, "provider": "go", "model": null}),
                )
                .await?
        }
        [thread, create, project, model] if thread == "thread" && create == "create" => {
            client
                .call(
                    "create_thread",
                    json!({"projectRoot": project, "provider": "go", "model": model}),
                )
                .await?
        }
        [thread, list, project] if thread == "thread" && list == "list" => {
            client
                .call("list_threads", json!({"projectRoot": project}))
                .await?
        }
        [thread, get, id] if thread == "thread" && get == "get" => {
            client.call("get_thread", json!({"threadId": id})).await?
        }
        [thread, history, id] if thread == "thread" && history == "history" => {
            client
                .call("get_thread_history", json!({"threadId": id}))
                .await?
        }
        [thread, send, id, text] if thread == "thread" && send == "send" => {
            send_message(&client, id, text, false).await?;
            return Ok(());
        }
        [thread, send, id, text, option]
            if thread == "thread" && send == "send" && option == "--json" =>
        {
            send_message(&client, id, text, true).await?;
            return Ok(());
        }
        [thread, send, id, text, option]
            if thread == "thread" && send == "send" && option == "--detach" =>
        {
            client
                .call(
                    "send_message",
                    json!({"threadId": id, "text": text, "reasoningEffort": null}),
                )
                .await?
        }
        [command, id] if command == "chat" => {
            use std::io::{IsTerminal, Write};
            if !std::io::stdin().is_terminal() {
                return Err(
                    "chat requires a terminal; use thread send ID TEXT for scripts".to_owned(),
                );
            }
            loop {
                print!("> ");
                std::io::stdout()
                    .flush()
                    .map_err(|error| error.to_string())?;
                let mut text = String::new();
                if std::io::stdin()
                    .read_line(&mut text)
                    .map_err(|error| error.to_string())?
                    == 0
                    || text.trim() == "/quit"
                {
                    return Ok(());
                }
                if !text.trim().is_empty() {
                    send_message(&client, id, text.trim_end(), false).await?;
                }
            }
        }
        [thread, stop, id] if thread == "thread" && stop == "stop" => {
            client.call("stop_thread", json!({"threadId": id})).await?
        }
        [command, method] if command == "call" => {
            if method == "set_secret" {
                return Err(
                    "use themis providers or desktop Settings to enter a key securely".to_owned(),
                );
            }
            client.call(method, json!({})).await?
        }
        [command, method, args] if command == "call" => {
            if method == "set_secret" {
                return Err(
                    "use themis providers or desktop Settings to enter a key securely".to_owned(),
                );
            }
            let input = if args == "-" {
                use std::io::Read;
                let mut input = String::new();
                std::io::stdin()
                    .take(crate::server::MAX_FRAME as u64 + 1)
                    .read_to_string(&mut input)
                    .map_err(|err| format!("cannot read JSON_ARGS: {err}"))?;
                if input.len() > crate::server::MAX_FRAME {
                    return Err("JSON_ARGS exceeds 8 MiB".into());
                }
                input
            } else {
                args.clone()
            };
            let args =
                serde_json::from_str(&input).map_err(|err| format!("invalid JSON_ARGS: {err}"))?;
            client.call(method, args).await?
        }
        [app, launch] if app == "app" && launch == "launch" => {
            launch_app()?;
            return Ok(());
        }
        [app, quit] if app == "app" && quit == "quit" => {
            quit_app()?;
            return Ok(());
        }
        _ => return Err(usage().to_owned()),
    };
    println!(
        "{}",
        serde_json::to_string(&result).map_err(|err| err.to_string())?
    );
    Ok(())
}

async fn configure_provider(client: &Client) -> Result<(), String> {
    use crate::secrets::{ProductionSecretStore, SecretStore};
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Err(
            "providers requires a terminal; enter keys in desktop Settings instead".to_owned(),
        );
    }
    eprint!("Providers:\n  1. OpenCode Go\nSelect provider [1] (q to cancel): ");
    std::io::stderr()
        .flush()
        .map_err(|_| "Cannot write terminal prompt")?;
    let mut selection = String::new();
    if std::io::stdin()
        .read_line(&mut selection)
        .map_err(|_| "Cannot read terminal selection")?
        == 0
    {
        return Err("Provider setup cancelled".to_owned());
    }
    match selection.trim() {
        "" | "1" | "go" => {}
        "q" => return Ok(()),
        _ => return Err("Select OpenCode Go (1) or q to cancel".to_owned()),
    }
    // rpassword raises SIGINT before its restore guard runs. Register a handler
    // so Ctrl-C returns an error and the guard can restore terminal echo.
    #[cfg(unix)]
    let _interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|_| "Cannot safely handle terminal interrupts")?;
    #[cfg(windows)]
    let _interrupt =
        tokio::signal::windows::ctrl_c().map_err(|_| "Cannot safely handle terminal interrupts")?;
    let key = zeroize::Zeroizing::new(
        rpassword::prompt_password("OpenCode Go API key (hidden): ")
            .map_err(|_| "Key entry cancelled or terminal unavailable; nothing saved")?,
    );
    crate::secrets::validate_provider_key(&key)?;
    if tokio::time::timeout(
        std::time::Duration::from_millis(500),
        client.call("ping", Value::Null),
    )
    .await
    .is_ok_and(|result| result.is_ok())
    {
        client
            .call("set_secret", json!({"provider": "go", "value": &*key}))
            .await
            .map_err(|_| "Could not save the API key; check credential-store access in Settings")?;
    } else {
        ProductionSecretStore::new()
            .set(crate::secrets::GO_ENTRY, &key)
            .map_err(|_| "Could not save the API key; check OS credential-store access")?;
    }
    println!("OpenCode Go key saved to the OS credential store shared with desktop Settings.");
    Ok(())
}

async fn ensure_server(client: &Client, directory: &std::path::Path) -> Result<(), String> {
    let ping = || {
        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            client.call("ping", Value::Null),
        )
    };
    if crate::server::in_agent_shell() {
        // An agent's shell uses the app's server; it never starts its own.
        return match client.call("ping", Value::Null).await {
            Err(error) if error == "unauthorized" => {
                Err("unauthorized: this shell's credential ended with the run that got it".into())
            }
            other => other.map(|_| ()),
        };
    }
    if client.answers(crate::server::SERVER_PATIENCE).await {
        return Ok(());
    }
    let mut child =
        std::process::Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
            .arg("--themis-cli")
            .arg("--data-dir")
            .arg(directory)
            .arg("serve")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|error| format!("could not start app server: {error}"))?;
    for _ in 0..50 {
        if ping().await.is_ok_and(|result| result.is_ok()) {
            return Ok(());
        }
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            // Another launcher can win the ownership lock during concurrent startup.
            if ping().await.is_ok_and(|result| result.is_ok()) {
                return Ok(());
            }
            return Err(format!(
                "app server exited during startup: {status}; run themis doctor"
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err("app server did not become ready; run themis doctor".to_owned())
}

async fn send_message(
    client: &Client,
    id: &str,
    text: &str,
    json_output: bool,
) -> Result<(), String> {
    use std::io::{IsTerminal, Write};
    // Subscribe before starting the run, including runs that finish immediately.
    let mut stream = client.subscribe().await?;
    let handle = client
        .call(
            "send_message",
            json!({"threadId": id, "text": text, "reasoningEffort": null}),
        )
        .await?;
    let run_id = handle["run_id"].as_str().ok_or("missing run ID")?;
    let mut printed_text = false;
    loop {
        let event = tokio::select! {
            event = Client::next_event(&mut stream) => event,
            interrupt = tokio::signal::ctrl_c() => {
                interrupt.map_err(|error| error.to_string())?;
                client.call("stop_thread", json!({"threadId": id})).await?;
                return Err("Run stopped; conversation remains available in thread history".to_owned());
            }
        }.map_err(|error| {
            format!("event stream interrupted ({error}); check thread history before retrying")
        })?;
        if event["name"] == "lagged" {
            return Err(
                "event stream lost messages; check thread history before retrying".to_owned(),
            );
        }
        let payload = &event["payload"];
        if payload["thread_id"] != id {
            continue;
        }
        if event["name"] == "approval-request" {
            let mut decision = "deny";
            if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
                eprint!(
                    "\n{}: {}\nAllow once? [y/N] ",
                    payload["tool"], payload["summary"]
                );
                std::io::stderr()
                    .flush()
                    .map_err(|error| error.to_string())?;
                let answer = tokio::select! {
                    answer = tokio::task::spawn_blocking(|| {
                        let mut answer = String::new();
                        std::io::stdin().read_line(&mut answer).map(|_| answer)
                    }) => answer.map_err(|error| error.to_string())?.map_err(|error| error.to_string())?,
                    interrupt = tokio::signal::ctrl_c() => {
                        interrupt.map_err(|error| error.to_string())?;
                        client.call("stop_thread", json!({"threadId": id})).await?;
                        // The blocked terminal reader cannot be cancelled; exit after stopping the run.
                        std::process::exit(130);
                    }
                };
                if matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                    decision = "once";
                }
            } else {
                eprintln!("Tool approval denied: terminal input unavailable. Use chat or a terminal send to approve.");
            }
            client.call("approve_action", json!({"threadId": id, "approvalId": payload["approval_id"], "decision": decision})).await?;
        }
        if event["name"] != "thread-event" || payload["run_id"] != run_id {
            continue;
        }
        if json_output {
            println!("{event}");
        }
        let item = &payload["event"];
        match item["kind"].as_str().unwrap_or_default() {
            "assistant_text" if !json_output => {
                print!("{}", item["text"].as_str().unwrap_or_default());
                std::io::stdout()
                    .flush()
                    .map_err(|error| error.to_string())?;
                printed_text = true;
            }
            "tool_started" => {
                printed_text = false;
            }
            "finished" => {
                if !json_output {
                    if !printed_text {
                        print!("{}", item["result"].as_str().unwrap_or_default());
                    }
                    println!();
                }
                return Ok(());
            }
            "failed" => return Err(item["error"].as_str().unwrap_or("run failed").to_owned()),
            "incomplete" => {
                if !json_output {
                    println!("\n{}", item["result"].as_str().unwrap_or_default());
                }
                return Err(
                    "Run paused with unfinished work; send a followup to continue".to_owned(),
                );
            }
            _ => {}
        }
    }
}
