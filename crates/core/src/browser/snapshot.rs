//! Page snapshots: Chrome's accessibility tree as a compact outline the model
//! can act on.
//!
//! ```text
//! - heading "Create your account" [level=2]
//! - textbox "Email" [e12]: test@example.com
//! - textbox "Password" [e13]: ••••••
//! - button "Create account" [e14]
//! ```
//!
//! Elements a model can act on get a ref (`e14`) that stays the same across
//! snapshots of one page. The tree comes from the page and is untrusted: any
//! shape renders without panicking, text is capped, password values are always
//! masked and the outline stops at a character budget.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// Default character budget of one snapshot.
pub const SNAPSHOT_CHARS: usize = 12_000;
const NAME_CHARS: usize = 120;
const TEXT_CHARS: usize = 200;
const MAX_DEPTH: usize = 40;

/// Roles the model can act on; each gets a ref.
const ACTIONABLE: &[&str] = &[
    "button",
    "link",
    "textbox",
    "searchbox",
    "combobox",
    "checkbox",
    "radio",
    "switch",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "tab",
    "option",
    "slider",
    "spinbutton",
    "listbox",
    "treeitem",
    "textfield",
    "ListBoxOption",
    "PopUpButton",
    "disclosuretriangle",
];

/// Roles that only wrap other content; their children take their place.
const TRANSPARENT: &[&str] = &[
    "generic",
    "none",
    "presentation",
    "RootWebArea",
    "WebArea",
    "InlineTextBox",
    "LineBreak",
    "paragraph",
    "Section",
    "group",
    "div",
    "LayoutTable",
    "LayoutTableRow",
    "LayoutTableCell",
    "Canvas",
    "Ruby",
];

/// One accessibility node, reduced to what the outline uses.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AxNode {
    pub id: String,
    pub role: String,
    pub name: String,
    pub value: Option<String>,
    pub ignored: bool,
    pub children: Vec<String>,
    pub backend_id: Option<i64>,
    pub properties: Vec<(String, String)>,
}

/// Parses `Accessibility.getFullAXTree`'s `nodes`. Malformed entries are skipped.
pub fn parse_ax_nodes(result: &Value) -> Vec<AxNode> {
    let text = |value: &Value| -> Option<String> {
        match &value["value"] {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            Value::Bool(flag) => Some(flag.to_string()),
            _ => None,
        }
    };
    result["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| {
            let id = match &node["nodeId"] {
                Value::String(id) => id.clone(),
                Value::Number(id) => id.to_string(),
                _ => return None,
            };
            Some(AxNode {
                id,
                role: text(&node["role"]).unwrap_or_default(),
                name: text(&node["name"]).unwrap_or_default(),
                value: text(&node["value"]),
                ignored: node["ignored"].as_bool().unwrap_or(false),
                children: node["childIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|child| match child {
                        Value::String(id) => Some(id.clone()),
                        Value::Number(id) => Some(id.to_string()),
                        _ => None,
                    })
                    .collect(),
                backend_id: node["backendDOMNodeId"].as_i64(),
                properties: node["properties"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|property| {
                        Some((
                            property["name"].as_str()?.to_owned(),
                            text(&property["value"])?,
                        ))
                    })
                    .collect(),
            })
        })
        .collect()
}

/// Refs of one page: a DOM node keeps its ref for as long as the page lives.
#[derive(Debug, Clone, Default)]
pub struct RefMap {
    by_node: HashMap<i64, u32>,
    by_ref: HashMap<u32, i64>,
    next: u32,
}

impl RefMap {
    /// The ref of `backend_id`, assigning the next free one the first time.
    pub fn assign(&mut self, backend_id: i64) -> u32 {
        if let Some(existing) = self.by_node.get(&backend_id) {
            return *existing;
        }
        self.next += 1;
        self.by_node.insert(backend_id, self.next);
        self.by_ref.insert(self.next, backend_id);
        self.next
    }

    /// The DOM node a ref (`e14` or `14`) names.
    pub fn resolve(&self, reference: &str) -> Option<i64> {
        let number: u32 = reference.trim().trim_start_matches('e').parse().ok()?;
        self.by_ref.get(&number).copied()
    }

