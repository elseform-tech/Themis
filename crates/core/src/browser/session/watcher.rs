//! Per-tab protocol events: notes, loading state, popups and dialogs.

use super::*;

pub(super) fn spawn_watcher(
    cdp: Arc<Cdp>,
    target_id: String,
    session_id: String,
    state: Arc<Mutex<TabState>>,
    chat: std::sync::Weak<ChatBrowser>,
) -> tokio::task::JoinHandle<()> {
    let mut events = cdp.subscribe();
    tokio::spawn(async move {
        // When the next frame may be acknowledged (see FRAME_GAP).
        let mut next_ack = std::time::Instant::now();
        // Windows this page opened that were already loaded here instead.
        let mut redirected: HashSet<String> = HashSet::new();
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    // A dropped frame is never acknowledged, and Chrome sends
                    // no more until it is: restart this tab's stream.
                    tracing_lagged(missed);
                    if let Some(chat) = chat.upgrade() {
                        let streaming = chat
                            .screencast
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .as_deref()
                            == Some(session_id.as_str());
                        if streaming {
                            STREAM_RESTARTS_AFTER_LAG.fetch_add(1, Ordering::Relaxed);
                            tokio::spawn(async move { chat.restart_screencast().await });
                        }
                    }
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };
            if event.session.as_deref() != Some(session_id.as_str()) {
                if redirect_popup(
                    &cdp,
                    &target_id,
                    &session_id,
                    &state,
                    &event,
                    &mut redirected,
                ) {
                    continue;
                }
                if event.method == "Target.targetInfoChanged"
                    && event.params["targetInfo"]["targetId"] == target_id.as_str()
                {
                    {
                        let mut tab = state.lock().unwrap_or_else(|e| e.into_inner());
                        let info = &event.params["targetInfo"];
                        let url = info["url"].as_str().unwrap_or(&tab.url).to_owned();
                        let title = info["title"].as_str().unwrap_or_default();
                        // Until a page has a title, Chrome reports its address without the
                        // scheme; that never replaces the page's real title (read on load).
                        let placeholder =
                            url.split_once("://").map(|(_, rest)| rest) == Some(title);
                        if url != tab.url || !placeholder {
                            tab.title = cap(title, 200);
                        }
                        tab.url = url;
                    }
                    if let Some(chat) = chat.upgrade() {
                        chat.publish_state().await;
                    }
                }
                continue;
            }
            handle_tab_event(&cdp, &target_id, &session_id, &state, &event);
            let committed = event.method == "Page.frameNavigated"
                && event.params["frame"]["parentId"].is_null();
            if committed || event.method == "Page.loadEventFired" {
                // A navigation to another site moves the page to a new renderer, which ends
                // its screencast: start it again once the new page is committed. Chrome
                // reports the title only when the address commits (the address itself
                // then), so read the page's own title at commit (a page restored by
                // Back/Forward has it already and fires no load) and again on load.
                if let Some(chat) = chat.upgrade() {
                    let restart = committed
                        && chat
                            .screencast
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .as_deref()
                            == Some(session_id.as_str());
                    let (cdp, session_id, state) =
                        (Arc::clone(&cdp), session_id.clone(), Arc::clone(&state));
                    tokio::spawn(async move {
                        let (document, frames) = {
                            let tab = state.lock().unwrap_or_else(|e| e.into_inner());
                            (tab.document, tab.frames)
                        };
                        if restart {
                            chat.restart_screencast().await;
                        }
                        refresh_title(&cdp, &session_id, &state).await;
                        // After the restart, so the page (and the end of loading) reaches
                        // the view with its stream.
                        chat.publish_state().await;
                        if committed {
                            settle_view(&chat, &cdp, &session_id, &state, document, frames).await;
                        }
                    });
                }
            }
            if event.method == "Debugger.paused" {
                devtools::resume_later(Arc::clone(&cdp), session_id.clone(), Arc::clone(&state));
            }
            if event.method == "Page.screencastFrame" {
                // Chrome sends the next frame only after this acknowledgement;
                // spacing them caps the stream without skipping a still page's
                // only frame.
                FRAMES.fetch_add(1, Ordering::Relaxed);
                state.lock().unwrap_or_else(|e| e.into_inner()).frames += 1;
                let ack = json!({"sessionId": event.params["sessionId"]});
                let now = std::time::Instant::now();
                let wait = next_ack.saturating_duration_since(now);
                next_ack = now.max(next_ack) + FRAME_GAP;
                if wait.is_zero() {
                    cdp.send(Some(&session_id), "Page.screencastFrameAck", ack);
                } else {
                    let (cdp, session_id) = (Arc::clone(&cdp), session_id.clone());
                    tokio::spawn(async move {
                        tokio::time::sleep(wait).await;
                        cdp.send(Some(&session_id), "Page.screencastFrameAck", ack);
                    });
                }
                if let Some(chat) = chat.upgrade() {
                    let streaming = chat
                        .screencast
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .as_deref()
                        == Some(session_id.as_str());
                    let data = event.params["data"].as_str().unwrap_or_default();
                    if streaming {
                        chat.record_frame(data);
                    }
                    if streaming && chat.ui_stream.load(Ordering::SeqCst) {
                        let _ = chat.updates.send(BrowserUpdate::Frame {
                            data: data.to_owned(),
                            width: event.params["metadata"]["deviceWidth"]
                                .as_f64()
                                .unwrap_or(0.0) as u32,
                            height: event.params["metadata"]["deviceHeight"]
                                .as_f64()
                                .unwrap_or(0.0) as u32,
                        });
                    }
                }
            } else if matches!(
                event.method.as_str(),
                "Page.frameNavigated"
                    | "Page.loadEventFired"
                    | "Page.navigatedWithinDocument"
                    | "Runtime.exceptionThrown"
                    | "Page.frameStartedLoading"
                    | "Page.frameStoppedLoading"
            ) {
                if let Some(chat) = chat.upgrade() {
                    chat.publish_state().await;
                }
            }
        }
    })
}

