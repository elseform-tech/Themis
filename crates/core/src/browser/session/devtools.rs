//! The debugging tools' side of a chat's browser: console messages and requests recorded
//! per tab, and the developer operations (evaluate, elements, network, debugger, storage,
//! performance, memory) on the current tab over the DevTools protocol. Results are bounded
//! and, unless `show_secrets`, have secrets hidden (see [`crate::browser::redact`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use super::{cap, resolve_object, ChatBrowser, TabState};
use crate::browser::cdp::{Cdp, CdpEvent};
use crate::browser::redact;

/// Console messages kept per tab.
const CONSOLE_KEPT: usize = 500;
/// Requests kept per tab.
const REQUESTS_KEPT: usize = 300;
/// Items one call returns at most (the newest).
const LISTED: usize = 50;
/// Bytes of one value or body in a result.
const VALUE_BYTES: usize = 16 * 1024;
/// A pause the agent leaves alone resumes after this.
const PAUSE_LIMIT: Duration = Duration::from_secs(30);
/// Longest performance trace.
const TRACE_LIMIT_SECONDS: u64 = 15;

/// One trace or heap snapshot at a time: they are heavy.
static HEAVY_JOB: AtomicBool = AtomicBool::new(false);

/// One console message or uncaught exception.
#[derive(Debug, Clone, Serialize)]
pub struct ConsoleEntry {
    pub seq: u64,
    pub level: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
}

/// One request the page made.
#[derive(Debug, Clone, Serialize)]
pub struct RequestEntry {
    pub seq: u64,
    pub id: String,
    pub method: String,
    pub url: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    #[serde(skip)]
    started: Option<Instant>,
    #[serde(skip)]
    request_headers: Value,
    #[serde(skip)]
    response_headers: Value,
}

fn next_seq(tab: &mut TabState) -> u64 {
    tab.log_seq += 1;
    tab.log_seq
}

fn push_console(
    tab: &mut TabState,
    level: &str,
    text: String,
    source: Option<String>,
    stack: Option<String>,
) {
    let seq = next_seq(tab);
    tab.console.push_back(ConsoleEntry {
        seq,
        level: level.to_owned(),
        text: cap(&text, 2000),
        source,
        stack: stack.map(|stack| cap(&stack, 2000)),
    });
    while tab.console.len() > CONSOLE_KEPT {
        tab.console.pop_front();
    }
}

fn call_frame_source(frame: &Value) -> Option<String> {
    let url = frame["url"].as_str().filter(|url| !url.is_empty())?;
    Some(format!(
        "{}:{}",
        cap(url, 300),
        frame["lineNumber"].as_u64().unwrap_or(0) + 1
    ))
}

fn stack_text(stack: &Value) -> Option<String> {
    let frames: Vec<String> = stack["callFrames"]
        .as_array()?
        .iter()
        .take(8)
        .map(|frame| {
            let name = frame["functionName"]
                .as_str()
                .filter(|n| !n.is_empty())
                .unwrap_or("(anonymous)");
            format!(
                "at {name} ({})",
                call_frame_source(frame).unwrap_or_default()
            )
        })
        .collect();
    (!frames.is_empty()).then(|| frames.join("\n"))
}

fn argument_text(argument: &Value) -> String {
    argument["value"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            argument
                .get("value")
                .filter(|value| !value.is_null())
                .map(Value::to_string)
        })
        .or_else(|| argument["description"].as_str().map(str::to_owned))
        .or_else(|| argument["unserializableValue"].as_str().map(str::to_owned))
        .unwrap_or_else(|| argument["type"].as_str().unwrap_or("undefined").to_owned())
}

