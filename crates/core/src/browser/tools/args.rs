//! The `browser` tool's schema: one object whose `action` picks what to do, and the union of
//! every action's fields, each described with the actions that read it. Actions the run does
//! not offer (screenshots and recordings without a place to save them, `devtools` without
//! Developer mode) are left out of the enum, with the fields only they read.

use autoagents::core::tool::ToolInputSchema;
use autoagents_derive::ToolInput;
use serde_json::Value;

use crate::tools::{tool_error, RiskLevel, ToolCallError};

/// What the `browser` tool can do, in the order the schema lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Action {
    Open,
    Navigate,
    Snapshot,
    Click,
    Type,
    Select,
    Press,
    Scroll,
    Move,
    Drag,
    CheckLooks,
    Screenshot,
    Record,
    Devtools,
}

/// Every action with its name and its approval risk (`devtools` rises above Read for calls
/// that run code or change the page or storage).
pub(crate) const ACTIONS: [(Action, &str, RiskLevel); 14] = [
    (Action::Open, "open", RiskLevel::Network),
    (Action::Navigate, "navigate", RiskLevel::Network),
    (Action::Snapshot, "snapshot", RiskLevel::Read),
    (Action::Click, "click", RiskLevel::Network),
    (Action::Type, "type", RiskLevel::Network),
    (Action::Select, "select", RiskLevel::Network),
    (Action::Press, "press", RiskLevel::Network),
    (Action::Scroll, "scroll", RiskLevel::Read),
    (Action::Move, "move", RiskLevel::Network),
    (Action::Drag, "drag", RiskLevel::Network),
    (Action::CheckLooks, "check_looks", RiskLevel::Read),
    (Action::Screenshot, "screenshot", RiskLevel::Read),
    (Action::Record, "record", RiskLevel::Read),
    (Action::Devtools, "devtools", RiskLevel::Read),
];

impl Action {
    pub(crate) fn name(self) -> &'static str {
        ACTIONS
            .iter()
            .find(|(action, ..)| *action == self)
            .map_or("", |(_, name, _)| name)
    }

    pub(crate) fn risk(self) -> RiskLevel {
        ACTIONS
            .iter()
            .find(|(action, ..)| *action == self)
            .map_or(RiskLevel::Read, |(.., risk)| *risk)
    }

    fn named(name: &str) -> Option<Self> {
        ACTIONS
            .iter()
            .find(|(_, known, _)| *known == name)
            .map(|(action, ..)| *action)
    }
}

/// Which actions read each field of the schema.
pub(crate) const FIELDS: &[(&str, &[Action])] = &[
    ("url", &[Action::Open, Action::Devtools]),
    ("direction", &[Action::Navigate, Action::Scroll]),
    (
        "reference",
        &[
            Action::Snapshot,
            Action::Screenshot,
            Action::Click,
            Action::Type,
            Action::Select,
            Action::Scroll,
            Action::Devtools,
        ],
    ),
    ("layout", &[Action::Snapshot]),
    ("full_page", &[Action::Screenshot]),
    ("operation", &[Action::Record, Action::Devtools]),
    ("max_seconds", &[Action::Record]),
    ("x", &[Action::Click, Action::Move]),
    ("y", &[Action::Click, Action::Move]),
    ("double", &[Action::Click]),
    ("text", &[Action::Type]),
    ("submit", &[Action::Type]),
    ("clear", &[Action::Type]),
    ("values", &[Action::Select]),
    ("key", &[Action::Press]),
    ("hold_ms", &[Action::Press]),
    ("dx", &[Action::Move]),
    ("dy", &[Action::Move]),
    ("from_reference", &[Action::Drag]),
    ("from_x", &[Action::Drag]),
    ("from_y", &[Action::Drag]),
    ("to_reference", &[Action::Drag]),
    ("to_x", &[Action::Drag]),
    ("to_y", &[Action::Drag]),
    ("panel", &[Action::Devtools]),
    ("expression", &[Action::Devtools]),
    ("since", &[Action::Devtools]),
    ("selector", &[Action::Devtools]),
    ("name", &[Action::Devtools]),
    ("value", &[Action::Devtools]),
    ("properties", &[Action::Devtools]),
    ("id", &[Action::Devtools]),
    ("block", &[Action::Devtools]),
    ("throttle", &[Action::Devtools]),
    ("line", &[Action::Devtools]),
    ("condition", &[Action::Devtools]),
    ("area", &[Action::Devtools]),
    ("seconds", &[Action::Devtools]),
];

