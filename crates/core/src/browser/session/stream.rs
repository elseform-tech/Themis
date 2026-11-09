//! The Workspace stream, recordings and screenshots.

use super::*;

impl ChatBrowser {
    /// Streams the current tab to the Workspace view, whose page area is `width` × `height`
    /// CSS pixels at `scale` device pixels each. Frames come at that density. With `resize`
    /// the page is also laid out at the view's size (the person resized the view, or nobody
    /// is using the page); without it the page keeps its layout size, so an agent mid-task
    /// keeps its coordinates, and the view shows it scaled to fit.
    pub async fn start_screencast(
        &self,
        width: u32,
        height: u32,
        scale: f64,
        resize: bool,
    ) -> Result<(), String> {
        self.ui_stream.store(true, Ordering::SeqCst);
        let scale = clamp_scale(scale);
        *self.frame_target.lock().unwrap_or_else(|e| e.into_inner()) = Some(FrameTarget::Panel(
            (f64::from(width) * scale).round() as u32,
            (f64::from(height) * scale).round() as u32,
        ));
        self.thaw_pages().await;
        if resize {
            return self.run_screencast(width, height, scale).await;
        }
        let page = *self.viewport.lock().unwrap_or_else(|e| e.into_inner());
        self.run_screencast(page.width, page.height, scale).await
    }

    /// Streams small frames for the chat's preview without resizing the page:
    /// the agent keeps the viewport it had, only the frames are scaled down.
    pub async fn start_preview(&self, max_width: u32) -> Result<(), String> {
        self.ui_stream.store(true, Ordering::SeqCst);
        let preview = max_width.clamp(160, 1280);
        *self.frame_target.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(FrameTarget::Preview(preview));
        let viewport = *self.viewport.lock().unwrap_or_else(|e| e.into_inner());
        self.run_screencast(viewport.width, viewport.height, viewport.scale)
            .await?;
        // A frozen page draws nothing, so the stream sends no frame: show how it was left,
        // at the preview's size (the page's own size would be several times the bytes).
        if self.frozen.load(Ordering::SeqCst) {
            if let Some((cdp, session)) = self.view().current() {
                let (width, height) =
                    frame_size(viewport, Some(FrameTarget::Preview(preview)), false);
                // A clip is in page coordinates: start it where the page is scrolled to.
                let metrics = cdp
                    .call(Some(&session), "Page.getLayoutMetrics", json!({}))
                    .await
                    .unwrap_or_default();
                let at = |key: &str| metrics["cssVisualViewport"][key].as_f64().unwrap_or(0.0);
                let shot = cdp
                    .call(
                        Some(&session),
                        "Page.captureScreenshot",
                        json!({"format": "jpeg", "quality": 60, "clip": {
                            "x": at("pageX"), "y": at("pageY"),
                            "width": viewport.width, "height": viewport.height,
                            "scale": f64::from(width) / f64::from(viewport.width.max(1)) / viewport.scale,
                        }}),
                    )
                    .await?;
                if let Some(data) = shot["data"].as_str() {
                    let _ = self.updates.send(BrowserUpdate::Frame {
                        data: data.to_owned(),
                        width,
                        height,
                    });
                }
            }
        }
        Ok(())
    }