    /// The ref text of a DOM node already in the map.
    pub fn of(&self, backend_id: i64) -> Option<String> {
        self.by_node.get(&backend_id).map(|n| format!("e{n}"))
    }

    /// Forget every ref (the page navigated to a new document).
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// What to render.
#[derive(Debug, Clone, Default)]
pub struct RenderOptions {
    /// DOM nodes of password fields: their values are masked.
    pub passwords: HashSet<i64>,
    /// Render only the subtree of this DOM node.
    pub root: Option<i64>,
    /// Character budget; 0 means [`SNAPSHOT_CHARS`].
    pub limit: usize,
}

/// The outline of a page, and whether it was cut at the budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outline {
    pub text: String,
    pub truncated: bool,
    /// Lines left out after the budget ran out.
    pub omitted_lines: usize,
}

fn cap(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = flat.chars();
    let mut out: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

fn quoted(text: &str) -> String {
    format!("\"{}\"", cap(text, NAME_CHARS).replace('"', "'"))
}

/// Renders `nodes` as an outline, assigning refs through `refs`.
pub fn render(nodes: &[AxNode], refs: &mut RefMap, options: &RenderOptions) -> Outline {
    let limit = if options.limit == 0 {
        SNAPSHOT_CHARS
    } else {
        options.limit
    };
    let by_id: HashMap<&str, &AxNode> = nodes.iter().map(|node| (node.id.as_str(), node)).collect();
    let children: HashSet<&str> = nodes
        .iter()
        .flat_map(|node| node.children.iter().map(String::as_str))
        .collect();
    let roots: Vec<&AxNode> = match options.root {
        Some(backend) => nodes
            .iter()
            .filter(|node| node.backend_id == Some(backend))
            .take(1)
            .collect(),
        // The node nobody lists as a child, or (a cyclic tree) the first one.
        None => nodes
            .iter()
            .find(|node| !children.contains(node.id.as_str()))
            .or_else(|| nodes.first())
            .into_iter()
            .collect(),
    };
    let mut lines = Vec::new();
    let mut visited = HashSet::new();
    for root in roots {
        walk(
            root,
            0,
            &by_id,
            refs,
            options,
            &mut lines,
            &mut visited,
            None,
        );
    }
    let mut text = String::new();
    let mut omitted = 0;
    for line in &lines {
        if omitted > 0 || text.len() + line.len() + 1 > limit {
            omitted += 1;
            continue;
        }
        text.push_str(line);
        text.push('\n');
    }
    Outline {
        text: text.trim_end().to_owned(),
        truncated: omitted > 0,
        omitted_lines: omitted,
    }
}

#[allow(clippy::too_many_arguments)]
fn walk<'a>(
    node: &'a AxNode,
    depth: usize,
    by_id: &HashMap<&str, &'a AxNode>,
    refs: &mut RefMap,
    options: &RenderOptions,
    lines: &mut Vec<String>,
    visited: &mut HashSet<&'a str>,
    parent_name: Option<&str>,
) {
    if depth > MAX_DEPTH || !visited.insert(node.id.as_str()) {
        return;
    }
    let role = node.role.as_str();
    let text_node = role == "StaticText" || role == "text";
    let shown = !node.ignored && !TRANSPARENT.contains(&role) && !role.is_empty();
    let mut child_depth = depth;
    let mut name_for_children = parent_name;
    if shown {
        if text_node {
            let text = node.name.trim();
            // Text already carried by the parent's name ("button "Save"") is not repeated.
            if !text.is_empty() && parent_name.is_none_or(|parent| !parent.contains(text)) {
                lines.push(format!(
                    "{}- text: {}",
                    "  ".repeat(depth),
                    cap(text, TEXT_CHARS)
                ));
            }
            return;
        }
        let mut line = format!("{}- {role}", "  ".repeat(depth));
        if !node.name.trim().is_empty() {
            line.push(' ');
            line.push_str(&quoted(&node.name));
        }
        // The ref comes right after the name, before any state, so it is
        // never confused with a flag like [expanded=false].
        let focusable = node
            .properties
            .iter()
            .any(|(name, value)| name == "focusable" && value == "true");
        if let Some(backend) = node.backend_id {
            if ACTIONABLE.contains(&role) || (focusable && role != "RootWebArea") {
                line.push_str(&format!(" [e{}]", refs.assign(backend)));
            }
        }
        for (name, value) in &node.properties {
            match (name.as_str(), value.as_str()) {
                ("level", level) => line.push_str(&format!(" [level={}]", cap(level, 3))),
                ("checked" | "pressed", "true") => line.push_str(&format!(" [{name}]")),
                ("checked", "mixed") => line.push_str(" [checked=mixed]"),
                ("disabled" | "required" | "selected" | "focused" | "readonly", "true") => {
                    line.push_str(&format!(" [{name}]"));
                }
                ("expanded", flag @ ("true" | "false")) => {
                    line.push_str(&format!(" [expanded={flag}]"));
                }
                _ => {}
            }
        }
        if let Some(value) = node.value.as_deref().filter(|value| !value.is_empty()) {
            let masked = node
                .backend_id
                .is_some_and(|id| options.passwords.contains(&id));
            if masked {
                line.push_str(": ••••••");
            } else if value != node.name {
                line.push_str(": ");
                line.push_str(&cap(value, TEXT_CHARS));
            }
        }
        lines.push(line);
        child_depth = depth + 1;
        name_for_children = Some(node.name.as_str()).filter(|name| !name.is_empty());
    }
    for child in &node.children {
        if let Some(child) = by_id.get(child.as_str()) {
            walk(
                child,
                child_depth,
                by_id,
                refs,
                options,
                lines,
                visited,
                name_for_children,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    fn tree() -> Value {
        json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "name": {"value": "Acme"}, "childIds": ["2", "3", "4", "5", "6", "9"], "backendDOMNodeId": 1},
            {"nodeId": "2", "role": {"value": "heading"}, "name": {"value": "Create your account"}, "properties": [{"name": "level", "value": {"value": 2}}], "childIds": ["7"], "backendDOMNodeId": 10},
            {"nodeId": "3", "role": {"value": "textbox"}, "name": {"value": "Email"}, "value": {"value": "test@example.com"}, "backendDOMNodeId": 11, "properties": [{"name": "focusable", "value": {"value": true}}]},
            {"nodeId": "4", "role": {"value": "textbox"}, "name": {"value": "Password"}, "value": {"value": "hunter2"}, "backendDOMNodeId": 12},
            {"nodeId": "5", "role": {"value": "generic"}, "childIds": ["8"]},
            {"nodeId": "6", "role": {"value": "checkbox"}, "name": {"value": "Remember me"}, "backendDOMNodeId": 14, "properties": [{"name": "checked", "value": {"value": "true"}}, {"name": "disabled", "value": {"value": true}}]},
            {"nodeId": "7", "role": {"value": "StaticText"}, "name": {"value": "Create your account"}},
            {"nodeId": "8", "role": {"value": "button"}, "name": {"value": "Create \"account\""}, "backendDOMNodeId": 13},
            {"nodeId": "9", "role": {"value": "StaticText"}, "name": {"value": "Already have an account?"}, "ignored": false}
        ]})
    }

    #[test]
    fn outline_shows_roles_names_refs_and_masks_passwords() {
        let nodes = parse_ax_nodes(&tree());
        let mut refs = RefMap::default();
        let options = RenderOptions {
            passwords: [12].into(),
            ..Default::default()
        };
        let outline = render(&nodes, &mut refs, &options);
        assert_eq!(
            outline.text,
            "- heading \"Create your account\" [level=2]\n- textbox \"Email\" [e1]: test@example.com\n- textbox \"Password\" [e2]: ••••••\n- button \"Create 'account'\" [e3]\n- checkbox \"Remember me\" [e4] [checked] [disabled]\n- text: Already have an account?"
        );
        assert!(!outline.text.contains("hunter2"));
        // Refs stay stable on the next snapshot and resolve to their DOM node.
        assert_eq!(render(&nodes, &mut refs, &options), outline);
        assert_eq!(refs.resolve("e3"), Some(13));
        assert_eq!(refs.resolve("3"), Some(13));
        assert_eq!(refs.resolve("e99"), None);
        assert_eq!(refs.of(13).as_deref(), Some("e3"));
    }

    #[test]
    fn a_subtree_and_a_small_budget_are_respected() {
        let nodes = parse_ax_nodes(&tree());
        let mut refs = RefMap::default();
        let part = render(
            &nodes,
            &mut refs,
            &RenderOptions {
                root: Some(13),
                ..Default::default()
            },
        );
        assert_eq!(part.text, "- button \"Create 'account'\" [e1]");
        let small = render(
            &nodes,
            &mut RefMap::default(),
            &RenderOptions {
                limit: 60,
                ..Default::default()
            },
        );
        assert!(small.truncated);
        assert!(small.text.len() <= 60);
        assert!(small.omitted_lines > 0);
    }

    #[test]
    fn cycles_and_dangling_children_do_not_loop() {
        let nodes = parse_ax_nodes(&json!({"nodes": [
            {"nodeId": "a", "role": {"value": "list"}, "childIds": ["b", "missing"]},
            {"nodeId": "b", "role": {"value": "listitem"}, "childIds": ["a"]}
        ]}));
        let outline = render(&nodes, &mut RefMap::default(), &RenderOptions::default());
        // Every node is someone's child: rendering starts at the first, once.
        assert_eq!(outline.text, "- list\n  - listitem");
        let nodes = parse_ax_nodes(&json!({"nodes": [
            {"nodeId": "r", "role": {"value": "list"}, "childIds": ["b"]},
            {"nodeId": "b", "role": {"value": "listitem"}, "name": {"value": "x"}, "childIds": ["r", "b"]}
        ]}));
        let outline = render(&nodes, &mut RefMap::default(), &RenderOptions::default());
        assert_eq!(outline.text, "- list\n  - listitem \"x\"");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn any_tree_renders_within_budget(
            shape in proptest::collection::vec((0usize..12, "(textbox|button|link|generic|heading|StaticText|[a-z]{0,8})", "[a-z ]{0,30}", proptest::option::of("[0-9]{4,10}"), proptest::option::of(0i64..20)), 0..40),
            limit in 1usize..400,
        ) {
            let count = shape.len().max(1);
            let nodes: Vec<Value> = shape.iter().enumerate().map(|(i, (child, role, name, value, backend))| json!({
                "nodeId": i.to_string(),
                "role": {"value": role},
                "name": {"value": name},
                "value": value.as_ref().map(|v| json!({"value": v})),
                "childIds": [(child % count).to_string(), ((i + 1) % count).to_string()],
                "backendDOMNodeId": backend,
            })).collect();
            let parsed = parse_ax_nodes(&json!({"nodes": nodes}));
            let mut refs = RefMap::default();
            let passwords: HashSet<i64> = (0..20).collect();
            let outline = render(&parsed, &mut refs, &RenderOptions { passwords, limit, ..Default::default() });
            prop_assert!(outline.text.len() <= limit);
            // Every password value is masked whatever the page says (values are
            // digits, names letters, so a value can only appear as a value).
            // A field without a DOM node is not a password and may show the same digits.
            let shown: HashSet<&str> = shape
                .iter()
                .filter(|(_, _, _, _, backend)| backend.is_none())
                .filter_map(|(_, _, _, value, _)| value.as_deref())
                .collect();
            for (_, _, _, value, backend) in &shape {
                if let (Some(value), Some(_)) = (value, backend) {
                    if shown.iter().any(|other| other.contains(value.as_str())) {
                        continue;
                    }
                    prop_assert!(!outline.text.contains(value.as_str()));
                }
            }
        }

        #[test]
        fn any_json_parses_without_panicking(text in ".{0,300}") {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                let nodes = parse_ax_nodes(&value);
                let _ = render(&nodes, &mut RefMap::default(), &RenderOptions::default());
            }
        }
    }
}
