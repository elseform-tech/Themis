//! The tools that act on the page like a person: click, type, select, press, scroll, move, drag.

use super::*;

/// Arguments for `browser_click`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct ClickArgs {
    #[serde(default, alias = "ref")]
    #[input(
        description = "The element: a ref from the latest snapshot like e14, a CSS selector, or text=Some text; omit when clicking a point"
    )]
    pub reference: String,
    #[serde(default)]
    #[input(
        description = "A point to click instead of a ref: CSS pixels from the left of the visible page (for canvas, maps and games). A page holding the pointer lock is clicked where the mouse is"
    )]
    pub x: Option<f64>,
    #[serde(default)]
    #[input(description = "The point's CSS pixels from the top of the visible page; goes with x")]
    pub y: Option<f64>,
    #[serde(default)]
    #[input(description = "Double-click instead")]
    pub double: bool,
}

#[tool(
    name = "browser_click",
    description = "Click an element of the current page by its ref (buttons, links, checkboxes, tabs), or a point of the visible page by x and y in CSS pixels when there is no ref (a canvas, map or game). Waits for the page to settle and returns a fresh snapshot. Examples: {\"ref\":\"e14\"}, {\"x\":412,\"y\":230}.",
    input = ClickArgs,
)]
pub(super) struct BrowserClick {
    pub(super) base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserClick {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: ClickArgs = parse_tool_args("browser_click", args)?;
        // Models sometimes quote a ref, as they saw it in a JSON result.
        let reference = model_reference(&args.reference);
        let point = match (args.x, args.y) {
            // Models fill every field: a zero point beside a ref means no point.
            (Some(x), Some(y)) if x == 0.0 && y == 0.0 && !reference.is_empty() => None,
            (None, None) if reference.is_empty() => {
                return Err(tool_error(
                    "browser_click: give a ref, or x and y".to_owned(),
                ))
            }
            (None, None) => None,
            (Some(x), Some(y)) if reference.is_empty() => Some((x, y)),
            (Some(_), Some(_)) => {
                return Err(tool_error(
                    "browser_click: give a ref or a point, not both".to_owned(),
                ))
            }
            _ => {
                return Err(tool_error(
                    "browser_click: a point needs both x and y".to_owned(),
                ))
            }
        };
        if let Some((x, y)) = point {
            let label = format!("the point ({}, {})", x.round(), y.round());
            return self
                .base
                .act(
                    "browser_click",
                    RiskLevel::Network,
                    None,
                    format!("browser_click {label}"),
                    format!("clicking {label}"),
                    |chat| async move { chat.click_point(x, y, args.double).await },
                )
                .await;
        }
        let name = self.base.scope.chat.name_of(&reference).await;
        let label = name.as_deref().map_or_else(|| reference.clone(), quote);
        self.base
            .act(
                "browser_click",
                RiskLevel::Network,
                None,
                format!("browser_click {reference} {label}"),
                format!("clicking {label}"),
                |chat| async move { chat.click(&reference, args.double).await },
            )
            .await
    }
}

/// Arguments for `browser_type`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct TypeArgs {
    #[serde(alias = "ref")]
    #[input(description = "The text field's ref from the latest snapshot")]
    pub reference: String,
    #[input(description = "Text to type")]
    pub text: String,
    #[serde(default)]
    #[input(description = "Press Enter afterwards (submits most forms)")]
    pub submit: bool,
    #[serde(default)]
    #[input(description = "Clear the field first")]
    pub clear: bool,
}

#[tool(
    name = "browser_type",
    description = "Type text into a field of the current page by its ref, optionally clearing it first and pressing Enter after. Returns a fresh snapshot. Example: {\"ref\":\"e12\",\"text\":\"test@example.com\",\"clear\":true}.",
    input = TypeArgs,
)]
pub(super) struct BrowserType {
    pub(super) base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserType {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: TypeArgs = parse_tool_args("browser_type", args)?;
        if args.text.chars().count() > 10_000 {
            return Err(tool_error(
                "browser_type: text is longer than 10,000 characters".into(),
            ));
        }
        let reference = model_reference(&args.reference);
        let name = self.base.scope.chat.name_of(&reference).await;
        let label = name.as_deref().map_or_else(|| reference.clone(), quote);
        self.base
            .act(
                "browser_type",
                RiskLevel::Network,
                None,
                format!("browser_type {reference} {label}: {}", quote(&args.text)),
                format!("typing into {label}"),
                |chat| async move {
                    chat.type_text(&reference, &args.text, args.submit, args.clear)
                        .await
                },
            )
            .await
    }
}

/// Arguments for `browser_select`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct SelectArgs {
    #[serde(alias = "ref")]
    #[input(description = "The dropdown's ref from the latest snapshot")]
    pub reference: String,
    #[input(
        description = "Option values or labels to choose (one, or several for a multi-select)"
    )]
    pub values: Vec<String>,
}

