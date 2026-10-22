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
        let (Some(browser), Some(tab)) = (&inner.browser, inner.tabs.get(inner.current)) else {
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
                    .tabs
                    .get(inner.current)
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

    /// Runs `expression` in the page (or, while paused, in the top call frame) and returns its
    /// value; without one, the console messages newer than `since`.
    pub async fn dev_console(
        &self,
        expression: Option<&str>,
        since: u64,
        show_secrets: bool,
    ) -> Result<Value, String> {
        let target = self.devtools_target().await?;
        let Some(expression) = expression.filter(|e| !e.trim().is_empty()) else {
            let tab = target.state.lock().unwrap_or_else(|e| e.into_inner());
            let newer: Vec<&ConsoleEntry> = tab
                .console
                .iter()
                .filter(|entry| entry.seq > since)
                .collect();
            let skipped = newer.len().saturating_sub(LISTED);
            let messages: Vec<Value> = newer[skipped..]
                .iter()
                .map(|entry| {
                    let mut entry = serde_json::to_value(entry).unwrap_or_default();
                    if let Some(text) = entry["text"].as_str() {
                        entry["text"] = json!(shown_text(text, show_secrets));
                    }
                    entry
                })
                .collect();
            return Ok(
                json!({"messages": messages, "older_not_shown": skipped, "next_since": tab.log_seq}),
            );
        };
        let paused_frame = target
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .paused
            .as_ref()
            .and_then(|paused| {
                paused["callFrames"][0]["callFrameId"]
                    .as_str()
                    .map(str::to_owned)
            });
        let result = match paused_frame {
            Some(frame) => {
                target
                    .cdp
                    .call(
                        Some(&target.session),
                        "Debugger.evaluateOnCallFrame",
                        json!({"callFrameId": frame, "expression": expression, "returnByValue": true, "timeout": 5000}),
                    )
                    .await?
            }
            None => {
                target
                    .cdp
                    .call(
                        Some(&target.session),
                        "Runtime.evaluate",
                        json!({"expression": expression, "returnByValue": true, "awaitPromise": true, "userGesture": true, "replMode": true, "timeout": 5000}),
                    )
                    .await?
            }
        };
        Ok(match evaluation(&result) {
            Ok(value) => json!({"value": bounded(&shown(value, show_secrets))}),
            Err(exception) => json!({"exception": shown_text(&exception, show_secrets)}),
        })
    }

    /// Inspects or changes one element, found by CSS `selector` or snapshot `reference`.
    #[allow(clippy::too_many_arguments)]
    pub async fn dev_elements(
        &self,
        selector: Option<&str>,
        reference: Option<&str>,
        action: &str,
        name: &str,
        value: &str,
        properties: &[String],
        show_secrets: bool,
    ) -> Result<Value, String> {
        let target = self.devtools_target().await?;
        let object = self.element_object(&target, selector, reference).await?;
        let defaults = [
            "display",
            "position",
            "width",
            "height",
            "margin",
            "padding",
            "color",
            "background-color",
            "font-size",
            "font-family",
            "visibility",
            "opacity",
            "z-index",
            "overflow",
        ];
        let properties: Vec<&str> = if properties.is_empty() {
            defaults.to_vec()
        } else {
            properties.iter().map(String::as_str).take(40).collect()
        };
        let function = match action {
            "inspect" => "function(props){const r=this.getBoundingClientRect();const s=getComputedStyle(this);const styles={};for(const p of props){styles[p]=s.getPropertyValue(p)}const attributes={};for(const a of this.attributes){attributes[a.name]=a.value}return{tag:this.tagName.toLowerCase(),attributes,box:{x:Math.round(r.x),y:Math.round(r.y),width:Math.round(r.width),height:Math.round(r.height)},styles,text:(this.innerText||'').slice(0,300),children:this.children.length,html:this.outerHTML.slice(0,4000)}}",
            "set_attribute" => "function(n,v){this.setAttribute(n,v);return this.outerHTML.slice(0,1000)}",
            "remove_attribute" => "function(n){this.removeAttribute(n);return this.outerHTML.slice(0,1000)}",
            "set_style" => "function(n,v){this.style.setProperty(n,v);return getComputedStyle(this).getPropertyValue(n)}",
            "set_text" => "function(n,v){this.textContent=v;return this.outerHTML.slice(0,1000)}",
            "remove" => "function(){this.remove();return 'removed'}",
            other => return Err(format!("action must be inspect, set_attribute, remove_attribute, set_style, set_text or remove, not '{}'", cap(other, 30))),
        };
        let arguments = if action == "inspect" {
            vec![json!({"value": properties})]
        } else {
            vec![json!({"value": name}), json!({"value": value})]
        };
        let result = target
            .cdp
            .call(
                Some(&target.session),
                "Runtime.callFunctionOn",
                json!({"objectId": object, "functionDeclaration": function, "arguments": arguments, "returnByValue": true}),
            )
            .await?;
        let value = evaluation(&result)?;
        Ok(
            json!({"action": action, "result": bounded(&shown(value, show_secrets)), "note": "Changes are live in the page only; nothing is saved."}),
        )
    }

    /// Lists requests newer than `since`, or one request in detail; can block URL patterns
    /// and throttle the tab.
    pub async fn dev_network(
        &self,
        id: Option<&str>,
        since: u64,
        block: Option<&[String]>,
        throttle: Option<&str>,
        show_secrets: bool,
    ) -> Result<Value, String> {
        let target = self.devtools_target().await?;
        let mut changed = Vec::new();
        if let Some(patterns) = block {
            let patterns: Vec<String> = patterns.iter().take(20).map(|p| cap(p, 300)).collect();
            target
                .cdp
                .call(
                    Some(&target.session),
                    "Network.setBlockedURLs",
                    json!({"urls": patterns}),
                )
                .await?;
            changed.push(json!({"blocked": patterns}));
        }
        if let Some(profile) = throttle {
            let (offline, latency, down, up) = match profile {
                "off" | "none" => (false, 0.0, -1.0, -1.0),
                "offline" => (true, 0.0, 0.0, 0.0),
                "slow3g" | "slow 3g" => (false, 2000.0, 500.0 * 1024.0 / 8.0, 500.0 * 1024.0 / 8.0),
                "fast3g" | "fast 3g" => (
                    false,
                    563.0,
                    1.6 * 1024.0 * 1024.0 / 8.0,
                    750.0 * 1024.0 / 8.0,
                ),
                other => {
                    return Err(format!(
                        "throttle must be off, offline, slow3g or fast3g, not '{}'",
                        cap(other, 20)
                    ))
                }
            };
            target
                .cdp
                .call(
                    Some(&target.session),
                    "Network.emulateNetworkConditions",
                    json!({"offline": offline, "latency": latency, "downloadThroughput": down, "uploadThroughput": up}),
                )
                .await?;
            changed.push(json!({"throttle": profile}));
        }
        if let Some(id) = id {
            let entry = {
                let tab = target.state.lock().unwrap_or_else(|e| e.into_inner());
                tab.requests
                    .iter()
                    .rev()
                    .find(|entry| entry.id == id)
                    .cloned()
            }
            .ok_or_else(|| {
                format!(
                    "no request {} on this tab; list requests first",
                    cap(id, 60)
                )
            })?;
            let body = match target
                .cdp
                .call(
                    Some(&target.session),
                    "Network.getResponseBody",
                    json!({"requestId": id}),
                )
                .await
            {
                Ok(body) if body["base64Encoded"].as_bool() == Some(true) => {
                    json!(format!(
                        "[binary, {} base64 characters]",
                        body["body"].as_str().unwrap_or_default().len()
                    ))
                }
                Ok(body) => json!(shown_text(
                    body["body"].as_str().unwrap_or_default(),
                    show_secrets
                )),
                Err(_) => json!("[no body available]"),
            };
            let headers = |headers: &Value| {
                if show_secrets {
                    headers.clone()
                } else {
                    redact::redact_headers(headers)
                }
            };
            let mut detail = serde_json::to_value(&entry).unwrap_or_default();
            detail["url"] = json!(shown_text(&entry.url, show_secrets));
            detail["request_headers"] = headers(&entry.request_headers);
            detail["response_headers"] = headers(&entry.response_headers);
            detail["body"] = body;
            return Ok(detail);
        }
        let tab = target.state.lock().unwrap_or_else(|e| e.into_inner());
        let newer: Vec<&RequestEntry> = tab
            .requests
            .iter()
            .filter(|entry| entry.seq > since)
            .collect();
        let skipped = newer.len().saturating_sub(LISTED);
        let requests: Vec<Value> = newer[skipped..]
            .iter()
            .map(|entry| {
                let mut row = serde_json::to_value(entry).unwrap_or_default();
                row["url"] = json!(shown_text(&cap(&entry.url, 300), show_secrets));
                row
            })
            .collect();
        Ok(
            json!({"requests": requests, "older_not_shown": skipped, "next_since": tab.log_seq, "changed": changed}),
        )
    }

    /// Breakpoints, stepping and the paused state (call stack and scope variables).
    pub async fn dev_debugger(
        &self,
        action: &str,
        url: &str,
        line: u64,
        condition: &str,
        id: &str,
        show_secrets: bool,
    ) -> Result<Value, String> {
        let target = self.devtools_target().await?;
        target
            .cdp
            .call(Some(&target.session), "Debugger.enable", json!({}))
            .await?;
        let wait_for_change = |paused_before: bool| {
            let state = Arc::clone(&target.state);
            async move {
                for _ in 0..20 {
                    let paused = state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .paused
                        .is_some();
                    if paused != paused_before {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        };
        let paused_now = || {
            target
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .paused
                .is_some()
        };
        match action {
            "set" => {
                if url.trim().is_empty() || line == 0 {
                    return Err("set needs a url (or part of one) and a line number (1-based)".into());
                }
                let mut params = json!({"lineNumber": line - 1, "condition": condition});
                if url.contains("://") {
                    params["url"] = json!(url);
                } else {
                    params["urlRegex"] = json!(regex_escape(url));
                }
                let set = target.cdp.call(Some(&target.session), "Debugger.setBreakpointByUrl", params).await?;
                return Ok(json!({"breakpoint": set["breakpointId"], "resolved_locations": set["locations"].as_array().map_or(0, Vec::len)}));
            }
            "remove" => {
                target.cdp.call(Some(&target.session), "Debugger.removeBreakpoint", json!({"breakpointId": id})).await?;
                return Ok(json!({"removed": id}));
            }
            "pause_on_exceptions" => {
                let state = match condition {
                    "all" | "uncaught" | "none" => condition,
                    _ => "uncaught",
                };
                target.cdp.call(Some(&target.session), "Debugger.setPauseOnExceptions", json!({"state": state})).await?;
                return Ok(json!({"pause_on_exceptions": state}));
            }
            "pause" => {
                target.cdp.call(Some(&target.session), "Debugger.pause", json!({})).await?;
                wait_for_change(false).await;
            }
            "resume" | "step_over" | "step_into" | "step_out" => {
                if !paused_now() {
                    return Err("the page is not paused".into());
                }
                let method = match action {
                    "resume" => "Debugger.resume",
                    "step_over" => "Debugger.stepOver",
                    "step_into" => "Debugger.stepInto",
                    _ => "Debugger.stepOut",
                };
                let pauses = target.state.lock().unwrap_or_else(|e| e.into_inner()).pauses;
                target.cdp.call(Some(&target.session), method, json!({})).await?;
                for _ in 0..20 {
                    let moved = {
                        let tab = target.state.lock().unwrap_or_else(|e| e.into_inner());
                        tab.paused.is_none() || tab.pauses != pauses
                    };
                    if moved {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
            "state" => {}
            other => return Err(format!("action must be set, remove, pause_on_exceptions, pause, resume, step_over, step_into, step_out or state, not '{}'", cap(other, 30))),
        }
        let paused = target
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .paused
            .clone();
        let Some(paused) = paused else {
            return Ok(json!({"paused": false}));
        };
        let frames: Vec<Value> = paused["callFrames"]
            .as_array()
            .into_iter()
            .flatten()
            .take(10)
            .map(|frame| {
                json!({
                    "function": frame["functionName"].as_str().filter(|n| !n.is_empty()).unwrap_or("(anonymous)"),
                    "source": format!("{}:{}", cap(frame["url"].as_str().unwrap_or_default(), 300), frame["location"]["lineNumber"].as_u64().unwrap_or(0) + 1),
                })
            })
            .collect();
        let mut variables = serde_json::Map::new();
        for scope in paused["callFrames"][0]["scopeChain"]
            .as_array()
            .into_iter()
            .flatten()
            .take(3)
        {
            if !matches!(
                scope["type"].as_str(),
                Some("local" | "closure" | "block" | "catch")
            ) {
                continue;
            }
            let Some(object) = scope["object"]["objectId"].as_str() else {
                continue;
            };
            let Ok(properties) = target
                .cdp
                .call(
                    Some(&target.session),
                    "Runtime.getProperties",
                    json!({"objectId": object, "ownProperties": true, "generatePreview": true}),
                )
                .await
            else {
                continue;
            };
            for property in properties["result"]
                .as_array()
                .into_iter()
                .flatten()
                .take(40)
            {
                let Some(name) = property["name"].as_str() else {
                    continue;
                };
                let value = &property["value"];
                let text = value
                    .get("value")
                    .filter(|v| !v.is_null())
                    .map(Value::to_string)
                    .or_else(|| value["description"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| value["type"].as_str().unwrap_or("undefined").to_owned());
                variables
                    .entry(name.to_owned())
                    .or_insert(json!(cap(&text, 200)));
            }
        }
        Ok(json!({
            "paused": true,
            "reason": paused["reason"],
            "call_stack": frames,
            "variables": shown(Value::Object(variables), show_secrets),
            "note": "The page stays paused until you resume or step; it resumes by itself after 30 s."
        }))
    }

    /// Cookies, localStorage or sessionStorage of the current page: read, set, remove or clear.
    pub async fn dev_storage(
        &self,
        action: &str,
        area: &str,
        key: &str,
        value: &str,
        show_secrets: bool,
    ) -> Result<Value, String> {
        let target = self.devtools_target().await?;
        let page = target.url.clone();
        match (area, action) {
            ("cookies", "read") => {
                let cookies = target
                    .cdp
                    .call(
                        Some(&target.session),
                        "Network.getCookies",
                        json!({"urls": [page]}),
                    )
                    .await?;
                let rows: Vec<Value> = cookies["cookies"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(100)
                    .map(|cookie| {
                        let raw = cookie["value"].as_str().unwrap_or_default();
                        let http_only = cookie["httpOnly"].as_bool().unwrap_or(false);
                        let shown_value = if http_only {
                            "[httpOnly: not readable by page scripts, so not shown]".to_owned()
                        } else if show_secrets {
                            cap(raw, 1000)
                        } else {
                            redact::hidden(raw)
                        };
                        json!({"name": cookie["name"], "value": shown_value, "domain": cookie["domain"], "path": cookie["path"], "expires": cookie["expires"], "httpOnly": http_only, "secure": cookie["secure"], "sameSite": cookie["sameSite"]})
                    })
                    .collect();
                Ok(json!({"cookies": rows}))
            }
            ("cookies", "set") => {
                target
                    .cdp
                    .call(
                        Some(&target.session),
                        "Network.setCookie",
                        json!({"name": key, "value": value, "url": page}),
                    )
                    .await?;
                Ok(json!({"set": key}))
            }
            ("cookies", "remove") => {
                target
                    .cdp
                    .call(
                        Some(&target.session),
                        "Network.deleteCookies",
                        json!({"name": key, "url": page}),
                    )
                    .await?;
                Ok(json!({"removed": key}))
            }
            ("cookies", "clear") => {
                let cookies = target
                    .cdp
                    .call(
                        Some(&target.session),
                        "Network.getCookies",
                        json!({"urls": [page]}),
                    )
                    .await?;
                let mut count = 0;
                for cookie in cookies["cookies"].as_array().into_iter().flatten() {
                    let _ = target
                        .cdp
                        .call(
                            Some(&target.session),
                            "Network.deleteCookies",
                            json!({"name": cookie["name"], "url": page}),
                        )
                        .await;
                    count += 1;
                }
                Ok(json!({"cleared": count}))
            }
            ("local" | "session", _) => {
                let store = if area == "local" {
                    "localStorage"
                } else {
                    "sessionStorage"
                };
                let (k, v) = (
                    serde_json::to_string(key).unwrap_or_default(),
                    serde_json::to_string(value).unwrap_or_default(),
                );
                let expression = match action {
                    "read" => format!("Object.fromEntries(Object.entries({store}).slice(0,200))"),
                    "set" => format!("({store}.setItem({k},{v}),'set')"),
                    "remove" => format!("({store}.removeItem({k}),'removed')"),
                    "clear" => format!("({store}.clear(),'cleared')"),
                    other => {
                        return Err(format!(
                            "action must be read, set, remove or clear, not '{}'",
                            cap(other, 20)
                        ))
                    }
                };
                let result = target
                    .cdp
                    .call(
                        Some(&target.session),
                        "Runtime.evaluate",
                        json!({"expression": expression, "returnByValue": true}),
                    )
                    .await?;
                let value = evaluation(&result)?;
                let value = if action == "read" && !show_secrets {
                    json!(value.as_object().map(|map| {
                        map.iter()
                            .map(|(name, value)| {
                                let text = value.as_str().unwrap_or_default();
                                let hidden =
                                    redact::is_secret_key(name) || redact::looks_like_token(text);
                                (
                                    name.clone(),
                                    if hidden {
                                        json!(redact::hidden(text))
                                    } else {
                                        redact::redact_value(value)
                                    },
                                )
                            })
                            .collect::<serde_json::Map<String, Value>>()
                    }))
                } else {
                    value
                };
                Ok(json!({"area": area, "result": bounded(&value)}))
            }
            _ => Err(
                "area must be cookies, local or session, and action read, set, remove or clear"
                    .into(),
            ),
        }
    }

    /// Page performance: a summary of metrics and web vitals, or a trace of `seconds`
    /// returned as bytes for the caller to save (with its summary).
    pub async fn dev_performance(
        &self,
        action: &str,
        seconds: u64,
    ) -> Result<(Value, Option<Vec<u8>>), String> {
        let target = self.devtools_target().await?;
        target
            .cdp
            .call(Some(&target.session), "Performance.enable", json!({}))
            .await?;
        match action {
            "metrics" | "audit" => {
                let metrics = target
                    .cdp
                    .call(Some(&target.session), "Performance.getMetrics", json!({}))
                    .await?;
                let metrics: serde_json::Map<String, Value> = metrics["metrics"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| Some((m["name"].as_str()?.to_owned(), m["value"].clone())))
                    .filter(|(name, _)| {
                        matches!(
                            name.as_str(),
                            "Documents"
                                | "Nodes"
                                | "JSEventListeners"
                                | "LayoutCount"
                                | "RecalcStyleCount"
                                | "LayoutDuration"
                                | "RecalcStyleDuration"
                                | "ScriptDuration"
                                | "TaskDuration"
                                | "JSHeapUsedSize"
                                | "JSHeapTotalSize"
                        )
                    })
                    .collect();
                let vitals = target
                    .cdp
                    .call(
                        Some(&target.session),
                        "Runtime.evaluate",
                        json!({"awaitPromise": true, "returnByValue": true, "expression": VITALS_SCRIPT}),
                    )
                    .await?;
                Ok((
                    json!({"metrics": metrics, "page": evaluation(&vitals).unwrap_or(Value::Null)}),
                    None,
                ))
            }
            "trace" => {
                let _heavy = HeavyJob::start()?;
                let seconds = seconds.clamp(1, TRACE_LIMIT_SECONDS);
                let mut events = target.cdp.subscribe();
                target
                    .cdp
                    .call(
                        Some(&target.session),
                        "Tracing.start",
                        json!({"transferMode": "ReturnAsStream", "traceConfig": {"includedCategories": ["devtools.timeline", "v8.execute", "blink.user_timing", "loading", "disabled-by-default-devtools.timeline"]}}),
                    )
                    .await?;
                tokio::time::sleep(Duration::from_secs(seconds)).await;
                target
                    .cdp
                    .call(Some(&target.session), "Tracing.end", json!({}))
                    .await?;
                let stream = tokio::time::timeout(Duration::from_secs(30), async {
                    loop {
                        match events.recv().await {
                            Ok(event)
                                if event.method == "Tracing.tracingComplete"
                                    && event.session.as_deref()
                                        == Some(target.session.as_str()) =>
                            {
                                return event.params["stream"].as_str().map(str::to_owned);
                            }
                            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                            Err(_) => return None,
                        }
                    }
                })
                .await
                .ok()
                .flatten()
                .ok_or("the trace did not finish")?;
                let bytes = read_stream(&target, &stream).await?;
                Ok((summarize_trace(&bytes, seconds), Some(bytes)))
            }
            other => Err(format!(
                "action must be metrics or trace, not '{}'",
                cap(other, 20)
            )),
        }
    }

    /// Memory: a summary (compared with the previous one), or a heap snapshot returned as
    /// bytes for the caller to save (with the largest object kinds).
    pub async fn dev_memory(&self, action: &str) -> Result<(Value, Option<Vec<u8>>), String> {
        let target = self.devtools_target().await?;
        match action {
            "summary" => {
                target
                    .cdp
                    .call(Some(&target.session), "Performance.enable", json!({}))
                    .await?;
                let heap = target
                    .cdp
                    .call(Some(&target.session), "Runtime.getHeapUsage", json!({}))
                    .await?;
                let metrics = target
                    .cdp
                    .call(Some(&target.session), "Performance.getMetrics", json!({}))
                    .await?;
                let metric = |name: &str| {
                    metrics["metrics"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|m| m["name"] == name)
                        .and_then(|m| m["value"].as_f64())
                        .unwrap_or(0.0)
                };
                let now = json!({
                    "heap_used_bytes": heap["usedSize"], "heap_total_bytes": heap["totalSize"],
                    "dom_nodes": metric("Nodes"), "event_listeners": metric("JSEventListeners"), "documents": metric("Documents"),
                });
                let previous = MEMORY_BASELINE
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .replace(now.clone());
                let growth = previous.map(|before| {
                    let delta = |key: &str| now[key].as_f64().unwrap_or(0.0) - before[key].as_f64().unwrap_or(0.0);
                    json!({"heap_used_bytes": delta("heap_used_bytes"), "dom_nodes": delta("dom_nodes"), "event_listeners": delta("event_listeners"), "documents": delta("documents")})
                });
                Ok((
                    json!({"now": now, "growth_since_last_summary": growth}),
                    None,
                ))
            }
            "snapshot" => {
                let _heavy = HeavyJob::start()?;
                let mut events = target.cdp.subscribe();
                let session = target.session.clone();
                let (done, mut finished) = tokio::sync::oneshot::channel::<()>();
                let collect = tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let mut finishing = false;
                    loop {
                        // After the call returns, drain what is already queued, then stop.
                        let event = if finishing {
                            match tokio::time::timeout(Duration::from_millis(300), events.recv())
                                .await
                            {
                                Ok(event) => event,
                                Err(_) => break,
                            }
                        } else {
                            tokio::select! {
                                event = events.recv() => event,
                                _ = &mut finished => { finishing = true; continue; }
                            }
                        };
                        match event {
                            Ok(event)
                                if event.method == "HeapProfiler.addHeapSnapshotChunk"
                                    && event.session.as_deref() == Some(session.as_str()) =>
                            {
                                bytes.extend_from_slice(
                                    event.params["chunk"]
                                        .as_str()
                                        .unwrap_or_default()
                                        .as_bytes(),
                                );
                            }
                            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                            Err(_) => break,
                        }
                        if bytes.len() > 512 * 1024 * 1024 {
                            break;
                        }
                    }
                    bytes
                });
                target
                    .cdp
                    .call(Some(&target.session), "HeapProfiler.enable", json!({}))
                    .await?;
                let taken = target
                    .cdp
                    .call_with_timeout(
                        Some(&target.session),
                        "HeapProfiler.takeHeapSnapshot",
                        json!({"reportProgress": false}),
                        Duration::from_secs(120),
                    )
                    .await;
                let _ = done.send(());
                let bytes = collect.await.unwrap_or_default();
                taken?;
                Ok((summarize_heap(&bytes), Some(bytes)))
            }
            other => Err(format!(
                "action must be summary or snapshot, not '{}'",
                cap(other, 20)
            )),
        }
    }
}

impl ChatBrowser {
    /// Ends the run's debugging on every tab of this chat: breakpoints and pause-on-exception
    /// are cleared (a paused page resumes), and request blocking and throttling are lifted.
    pub async fn end_debugging(&self) {
        let targets: Vec<(Arc<Cdp>, String, bool)> = {
            let inner = self.inner.lock().await;
            let Some(browser) = &inner.browser else {
                return;
            };
            inner
                .tabs
                .iter()
                .map(|tab| {
                    let paused = tab
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .paused
                        .is_some();
                    (Arc::clone(&browser.cdp), tab.session_id.clone(), paused)
                })
                .collect()
        };
        for (cdp, session, paused) in targets {
            if paused {
                let _ = cdp.call(Some(&session), "Debugger.resume", json!({})).await;
            }
            // Disabling the debugger drops its breakpoints; the agent enables it again on use.
            let _ = cdp
                .call(Some(&session), "Debugger.disable", json!({}))
                .await;
            let _ = cdp
                .call(
                    Some(&session),
                    "Network.setBlockedURLs",
                    json!({"urls": []}),
                )
                .await;
            let _ = cdp
                .call(
                    Some(&session),
                    "Network.emulateNetworkConditions",
                    json!({"offline": false, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1}),
                )
                .await;
        }
    }
}

static MEMORY_BASELINE: Mutex<Option<Value>> = Mutex::new(None);

struct HeavyJob;
impl HeavyJob {
    fn start() -> Result<Self, String> {
        if HEAVY_JOB.swap(true, Ordering::SeqCst) {
            return Err("another trace or heap snapshot is running; try again when it ends".into());
        }
        Ok(Self)
    }
}
impl Drop for HeavyJob {
    fn drop(&mut self) {
        HEAVY_JOB.store(false, Ordering::SeqCst);
    }
}

async fn read_stream(target: &Target, handle: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let mut bytes = Vec::new();
    loop {
        let chunk = target
            .cdp
            .call(
                Some(&target.session),
                "IO.read",
                json!({"handle": handle, "size": 1 << 20}),
            )
            .await?;
        let data = chunk["data"].as_str().unwrap_or_default();
        if chunk["base64Encoded"].as_bool() == Some(true) {
            bytes.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|e| e.to_string())?,
            );
        } else {
            bytes.extend_from_slice(data.as_bytes());
        }
        if chunk["eof"].as_bool() == Some(true) || bytes.len() > 256 * 1024 * 1024 {
            break;
        }
    }
    let _ = target
        .cdp
        .call(Some(&target.session), "IO.close", json!({"handle": handle}))
        .await;
    Ok(bytes)
}

/// Long tasks and main-thread time by kind, from a trace's complete events.
fn summarize_trace(bytes: &[u8], seconds: u64) -> Value {
    let trace: Value = serde_json::from_slice(bytes).unwrap_or(Value::Null);
    let events = trace["traceEvents"]
        .as_array()
        .or_else(|| trace.as_array())
        .cloned()
        .unwrap_or_default();
    let (mut long_tasks, mut longest, mut scripting, mut rendering, mut painting, mut total) =
        (0u64, 0f64, 0f64, 0f64, 0f64, 0f64);
    for event in &events {
        let ms = event["dur"].as_f64().unwrap_or(0.0) / 1000.0;
        match event["name"].as_str().unwrap_or_default() {
            "RunTask" => {
                total += ms;
                if ms > 50.0 {
                    long_tasks += 1;
                }
                longest = longest.max(ms);
            }
            "FunctionCall" | "EvaluateScript" | "v8.compile" | "TimerFire" | "EventDispatch" => {
                scripting += ms
            }
            "Layout" | "UpdateLayoutTree" | "RecalculateStyles" | "ParseHTML" => rendering += ms,
            "Paint" | "CompositeLayers" | "RasterTask" => painting += ms,
            _ => {}
        }
    }
    let round = |ms: f64| (ms * 10.0).round() / 10.0;
    json!({
        "seconds": seconds,
        "events": events.len(),
        "long_tasks_over_50ms": long_tasks,
        "longest_task_ms": round(longest),
        "main_thread_busy_ms": round(total),
        "scripting_ms": round(scripting),
        "rendering_ms": round(rendering),
        "painting_ms": round(painting),
    })
}

/// Object kinds with the largest self size, from a heap snapshot.
fn summarize_heap(bytes: &[u8]) -> Value {
    let snapshot: Value = match serde_json::from_slice(bytes) {
        Ok(snapshot) => snapshot,
        Err(_) => return json!({"bytes": bytes.len(), "summary": "the snapshot could not be read"}),
    };
    let fields: Vec<&str> = snapshot["snapshot"]["meta"]["node_fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let width = fields.len().max(1);
    let (name_at, size_at) = (
        fields.iter().position(|f| *f == "name"),
        fields.iter().position(|f| *f == "self_size"),
    );
    let (Some(name_at), Some(size_at)) = (name_at, size_at) else {
        return json!({"bytes": bytes.len()});
    };
    let strings: Vec<&str> = snapshot["strings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let nodes = snapshot["nodes"].as_array().cloned().unwrap_or_default();
    let mut by_name: std::collections::HashMap<&str, (u64, u64)> = std::collections::HashMap::new();
    let mut total = 0u64;
    for node in nodes.chunks(width) {
        let size = node.get(size_at).and_then(Value::as_u64).unwrap_or(0);
        let name = node
            .get(name_at)
            .and_then(Value::as_u64)
            .and_then(|i| strings.get(i as usize))
            .copied()
            .unwrap_or("?");
        let entry = by_name.entry(name).or_default();
        entry.0 += 1;
        entry.1 += size;
        total += size;
    }
    let mut largest: Vec<(&str, (u64, u64))> = by_name.into_iter().collect();
    largest.sort_by_key(|entry| std::cmp::Reverse(entry.1 .1));
    let largest: Vec<Value> = largest
        .into_iter()
        .take(15)
        .map(|(name, (count, bytes))| json!({"name": cap(name, 80), "count": count, "self_bytes": bytes}))
        .collect();
    json!({"objects": nodes.len() / width, "self_bytes": total, "largest": largest})
}

fn regex_escape(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            let special = "\\.+*?()|[]{}^$".contains(c);
            special
                .then_some('\\')
                .into_iter()
                .chain(std::iter::once(c))
        })
        .collect()
}

/// Web vitals and load timing the page can report about itself.
const VITALS_SCRIPT: &str = r#"(async()=>{const get=t=>new Promise(r=>{try{new PerformanceObserver((l,o)=>{o.disconnect();r(l.getEntries())}).observe({type:t,buffered:true});setTimeout(()=>r([]),300)}catch(e){r([])}});const [lcp,shifts]=await Promise.all([get('largest-contentful-paint'),get('layout-shift')]);const nav=performance.getEntriesByType('navigation')[0]||{};const res=performance.getEntriesByType('resource');const round=n=>Math.round(n||0);return{url:location.href.slice(0,300),ttfb_ms:round(nav.responseStart),dom_content_loaded_ms:round(nav.domContentLoadedEventEnd),load_ms:round(nav.loadEventEnd),largest_contentful_paint_ms:lcp.length?round(lcp[lcp.length-1].startTime):null,cumulative_layout_shift:Math.round(shifts.filter(s=>!s.hadRecentInput).reduce((a,s)=>a+s.value,0)*1000)/1000,resources:res.length,transfer_bytes:res.reduce((a,r)=>a+(r.transferSize||0),0),largest_resources:res.sort((a,b)=>(b.transferSize||0)-(a.transferSize||0)).slice(0,5).map(r=>({url:r.name.slice(0,200),bytes:r.transferSize||0})),images_without_alt:document.querySelectorAll('img:not([alt])').length}})()"#;

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