/// The schema only: each action parses its own arguments from the same object.
#[allow(dead_code)]
#[derive(Debug, Default, ToolInput)]
pub(crate) struct BrowserArgs {
    #[input(
        description = "What to do. Each action reads only the fields that name it.",
        choice = ["open", "navigate", "snapshot", "click", "type", "select", "press", "scroll", "move", "drag", "check_looks", "screenshot", "record", "devtools"]
    )]
    action: String,
    #[input(
        description = "open: full http or https URL. devtools debugger set: script URL or part of it"
    )]
    url: Option<String>,
    #[input(description = "navigate: back, forward or reload. scroll: up, down, left or right")]
    direction: Option<String>,
    #[input(
        description = "An element: a ref from the latest snapshot like e14, a CSS selector, or text=Some text. click, type, select: the element to act on (click: or a point with x and y). snapshot, screenshot: only this element; scroll: scroll inside it; omit for the page. devtools elements: or a snapshot ref"
    )]
    reference: Option<String>,
    #[input(
        description = "snapshot: also return the page's structure: its containers with positions and a selector for each, and whether each canvas drew anything. Use this rather than guessing where something is."
    )]
    layout: Option<bool>,
    #[input(description = "screenshot: the whole scrollable page instead of the visible part")]
    full_page: Option<bool>,
    #[input(
        description = "record: start or stop. devtools: elements: inspect|set_attribute|remove_attribute|set_style|set_text|remove; debugger: set|remove|pause_on_exceptions|pause|resume|step_over|step_into|step_out|state; storage: read|set|remove|clear; performance: metrics|trace; memory: summary|snapshot"
    )]
    operation: Option<String>,
    #[input(description = "record start: longest length in seconds (default 30, max 120)")]
    max_seconds: Option<u64>,
    #[input(
        description = "click: a point to click instead of a ref, CSS pixels from the left of the visible page (canvas, maps and games; a page holding the pointer lock is clicked where the mouse is). move: go to this point; goes with y"
    )]
    x: Option<f64>,
    #[input(
        description = "click, move: the point's CSS pixels from the top of the visible page; goes with x"
    )]
    y: Option<f64>,
    #[input(description = "click: double-click instead")]
    double: Option<bool>,
    #[input(description = "type: text to type")]
    text: Option<String>,
    #[input(description = "type: press Enter afterwards (submits most forms)")]
    submit: Option<bool>,
    #[input(description = "type: clear the field first")]
    clear: Option<bool>,
    #[input(
        description = "select: option values or labels to choose (one, or several for a multi-select)"
    )]
    values: Option<Vec<String>>,
    #[input(
        description = "press: key name: Enter, Escape, Tab, ArrowDown, Backspace, PageDown, a letter or digit like w or 1, a shortcut like Control+a, or mouseDown and mouseUp to hold the mouse button"
    )]
    key: Option<String>,
    #[input(
        description = "press: milliseconds to keep the key down, up to 5000, to move in a game; omit for a plain press"
    )]
    hold_ms: Option<u64>,
    #[input(
        description = "move: instead of x and y, CSS pixels to move right (negative for left) from where the mouse is"
    )]
    dx: Option<f64>,
    #[input(
        description = "move: instead of x and y, CSS pixels to move down (negative for up) from where the mouse is"
    )]
    dy: Option<f64>,
    #[input(
        description = "drag: where to press: a ref from the latest snapshot like e14, a CSS selector like .card, or text=Some text; or use from_x and from_y"
    )]
    from_reference: Option<String>,
    #[input(
        description = "drag: where to press: CSS pixels from the left of the visible page; goes with from_y"
    )]
    from_x: Option<f64>,
    #[input(
        description = "drag: where to press: CSS pixels from the top of the visible page; goes with from_x"
    )]
    from_y: Option<f64>,
    #[input(
        description = "drag: where to let go: a ref, a CSS selector like .column, or text=Some text; or use to_x and to_y"
    )]
    to_reference: Option<String>,
    #[input(
        description = "drag: where to let go: CSS pixels from the left of the visible page; goes with to_y"
    )]
    to_x: Option<f64>,
    #[input(
        description = "drag: where to let go: CSS pixels from the top of the visible page; goes with to_x"
    )]
    to_y: Option<f64>,
    #[input(
        description = "devtools: console, elements, network, debugger, storage, performance or memory"
    )]
    panel: Option<String>,
    #[input(description = "devtools console: JavaScript to run (awaited); omit to read messages")]
    expression: Option<String>,
    #[input(description = "devtools console/network: only entries newer than this next_since")]
    since: Option<u64>,
    #[input(description = "devtools elements: CSS selector")]
    selector: Option<String>,
    #[input(
        description = "devtools elements: attribute or CSS property; storage: cookie name or key"
    )]
    name: Option<String>,
    #[input(description = "devtools: new value (elements, storage)")]
    value: Option<String>,
    #[input(description = "devtools elements: computed styles to read")]
    properties: Option<Vec<String>>,
    #[input(
        description = "devtools network: request id for headers and body; debugger: breakpoint id"
    )]
    id: Option<String>,
    #[input(description = "devtools network: URL patterns to block (* wildcards); [] unblocks")]
    block: Option<Vec<String>>,
    #[input(description = "devtools network: off|offline|slow3g|fast3g")]
    throttle: Option<String>,
    #[input(description = "devtools debugger set: line (1-based)")]
    line: Option<u64>,
    #[input(
        description = "devtools debugger: breakpoint condition, or all|uncaught|none for pause_on_exceptions"
    )]
    condition: Option<String>,
    #[input(description = "devtools storage: cookies, local or session")]
    area: Option<String>,
    #[input(description = "devtools performance trace: seconds, 1-15 (default 5)")]
    seconds: Option<u64>,
}

