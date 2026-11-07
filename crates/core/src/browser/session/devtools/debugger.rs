//! The page debugger: breakpoints, pauses, stepping and frame variables, and ending it all.

use super::*;

impl ChatBrowser {
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
                // Keep strings as strings, so redaction sees the real value.
                let raw = value
                    .get("value")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .or_else(|| value["description"].as_str().map(|d| json!(d)))
                    .unwrap_or_else(|| json!(value["type"].as_str().unwrap_or("undefined")));
                variables.entry(name.to_owned()).or_insert(raw);
            }
        }
        Ok(json!({
            "paused": true,
            "reason": paused["reason"],
            "call_stack": frames,
            "variables": capped_strings(shown(Value::Object(variables), show_secrets), 200),
            "note": crate::prompts::TOOL_DEVTOOLS_PAUSED
        }))
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
                .tab
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

pub(super) fn regex_escape(text: &str) -> String {
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