    /// Starts (or moves) the protocol screencast to the current tab.
    pub(super) async fn run_screencast(
        &self,
        width: u32,
        height: u32,
        scale: f64,
    ) -> Result<(), String> {
        {
            let mut viewport = self.viewport.lock().unwrap_or_else(|e| e.into_inner());
            *viewport = Viewport {
                width: width.clamp(320, 3840),
                height: height.clamp(240, 2400),
                scale: clamp_scale(scale),
            };
        }
        let _one = self.stream_start.lock().await;
        let Some((cdp, session_id)) = self.view().current() else {
            *self.screencast.lock().unwrap_or_else(|e| e.into_inner()) = Some(String::new());
            return Ok(());
        };
        let viewport = *self.viewport.lock().unwrap_or_else(|e| e.into_inner());
        let _ = cdp
            .call(
                Some(&session_id),
                "Emulation.setDeviceMetricsOverride",
                json!({"width": viewport.width, "height": viewport.height, "deviceScaleFactor": viewport.scale, "mobile": false}),
            )
            .await;
        let previous = self
            .screencast
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(session_id.clone());
        // Chrome refuses a second start on a streaming tab, and a restart is
        // what applies a new size, so stop whichever tab is streaming first.
        if let Some(previous) = previous.filter(|p| !p.is_empty()) {
            let _ = cdp
                .call(Some(&previous), "Page.stopScreencast", json!({}))
                .await;
        }
        STREAM_STARTS.fetch_add(1, Ordering::Relaxed);
        let recording = self
            .recording
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        let (max_width, max_height) = frame_size(
            viewport,
            *self.frame_target.lock().unwrap_or_else(|e| e.into_inner()),
            recording,
        );
        cdp.call(
            Some(&session_id),
            "Page.startScreencast",
            json!({"format": "jpeg", "quality": STREAM_QUALITY, "maxWidth": max_width, "maxHeight": max_height, "everyNthFrame": 1}),
        )
        .await?;
        Ok(())
    }

