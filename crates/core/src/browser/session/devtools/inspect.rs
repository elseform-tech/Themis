//! Reading a page as it runs: console messages, elements and network requests.

use super::*;

impl ChatBrowser {
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
                // REPL mode (top-level await, re-declared `let`) returns a promise as the
                // Promise itself, so it is awaited here, and objects are read by value.
                let evaluated = target
                    .cdp
                    .call(
                        Some(&target.session),
                        "Runtime.evaluate",
                        json!({"expression": expression, "awaitPromise": true, "userGesture": true, "replMode": true, "timeout": 5000}),
                    )
                    .await?;
                let remote = &evaluated["result"];
                match remote["objectId"].as_str() {
                    Some(object) if evaluated.get("exceptionDetails").is_none() => {
                        let (method, params) = if remote["subtype"] == "promise" {
                            ("Runtime.awaitPromise", json!({"promiseObjectId": object, "returnByValue": true}))
                        } else {
                            (
                                "Runtime.callFunctionOn",
                                json!({"objectId": object, "functionDeclaration": "function(){return this}", "returnByValue": true}),
                            )
                        };
                        match tokio::time::timeout(
                            Duration::from_secs(10),
                            target.cdp.call(Some(&target.session), method, params),
                        )
                        .await
                        {
                            Ok(Ok(value)) => value,
                            // Not serializable (a DOM node, a cycle): its description.
                            _ => evaluated,
                        }
                    }
                    _ => evaluated,
                }
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
}
