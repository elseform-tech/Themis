//! Browser protocol messages and accessibility trees come from web pages:
//! parsing either must never panic, a snapshot stays within its budget, and
//! a password field's value never appears in it.
#![no_main]
use std::collections::HashSet;

use libfuzzer_sys::fuzz_target;
use themis_core::browser::cdp::parse_incoming;
use themis_core::browser::snapshot::{parse_ax_nodes, render, RefMap, RenderOptions};

fuzz_target!(|data: &[u8]| {
    let Some((&budget, body)) = data.split_first() else {
        return;
    };
    let text = String::from_utf8_lossy(body);
    let _ = parse_incoming(&text);
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let nodes = parse_ax_nodes(&value);
    let passwords: HashSet<i64> = nodes.iter().filter_map(|node| node.backend_id).collect();
    let limit = usize::from(budget) * 8 + 1;
    let outline = render(
        &nodes,
        &mut RefMap::default(),
        &RenderOptions {
            passwords,
            root: None,
            limit,
        },
    );
    assert!(outline.text.len() <= limit);
    // Every node with a DOM id is a password field here: no element line may
    // show its value (text lines print names, which the page controls).
    let element_lines: Vec<&str> = outline
        .text
        .lines()
        .filter(|line| !line.trim_start().starts_with("- text:"))
        .collect();
    for node in &nodes {
        if let (Some(value), Some(_)) = (&node.value, node.backend_id) {
            if value.chars().count() >= 8 && value.chars().all(|c| c.is_ascii_alphanumeric()) {
                let shown = format!(": {value}");
                assert!(
                    element_lines.iter().all(|line| !line.ends_with(&shown) || line.contains(&format!("\"{value}\""))),
                    "password value shown"
                );
            }
        }
    }
});
