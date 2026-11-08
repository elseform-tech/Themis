//! Protocol helpers for pointer, keyboard and element work on a page.

use super::*;

pub(super) const SELECT_ALL_JS: &str = "function() { this.focus(); if (typeof this.select === 'function') { this.select(); } else { const range = document.createRange(); range.selectNodeContents(this); const selection = window.getSelection(); selection.removeAllRanges(); selection.addRange(range); } }";

pub(super) const SELECT_JS: &str = "function(values) { if (this.tagName !== 'SELECT') { throw new Error('element is ' + this.tagName.toLowerCase()); } const wanted = new Set(values); let found = 0; for (const option of this.options) { const match = wanted.has(option.value) || wanted.has(option.label) || wanted.has(option.textContent.trim()); option.selected = match && (this.multiple || found === 0); if (match) { found += 1; } } this.dispatchEvent(new Event('input', { bubbles: true })); this.dispatchEvent(new Event('change', { bubbles: true })); return found; }";

pub(super) async fn history_step(cdp: &Cdp, session: &str, action: &str) -> Result<(), String> {
    match action.trim().to_ascii_lowercase().as_str() {
        "reload" => {
            cdp.call(Some(session), "Page.reload", json!({})).await?;
        }
        step @ ("back" | "forward") => {
            let history = cdp
                .call(Some(session), "Page.getNavigationHistory", json!({}))
                .await?;
            let index = history["currentIndex"].as_i64().unwrap_or(0);
            let target = if step == "back" { index - 1 } else { index + 1 };
            let entry = usize::try_from(target)
                .ok()
                .and_then(|target| history["entries"].as_array()?.get(target).cloned())
                .ok_or_else(|| format!("there is no page to go {step} to"))?;
            cdp.call(
                Some(session),
                "Page.navigateToHistoryEntry",
                json!({"entryId": entry["id"]}),
            )
            .await?;
        }
        other => {
            return Err(format!(
                "action must be back, forward or reload, not '{}'",
                cap(other, 20)
            ))
        }
    }
    Ok(())
}

pub(super) async fn resolve_object(
    cdp: &Cdp,
    session: &str,
    backend: i64,
) -> Result<String, String> {
    let resolved = cdp
        .call(
            Some(session),
            "DOM.resolveNode",
            json!({"backendNodeId": backend}),
        )
        .await
        .map_err(|_| "that element is gone from the page; take a new snapshot".to_owned())?;
    resolved["object"]["objectId"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "that element is gone from the page; take a new snapshot".to_owned())
}

/// Scrolls an element into view and returns its center in CSS pixels.
/// Refuses a point outside the visible page, in CSS pixels.
pub(super) async fn visible_size(cdp: &Cdp, session: &str) -> Result<(f64, f64), String> {
    let metrics = cdp
        .call(Some(session), "Page.getLayoutMetrics", json!({}))
        .await?;
    let view = &metrics["cssVisualViewport"];
    Ok((
        view["clientWidth"].as_f64().unwrap_or(0.0),
        view["clientHeight"].as_f64().unwrap_or(0.0),
    ))
}

/// Whether the page holds the pointer lock (see `pointer_lock.js`).
pub(super) async fn pointer_locked(cdp: &Cdp, session: &str) -> bool {
    cdp.call(
        Some(session),
        "Runtime.evaluate",
        json!({"expression": "!!document.pointerLockElement", "returnByValue": true}),
    )
    .await
    .is_ok_and(|reply| reply["result"]["value"].as_bool() == Some(true))
}

/// The mouse events of a relative move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct RelativePath {
    /// A point outside the page to go to first; set when the move would leave the page.
    pub(super) start: Option<(f64, f64)>,
    /// The point on the page the move ends at.
    pub(super) end: (f64, f64),
}