/// Records a tab event the debugging tools read later (console, exceptions, requests,
/// debugger pauses).
pub(super) fn record(state: &Mutex<TabState>, event: &CdpEvent) {
    let params = &event.params;
    let mut tab = state.lock().unwrap_or_else(|e| e.into_inner());
    match event.method.as_str() {
        "Runtime.consoleAPICalled" => {
            let text: Vec<String> = params["args"]
                .as_array()
                .into_iter()
                .flatten()
                .take(8)
                .map(|a| cap(&argument_text(a), 500))
                .collect();
            let frame = &params["stackTrace"]["callFrames"][0];
            let level = params["type"].as_str().unwrap_or("log");
            let stack = if matches!(level, "error" | "trace" | "assert") {
                stack_text(&params["stackTrace"])
            } else {
                None
            };
            push_console(
                &mut tab,
                level,
                text.join(" "),
                call_frame_source(frame),
                stack,
            );
        }
        "Runtime.exceptionThrown" => {
            let details = &params["exceptionDetails"];
            let text = details["exception"]["description"]
                .as_str()
                .or_else(|| details["text"].as_str())
                .unwrap_or("error")
                .to_owned();
            let source = details["url"]
                .as_str()
                .filter(|url| !url.is_empty())
                .map(|url| {
                    format!(
                        "{}:{}",
                        cap(url, 300),
                        details["lineNumber"].as_u64().unwrap_or(0) + 1
                    )
                });
            push_console(
                &mut tab,
                "exception",
                text,
                source,
                stack_text(&details["stackTrace"]),
            );
        }
        "Log.entryAdded" => {
            let entry = &params["entry"];
            let level = entry["level"].as_str().unwrap_or("info");
            let text = entry["text"].as_str().unwrap_or_default().to_owned();
            let source = entry["url"]
                .as_str()
                .filter(|url| !url.is_empty())
                .map(|url| cap(url, 300));
            push_console(&mut tab, level, text, source, None);
        }
        "Network.requestWillBeSent" => {
            let Some(id) = params["requestId"].as_str() else {
                return;
            };
            let request = &params["request"];
            let url = cap(request["url"].as_str().unwrap_or_default(), 2000);
            if let Some(existing) = tab.requests.iter_mut().rev().find(|entry| entry.id == id) {
                // A redirect keeps the request id.
                existing.url = url;
                return;
            }
            let seq = next_seq(&mut tab);
            tab.requests.push_back(RequestEntry {
                seq,
                id: id.to_owned(),
                method: request["method"].as_str().unwrap_or("GET").to_owned(),
                url,
                kind: params["type"].as_str().unwrap_or("Other").to_owned(),
                status: None,
                mime: None,
                bytes: None,
                ms: None,
                failed: None,
                started: Some(Instant::now()),
                request_headers: request["headers"].clone(),
                response_headers: Value::Null,
            });
            while tab.requests.len() > REQUESTS_KEPT {
                tab.requests.pop_front();
            }
        }
        "Network.responseReceived" => {
            let Some(id) = params["requestId"].as_str() else {
                return;
            };
            if let Some(entry) = tab.requests.iter_mut().rev().find(|entry| entry.id == id) {
                let response = &params["response"];
                entry.status = response["status"].as_u64();
                entry.mime = response["mimeType"].as_str().map(str::to_owned);
                entry.response_headers = response["headers"].clone();
            }
        }
        "Network.loadingFinished" | "Network.loadingFailed" => {
            let Some(id) = params["requestId"].as_str() else {
                return;
            };
            if let Some(entry) = tab.requests.iter_mut().rev().find(|entry| entry.id == id) {
                entry.ms = entry
                    .started
                    .map(|started| started.elapsed().as_millis() as u64);
                entry.bytes = params["encodedDataLength"]
                    .as_f64()
                    .map(|bytes| bytes as u64);
                if event.method == "Network.loadingFailed" {
                    entry.failed =
                        Some(params["errorText"].as_str().unwrap_or("failed").to_owned());
                }
            }
        }
        "Debugger.paused" => {
            tab.paused = Some(params.clone());
            tab.pauses += 1;
        }
        "Debugger.resumed" => tab.paused = None,
        "Page.frameNavigated" if params["frame"]["parentId"].is_null() => {
            tab.paused = None;
        }
        _ => {}
    }
}

/// Resumes a pause the agent leaves alone for [`PAUSE_LIMIT`], so a forgotten breakpoint
/// cannot hang the page (and the person's view of it).
pub(super) fn resume_later(cdp: Arc<Cdp>, session: String, state: Arc<Mutex<TabState>>) {
    let pause = state.lock().unwrap_or_else(|e| e.into_inner()).pauses;
    tokio::spawn(async move {
        tokio::time::sleep(PAUSE_LIMIT).await;
        let still = {
            let tab = state.lock().unwrap_or_else(|e| e.into_inner());
            tab.paused.is_some() && tab.pauses == pause
        };
        if still {
            cdp.send(Some(&session), "Debugger.resume", json!({}));
        }
    });
}

/// The current tab, for one debugging operation.
struct Target {
    cdp: Arc<Cdp>,
    session: String,
    state: Arc<Mutex<TabState>>,
    url: String,
}

fn bounded(value: &Value) -> Value {
    let text = value.to_string();
    if text.len() <= VALUE_BYTES {
        value.clone()
    } else {
        json!({"truncated": true, "bytes": text.len(), "start": cap(&text, VALUE_BYTES)})
    }
}

fn shown(value: Value, show_secrets: bool) -> Value {
    if show_secrets {
        value
    } else {
        redact::redact_value(&value)
    }
}

/// `value` with every string shortened to `limit` characters (after redaction).
fn capped_strings(value: Value, limit: usize) -> Value {
    match value {
        Value::String(text) => Value::String(cap(&text, limit)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|v| capped_strings(v, limit))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, capped_strings(v, limit)))
                .collect(),
        ),
        other => bounded(&other),
    }
}

fn shown_text(text: &str, show_secrets: bool) -> String {
    let text = cap(text, VALUE_BYTES);
    if show_secrets {
        text
    } else {
        redact::redact_text(&text)
    }
}

