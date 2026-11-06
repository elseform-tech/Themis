//! Opening, reusing and closing the chat's page and the shared browser.

use super::*;

impl ChatBrowser {
    pub(super) async fn browser(
        &self,
        inner: &mut ChatInner,
    ) -> Result<Arc<SharedBrowser>, String> {
        if let Some(browser) = &inner.browser {
            if !browser.cdp.is_closed() {
                return Ok(Arc::clone(browser));
            }
            // The browser exited: its page is gone too.
            inner.tab = None;
            self.targets
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
        }
        let hub = self.hub.upgrade().ok_or("the app is closing")?;
        let browser = hub.shared_for(&self.profile_key).await?;
        inner.browser = Some(Arc::clone(&browser));
        self.remember(inner);
        Ok(browser)
    }

    pub(super) async fn open_page(self: &Arc<Self>, inner: &mut ChatInner) -> Result<(), String> {
        if let Some(hub) = self.hub.upgrade() {
            hub.make_room(self).await;
        }
        let browser = self.browser(inner).await?;
        let cdp = &browser.cdp;
        let created = cdp
            .call(None, "Target.createTarget", json!({"url": "about:blank"}))
            .await?;
        let target_id = created["targetId"].as_str().ok_or("no tab id")?.to_owned();
        let attached = cdp
            .call(
                None,
                "Target.attachToTarget",
                json!({"targetId": target_id, "flatten": true}),
            )
            .await?;
        let session_id = attached["sessionId"]
            .as_str()
            .ok_or("no session id")?
            .to_owned();
        let state = Arc::new(Mutex::new(TabState {
            url: "about:blank".into(),
            ..Default::default()
        }));
        let watcher = spawn_watcher(
            Arc::clone(cdp),
            target_id.clone(),
            session_id.clone(),
            Arc::clone(&state),
            Arc::downgrade(self),
        );
        let _ = cdp
            .call(None, "Target.setDiscoverTargets", json!({"discover": true}))
            .await;
        for method in [
            "Page.enable",
            "Network.enable",
            "Runtime.enable",
            "DOM.enable",
            "Accessibility.enable",
            "Log.enable",
        ] {
            cdp.call(Some(&session_id), method, json!({})).await?;
        }
        let viewport = *self.viewport.lock().unwrap_or_else(|e| e.into_inner());
        let _ = cdp
            .call(
                Some(&session_id),
                "Emulation.setDeviceMetricsOverride",
                json!({"width": viewport.width, "height": viewport.height, "deviceScaleFactor": viewport.scale, "mobile": false}),
            )
            .await;
        let _ = cdp
            .call(
                Some(&session_id),
                "Page.addScriptToEvaluateOnNewDocument",
                json!({"source": include_str!("../no_bounce.js")}),
            )
            .await;
        // The browser runs off screen (or headless) and can't lock a real pointer, so a
        // stand-in does it for every page: the agent's and the person's moves turn the view.
        // A page that refuses it just has no pointer lock.
        let _ = cdp
            .call(
                Some(&session_id),
                "Page.addScriptToEvaluateOnNewDocument",
                json!({"source": include_str!("../pointer_lock.js")}),
            )
            .await;
        // A tab that cannot take the sound setting is not opened.
        let audio_script =
            Some(install_audio(cdp, &session_id, self.muted.load(Ordering::SeqCst)).await?);
        self.targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(target_id.clone());
        inner.tab = Some(Tab {
            target_id,
            session_id,
            state,
            refs: RefMap::default(),
            names: HashMap::new(),
            seen_document: 0,
            watcher,
            audio_script,
        });
        self.remember(inner);
        Ok(())
    }

