//! The agent's page actions: navigation, snapshots, pointer and keyboard.

use super::*;

impl ChatBrowser {
    /// Loads `url` in the chat's page.
    pub async fn open(self: &Arc<Self>, url: &str) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        let (cdp, session) = self.current(&mut inner).await?;
        let navigated = cdp
            .call_with_timeout(
                Some(&session),
                "Page.navigate",
                json!({"url": url}),
                NAVIGATION_WAIT,
            )
            .await?;
        let mut note = None;
        if let Some(error) = navigated["errorText"].as_str().filter(|e| !e.is_empty()) {
            note = Some(format!("could not open {}: {error}", short_url(url)));
        }
        self.settle(&inner, true).await;
        let mut report = self.report(&mut inner, None).await?;
        if let Some(note) = note {
            report.new_since_last_action.insert(0, note);
        }
        drop(inner);
        self.publish_state().await;
        self.restart_screencast().await;
        Ok(report)
    }

    /// The person typed an address in the Workspace view.
    pub async fn open_for_user(self: &Arc<Self>, url: &str) -> Result<(), String> {
        self.person_acted();
        // With a page open, go there at once, even while an agent action runs; the
        // stream follows when the new page commits.
        if let Some((cdp, session)) = self.view().current() {
            cdp.call(Some(&session), "Page.navigate", json!({"url": url}))
                .await?;
            self.publish_state().await;
            return Ok(());
        }
        let mut inner = self.inner.lock().await;
        let (cdp, session) = self.current(&mut inner).await?;
        cdp.call(Some(&session), "Page.navigate", json!({"url": url}))
            .await?;
        drop(inner);
        self.publish_state().await;
        self.restart_screencast().await;
        Ok(())
    }

    /// Back, forward or reload.
    pub async fn navigate(self: &Arc<Self>, action: &str) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        if !self.has_page(&inner) {
            return Err("no page is open; use browser_open first".into());
        }
        self.reopen_if_closed(&mut inner).await?;
        let (cdp, session) = self.current(&mut inner).await?;
        history_step(&cdp, &session, action).await?;
        self.settle(&inner, true).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report
    }

    /// Back/forward/reload for the person's toolbar.
    pub async fn navigate_for_user(&self, action: &str) -> Result<(), String> {
        self.thaw_pages().await;
        self.person_acted();
        let Some((cdp, session)) = self.view().current() else {
            return Ok(());
        };
        history_step(&cdp, &session, action).await
    }

    /// A fresh snapshot (of one element's subtree with `reference`).
    pub async fn snapshot(
        self: &Arc<Self>,
        reference: Option<&str>,
        layout: bool,
    ) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        if !self.has_page(&inner) {
            return Err("no page is open; use browser_open first".into());
        }
        self.reopen_if_closed(&mut inner).await?;
        let report = self.report(&mut inner, reference).await?;
        if !layout {
            return Ok(report);
        }
        let browser = inner.browser.clone().ok_or("no page is open")?;
        let session = inner
            .tab
            .as_ref()
            .ok_or("no page is open")?
            .session_id
            .clone();
        let read = browser
            .cdp
            .call(
                Some(&session),
                "Runtime.evaluate",
                json!({"expression": include_str!("../layout.js"), "returnByValue": true}),
            )
            .await?;
        Ok(ActionReport {
            layout: read["result"]["value"].as_str().map(str::to_owned),
            ..report
        })
    }

    /// The accessible name of a ref from the last snapshot, for labels.
    pub async fn name_of(&self, reference: &str) -> Option<String> {
        let inner = self.inner.lock().await;
        let tab = inner.tab.as_ref()?;
        let backend = tab.refs.resolve(reference)?;
        tab.names.get(&backend).cloned()
    }

    pub(super) async fn element(
        &self,
        inner: &ChatInner,
        reference: &str,
    ) -> Result<(Arc<Cdp>, String, i64), String> {
        self.thaw_pages().await;
        let browser = inner
            .browser
            .as_ref()
            .ok_or("no page is open; use browser_open first")?;
        let tab = inner
            .tab
            .as_ref()
            .ok_or("no page is open; use browser_open first")?;
        if !is_ref(reference) {
            let backend = selected_element(&browser.cdp, &tab.session_id, reference).await?;
            return Ok((Arc::clone(&browser.cdp), tab.session_id.clone(), backend));
        }
        let backend = tab.refs.resolve(reference).ok_or_else(|| {
            format!("no element {reference} on this page; take a new browser_snapshot and use a ref from it")
        })?;
        Ok((Arc::clone(&browser.cdp), tab.session_id.clone(), backend))
    }

    /// Clicks an element (twice with `double`).
    pub async fn click(
        self: &Arc<Self>,
        reference: &str,
        double: bool,
    ) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        let (cdp, session, backend) = self.element(&inner, reference).await?;
        let (x, y) = element_center(&cdp, &session, backend).await?;
        self.glide(&cdp, &session, (x, y)).await?;
        press_at(&cdp, &session, x, y, if double { 2 } else { 1 }).await?;
        self.settle(&inner, false).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report
    }

    /// Clicks a point of the visible page in CSS pixels, as a viewport
    /// screenshot shows it: for canvas, maps and games, which have no refs.
    pub async fn click_point(
        self: &Arc<Self>,
        x: f64,
        y: f64,
        double: bool,
    ) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        let browser = inner
            .browser
            .clone()
            .ok_or("no page is open; use browser_open first")?;
        let session = inner
            .tab
            .as_ref()
            .ok_or("no page is open; use browser_open first")?
            .session_id
            .clone();
        check_visible_point(&browser.cdp, &session, x, y).await?;
        let clicks = if double { 2 } else { 1 };
        // A locked pointer has no position to go to: the click must not turn the view.
        let held = *self.pointer.lock().unwrap_or_else(|e| e.into_inner());
        match held {
            Some((held_x, held_y)) if pointer_locked(&browser.cdp, &session).await => {
                press_at(&browser.cdp, &session, held_x, held_y, clicks).await?;
            }
            _ => {
                self.glide(&browser.cdp, &session, (x, y)).await?;
                press_at(&browser.cdp, &session, x, y, clicks).await?;
            }
        }
        self.settle(&inner, false).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report
    }

    /// Presses the mouse at one place, moves to another and lets go, as a person drags. The page
    /// sees real input, whether it listens to mouse events or uses native drag and drop.
    pub async fn drag(
        self: &Arc<Self>,
        from: &DragEnd,
        to: &DragEnd,
    ) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        let browser = inner
            .browser
            .clone()
            .ok_or("no page is open; use browser_open first")?;
        let session = inner
            .tab
            .as_ref()
            .ok_or("no page is open; use browser_open first")?
            .session_id
            .clone();
        // The far end first: reaching it may scroll, and the near end is read where it then is.
        let end = self.drag_point(&inner, &browser.cdp, &session, to).await?;
        let start = self
            .drag_point(&inner, &browser.cdp, &session, from)
            .await?;
        let pressed = under_point(&browser.cdp, &session, start).await;
        let target = under_point(&browser.cdp, &session, end).await;
        let native = drag_between(&browser.cdp, &session, start, end).await?;
        *self.pointer.lock().unwrap_or_else(|e| e.into_inner()) = Some(end);
        self.settle(&inner, false).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report.map(|report| ActionReport {
            drag: Some(drag_outcome(&pressed, &target, native)),
            ..report
        })
    }

    pub(super) async fn drag_point(
        &self,
        inner: &ChatInner,
        cdp: &Cdp,
        session: &str,
        end: &DragEnd,
    ) -> Result<(f64, f64), String> {
        match end {
            DragEnd::Element(reference) => {
                let (cdp, session, backend) = self.element(inner, reference).await?;
                element_center(&cdp, &session, backend).await
            }
            DragEnd::Point(x, y) => {
                check_visible_point(cdp, session, *x, *y).await?;
                Ok((*x, *y))
            }
        }
    }

    /// Moves the mouse to a point of the visible page without clicking, for aiming in games and
    /// for hover menus. The page sees real movement.
    pub async fn move_point(self: &Arc<Self>, x: f64, y: f64) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        let browser = inner
            .browser
            .clone()
            .ok_or("no page is open; use browser_open first")?;
        let session = inner
            .tab
            .as_ref()
            .ok_or("no page is open; use browser_open first")?
            .session_id
            .clone();
        check_visible_point(&browser.cdp, &session, x, y).await?;
        let holding = self
            .held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        if holding {
            let from = self
                .pointer
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .unwrap_or((x, y));
            self.move_holding(&browser.cdp, &session, from, (x, y))
                .await?;
        } else {
            self.glide(&browser.cdp, &session, (x, y)).await?;
        }
        *self.pointer.lock().unwrap_or_else(|e| e.into_inner()) = Some((x, y));
        self.settle(&inner, false).await;
        let over = under_point(&browser.cdp, &session, (x, y)).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report.map(|report| ActionReport {
            under_mouse: Some(over),
            ..report
        })
    }

    /// Moves with the button held, in steps a page can follow, watching for a native drag.
    pub(super) async fn move_holding(
        &self,
        cdp: &Cdp,
        session: &str,
        from: (f64, f64),
        to: (f64, f64),
    ) -> Result<(), String> {
        for step in 1..=DRAG_STEPS {
            let share = f64::from(step) / f64::from(DRAG_STEPS);
            let (x, y) = (
                from.0 + (to.0 - from.0) * share,
                from.1 + (to.1 - from.1) * share,
            );
            mouse(
                cdp,
                session,
                json!({"type": "mouseMoved", "x": x, "y": y, "button": "left", "buttons": 1}),
            )
            .await?;
            tokio::time::sleep(DRAG_STEP_PAUSE).await;
            let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(held) = held.as_mut() {
                if held.dragging.is_none() {
                    held.dragging = drag_started(&mut held.events, session);
                }
            }
        }
        Ok(())
    }

    /// Holds the button down where the mouse is, for `browser_press` with `mouseDown`.
    pub(super) async fn mouse_down(self: &Arc<Self>) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        let browser = inner
            .browser
            .clone()
            .ok_or("no page is open; use browser_open first")?;
        let session = inner
            .tab
            .as_ref()
            .ok_or("no page is open; use browser_open first")?
            .session_id
            .clone();
        let at = (*self.pointer.lock().unwrap_or_else(|e| e.into_inner()))
            .ok_or("the mouse isn't anywhere yet: put it there with browser_move first")?;
        let pressed = under_point(&browser.cdp, &session, at).await;
        let events = browser.cdp.subscribe();
        browser
            .cdp
            .call(
                Some(&session),
                "Input.setInterceptDrags",
                json!({"enabled": true}),
            )
            .await?;
        mouse(&browser.cdp, &session, json!({"type": "mousePressed", "x": at.0, "y": at.1, "button": "left", "buttons": 1, "clickCount": 1})).await?;
        *self.held.lock().unwrap_or_else(|e| e.into_inner()) = Some(HeldMouse {
            events,
            dragging: None,
            pressed: pressed.clone(),
        });
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report.map(|report| ActionReport {
            under_mouse: Some(pressed),
            ..report
        })
    }

    /// Lets go of the button, for `browser_press` with `mouseUp`.
    pub(super) async fn mouse_up(self: &Arc<Self>) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        let browser = inner
            .browser
            .clone()
            .ok_or("no page is open; use browser_open first")?;
        let session = inner
            .tab
            .as_ref()
            .ok_or("no page is open; use browser_open first")?
            .session_id
            .clone();
        let held = self.held.lock().unwrap_or_else(|e| e.into_inner()).take();
        let Some(mut held) = held else {
            return Err("nothing is held: press mouseDown first".to_owned());
        };
        let at = (*self.pointer.lock().unwrap_or_else(|e| e.into_inner())).unwrap_or((0.0, 0.0));
        if held.dragging.is_none() {
            tokio::time::sleep(DRAG_START_WAIT).await;
            held.dragging = drag_started(&mut held.events, &session);
        }
        let target = under_point(&browser.cdp, &session, at).await;
        let native = held.dragging.is_some();
        let released = finish_drag(&browser.cdp, &session, held.dragging.take(), at).await;
        let _ = browser
            .cdp
            .call(
                Some(&session),
                "Input.setInterceptDrags",
                json!({"enabled": false}),
            )
            .await;
        released?;
        self.settle(&inner, false).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report.map(|report| ActionReport {
            drag: Some(drag_outcome(&held.pressed, &target, native)),
            ..report
        })
    }

    /// Moves the mouse by a distance from where it is. While the page holds the pointer lock
    /// the distance has no limit, as with a real locked mouse: past the page's edge it starts
    /// from a point outside the page, which sends no event, so the page sees just this
    /// movement. A page without the lock has an edge, and a move past it is refused.
    pub async fn move_by(self: &Arc<Self>, dx: f64, dy: f64) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        let browser = inner
            .browser
            .clone()
            .ok_or("no page is open; use browser_open first")?;
        let session = inner
            .tab
            .as_ref()
            .ok_or("no page is open; use browser_open first")?
            .session_id
            .clone();
        let (width, height) = visible_size(&browser.cdp, &session).await?;
        let from = self
            .pointer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unwrap_or((width / 2.0, height / 2.0));
        let path = relative_path(from, (dx, dy), (width, height))?;
        if path.start.is_some() && !pointer_locked(&browser.cdp, &session).await {
            return Err(format!(
                "moving by ({dx}, {dy}) from ({}, {}) leaves the visible page ({}×{} CSS pixels), and only a page holding the pointer lock can be moved past its edge",
                from.0.round(), from.1.round(), width.round(), height.round()
            ));
        }
        for (x, y) in path.start.into_iter().chain([path.end]) {
            browser
                .cdp
                .call(
                    Some(&session),
                    "Input.dispatchMouseEvent",
                    json!({"type": "mouseMoved", "x": x, "y": y}),
                )
                .await?;
        }
        *self.pointer.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.end);
        self.settle(&inner, false).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report
    }

    /// Types into an element, optionally clearing it first and pressing Enter after.
    pub async fn type_text(
        self: &Arc<Self>,
        reference: &str,
        text: &str,
        submit: bool,
        clear: bool,
    ) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        let (cdp, session, backend) = self.element(&inner, reference).await?;
        let object = resolve_object(&cdp, &session, backend).await?;
        cdp.call(
            Some(&session),
            "DOM.focus",
            json!({"backendNodeId": backend}),
        )
        .await?;
        if clear {
            cdp.call(
                Some(&session),
                "Runtime.callFunctionOn",
                json!({"objectId": object, "functionDeclaration": SELECT_ALL_JS, "awaitPromise": false}),
            )
            .await?;
            press_key(&cdp, &session, &parse_key("Backspace").expect("known key")).await?;
        }
        cdp.call(Some(&session), "Input.insertText", json!({"text": text}))
            .await?;
        if submit {
            press_key(&cdp, &session, &parse_key("Enter").expect("known key")).await?;
        }
        self.settle(&inner, submit).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report
    }

    /// Chooses options of a `<select>` by value or label.
    pub async fn select(
        self: &Arc<Self>,
        reference: &str,
        values: &[String],
    ) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        let (cdp, session, backend) = self.element(&inner, reference).await?;
        let object = resolve_object(&cdp, &session, backend).await?;
        let result = cdp
            .call(
                Some(&session),
                "Runtime.callFunctionOn",
                json!({"objectId": object, "functionDeclaration": SELECT_JS, "arguments": [{"value": values}], "returnByValue": true}),
            )
            .await?;
        if let Some(error) = result["exceptionDetails"]["exception"]["description"].as_str() {
            return Err(format!(
                "{} is not a dropdown ({}); click it and pick the option instead",
                reference,
                cap(error.lines().next().unwrap_or(error), 100)
            ));
        }
        if result["result"]["value"].as_u64() == Some(0) {
            return Err(format!("no option of {reference} matches {values:?}; check the option labels in a snapshot"));
        }
        self.settle(&inner, false).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report
    }

    /// Presses a key (`Enter`, `Escape`, `Control+a`) in the focused element.
    pub async fn press(
        self: &Arc<Self>,
        key: &str,
        hold: Duration,
    ) -> Result<ActionReport, String> {
        match key
            .to_ascii_lowercase()
            .replace(['_', '-', ' '], "")
            .as_str()
        {
            "mousedown" => return self.mouse_down().await,
            "mouseup" => return self.mouse_up().await,
            _ => {}
        }
        let press = parse_key(key).ok_or_else(|| {
            format!(
                "unknown key '{}'; use names like Enter, Escape, Tab, ArrowDown, w, 1 or Control+a",
                cap(key, 30)
            )
        })?;
        let mut inner = self.inner.lock().await;
        if !self.has_page(&inner) {
            return Err("no page is open; use browser_open first".into());
        }
        self.reopen_if_closed(&mut inner).await?;
        let (cdp, session) = self.current(&mut inner).await?;
        press_key_for(&cdp, &session, &press, hold).await?;
        self.settle(&inner, press.key == "Enter").await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report
    }

    /// Scrolls the page (or the element `reference`) up or down.
    pub async fn scroll(
        self: &Arc<Self>,
        direction: &str,
        reference: Option<&str>,
    ) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        if !self.has_page(&inner) {
            return Err("no page is open; use browser_open first".into());
        }
        self.reopen_if_closed(&mut inner).await?;
        let (cdp, session) = self.current(&mut inner).await?;
        let metrics = cdp
            .call(Some(&session), "Page.getLayoutMetrics", json!({}))
            .await?;
        let width = metrics["cssVisualViewport"]["clientWidth"]
            .as_f64()
            .unwrap_or(1280.0);
        let height = metrics["cssVisualViewport"]["clientHeight"]
            .as_f64()
            .unwrap_or(800.0);
        let (x, y) = match reference {
            Some(reference) => {
                let (_, _, backend) = self.element(&inner, reference).await?;
                element_center(&cdp, &session, backend).await?
            }
            None => (width / 2.0, height / 2.0),
        };
        let step = height * 0.8;
        let (dx, dy) = match direction.trim().to_ascii_lowercase().as_str() {
            "down" => (0.0, step),
            "up" => (0.0, -step),
            "right" => (width * 0.8, 0.0),
            "left" => (-width * 0.8, 0.0),
            other => {
                return Err(format!(
                    "direction must be up, down, left or right, not '{}'",
                    cap(other, 20)
                ))
            }
        };
        cdp.call(
            Some(&session),
            "Input.dispatchMouseEvent",
            json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy}),
        )
        .await?;
        self.settle(&inner, false).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report
    }
}