#[tool(
    name = "browser_select",
    description = "Choose options of a dropdown (<select>) by value or label. For custom dropdowns, click them and then the option. Returns a fresh snapshot. Example: {\"ref\":\"e9\",\"values\":[\"Germany\"]}.",
    input = SelectArgs,
)]
pub(super) struct BrowserSelect {
    pub(super) base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserSelect {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: SelectArgs = parse_tool_args("browser_select", args)?;
        if args.values.is_empty() || args.values.len() > 50 {
            return Err(tool_error("browser_select: give 1..50 values".into()));
        }
        let reference = model_reference(&args.reference);
        let name = self.base.scope.chat.name_of(&reference).await;
        let label = name.as_deref().map_or_else(|| reference.clone(), quote);
        self.base
            .act(
                "browser_select",
                RiskLevel::Network,
                None,
                format!(
                    "browser_select {reference} {label}: {}",
                    quote(&args.values.join(", "))
                ),
                format!("choosing in {label}"),
                |chat| async move { chat.select(&reference, &args.values).await },
            )
            .await
    }
}

const MAX_KEY_HOLD_MS: u64 = 5000;

/// Arguments for `browser_press`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct PressArgs {
    #[input(
        description = "Key name: Enter, Escape, Tab, ArrowDown, Backspace, PageDown, a letter or digit like w or 1, or a shortcut like Control+a"
    )]
    pub key: String,
    #[serde(default)]
    #[input(
        description = "Milliseconds to keep the key down, up to 5000, to move in a game; omit for a plain press"
    )]
    pub hold_ms: Option<u64>,
}

#[tool(
    name = "browser_press",
    description = "Press a key in the focused element of the current page (Enter, Escape, Tab, arrows, letters, or shortcuts like Control+a), or hold it with hold_ms. Returns a fresh snapshot. Also mouseDown and mouseUp, to hold the mouse button: browser_move there, mouseDown, browser_move across, mouseUp.",
    input = PressArgs,
)]
pub(super) struct BrowserPress {
    pub(super) base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserPress {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: PressArgs = parse_tool_args("browser_press", args)?;
        let key = args.key.trim().to_owned();
        let hold = std::time::Duration::from_millis(args.hold_ms.unwrap_or(0).min(MAX_KEY_HOLD_MS));
        self.base
            .act(
                "browser_press",
                RiskLevel::Network,
                None,
                format!("browser_press {key}"),
                format!("pressing {key}"),
                |chat| async move { chat.press(&key, hold).await },
            )
            .await
    }
}

/// Arguments for `browser_scroll`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct ScrollArgs {
    #[input(description = "up, down, left or right")]
    pub direction: String,
    #[serde(default, alias = "ref")]
    #[input(description = "Scroll inside this element (a ref) instead of the page")]
    pub reference: String,
}

#[tool(
    name = "browser_scroll",
    description = "Scroll the current page (or one scrollable element) by most of a screen, for infinite feeds and content that loads as you scroll. Snapshots already include off-screen content. Returns a fresh snapshot.",
    input = ScrollArgs,
)]
pub(super) struct BrowserScroll {
    pub(super) base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserScroll {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: ScrollArgs = parse_tool_args("browser_scroll", args)?;
        let direction = args.direction.trim().to_ascii_lowercase();
        let reference = Some(model_reference(&args.reference)).filter(|r| !r.is_empty());
        self.base
            .act(
                "browser_scroll",
                RiskLevel::Read,
                None,
                format!("browser_scroll {direction}"),
                "scrolling".to_owned(),
                |chat| async move { chat.scroll(&direction, reference.as_deref()).await },
            )
            .await
    }
}

/// A ref as a model sends it: models quote refs they saw in JSON, and fill unused fields with
/// the word for nothing.
pub(super) fn model_reference(text: &str) -> String {
    let text = text.trim().trim_matches('"').trim();
    match text.to_ascii_lowercase().as_str() {
        "null" | "none" | "nil" | "undefined" => String::new(),
        _ => text.to_owned(),
    }
}

/// Arguments for `browser_move`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct MoveArgs {
    #[serde(default)]
    #[input(description = "CSS pixels from the left of the visible page; goes with y")]
    pub x: Option<f64>,
    #[serde(default)]
    #[input(description = "CSS pixels from the top of the visible page; goes with x")]
    pub y: Option<f64>,
    #[serde(default)]
    #[input(
        description = "Instead of x and y: CSS pixels to move right (negative for left) from where the mouse is"
    )]
    pub dx: Option<f64>,
    #[serde(default)]
    #[input(
        description = "Instead of x and y: CSS pixels to move down (negative for up) from where the mouse is"
    )]
    pub dy: Option<f64>,
}

