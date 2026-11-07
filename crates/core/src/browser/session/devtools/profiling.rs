//! What a page keeps and what it costs: storage, performance traces and memory snapshots.

use super::*;

impl ChatBrowser {
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

pub(super) static MEMORY_BASELINE: Mutex<Option<Value>> = Mutex::new(None);

pub(super) struct HeavyJob;

impl HeavyJob {
    pub(super) fn start() -> Result<Self, String> {
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

pub(super) async fn read_stream(target: &Target, handle: &str) -> Result<Vec<u8>, String> {
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
pub(super) fn summarize_trace(bytes: &[u8], seconds: u64) -> Value {
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
pub(super) fn summarize_heap(bytes: &[u8]) -> Value {
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

/// Web vitals and load timing the page can report about itself.
pub(super) const VITALS_SCRIPT: &str = r#"(async()=>{const get=t=>new Promise(r=>{try{new PerformanceObserver((l,o)=>{o.disconnect();r(l.getEntries())}).observe({type:t,buffered:true});setTimeout(()=>r([]),300)}catch(e){r([])}});const [lcp,shifts]=await Promise.all([get('largest-contentful-paint'),get('layout-shift')]);const nav=performance.getEntriesByType('navigation')[0]||{};const res=performance.getEntriesByType('resource');const round=n=>Math.round(n||0);return{url:location.href.slice(0,300),ttfb_ms:round(nav.responseStart),dom_content_loaded_ms:round(nav.domContentLoadedEventEnd),load_ms:round(nav.loadEventEnd),largest_contentful_paint_ms:lcp.length?round(lcp[lcp.length-1].startTime):null,cumulative_layout_shift:Math.round(shifts.filter(s=>!s.hadRecentInput).reduce((a,s)=>a+s.value,0)*1000)/1000,resources:res.length,transfer_bytes:res.reduce((a,r)=>a+(r.transferSize||0),0),largest_resources:res.sort((a,b)=>(b.transferSize||0)-(a.transferSize||0)).slice(0,5).map(r=>({url:r.name.slice(0,200),bytes:r.transferSize||0})),images_without_alt:document.querySelectorAll('img:not([alt])').length}})()"#;