/// Plans a move by `delta` from `from`. Inside the page that is one plain move. Past the edge,
/// `start` lies outside the page, where the browser sends no event but still counts the
/// distance, so the one event the page gets from `start` to `end` reports exactly `delta`.
pub(super) fn relative_path(
    from: (f64, f64),
    delta: (f64, f64),
    size: (f64, f64),
) -> Result<RelativePath, String> {
    // The browser counts whole pixels, so a half anywhere would lose one on every move.
    let (dx, dy) = (delta.0.round(), delta.1.round());
    let (width, height) = (size.0.floor(), size.1.floor());
    if !dx.is_finite() || !dy.is_finite() {
        return Err("dx and dy must be numbers".to_owned());
    }
    if width < 1.0 || height < 1.0 {
        return Err("the page has no visible size yet; take a snapshot".to_owned());
    }
    // A page that shrank may have left the last position outside it.
    let from = (
        from.0.round().min(width - 1.0),
        from.1.round().min(height - 1.0),
    );
    let (x, y) = (from.0 + dx, from.1 + dy);
    if (0.0..width).contains(&x) && (0.0..height).contains(&y) {
        return Ok(RelativePath {
            start: None,
            end: (x, y),
        });
    }
    // The longer side of the move puts the start outside the page; the other side lands
    // anywhere on the page, since a start outside on one side is outside.
    let onto = |value: f64, size: f64| value.clamp(0.0, size - 1.0);
    let end = if dx.abs() >= dy.abs() {
        (entry(dx, width), onto(y, height))
    } else {
        (onto(x, width), entry(dy, height))
    };
    Ok(RelativePath {
        start: Some((end.0 - dx, end.1 - dy)),
        end,
    })
}

/// Where on one side of the page a move of `delta` ends when it began outside the page: as
/// close to the middle as the distance allows.
pub(super) fn entry(delta: f64, size: f64) -> f64 {
    if delta > 0.0 {
        (delta - 1.0).clamp(0.0, (size / 2.0).floor())
    } else {
        (size + delta).max((size / 2.0).floor())
    }
}

pub(super) async fn check_visible_point(
    cdp: &Cdp,
    session: &str,
    x: f64,
    y: f64,
) -> Result<(), String> {
    let (width, height) = visible_size(cdp, session).await?;
    if x.is_finite() && y.is_finite() && (0.0..width).contains(&x) && (0.0..height).contains(&y) {
        return Ok(());
    }
    Err(format!(
        "the point ({x}, {y}) is outside the visible page ({}×{} CSS pixels); take a screenshot",
        width.round(),
        height.round()
    ))
}

pub(super) async fn element_center(
    cdp: &Cdp,
    session: &str,
    backend: i64,
) -> Result<(f64, f64), String> {
    let _ = cdp
        .call(
            Some(session),
            "DOM.scrollIntoViewIfNeeded",
            json!({"backendNodeId": backend}),
        )
        .await;
    let quads = cdp
        .call(
            Some(session),
            "DOM.getContentQuads",
            json!({"backendNodeId": backend}),
        )
        .await
        .map_err(|_| "that element is gone or not visible; take a new snapshot".to_owned())?;
    let quad: Vec<f64> = quads["quads"][0]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_f64)
        .collect();
    if quad.len() != 8 {
        return Err("that element is not visible on the page (no size); scroll or open what contains it first".into());
    }
    let x = (quad[0] + quad[2] + quad[4] + quad[6]) / 4.0;
    let y = (quad[1] + quad[3] + quad[5] + quad[7]) / 4.0;
    Ok((x, y))
}

/// JPEG quality of the Browser tab's stream: lower visibly softens the edges of text.
pub(super) const STREAM_QUALITY: u32 = 92;

/// Draggable elements given a ref in one snapshot.
pub(super) const MAX_DRAGGABLE: usize = 40;

/// What is at a point, in a few words: the agent has no pointer to look at, and a screenshot is
/// no use to a model that can't read images. It is page text, capped, in a result that already
/// says page content is untrusted.
pub(super) async fn under_point(cdp: &Cdp, session: &str, (x, y): (f64, f64)) -> String {
    let expression = format!(
        r#"(function(){{const e=document.elementFromPoint({x},{y});if(!e)return "nothing";const d=e.closest('[draggable="true"]');const t=d||e;const c=typeof t.className==="string"&&t.className.trim()?"."+t.className.trim().split(/\s+/)[0]:"";const n=(t.innerText||"").trim().replace(/\s+/g," ").slice(0,60).replace(/"/g,"'");return t.tagName.toLowerCase()+c+(n?' "'+n+'"':"")+(d?" (draggable)":"");}})()"#
    );
    cdp.call(
        Some(session),
        "Runtime.evaluate",
        json!({"expression": expression, "returnByValue": true}),
    )
    .await
    .ok()
    .and_then(|value| value["result"]["value"].as_str().map(|text| cap(text, 120)))
    .unwrap_or_else(|| "something unreadable".to_owned())
}