/// The result of `Runtime.evaluate`/`callFunctionOn`: its value, or the exception.
fn evaluation(result: &Value) -> Result<Value, String> {
    if let Some(details) = result.get("exceptionDetails") {
        let text = details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("the expression threw");
        return Err(cap(text, 2000));
    }
    let remote = &result["result"];
    Ok(match remote.get("value") {
        Some(value) => value.clone(),
        None => match remote["type"].as_str() {
            Some("undefined") => json!("undefined"),
            _ => json!(remote["description"]
                .as_str()
                .or_else(|| remote["unserializableValue"].as_str())
                .unwrap_or("(no value)")),
        },
    })
}

impl ChatBrowser {
    async fn devtools_target(&self) -> Result<Target, String> {
        let inner = self.inner.lock().await;
        let (Some(browser), Some(tab)) = (&inner.browser, inner.tab.as_ref()) else {
            return Err("no page is open; use browser_open first".into());
        };
        let url = tab
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .url
            .clone();
        Ok(Target {
            cdp: Arc::clone(&browser.cdp),
            session: tab.session_id.clone(),
            state: Arc::clone(&tab.state),
            url,
        })
    }

    async fn element_object(
        &self,
        target: &Target,
        selector: Option<&str>,
        reference: Option<&str>,
    ) -> Result<String, String> {
        if let Some(reference) = reference {
            let backend = {
                let inner = self.inner.lock().await;
                inner
                    .tab
                    .as_ref()
                    .and_then(|tab| tab.refs.resolve(reference))
            }
            .ok_or_else(|| {
                format!("no element {reference} on this page; take a new browser_snapshot")
            })?;
            return resolve_object(&target.cdp, &target.session, backend).await;
        }
        let selector = selector.ok_or("give a selector or a ref")?;
        let found = target
            .cdp
            .call(
                Some(&target.session),
                "Runtime.evaluate",
                json!({"expression": format!("document.querySelector({})", serde_json::to_string(selector).unwrap_or_default())}),
            )
            .await?;
        if found.get("exceptionDetails").is_some() {
            return Err(format!(
                "'{}' is not a valid CSS selector",
                cap(selector, 200)
            ));
        }
        found["result"]["objectId"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("no element matches '{}'", cap(selector, 200)))
    }
}

mod debugger;
mod inspect;
mod profiling;
#[cfg(test)]
use debugger::regex_escape;
#[cfg(test)]
use profiling::{summarize_heap, summarize_trace};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traces_and_heap_snapshots_are_summarized() {
        let trace = json!({"traceEvents": [
            {"name": "RunTask", "dur": 120_000}, {"name": "RunTask", "dur": 4_000},
            {"name": "FunctionCall", "dur": 90_000}, {"name": "Layout", "dur": 10_000}, {"name": "Paint", "dur": 2_000}
        ]});
        let summary = summarize_trace(trace.to_string().as_bytes(), 3);
        assert_eq!(summary["long_tasks_over_50ms"], 1);
        assert_eq!(summary["longest_task_ms"], 120.0);
        assert_eq!(summary["scripting_ms"], 90.0);
        let heap = json!({
            "snapshot": {"meta": {"node_fields": ["type", "name", "id", "self_size"]}},
            "nodes": [0, 0, 1, 100, 0, 1, 2, 30, 0, 0, 3, 50],
            "strings": ["Array", "Leaky"]
        });
        let summary = summarize_heap(heap.to_string().as_bytes());
        assert_eq!(summary["objects"], 3);
        assert_eq!(summary["largest"][0]["name"], "Array");
        assert_eq!(summary["largest"][0]["self_bytes"], 150);
        assert_eq!(regex_escape("app.js?v=1"), "app\\.js\\?v=1");
    }

    #[test]
    fn frame_variables_are_redacted_as_values_before_they_are_shortened() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.c2lnbmF0dXJlLXZhbHVl";
        let variables =
            json!({"session": "abc", "header": jwt, "count": 3, "label": "x".repeat(300)});
        let shown = capped_strings(shown(variables, false), 200);
        assert!(
            shown["header"].as_str().unwrap().starts_with("[hidden"),
            "{shown}"
        );
        assert!(
            shown["session"].as_str().unwrap().starts_with("[hidden"),
            "{shown}"
        );
        assert_eq!(shown["count"], 3);
        assert!(shown["label"].as_str().unwrap().chars().count() <= 201);
    }

    #[test]
    fn evaluation_returns_values_descriptions_and_exceptions() {
        assert_eq!(
            evaluation(&json!({"result": {"type": "number", "value": 3}})).unwrap(),
            3
        );
        assert_eq!(
            evaluation(&json!({"result": {"type": "undefined"}})).unwrap(),
            "undefined"
        );
        assert_eq!(
            evaluation(&json!({"result": {"type": "function", "description": "function f() {}"}}))
                .unwrap(),
            "function f() {}"
        );
        assert!(evaluation(&json!({"exceptionDetails": {"exception": {"description": "ReferenceError: x is not defined"}}}))
            .unwrap_err()
            .contains("ReferenceError"));
    }
}
