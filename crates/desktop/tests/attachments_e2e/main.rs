use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use themis_desktop::{
    server::{Client, Server},
    state::AppState,
};
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

/// Mock assertion messages, reported by the test's final assertion.
static MOCK_FAILURES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Turns a panicking mock assertion into an HTTP 400 that names it, instead
/// of a dropped connection that only reports "error sending request".
fn reporting(
    handler: impl Fn(&Request) -> ResponseTemplate + Send + Sync + 'static,
) -> impl Fn(&Request) -> ResponseTemplate + Send + Sync + 'static {
    move |request: &Request| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(request))).unwrap_or_else(
            |panic| {
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
                    .unwrap_or_default();
                ResponseTemplate::new(400)
                    .set_body_string(format!("mock assertion failed: {message}"))
            },
        )
    }
}

/// Compares paths across platforms: JSON-escaped and native Windows
/// separators both become `/`.
fn slashes(text: &str) -> String {
    text.replace("\\\\", "/").replace('\\', "/")
}

fn cli(dir: &Path, command: &str, args: Value) -> Value {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir)
        .args(["call", command, "-"])
        .env_remove("OPENCODE_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(args.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

// Legacy import intentionally bounds each message; pressure spans realistic turns.
async fn import_pressure(client: &Client, id: &str, text: &str) {
    let mut records = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let mut end = (offset + 120_000).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        records.push(json!({"id":format!("pressure-{}", records.len()),"role":"user","text":&text[offset..end]}));
        offset = end;
    }
    client
        .call(
            "import_legacy_history",
            json!({"threadId":id,"messages":records}),
        )
        .await
        .unwrap();
}

mod checkpoints_and_recovery;
mod preparation_and_maintenance;