    /// Ensures a current tab exists; returns the browser connection and its session.
    pub(super) async fn current(
        self: &Arc<Self>,
        inner: &mut ChatInner,
    ) -> Result<(Arc<Cdp>, String), String> {
        self.thaw_pages().await;
        self.touch();
        let browser = self.browser(inner).await?;
        if inner.tab.is_none() {
            self.open_page(inner).await?;
            let reopen = self.reopen.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(url) = reopen {
                let tab = inner.tab.as_ref().ok_or("no page is open")?;
                browser
                    .cdp
                    .call_with_timeout(
                        Some(&tab.session_id),
                        "Page.navigate",
                        json!({"url": url}),
                        NAVIGATION_WAIT,
                    )
                    .await?;
                self.settle(inner, true).await;
                if let Some(tab) = &inner.tab {
                    push_note(
                        &tab.state,
                        format!(
                            "this page was closed while unused to save memory and has been reopened at {}; take a new snapshot before using old refs",
                            short_url(&url)
                        ),
                    );
                }
            }
        }
        let tab = inner.tab.as_ref().ok_or("no page is open")?;
        Ok((Arc::clone(&browser.cdp), tab.session_id.clone()))
    }

    /// Reopens the page closed to make room, so an action finds it where it was.
    pub(super) async fn reopen_if_closed(
        self: &Arc<Self>,
        inner: &mut ChatInner,
    ) -> Result<(), String> {
        if inner.tab.is_none() {
            self.current(inner).await?;
        }
        Ok(())
    }

    /// Whether the chat has a page, open now or closed to make room (reopened on use).
    pub(super) fn has_page(&self, inner: &ChatInner) -> bool {
        inner.tab.is_some()
            || self
                .reopen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some()
    }

    pub(super) fn touch(&self) {
        *self.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    /// No page open (closed to make room, or never opened) and nothing running in it.
    pub(super) fn page_closed_and_quiet(&self) -> bool {
        self.view
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tab
            .is_none()
            && !self.ui_stream.load(Ordering::SeqCst)
            && self
                .activity
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_none()
            && self
                .recording
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_none()
    }

    /// Whether the page may be closed to make room: nobody watches, records or acts in it.
    pub(super) fn idle_page(&self) -> bool {
        !self.ui_stream.load(Ordering::SeqCst)
            && self
                .activity
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_none()
            && self
                .recording
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_none()
            && self
                .view
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .tab
                .is_some()
    }

    /// Closes the page to make room, remembering its address. False when an action holds it.
    pub(super) async fn close_for_room(&self) -> bool {
        let Ok(mut inner) = self.inner.try_lock() else {
            return false;
        };
        let Some(tab) = inner.tab.take() else {
            return false;
        };
        let url = tab
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .url
            .clone();
        if !url.is_empty() && url != "about:blank" {
            *self.reopen.lock().unwrap_or_else(|e| e.into_inner()) = Some(url);
        }
        self.targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&tab.target_id);
        if let Some(browser) = &inner.browser {
            let _ = browser
                .cdp
                .call(
                    None,
                    "Target.closeTarget",
                    json!({"targetId": tab.target_id}),
                )
                .await;
        }
        self.remember(&inner);
        drop(inner);
        self.publish_state().await;
        true
    }

    /// Lets go of the browser: closes the page (its address kept to reopen) and drops the
    /// chat's handle, so the process can exit. False when the chat is busy right now.
    pub(super) async fn release_browser(&self) -> bool {
        if self.view().tab.is_some() && !self.close_for_room().await {
            return false;
        }
        let Ok(mut inner) = self.inner.try_lock() else {
            return false;
        };
        inner.browser = None;
        self.remember(&inner);
        drop(inner);
        self.publish_state().await;
        true
    }

    pub(super) async fn close_all(&self) {
        self.stop_screencast().await;
        let mut inner = self.inner.lock().await;
        if let Some(browser) = &inner.browser {
            if let Some(tab) = &inner.tab {
                let _ = browser
                    .cdp
                    .call(
                        None,
                        "Target.closeTarget",
                        json!({"targetId": tab.target_id}),
                    )
                    .await;
            }
        }
        inner.tab = None;
        self.targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        inner.browser = None;
        self.remember(&inner);
    }

