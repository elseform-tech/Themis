//! The Workspace terminal through the real shared server: a real PTY and a real shell, driven by
//! the `themis` CLI, with its output read over the server's terminal stream. Unix only: the
//! scripts are POSIX shell. `sh` keeps a person's profile out of it.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_desktop::server::{Client, Server};
use themis_desktop::state::AppState;

static ENVIRONMENT: Once = Once::new();

/// Before anything spawns: the secrets the app holds, and a plain shell.
fn environment() {
    ENVIRONMENT.call_once(|| {
        std::env::set_var("OPENCODE_KEY", "SECRET-OPENCODE-VALUE");
        std::env::set_var("ANTHROPIC_API_KEY", "SECRET-ANTHROPIC-VALUE");
        std::env::set_var("SOME_SERVICE_TOKEN", "SECRET-TOKEN-VALUE");
        std::env::set_var("SHELL", "/bin/sh");
        std::env::set_var("ENV", "");
    });
}

fn call(data: &Path, command: &str, args: &Value) -> Result<Value, String> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(["call", command, &args.to_string()])
        .output()
        .expect("CLI");
    if output.status.success() {
        Ok(serde_json::from_slice(&output.stdout).unwrap_or(Value::Null))
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

struct App {
    data: tempfile::TempDir,
    project: tempfile::TempDir,
    root: String,
    serving: tokio::task::JoinHandle<()>,
}

async fn app() -> App {
    environment();
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(async move {
        let _ = server.run().await;
    });
    let root = project
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    App {
        data,
        project,
        root,
        serving,
    }
}

async fn on_cli<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(job).await.unwrap()
}

impl App {
    async fn open(&self) -> Value {
        self.open_at(&self.root.clone()).await.unwrap()
    }

    async fn open_at(&self, root: &str) -> Result<Value, String> {
        let data: PathBuf = self.data.path().to_owned();
        let args = json!({"projectRoot": root, "cols": 80, "rows": 24});
        on_cli(move || call(&data, "terminal_open", &args)).await
    }

    async fn call(&self, command: &'static str, mut args: Value) -> Result<Value, String> {
        if args.get("root").is_none() {
            args["root"] = json!(self.root);
        }
        let data: PathBuf = self.data.path().to_owned();
        on_cli(move || call(&data, command, &args)).await
    }

    async fn send(&self, text: &str) {
        self.call("terminal_input", json!({"data": text}))
            .await
            .unwrap();
    }

    async fn close(&self) {
        self.call("terminal_close", json!({})).await.unwrap();
    }

    async fn stream(&self) -> Stream {
        let client = Client::new(self.data.path().to_owned());
        let (stream, first) = client.subscribe_terminal(&self.root).await.unwrap();
        Stream {
            stream,
            seen: first["scrollback"].as_str().unwrap_or_default().to_owned(),
            exit: None,
        }
    }
}

struct Stream {
    stream: tokio::net::TcpStream,
    seen: String,
    exit: Option<Value>,
}

impl Stream {
    fn take(&mut self, frame: &Value) {
        match frame["kind"].as_str() {
            Some("output") => self.seen.push_str(frame["data"].as_str().unwrap()),
            Some("exit") => self.exit = Some(frame["code"].clone()),
            other => panic!("unknown frame {other:?}"),
        }
    }

    async fn next(&mut self, deadline: Instant) {
        let left = deadline.saturating_duration_since(Instant::now());
        let frame = tokio::time::timeout(left, Client::next_event(&mut self.stream))
            .await
            .unwrap_or_else(|_| panic!("nothing more came: {:?}", self.seen))
            .unwrap();
        self.take(&frame);
    }

    /// Reads until `needle` shows, and returns all that came.
    async fn until(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.seen.contains(needle) {
            assert!(
                self.exit.is_none(),
                "ended before {needle:?}: {}",
                self.seen
            );
            self.next(deadline).await;
        }
        self.seen.clone()
    }

    /// Reads until `tag` shows followed by digits: the output, not the echo of the command.
    async fn number(&mut self, tag: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(number) = number_after(&self.seen, tag) {
                return number;
            }
            self.next(deadline).await;
        }
    }

    /// Reads to the end of the session and returns its exit code.
    async fn ended(&mut self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.exit.is_none() {
            self.next(deadline).await;
        }
        self.exit.clone().unwrap()
    }
}

