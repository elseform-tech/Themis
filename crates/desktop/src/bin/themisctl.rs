//! CLI client and headless host for the same application state used by Tauri.

use serde_json::{json, Value};
use std::path::PathBuf;
use themis_desktop::server::Client;

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

fn usage() -> &'static str {
    "usage: themisctl [--data-dir DIR] serve|server stop|status|watch|models|auth status|thread create PROJECT [MODEL]|thread list PROJECT|thread get ID|thread history ID|thread send ID TEXT|thread stop ID|call METHOD [JSON_ARGS]|app launch|app quit\n\
     call exposes the desktop backend methods with camelCase JSON arguments.\n\
     Credentials must be entered through the desktop settings; never pass a real key on the command line."
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("themisctl: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let mut words = std::env::args().skip(1).collect::<Vec<_>>();
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
    let result = match words.as_slice() {
        [command] if command == "serve" => {
            themis_desktop::run_headless_server_async(directory).await?;
            return Ok(());
        }
        [command] if command == "status" => client.call("ping", Value::Null).await?,
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
            client
                .call(
                    "send_message",
                    json!({"threadId": id, "text": text, "reasoningEffort": null}),
                )
                .await?
        }
        [thread, stop, id] if thread == "thread" && stop == "stop" => {
            client.call("stop_thread", json!({"threadId": id})).await?
        }
        [command, method] if command == "call" => {
            if method == "set_secret" {
                return Err("set_secret is available only through the desktop settings".to_owned());
            }
            client.call(method, json!({})).await?
        }
        [command, method, args] if command == "call" => {
            if method == "set_secret" {
                return Err("set_secret is available only through the desktop settings".to_owned());
            }
            let args =
                serde_json::from_str(args).map_err(|err| format!("invalid JSON_ARGS: {err}"))?;
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

#[cfg(target_os = "macos")]
fn local_app_bundle() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
        .and_then(|dir| {
            [
                dir.parent()?.join("release/bundle/macos/Themis.app"),
                dir.join("bundle/macos/Themis.app"),
            ]
            .into_iter()
            .find(|path| path.is_dir())
        })
}

fn launch_app() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = {
        let mut command = std::process::Command::new("open");
        if let Some(bundle) = local_app_bundle() {
            command.arg("-n").arg(bundle);
        } else {
            command.args(["-a", "Themis"]);
        }
        command.status()
    };
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("cmd")
        .args(["/C", "start", "", "Themis.exe"])
        .status();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let status = std::process::Command::new("gtk-launch")
        .arg("ai.themis.desktop")
        .status();
    app_command_result(status, "launch")
}

fn quit_app() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "tell application (item 1 of argv) to quit",
            "-e",
            "end run",
        ])
        .arg(local_app_bundle().map_or_else(
            || "Themis".to_owned(),
            |path| path.to_string_lossy().into_owned(),
        ))
        .status();
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-Process -Name 'Themis','themis-desktop' -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | ForEach-Object { $_.CloseMainWindow() | Out-Null }",
        ])
        .status();
    #[cfg(target_os = "linux")]
    return quit_linux_app();
    #[cfg(not(target_os = "linux"))]
    app_command_result(status, "quit")
}

#[cfg(target_os = "linux")]
fn quit_linux_app() -> Result<(), String> {
    for entry in std::fs::read_dir("/proc").map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let name = std::fs::read_link(path.join("exe"))
            .ok()
            .and_then(|exe| exe.file_name().map(std::ffi::OsStr::to_os_string));
        if name.as_deref() != Some(std::ffi::OsStr::new("themis-desktop")) {
            continue;
        }
        let args = std::fs::read(path.join("cmdline")).unwrap_or_default();
        if args
            .split(|byte| *byte == 0)
            .any(|arg| arg == b"--themis-server")
        {
            continue;
        }
        let status = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
        app_command_result(status, "quit")?;
    }
    Ok(())
}

fn app_command_result(
    status: std::io::Result<std::process::ExitStatus>,
    action: &str,
) -> Result<(), String> {
    match status {
        Ok(code) if code.success() => Ok(()),
        Ok(code) => Err(format!("could not {action} Themis: {code}")),
        Err(error) => Err(format!("could not {action} Themis: {error}")),
    }
}