/// A chat has one page: a window this page opens (a `target="_blank"` link, `window.open`)
/// is closed and its address loaded in the page instead. A window that never gets an
/// address (one a script writes into) is closed after [`POPUP_WAIT`]. Returns whether
/// `event` was about such a window.
pub(super) fn redirect_popup(
    cdp: &Arc<Cdp>,
    target_id: &str,
    session_id: &str,
    state: &Mutex<TabState>,
    event: &CdpEvent,
    redirected: &mut HashSet<String>,
) -> bool {
    let info = &event.params["targetInfo"];
    if event.method == "Target.targetDestroyed" {
        let gone = event.params["targetId"].as_str().unwrap_or_default();
        return redirected.remove(gone);
    }
    if !matches!(
        event.method.as_str(),
        "Target.targetCreated" | "Target.targetInfoChanged"
    ) || info["type"] != "page"
        || info["openerId"].as_str() != Some(target_id)
    {
        return false;
    }
    let Some(popup) = info["targetId"].as_str().map(str::to_owned) else {
        return false;
    };
    if redirected.contains(&popup) {
        return true;
    }
    let url = info["url"].as_str().unwrap_or_default();
    if url.is_empty() || url == "about:blank" {
        if event.method == "Target.targetCreated" {
            let cdp = Arc::clone(cdp);
            tokio::spawn(async move {
                tokio::time::sleep(POPUP_WAIT).await;
                cdp.send(None, "Target.closeTarget", json!({"targetId": popup}));
            });
        }
        return true;
    }
    cdp.send(Some(session_id), "Page.navigate", json!({"url": url}));
    cdp.send(None, "Target.closeTarget", json!({"targetId": popup}));
    push_note(
        state,
        format!(
            "the page tried to open a new window; {} loaded in this page instead",
            short_url(url)
        ),
    );
    redirected.insert(popup);
    true
}

