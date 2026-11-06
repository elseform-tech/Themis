//! The person's own input on the Workspace view.

use super::*;

impl ChatBrowser {
    /// The person's mouse, wheel or key input on the Workspace view. The
    /// person and the agent use the browser at the same time; neither waits.
    pub async fn user_input(&self, input: &UserInput) -> Result<(), String> {
        self.person_acted();
        self.thaw_pages().await;
        let Some((cdp, session_id)) = self.view().current() else {
            return Ok(());
        };
        let session = Some(session_id.as_str());
        let cdp = &cdp;
        match input {
            UserInput::Mouse {
                kind,
                x,
                y,
                button,
                clicks,
                modifiers,
                delta_x,
                delta_y,
            } => {
                // A locked page has no pointer position: presses happen where it is held.
                let locked = self.page_locked.load(Ordering::SeqCst) && kind != "wheel";
                let (x, y) = if locked {
                    self.pointer
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .unwrap_or((*x, *y))
                } else {
                    *self.pointer.lock().unwrap_or_else(|e| e.into_inner()) = Some((*x, *y));
                    (*x, *y)
                };
                if locked && kind == "move" {
                    return Ok(());
                }
                let pressed = kind == "down";
                let kind = match kind.as_str() {
                    "down" => "mousePressed",
                    "up" => "mouseReleased",
                    "move" => "mouseMoved",
                    "wheel" => "mouseWheel",
                    _ => return Err("unknown mouse event".into()),
                };
                let button = match button.as_str() {
                    "left" | "right" | "middle" => button.as_str(),
                    _ => "none",
                };
                let buttons = match (kind, button) {
                    ("mousePressed" | "mouseMoved", "left") => 1,
                    ("mousePressed" | "mouseMoved", "right") => 2,
                    ("mousePressed" | "mouseMoved", "middle") => 4,
                    _ => 0,
                };
                let params = json!({"type": kind, "x": x, "y": y, "button": button, "buttons": buttons, "clickCount": clicks.min(&3), "modifiers": modifiers & 15, "deltaX": delta_x, "deltaY": delta_y});
                // A click is when a page takes (or gives up) the pointer lock: the press is
                // waited for, then the lock read.
                if pressed {
                    let _ = cdp.call(session, "Input.dispatchMouseEvent", params).await;
                    self.check_lock(cdp, &session_id).await;
                } else {
                    cdp.send(session, "Input.dispatchMouseEvent", params);
                }
            }
            UserInput::MouseBy { dx, dy } => {
                if !self.page_locked.load(Ordering::SeqCst) {
                    return Ok(());
                }
                let viewport = *self.viewport.lock().unwrap_or_else(|e| e.into_inner());
                let size = (f64::from(viewport.width), f64::from(viewport.height));
                let from = self
                    .pointer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .unwrap_or((size.0 / 2.0, size.1 / 2.0));
                let delta = (
                    dx.clamp(-MAX_PERSON_MOVE, MAX_PERSON_MOVE),
                    dy.clamp(-MAX_PERSON_MOVE, MAX_PERSON_MOVE),
                );
                let Ok(path) = relative_path(from, delta, size) else {
                    return Ok(());
                };
                for (x, y) in path.start.into_iter().chain([path.end]) {
                    cdp.send(
                        session,
                        "Input.dispatchMouseEvent",
                        json!({"type": "mouseMoved", "x": x, "y": y}),
                    );
                }
                *self.pointer.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.end);
            }
            UserInput::Key {
                kind,
                key,
                code,
                text,
                key_code,
                modifiers,
            } => {
                let kind = match kind.as_str() {
                    "down" if text.is_some() => "keyDown",
                    "down" => "rawKeyDown",
                    "up" => "keyUp",
                    _ => return Err("unknown key event".into()),
                };
                let mut params = json!({"type": kind, "key": cap(key, 20), "code": cap(code, 30), "windowsVirtualKeyCode": key_code, "modifiers": modifiers & 15});
                if let Some(text) = text.as_deref().filter(|_| kind == "keyDown") {
                    params["text"] = json!(cap(text, 8));
                }
                // Escape releases a held pointer lock: waited for, then the lock read.
                if kind == "rawKeyDown" && key == "Escape" {
                    let _ = cdp.call(session, "Input.dispatchKeyEvent", params).await;
                    self.check_lock(cdp, &session_id).await;
                } else {
                    cdp.send(session, "Input.dispatchKeyEvent", params);
                }
            }
            UserInput::Text { text } => {
                cdp.send(
                    session,
                    "Input.insertText",
                    json!({"text": text.chars().take(10_000).collect::<String>()}),
                );
            }
        }
        Ok(())
    }
}

/// Most CSS pixels one relative move from the person may carry.
pub(super) const MAX_PERSON_MOVE: f64 = 2_000.0;

/// Input from the Workspace view, in page CSS pixels.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserInput {
    Mouse {
        kind: String,
        x: f64,
        y: f64,
        #[serde(default)]
        button: String,
        #[serde(default)]
        clicks: u32,
        #[serde(default)]
        modifiers: u32,
        #[serde(default)]
        delta_x: f64,
        #[serde(default)]
        delta_y: f64,
    },
    Key {
        kind: String,
        key: String,
        #[serde(default)]
        code: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        key_code: u32,
        #[serde(default)]
        modifiers: u32,
    },
    Text {
        text: String,
    },
    /// A relative move from the person's locked mouse, in page CSS pixels.
    MouseBy {
        dx: f64,
        dy: f64,
    },
}