/// Lines for draggable elements the outline doesn't already give a ref: a plain `div` has no role,
/// so the accessibility tree leaves it out and only a guessed point could reach it.
pub(super) async fn draggable_lines(cdp: &Cdp, session: &str, refs: &mut RefMap) -> Vec<String> {
    pub(super) const SELECTOR: &str = r#"[draggable="true"]"#;
    let names = cdp
        .call(
            Some(session),
            "Runtime.evaluate",
            json!({"expression": format!(r#"[...document.querySelectorAll('{SELECTOR}')].map(e=>(e.innerText||"").trim().replace(/\s+/g," "))"#), "returnByValue": true}),
        )
        .await;
    let names: Vec<String> = names
        .ok()
        .and_then(|value| serde_json::from_value(value["result"]["value"].clone()).ok())
        .unwrap_or_default();
    if names.is_empty() {
        return Vec::new();
    }
    let Ok(document) = cdp.call(Some(session), "DOM.getDocument", json!({})).await else {
        return Vec::new();
    };
    let Ok(found) = cdp
        .call(
            Some(session),
            "DOM.querySelectorAll",
            json!({"nodeId": document["root"]["nodeId"], "selector": SELECTOR}),
        )
        .await
    else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    let ids = found["nodeIds"].as_array().cloned().unwrap_or_default();
    for (id, name) in ids.iter().zip(&names).take(MAX_DRAGGABLE) {
        let Ok(node) = cdp
            .call(Some(session), "DOM.describeNode", json!({"nodeId": id}))
            .await
        else {
            continue;
        };
        let Some(backend) = node["node"]["backendNodeId"].as_i64() else {
            continue;
        };
        if refs.of(backend).is_none() {
            refs.assign(backend);
            if let Some(reference) = refs.of(backend) {
                lines.push(format!(
                    "- draggable \"{}\" [{reference}]",
                    cap(name, 60).replace('"', "'")
                ));
            }
        }
    }
    lines
}

/// The button held down by hand, between `mouseDown` and `mouseUp`.
pub(super) struct HeldMouse {
    pub(super) events: tokio::sync::broadcast::Receiver<crate::browser::cdp::CdpEvent>,
    /// The native drag Chrome handed over, once one starts.
    pub(super) dragging: Option<Value>,
    /// What the button came down on.
    pub(super) pressed: String,
}

pub(super) fn drag_outcome(pressed: &str, target: &str, native: bool) -> String {
    if native {
        format!("pressed on {pressed}; a native drag started and was dropped on {target}")
    } else {
        format!("pressed on {pressed}; no native drag started, so nothing was dragged unless this page follows the mouse itself; let go over {target}. a snapshot with layout shows where everything is")
    }
}

/// Whether this names a snapshot's ref (`e12`) rather than a selector.
pub(super) fn is_ref(reference: &str) -> bool {
    reference
        .strip_prefix('e')
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// An element by `text=…` or a CSS selector, as agents write them for Playwright. Text finds the
/// innermost element holding it, so a drop lands on what is named and bubbles to its container.
pub(super) async fn selected_element(
    cdp: &Cdp,
    session: &str,
    selector: &str,
) -> Result<i64, String> {
    let missing = || {
        format!(
            "nothing on this page matches {}; take a new snapshot and use a ref from it, a CSS selector or text=...",
            cap(selector, 80)
        )
    };
    let quoted = |text: &str| serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned());
    let expression = match selector.strip_prefix("text=") {
        Some(text) => format!(
            r#"(function(t){{let f=null;for(const e of document.body.querySelectorAll("*")){{if(e.offsetParent!==null&&(e.innerText||"").includes(t))f=e;}}return f;}})({})"#,
            quoted(text.trim().trim_matches('"').trim_matches('\''))
        ),
        None => format!(
            r#"(function(s){{try{{return document.querySelector(s);}}catch(e){{return null;}}}})({})"#,
            quoted(selector.strip_prefix("css=").unwrap_or(selector))
        ),
    };
    let found = cdp
        .call(
            Some(session),
            "Runtime.evaluate",
            json!({"expression": expression}),
        )
        .await?;
    let object = found["result"]["objectId"].as_str().ok_or_else(missing)?;
    let node = cdp
        .call(
            Some(session),
            "DOM.describeNode",
            json!({"objectId": object}),
        )
        .await
        .map_err(|_| missing())?;
    node["node"]["backendNodeId"].as_i64().ok_or_else(missing)
}

/// Finishes a gesture at `at`: the handed-over drag goes in as drag events, then the button lifts.
pub(super) async fn finish_drag(
    cdp: &Cdp,
    session: &str,
    dragging: Option<Value>,
    at: (f64, f64),
) -> Result<(), String> {
    if let Some(data) = dragging {
        for kind in ["dragEnter", "dragOver", "drop"] {
            cdp.call(
                Some(session),
                "Input.dispatchDragEvent",
                json!({"type": kind, "x": at.0, "y": at.1, "data": data}),
            )
            .await?;
        }
    }
    mouse(cdp, session, json!({"type": "mouseReleased", "x": at.0, "y": at.1, "button": "left", "buttons": 0, "clickCount": 1})).await
}

/// One end of a drag.
#[derive(Debug, Clone, PartialEq)]
pub enum DragEnd {
    /// A ref from the latest snapshot.
    Element(String),
    /// CSS pixels of the visible page.
    Point(f64, f64),
}

/// Moves between the two ends, enough for a page to see the distance covered.
pub(super) const DRAG_STEPS: u32 = 12;

/// Pixels per glide step, and the bounds on steps.
pub(super) const GLIDE_STEP_PX: f64 = 40.0;

pub(super) const GLIDE_MIN_STEPS: u32 = 4;

pub(super) const GLIDE_MAX_STEPS: u32 = 12;

/// The points a glide from `from` to `to` passes through, eased in and out, ending exactly at
/// `to`; empty when the pointer is already there.
pub(super) fn glide_path(from: (f64, f64), to: (f64, f64)) -> Vec<(f64, f64)> {
    let distance = (to.0 - from.0).hypot(to.1 - from.1);
    if distance < 2.0 {
        return Vec::new();
    }
    let steps = ((distance / GLIDE_STEP_PX).ceil() as u32).clamp(GLIDE_MIN_STEPS, GLIDE_MAX_STEPS);
    (1..=steps)
        .map(|step| {
            let t = f64::from(step) / f64::from(steps);
            let eased = 0.5 - 0.5 * (std::f64::consts::PI * t).cos();
            (
                from.0 + (to.0 - from.0) * eased,
                from.1 + (to.1 - from.1) * eased,
            )
        })
        .collect()
}

pub(super) const DRAG_STEP_PAUSE: Duration = Duration::from_millis(16);

/// A page starts native drag and drop a moment after the mouse leaves; waits this long at most.
pub(super) const DRAG_START_WAIT: Duration = Duration::from_millis(250);

pub(super) async fn mouse(cdp: &Cdp, session: &str, params: Value) -> Result<(), String> {
    cdp.call(Some(session), "Input.dispatchMouseEvent", params)
        .await
        .map(|_| ())
}

/// Native drag and drop doesn't follow synthetic mouse events by itself: Chrome hands over the
/// drag it would have started, and the rest of the gesture goes in as drag events. A page that
/// listens to mouse events never starts one and simply sees the moves.
pub(super) async fn drag_between(
    cdp: &Cdp,
    session: &str,
    from: (f64, f64),
    to: (f64, f64),
) -> Result<bool, String> {
    let mut events = cdp.subscribe();
    cdp.call(
        Some(session),
        "Input.setInterceptDrags",
        json!({"enabled": true}),
    )
    .await?;
    let result = drag_gesture(cdp, session, &mut events, from, to).await;
    let _ = cdp
        .call(
            Some(session),
            "Input.setInterceptDrags",
            json!({"enabled": false}),
        )
        .await;
    result
}

pub(super) fn drag_started(
    events: &mut tokio::sync::broadcast::Receiver<crate::browser::cdp::CdpEvent>,
    session: &str,
) -> Option<Value> {
    while let Ok(event) = events.try_recv() {
        if event.method == "Input.dragIntercepted" && event.session.as_deref() == Some(session) {
            return Some(event.params["data"].clone());
        }
    }
    None
}

pub(super) async fn drag_gesture(
    cdp: &Cdp,
    session: &str,
    events: &mut tokio::sync::broadcast::Receiver<crate::browser::cdp::CdpEvent>,
    from: (f64, f64),
    to: (f64, f64),
) -> Result<bool, String> {
    mouse(
        cdp,
        session,
        json!({"type": "mouseMoved", "x": from.0, "y": from.1}),
    )
    .await?;
    mouse(cdp, session, json!({"type": "mousePressed", "x": from.0, "y": from.1, "button": "left", "buttons": 1, "clickCount": 1})).await?;
    let mut intercepted = None;
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
        intercepted = drag_started(events, session);
        if intercepted.is_some() {
            break;
        }
    }
    if intercepted.is_none() {
        tokio::time::sleep(DRAG_START_WAIT).await;
        intercepted = drag_started(events, session);
    }
    let native = intercepted.is_some();
    finish_drag(cdp, session, intercepted, to).await?;
    Ok(native)
}

pub(super) async fn press_at(
    cdp: &Cdp,
    session: &str,
    x: f64,
    y: f64,
    clicks: u32,
) -> Result<(), String> {
    for count in 1..=clicks {
        for kind in ["mousePressed", "mouseReleased"] {
            cdp.call(
                Some(session),
                "Input.dispatchMouseEvent",
                json!({"type": kind, "x": x, "y": y, "button": "left", "buttons": 1, "clickCount": count}),
            )
            .await?;
        }
    }
    Ok(())
}

pub(super) async fn press_key(cdp: &Cdp, session: &str, press: &KeyPress) -> Result<(), String> {
    press_key_for(cdp, session, press, Duration::ZERO).await
}

pub(super) async fn press_key_for(
    cdp: &Cdp,
    session: &str,
    press: &KeyPress,
    hold: Duration,
) -> Result<(), String> {
    let mut down = json!({
        "type": if press.text.is_some() { "keyDown" } else { "rawKeyDown" },
        "key": press.key, "code": press.code,
        "windowsVirtualKeyCode": press.key_code, "nativeVirtualKeyCode": press.key_code,
        "modifiers": press.modifiers,
    });
    if let Some(text) = &press.text {
        down["text"] = json!(text);
        down["unmodifiedText"] = json!(text);
    }
    cdp.call(Some(session), "Input.dispatchKeyEvent", down)
        .await?;
    tokio::time::sleep(hold).await;
    cdp.call(
        Some(session),
        "Input.dispatchKeyEvent",
        json!({"type": "keyUp", "key": press.key, "code": press.code, "windowsVirtualKeyCode": press.key_code, "modifiers": press.modifiers}),
    )
    .await?;
    Ok(())
}

/// DOM nodes of `<input type=password>` among the snapshot's text fields.
pub(super) async fn password_fields(
    cdp: &Cdp,
    session: &str,
    nodes: &[crate::browser::snapshot::AxNode],
) -> HashSet<i64> {
    let fields: Vec<i64> = nodes
        .iter()
        .filter(|node| matches!(node.role.as_str(), "textbox" | "textfield" | "searchbox"))
        .filter_map(|node| node.backend_id)
        .take(64)
        .collect();
    let mut passwords = HashSet::new();
    for backend in fields {
        let described = cdp
            .call_with_timeout(
                Some(session),
                "DOM.describeNode",
                json!({"backendNodeId": backend}),
                Duration::from_secs(5),
            )
            .await;
        // When a field cannot be described, mask it: never risk showing a password.
        let password = described.map_or(true, |node| {
            let attributes: Vec<&str> = node["node"]["attributes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            attributes.chunks(2).any(|pair| {
                pair.len() == 2
                    && pair[0].eq_ignore_ascii_case("type")
                    && pair[1].eq_ignore_ascii_case("password")
            })
        });
        if password {
            passwords.insert(backend);
        }
    }
    passwords
}