#[tool(
    name = "browser_move",
    description = "Move the mouse without clicking; `under_mouse` in the result says what it is over. x and y go to a point of the visible page, as for a hover menu; dx and dy move by a distance from where the mouse is, to aim in a game, where the view turns with the distance moved and a page holding the pointer lock has no edge. A snapshot cannot show what a canvas draws: read where things are with browser_devtools, then act here and with browser_click, since only real input tests the page.",
    input = MoveArgs,
)]
pub(super) struct BrowserMove {
    pub(super) base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserMove {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let MoveArgs { x, y, dx, dy } = parse_tool_args("browser_move", args)?;
        let relative = dx.is_some() || dy.is_some();
        let target = match (x, y, relative) {
            (Some(x), Some(y), false) => (x, y),
            (None, None, true) => (dx.unwrap_or(0.0), dy.unwrap_or(0.0)),
            _ => {
                return Err(tool_error(
                    "browser_move: give either x and y, or dx and dy".to_owned(),
                ))
            }
        };
        let (first, second) = target;
        let label = if relative {
            format!("by ({}, {})", first.round(), second.round())
        } else {
            format!("to ({}, {})", first.round(), second.round())
        };
        self.base
            .act(
                "browser_move",
                RiskLevel::Network,
                None,
                format!("browser_move {label}"),
                format!("moving the mouse {label}"),
                |chat| async move {
                    if relative {
                        chat.move_by(first, second).await
                    } else {
                        chat.move_point(first, second).await
                    }
                },
            )
            .await
    }
}

/// Arguments for `browser_drag`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct DragArgs {
    #[serde(default, alias = "from_ref")]
    #[input(
        description = "Where to press: a ref from the latest snapshot like e14, a CSS selector like .card, or text=Some text; or use from_x and from_y"
    )]
    pub from_reference: String,
    #[serde(default)]
    #[input(
        description = "Where to press: CSS pixels from the left of the visible page; goes with from_y"
    )]
    pub from_x: Option<f64>,
    #[serde(default)]
    #[input(
        description = "Where to press: CSS pixels from the top of the visible page; goes with from_x"
    )]
    pub from_y: Option<f64>,
    #[serde(default, alias = "to_ref")]
    #[input(
        description = "Where to let go: a ref, a CSS selector like .column, or text=Some text; or use to_x and to_y"
    )]
    pub to_reference: String,
    #[serde(default)]
    #[input(
        description = "Where to let go: CSS pixels from the left of the visible page; goes with to_y"
    )]
    pub to_x: Option<f64>,
    #[serde(default)]
    #[input(
        description = "Where to let go: CSS pixels from the top of the visible page; goes with to_x"
    )]
    pub to_y: Option<f64>,
}

/// One end of a drag from what a model gave for it.
fn drag_end(
    which: &str,
    reference: &str,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<DragEnd, ToolCallError> {
    let reference = model_reference(reference);
    match (x, y) {
        // Models fill every field: a zero point beside a ref means no point.
        (Some(x), Some(y)) if x == 0.0 && y == 0.0 && !reference.is_empty() => {
            Ok(DragEnd::Element(reference))
        }
        (None, None) if !reference.is_empty() => Ok(DragEnd::Element(reference)),
        (Some(x), Some(y)) if reference.is_empty() => Ok(DragEnd::Point(x, y)),
        (Some(_), Some(_)) => Err(tool_error(format!(
            "browser_drag: give {which} as a ref or a point, not both"
        ))),
        _ => Err(tool_error(format!(
            "browser_drag: give {which} as a ref, or both its x and y"
        ))),
    }
}

#[tool(
    name = "browser_drag",
    description = "Press the mouse at one place, move to another and let go, as a person drags: a card between columns, a slider, a shape on a canvas. Each end is a ref from the latest snapshot, a CSS selector, text=Some text, or a point of the visible page. It is real input, for pages using native drag and drop and for pages listening to the mouse. The result says what was pressed on, whether anything was dragged, and where it let go.",
    input = DragArgs,
)]
pub(super) struct BrowserDrag {
    pub(super) base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserDrag {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: DragArgs = parse_tool_args("browser_drag", args)?;
        let from = drag_end("the start", &args.from_reference, args.from_x, args.from_y)?;
        let to = drag_end("the end", &args.to_reference, args.to_x, args.to_y)?;
        let name = |end: &DragEnd| match end {
            DragEnd::Element(reference) => reference.clone(),
            DragEnd::Point(x, y) => format!("({}, {})", x.round(), y.round()),
        };
        let label = format!("from {} to {}", name(&from), name(&to));
        self.base
            .act(
                "browser_drag",
                RiskLevel::Network,
                None,
                format!("browser_drag {label}"),
                format!("dragging {label}"),
                |chat| async move { chat.drag(&from, &to).await },
            )
            .await
    }
}