    /// Waits until the page has settled: loaded (after a navigation) and the
    /// network quiet, bounded by [`NAVIGATION_WAIT`] / [`SETTLE_WAIT`].
    pub(super) async fn settle(&self, inner: &ChatInner, navigation: bool) {
        let Some(tab) = inner.tab.as_ref() else {
            return;
        };
        let started = Instant::now();
        let limit = if navigation {
            NAVIGATION_WAIT
        } else {
            SETTLE_WAIT
        };
        tokio::time::sleep(Duration::from_millis(150)).await;
        let mut loaded_at: Option<Instant> = None;
        loop {
            let (loading, busy) = {
                let state = tab.state.lock().unwrap_or_else(|e| e.into_inner());
                let busy = state
                    .inflight
                    .values()
                    .any(|start| start.elapsed() < LONG_REQUEST)
                    || state
                        .last_network
                        .is_some_and(|last| last.elapsed() < QUIET);
                (state.loading, busy)
            };
            if (!loading && !busy) || started.elapsed() > limit {
                break;
            }
            if loading {
                loaded_at = None;
            } else if loaded_at.get_or_insert_with(Instant::now).elapsed() > BUSY_AFTER_LOAD {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Snapshot of the current tab plus notes since the last report.
    pub(super) async fn report(
        &self,
        inner: &mut ChatInner,
        reference: Option<&str>,
    ) -> Result<ActionReport, String> {
        let browser = inner.browser.clone().ok_or("no page is open")?;
        let tab = inner.tab.as_mut().ok_or("no page is open")?;
        let (document, url, title) = {
            let state = tab.state.lock().unwrap_or_else(|e| e.into_inner());
            (state.document, state.url.clone(), state.title.clone())
        };
        tab.forget_refs_of_old_page(document);
        let root = match reference {
            Some(reference) => Some(tab.refs.resolve(reference).ok_or_else(|| {
                format!("no element {reference} on this page; take a new browser_snapshot")
            })?),
            None => None,
        };
        let cdp = &browser.cdp;
        let tree = cdp
            .call(
                Some(&tab.session_id),
                "Accessibility.getFullAXTree",
                json!({}),
            )
            .await?;
        let nodes = parse_ax_nodes(&tree);
        let passwords = password_fields(cdp, &tab.session_id, &nodes).await;
        let Outline {
            text,
            truncated,
            omitted_lines,
        } = render(
            &nodes,
            &mut tab.refs,
            &RenderOptions {
                passwords,
                root,
                limit: 0,
            },
        );
        let text = if root.is_none() {
            draggable_lines(cdp, &tab.session_id, &mut tab.refs)
                .await
                .into_iter()
                .fold(text, |text, line| format!("{text}\n{line}"))
        } else {
            text
        };
        for node in &nodes {
            if let Some(backend) = node.backend_id {
                if !node.name.is_empty() && tab.refs.of(backend).is_some() {
                    tab.names.insert(backend, cap(&node.name, 60));
                }
            }
        }
        // The tab's own title briefly holds the URL while a page loads.
        let title = cdp
            .call(
                Some(&tab.session_id),
                "Runtime.evaluate",
                json!({"expression": "document.title", "returnByValue": true}),
            )
            .await
            .ok()
            .and_then(|value| value["result"]["value"].as_str().map(|t| cap(t, 200)))
            .unwrap_or(title);
        let visible_page = cdp
            .call(Some(&tab.session_id), "Page.getLayoutMetrics", json!({}))
            .await
            .ok()
            .and_then(|metrics| {
                let view = &metrics["cssVisualViewport"];
                Some(format!(
                    "{}x{}",
                    view["clientWidth"].as_f64()?.round(),
                    view["clientHeight"].as_f64()?.round()
                ))
            });
        // Notes are taken last: a console error or failed request the action caused can reach
        // the tab's event task while the snapshot is read, and belongs to this action, not the next.
        let notes = std::mem::take(&mut tab.state.lock().unwrap_or_else(|e| e.into_inner()).notes);
        Ok(ActionReport {
            url,
            title,
            snapshot: if text.is_empty() { "(empty page)".to_owned() } else { text },
            visible_page,
            new_since_last_action: notes,
            under_mouse: None,
            drag: None,
            layout: None,
            snapshot_note: truncated.then(|| {
                format!("{omitted_lines} more lines not shown; use browser_snapshot with a ref to read one part, or browser_scroll")
            }),
        })
    }
}