/// Applies one event of a tab to its state (and answers dialogs).
pub(super) fn handle_tab_event(
    cdp: &Cdp,
    target_id: &str,
    session_id: &str,
    state: &Mutex<TabState>,
    event: &CdpEvent,
) {
    let params = &event.params;
    let main_frame = |frame: &Value| frame.as_str() == Some(target_id);
    devtools::record(state, event);
    match event.method.as_str() {
        "Page.frameStartedLoading" if main_frame(&params["frameId"]) => {
            state.lock().unwrap_or_else(|e| e.into_inner()).loading = true;
        }
        "Page.frameStoppedLoading" if main_frame(&params["frameId"]) => {
            state.lock().unwrap_or_else(|e| e.into_inner()).loading = false;
        }
        "Page.loadEventFired" => {
            state.lock().unwrap_or_else(|e| e.into_inner()).loading = false;
        }
        "Page.frameNavigated" if params["frame"]["parentId"].is_null() => {
            let mut tab = state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(url) = params["frame"]["url"].as_str() {
                tab.url = url.to_owned();
            }
            // A new document: the old page's title no longer applies (read again on load).
            tab.title.clear();
            tab.document += 1;
            tab.document_seq = tab.log_seq;
            tab.console_errors = 0;
        }
        "Page.navigatedWithinDocument" if main_frame(&params["frameId"]) => {
            if let Some(url) = params["url"].as_str() {
                state.lock().unwrap_or_else(|e| e.into_inner()).url = url.to_owned();
            }
        }
        "Network.requestWillBeSent" => {
            if let Some(id) = params["requestId"].as_str() {
                let mut tab = state.lock().unwrap_or_else(|e| e.into_inner());
                tab.inflight.insert(id.to_owned(), Instant::now());
                tab.last_network = Some(Instant::now());
            }
        }
        "Network.loadingFinished" | "Network.loadingFailed" => {
            if let Some(id) = params["requestId"].as_str() {
                let mut tab = state.lock().unwrap_or_else(|e| e.into_inner());
                tab.inflight.remove(id);
                tab.last_network = Some(Instant::now());
            }
            if event.method == "Network.loadingFailed"
                && !params["canceled"].as_bool().unwrap_or(false)
                && matches!(params["type"].as_str(), Some("Document" | "XHR" | "Fetch"))
            {
                push_note(
                    state,
                    format!(
                        "request failed: {}",
                        params["errorText"].as_str().unwrap_or("network error")
                    ),
                );
            }
        }
        "Network.responseReceived" => {
            let status = params["response"]["status"].as_u64().unwrap_or(0);
            if status >= 400
                && matches!(params["type"].as_str(), Some("Document" | "XHR" | "Fetch"))
            {
                push_note(
                    state,
                    format!(
                        "HTTP {status} for {}",
                        short_url(params["response"]["url"].as_str().unwrap_or_default())
                    ),
                );
            }
        }
        "Runtime.exceptionThrown" => {
            let details = &params["exceptionDetails"];
            let text = details["exception"]["description"]
                .as_str()
                .or_else(|| details["text"].as_str())
                .unwrap_or("error");
            let first_line = text.lines().next().unwrap_or(text);
            state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .console_errors += 1;
            push_note(state, format!("uncaught {first_line}"));
        }
        "Runtime.consoleAPICalled" if params["type"] == "error" => {
            let text: Vec<String> = params["args"]
                .as_array()
                .into_iter()
                .flatten()
                .take(4)
                .map(|arg| {
                    arg["value"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| arg["description"].as_str().map(str::to_owned))
                        .unwrap_or_else(|| arg["value"].to_string())
                })
                .collect();
            state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .console_errors += 1;
            push_note(state, format!("console.error: {}", text.join(" ")));
        }
        "Page.javascriptDialogOpening" => {
            let kind = params["type"].as_str().unwrap_or("alert");
            // Alerts and leave-page prompts are accepted; confirm and prompt are
            // dismissed, so a page cannot get a "yes" the agent never gave.
            let accept = matches!(kind, "alert" | "beforeunload");
            cdp.send(
                Some(session_id),
                "Page.handleJavaScriptDialog",
                json!({"accept": accept}),
            );
            push_note(
                state,
                format!(
                    "{kind} dialog \"{}\" was {}",
                    cap(params["message"].as_str().unwrap_or_default(), 150),
                    if accept { "accepted" } else { "dismissed" }
                ),
            );
        }
        _ => {}
    }
}
