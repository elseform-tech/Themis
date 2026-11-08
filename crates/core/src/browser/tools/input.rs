//! The actions that work the page like a person: click, type, select, press, scroll, move, drag.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{action_error, model_reference, parse_action, quote, Action, Base};
use crate::browser::session::DragEnd;
use crate::tools::ToolCallError;

/// Arguments for `click`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ClickArgs {
    #[serde(default, alias = "ref")]
    pub reference: String,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default)]
    pub double: bool,
}

pub(super) async fn click(base: &Base, args: Value) -> Result<Value, ToolCallError> {
    let action = Action::Click;
    let args: ClickArgs = parse_action(action, args)?;
    // Models sometimes quote a ref, as they saw it in a JSON result.
    let reference = model_reference(&args.reference);
    let point = match (args.x, args.y) {
        // Models fill every field: a zero point beside a ref means no point.
        (Some(x), Some(y)) if x == 0.0 && y == 0.0 && !reference.is_empty() => None,
        (None, None) if reference.is_empty() => {
            return Err(action_error(action, "give a ref, or x and y"))
        }
        (None, None) => None,
        (Some(x), Some(y)) if reference.is_empty() => Some((x, y)),
        (Some(_), Some(_)) => return Err(action_error(action, "give a ref or a point, not both")),
        _ => return Err(action_error(action, "a point needs both x and y")),
    };
    if let Some((x, y)) = point {
        let label = format!("the point ({}, {})", x.round(), y.round());
        return base
            .act(
                action,
                None,
                label.clone(),
                format!("clicking {label}"),
                |chat| async move { chat.click_point(x, y, args.double).await },
            )
            .await;
    }
    let (reference, label) = element(base, &reference).await;
    base.act(
        action,
        None,
        format!("{reference} {label}"),
        format!("clicking {label}"),
        |chat| async move { chat.click(&reference, args.double).await },
    )
    .await
}

/// The element a model named, and how the person reads it: its quoted name on the page,
/// or the ref itself.
async fn element(base: &Base, given: &str) -> (String, String) {
    let reference = model_reference(given);
    let name = base.scope.chat.name_of(&reference).await;
    let label = name.as_deref().map_or_else(|| reference.clone(), quote);
    (reference, label)
}

/// Arguments for `type`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct TypeArgs {
    #[serde(alias = "ref")]
    pub reference: String,
    pub text: String,
    #[serde(default)]
    pub submit: bool,
    #[serde(default)]
    pub clear: bool,
}

pub(super) async fn type_text(base: &Base, args: Value) -> Result<Value, ToolCallError> {
    let action = Action::Type;
    let args: TypeArgs = parse_action(action, args)?;
    if args.text.chars().count() > 10_000 {
        return Err(action_error(
            action,
            "text is longer than 10,000 characters",
        ));
    }
    let (reference, label) = element(base, &args.reference).await;
    base.act(
        action,
        None,
        format!("{reference} {label}: {}", quote(&args.text)),
        format!("typing into {label}"),
        |chat| async move {
            chat.type_text(&reference, &args.text, args.submit, args.clear)
                .await
        },
    )
    .await
}

/// Arguments for `select`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct SelectArgs {
    #[serde(alias = "ref")]
    pub reference: String,
    pub values: Vec<String>,
}

pub(super) async fn select(base: &Base, args: Value) -> Result<Value, ToolCallError> {
    let action = Action::Select;
    let args: SelectArgs = parse_action(action, args)?;
    if args.values.is_empty() || args.values.len() > 50 {
        return Err(action_error(action, "give 1..50 values"));
    }
    let (reference, label) = element(base, &args.reference).await;
    base.act(
        action,
        None,
        format!("{reference} {label}: {}", quote(&args.values.join(", "))),
        format!("choosing in {label}"),
        |chat| async move { chat.select(&reference, &args.values).await },
    )
    .await
}

const MAX_KEY_HOLD_MS: u64 = 5000;

/// Arguments for `press`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct PressArgs {
    pub key: String,
    #[serde(default)]
    pub hold_ms: Option<u64>,
}

pub(super) async fn press(base: &Base, args: Value) -> Result<Value, ToolCallError> {
    let args: PressArgs = parse_action(Action::Press, args)?;
    let key = args.key.trim().to_owned();
    let hold = std::time::Duration::from_millis(args.hold_ms.unwrap_or(0).min(MAX_KEY_HOLD_MS));
    base.act(
        Action::Press,
        None,
        key.clone(),
        format!("pressing {key}"),
        |chat| async move { chat.press(&key, hold).await },
    )
    .await
}

/// Arguments for `scroll`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ScrollArgs {
    /// up, down, left or right.
    pub direction: String,
    #[serde(default, alias = "ref")]
    pub reference: String,
}

pub(super) async fn scroll(base: &Base, args: Value) -> Result<Value, ToolCallError> {
    let args: ScrollArgs = parse_action(Action::Scroll, args)?;
    let direction = args.direction.trim().to_ascii_lowercase();
    let reference = Some(model_reference(&args.reference)).filter(|r| !r.is_empty());
    base.act(
        Action::Scroll,
        None,
        direction.clone(),
        "scrolling".to_owned(),
        |chat| async move { chat.scroll(&direction, reference.as_deref()).await },
    )
    .await
}

/// Arguments for `move`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct MoveArgs {
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default)]
    pub dx: Option<f64>,
    #[serde(default)]
    pub dy: Option<f64>,
}

pub(super) async fn move_mouse(base: &Base, args: Value) -> Result<Value, ToolCallError> {
    let action = Action::Move;
    let MoveArgs { x, y, dx, dy } = parse_action(action, args)?;
    let relative = dx.is_some() || dy.is_some();
    let target = match (x, y, relative) {
        (Some(x), Some(y), false) => (x, y),
        (None, None, true) => (dx.unwrap_or(0.0), dy.unwrap_or(0.0)),
        _ => return Err(action_error(action, "give either x and y, or dx and dy")),
    };
    let (first, second) = target;
    let label = if relative {
        format!("by ({}, {})", first.round(), second.round())
    } else {
        format!("to ({}, {})", first.round(), second.round())
    };
    base.act(
        action,
        None,
        label.clone(),
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

/// Arguments for `drag`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct DragArgs {
    #[serde(default, alias = "from_ref")]
    pub from_reference: String,
    #[serde(default)]
    pub from_x: Option<f64>,
    #[serde(default)]
    pub from_y: Option<f64>,
    #[serde(default, alias = "to_ref")]
    pub to_reference: String,
    #[serde(default)]
    pub to_x: Option<f64>,
    #[serde(default)]
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
        (Some(_), Some(_)) => Err(action_error(
            Action::Drag,
            format!("give {which} as a ref or a point, not both"),
        )),
        _ => Err(action_error(
            Action::Drag,
            format!("give {which} as a ref, or both its x and y"),
        )),
    }
}

pub(super) async fn drag(base: &Base, args: Value) -> Result<Value, ToolCallError> {
    let args: DragArgs = parse_action(Action::Drag, args)?;
    let from = drag_end("the start", &args.from_reference, args.from_x, args.from_y)?;
    let to = drag_end("the end", &args.to_reference, args.to_x, args.to_y)?;
    let name = |end: &DragEnd| match end {
        DragEnd::Element(reference) => reference.clone(),
        DragEnd::Point(x, y) => format!("({}, {})", x.round(), y.round()),
    };
    let label = format!("from {} to {}", name(&from), name(&to));
    base.act(
        Action::Drag,
        None,
        label.clone(),
        format!("dragging {label}"),
        |chat| async move { chat.drag(&from, &to).await },
    )
    .await
}