    /// Stops streaming (the Browser tab was closed or hidden). A recording
    /// keeps the protocol screencast running.
    pub async fn stop_screencast(&self) {
        self.ui_stream.store(false, Ordering::SeqCst);
        if self
            .recording
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            return;
        }
        self.end_screencast().await;
        // Nobody is looking; an agent's next action brings the pages back.
        self.freeze_pages().await;
    }

    /// Stops the protocol screencast. In turn with starts (`stream_start`) and never behind
    /// an agent action: a stop that waited for one landed after the chat's preview had
    /// started on the same tab, and ended the preview's stream (or made its start fail).
    pub(super) async fn end_screencast(&self) {
        let _one = self.stream_start.lock().await;
        let session = self
            .screencast
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let (Some(session), Some(cdp)) = (session.filter(|s| !s.is_empty()), self.view().cdp) {
            STREAM_STOPS.fetch_add(1, Ordering::Relaxed);
            let _ = cdp
                .call(Some(&session), "Page.stopScreencast", json!({}))
                .await;
        }
    }

    pub(super) async fn restart_screencast(&self) {
        let streaming = self
            .screencast
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        if streaming {
            let viewport = *self.viewport.lock().unwrap_or_else(|e| e.into_inner());
            let _ = self
                .run_screencast(viewport.width, viewport.height, viewport.scale)
                .await;
        }
    }

    /// Adds a screencast frame (base64 JPEG) to a recording in progress.
    pub(super) fn record_frame(&self, data: &str) {
        use base64::Engine;
        let mut recording = self.recording.lock().unwrap_or_else(|e| e.into_inner());
        let Some(recording) = recording.as_mut() else {
            return;
        };
        if let Ok(jpeg) = base64::engine::general_purpose::STANDARD.decode(data) {
            recording.push(jpeg);
        }
    }

    /// Starts recording the current tab for up to `max`.
    pub async fn start_recording(&self, by: Recorder, max: Duration) -> Result<(), String> {
        {
            let mut recording = self.recording.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(current) = recording.as_ref() {
                let seconds = current.elapsed().as_secs();
                return Err(format!(
                    "already recording since {}:{:02}; stop it first",
                    seconds / 60,
                    seconds % 60
                ));
            }
            if self
                .inner
                .try_lock()
                .is_ok_and(|inner| !self.has_page(&inner))
            {
                return Err(
                    "no page is open to record; use the browser's open action first".into(),
                );
            }
            *recording = Some(Recording::new(by, max));
        }
        let viewport = *self.viewport.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(error) = self
            .run_screencast(viewport.width, viewport.height, viewport.scale)
            .await
        {
            *self.recording.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return Err(error);
        }
        self.publish_state().await;
        Ok(())
    }

    /// Who is recording, if anyone.
    pub fn recorder(&self) -> Option<Recorder> {
        self.recording
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|recording| recording.by)
    }

    /// Stops the recording and returns it as an MP4 with its length. The agent
    /// may stop only its own recordings (`by` is who asks).
    pub async fn stop_recording(&self, by: Recorder) -> Result<(Vec<u8>, Duration), String> {
        let recording = {
            let mut recording = self.recording.lock().unwrap_or_else(|e| e.into_inner());
            match recording.as_ref() {
                None => return Err("nothing is being recorded".into()),
                Some(current) if by == Recorder::Agent && current.by == Recorder::User => {
                    return Err("the user started this recording; only they can stop it".into());
                }
                Some(_) => recording.take().expect("checked above"),
            }
        };
        if self.ui_stream.load(Ordering::SeqCst) {
            // A recording takes frames at the page's full size; the view's are smaller again.
            self.restart_screencast().await;
        } else {
            self.end_screencast().await;
        }
        self.publish_state().await;
        let length = recording.elapsed();
        let frames = recording.frames;
        let css = {
            let viewport = self.viewport.lock().unwrap_or_else(|e| e.into_inner());
            (viewport.width as usize, viewport.height as usize)
        };
        if frames.is_empty() {
            return Err(
                "the page did not paint while recording, so there is nothing to save".into(),
            );
        }
        // Dropping this future (the run was stopped) cancels the encode, which
        // otherwise keeps a core busy to the end.
        struct CancelOnDrop(Arc<AtomicBool>);
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let cancel = CancelOnDrop(Arc::new(AtomicBool::new(false)));
        let cancelled = cancel.0.clone();
        let video = tokio::task::spawn_blocking(move || {
            crate::browser::media::encode_mp4(&frames, length, css, &cancelled)
        })
        .await
        .map_err(|e| e.to_string())??;
        drop(cancel);
        Ok((video, length))
    }

    /// A PNG of the current page (the visible part, the whole page, or one
    /// element) with its size in CSS pixels.
    pub async fn screenshot(
        &self,
        reference: Option<&str>,
        full_page: bool,
    ) -> Result<(Vec<u8>, u32, u32), String> {
        self.capture(reference, full_page, None).await
    }

    /// The same view as [`Self::screenshot`] as a model sees it: a JPEG at most `max_side`
    /// pixels on its longest side, scaled by the browser itself.
    pub async fn screenshot_for_model(
        &self,
        reference: Option<&str>,
        full_page: bool,
        max_side: u32,
    ) -> Result<Vec<u8>, String> {
        Ok(self.capture(reference, full_page, Some(max_side)).await?.0)
    }

    /// A PNG in CSS pixels, or with `fit`, a JPEG scaled down to at most `fit` pixels on its
    /// longest side.
    pub(super) async fn capture(
        &self,
        reference: Option<&str>,
        full_page: bool,
        fit: Option<u32>,
    ) -> Result<(Vec<u8>, u32, u32), String> {
        use base64::Engine;
        let inner = self.inner.lock().await;
        let browser = inner
            .browser
            .clone()
            .ok_or("no page is open; use the browser's open action first")?;
        let tab = inner
            .tab
            .as_ref()
            .ok_or("no page is open; use the browser's open action first")?;
        let cdp = &browser.cdp;
        let session = tab.session_id.clone();
        let metrics = cdp
            .call(Some(&session), "Page.getLayoutMetrics", json!({}))
            .await?;
        let view = &metrics["cssVisualViewport"];
        let (page_x, page_y) = (
            view["pageX"].as_f64().unwrap_or(0.0),
            view["pageY"].as_f64().unwrap_or(0.0),
        );
        let mut params = json!({"format": "png", "fromSurface": true});
        // The Workspace view emulates its screen's device scale; dividing it
        // out of the clip gives the model CSS pixels.
        let scale = 1.0
            / self
                .viewport
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .scale;
        let (width, height) = match reference {
            Some(reference) => {
                // A CSS selector or text=… works too, as for clicks: agents write them for
                // screenshots the same way, and a ref-only error sent them in circles.
                let backend = if is_ref(reference) {
                    tab.refs.resolve(reference).ok_or_else(|| {
                        format!("no element {reference} on this page; take a new snapshot")
                    })?
                } else {
                    selected_element(cdp, &session, reference).await?
                };
                let _ = cdp
                    .call(
                        Some(&session),
                        "DOM.scrollIntoViewIfNeeded",
                        json!({"backendNodeId": backend}),
                    )
                    .await;
                let model = cdp
                    .call(
                        Some(&session),
                        "DOM.getBoxModel",
                        json!({"backendNodeId": backend}),
                    )
                    .await
                    .map_err(|_| "that element is not visible on the page".to_owned())?;
                let quad: Vec<f64> = model["model"]["border"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_f64)
                    .collect();
                if quad.len() != 8 {
                    return Err("that element has no size on the page".into());
                }
                let xs = [quad[0], quad[2], quad[4], quad[6]];
                let ys = [quad[1], quad[3], quad[5], quad[7]];
                let (x, y) = (
                    xs.iter().copied().fold(f64::MAX, f64::min),
                    ys.iter().copied().fold(f64::MAX, f64::min),
                );
                let w = xs.iter().copied().fold(f64::MIN, f64::max) - x;
                let h = ys.iter().copied().fold(f64::MIN, f64::max) - y;
                // After scrolling into view the box is in viewport coordinates.
                let metrics = cdp
                    .call(Some(&session), "Page.getLayoutMetrics", json!({}))
                    .await?;
                let view = &metrics["cssVisualViewport"];
                let (scroll_x, scroll_y) = (
                    view["pageX"].as_f64().unwrap_or(page_x),
                    view["pageY"].as_f64().unwrap_or(page_y),
                );
                params["clip"] = json!({"x": x + scroll_x, "y": y + scroll_y, "width": w.max(1.0), "height": h.max(1.0), "scale": scale});
                params["captureBeyondViewport"] = json!(true);
                (w, h)
            }
            None if full_page => {
                let content = &metrics["cssContentSize"];
                let w = content["width"].as_f64().unwrap_or(1280.0);
                let h = content["height"].as_f64().unwrap_or(800.0).min(16_384.0);
                params["clip"] = json!({"x": 0, "y": 0, "width": w, "height": h, "scale": scale});
                params["captureBeyondViewport"] = json!(true);
                (w, h)
            }
            None => {
                let w = view["clientWidth"].as_f64().unwrap_or(1280.0);
                let h = view["clientHeight"].as_f64().unwrap_or(800.0);
                params["clip"] =
                    json!({"x": page_x, "y": page_y, "width": w, "height": h, "scale": scale});
                (w, h)
            }
        };
        if let Some(fit) = fit {
            let longest = width.max(height).max(1.0);
            let shrink = (f64::from(fit) / longest).min(1.0);
            if let Some(clip_scale) = params["clip"]["scale"].as_f64() {
                params["clip"]["scale"] = json!(clip_scale * shrink);
            }
            params["format"] = json!("jpeg");
            params["quality"] = json!(75);
        }
        let shot = cdp
            .call_with_timeout(
                Some(&session),
                "Page.captureScreenshot",
                params,
                NAVIGATION_WAIT,
            )
            .await?;
        let png = base64::engine::general_purpose::STANDARD
            .decode(shot["data"].as_str().unwrap_or_default())
            .map_err(|_| "the browser returned an unreadable screenshot".to_owned())?;
        Ok((png, width.round() as u32, height.round() as u32))
    }
}