fn number_after(text: &str, tag: &str) -> Option<String> {
    text.match_indices(tag).find_map(|(at, _)| {
        let digits: String = text[at + tag.len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        (!digits.is_empty()).then_some(digits)
    })
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn gone(pid: &str) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while alive(pid) {
        assert!(
            Instant::now() < deadline,
            "process {pid} outlived its terminal"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_command_runs_in_a_real_terminal_in_the_project() {
    let app = app().await;
    std::fs::write(app.project.path().join("marker.txt"), "x").unwrap();
    let opened = app.open().await;
    assert_eq!(opened["root"], app.root);
    assert_eq!(opened["running"], true);
    let mut stream = app.stream().await;
    app.send("printf 'A%sB\\n' -OK-; tty; pwd; ls; echo DONE-$((1+1))\n")
        .await;
    let seen = stream.until("DONE-2").await;
    assert!(seen.contains("A-OK-B"), "{seen}");
    assert!(seen.contains("/dev/"), "not a terminal: {seen}");
    assert!(seen.contains(&app.root), "{seen}");
    assert!(seen.contains("marker.txt"), "{seen}");
    // What it printed stays out of the logs.
    let logs = std::fs::read_dir(app.data.path().join("logs"))
        .unwrap()
        .flatten()
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
        .collect::<String>();
    assert!(logs.contains("\"terminal\""), "no metadata logged: {logs}");
    assert!(!logs.contains("A-OK-B"), "output in the logs");
    assert!(!logs.contains("marker.txt"), "output in the logs");
    app.close().await;
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_shell_gets_no_secret_the_app_holds() {
    let app = app().await;
    app.open().await;
    let mut stream = app.stream().await;
    app.send("env; echo K=[$OPENCODE_KEY] T=$TERM; echo DONE-$((1+1))\n")
        .await;
    let seen = stream.until("DONE-2").await;
    assert!(!seen.contains("SECRET-"), "{seen}");
    assert!(seen.contains("K=[]"), "{seen}");
    assert!(seen.contains("T=xterm-256color"), "{seen}");
    app.close().await;
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn resizing_reaches_the_program() {
    let app = app().await;
    app.open().await;
    let mut stream = app.stream().await;
    app.call("terminal_resize", json!({"cols": 101, "rows": 31}))
        .await
        .unwrap();
    app.send("stty size; echo DONE-$((1+1))\n").await;
    let seen = stream.until("DONE-2").await;
    assert!(seen.contains("31 101"), "{seen}");
    app.close().await;
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn it_survives_leaving_says_how_it_ended_and_restarts() {
    let app = app().await;
    app.open().await;
    let mut first = app.stream().await;
    app.send("export KEPT=$((40+2)); echo DONE-$((1+1))\n")
        .await;
    first.until("DONE-2").await;
    drop(first);
    // Opening it again, as coming back to the tab does, is the same shell, with what it printed.
    app.open().await;
    let mut back = app.stream().await;
    assert!(back.seen.contains("DONE-2"), "{}", back.seen);
    app.send("echo KEPT-$KEPT\n").await;
    back.until("KEPT-42").await;
    app.send("exit 3\n").await;
    assert_eq!(back.ended().await, 3);
    // Exited: it says so, then a new one starts on request.
    let state = app.call("terminal_state", json!({})).await.unwrap();
    assert_eq!(state["running"], false);
    assert_eq!(state["exit_code"], 3);
    assert!(app
        .call("terminal_input", json!({"data": "x"}))
        .await
        .is_err());
    assert_eq!(app.open().await["running"], true);
    let mut again = app.stream().await;
    assert!(!again.seen.contains("KEPT-42"), "{}", again.seen);
    app.send("echo FRESH-$((1+1))-[$KEPT]\n").await;
    again.until("FRESH-2-[]").await;
    app.close().await;
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_leaves_nothing_running() {
    let app = app().await;
    app.open().await;
    let mut stream = app.stream().await;
    app.send("set +H\n").await;
    // The shell, a job in the background, one in a process group of its own, and one in front.
    app.send("sleep 3011 & A=$!; set -m; sleep 3012 & B=$!; echo SH=$$ BG=$A GROUP=$B; sleep 3013 & C=$!; echo LAST=$C; wait $C\n")
        .await;
    let mut pids = Vec::new();
    for tag in ["SH=", "BG=", "GROUP=", "LAST="] {
        pids.push(stream.number(tag).await);
    }
    for pid in &pids {
        assert!(alive(pid), "{pid} should be running: {}", stream.seen);
    }
    app.close().await;
    for pid in &pids {
        gone(pid).await;
    }
    assert_eq!(stream.ended().await, Value::Null);
    let state = app.call("terminal_state", json!({})).await.unwrap();
    assert_eq!(state["running"], false);
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_that_exits_takes_its_background_jobs_with_it() {
    let app = app().await;
    app.open().await;
    let mut stream = app.stream().await;
    app.send("set +H\n").await;
    app.send("sleep 3031 & echo BG=$!; exit 0\n").await;
    let pid = stream.number("BG=").await;
    assert_eq!(stream.ended().await, 0);
    gone(&pid).await;
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_the_server_stops_the_shell() {
    environment();
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state, data.path()).await.unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(async move {
        let _ = server
            .run_until(async {
                let _ = stopped.await;
            })
            .await;
    });
    let root = project.path().to_string_lossy().into_owned();
    let client = Client::new(data.path().to_owned());
    let opened = client
        .call(
            "terminal_open",
            json!({"projectRoot": root, "cols": 80, "rows": 24}),
        )
        .await
        .unwrap();
    let root = opened["root"].as_str().unwrap().to_owned();
    let (mut stream, _) = client.subscribe_terminal(&root).await.unwrap();
    client
        .call(
            "terminal_input",
            json!({"root": root, "data": "sleep 3021 & echo BG=$!\n"}),
        )
        .await
        .unwrap();
    let mut seen = String::new();
    let pid = loop {
        if let Some(pid) = number_after(&seen, "BG=") {
            break pid;
        }
        let frame = Client::next_event(&mut stream).await.unwrap();
        seen.push_str(frame["data"].as_str().unwrap_or_default());
    };
    assert!(alive(&pid));
    stop.send(()).unwrap();
    serving.await.unwrap();
    gone(&pid).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn output_is_bounded_and_the_end_still_arrives() {
    let app = app().await;
    app.open().await;
    let mut stream = app.stream().await;
    app.send("head -c 3000000 /dev/zero | tr '\\0' x; echo; echo DONE-$((1+1))\n")
        .await;
    stream.until("DONE-2").await;
    // Someone who comes back gets the tail, within the limit.
    let late = app.stream().await;
    assert!(late.seen.contains("DONE-2"));
    assert!(late.seen.len() <= 512 * 1024, "{}", late.seen.len());
    app.close().await;
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn input_size_and_count_are_limited() {
    let app = app().await;
    assert!(app.open_at("/no/such/folder").await.is_err());
    assert!(app
        .call("terminal_input", json!({"data": "x"}))
        .await
        .is_err());
    app.open().await;
    assert!(app
        .call("terminal_input", json!({"data": "x".repeat(64 * 1024 + 1)}))
        .await
        .is_err());
    for (cols, rows) in [(0, 24), (80, 0), (5000, 24), (80, 5000)] {
        assert!(
            app.call("terminal_resize", json!({"cols": cols, "rows": rows}))
                .await
                .is_err(),
            "{cols}x{rows}"
        );
    }
    let mut folders = Vec::new();
    let mut refused = None;
    for _ in 0..10 {
        let folder = tempfile::tempdir().unwrap();
        let result = app.open_at(&folder.path().to_string_lossy()).await;
        folders.push(folder);
        if let Err(error) = result {
            refused = Some(error);
            break;
        }
    }
    let refused = refused.expect("no limit on terminals");
    assert!(refused.contains("terminals"), "{refused}");
    assert_eq!(folders.len(), 8, "the first one is already open");
    // Coming back to one that is open is never refused.
    app.open().await;
    for folder in &folders[..folders.len() - 1] {
        let root = folder.path().canonicalize().unwrap();
        app.call("terminal_close", json!({"root": root}))
            .await
            .unwrap();
    }
    app.close().await;
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_project_ends_its_shell() {
    let app = app().await;
    let data: PathBuf = app.data.path().to_owned();
    let project = on_cli(move || call(&data, "create_project", &json!({"name": "Scratch"})))
        .await
        .unwrap();
    let root = project["root"].as_str().unwrap().to_owned();
    let opened = app.open_at(&root).await.unwrap();
    let key = opened["root"].as_str().unwrap().to_owned();
    let client = Client::new(app.data.path().to_owned());
    let (mut stream, _) = client.subscribe_terminal(&key).await.unwrap();
    client
        .call(
            "terminal_input",
            json!({"root": key, "data": "sleep 3041 & echo BG=$!\n"}),
        )
        .await
        .unwrap();
    let mut seen = String::new();
    let pid = loop {
        if let Some(pid) = number_after(&seen, "BG=") {
            break pid;
        }
        let frame = Client::next_event(&mut stream).await.unwrap();
        seen.push_str(frame["data"].as_str().unwrap_or_default());
    };
    let data: PathBuf = app.data.path().to_owned();
    on_cli(move || call(&data, "delete_project", &json!({"path": root})))
        .await
        .unwrap();
    gone(&pid).await;
    app.serving.abort();
}