/// The schema for the actions on offer: their names in the enum, and only the fields one of
/// them reads.
pub(crate) fn schema(offered: &[Action]) -> Value {
    let mut schema = BrowserArgs::io_schema_value().clone();
    if let Some(properties) = schema["properties"].as_object_mut() {
        properties.retain(|field, _| {
            field == "action"
                || FIELDS.iter().any(|(name, readers)| {
                    name == field && readers.iter().any(|reader| offered.contains(reader))
                })
        });
    }
    schema["properties"]["action"]["enum"] = offered.iter().map(|action| action.name()).collect();
    schema
}

/// The action a call asks for, if this run offers it; otherwise an error that lists the ones
/// it does.
pub(crate) fn action_of(args: &Value, offered: &[Action]) -> Result<Action, ToolCallError> {
    let valid = || {
        offered
            .iter()
            .map(|action| action.name())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let Some(given) = args.get("action") else {
        return Err(tool_error(format!(
            "browser: argument 'action' is missing. Give one of: {}.",
            valid()
        )));
    };
    let Some(text) = given.as_str() else {
        return Err(tool_error(format!(
            "browser: argument 'action' is invalid: expected a string, one of: {}.",
            valid()
        )));
    };
    let name = text.trim().to_ascii_lowercase();
    match Action::named(&name).filter(|action| offered.contains(action)) {
        Some(action) => Ok(action),
        None => Err(tool_error(format!(
            "browser: unknown action '{}'. Use one of: {}.",
            text.chars().take(40).collect::<String>(),
            valid()
        ))),
    }
}
