//! The live check's coverage table (`docs/live-verification.md`) has a row for every agent
//! tool, Settings section, Workspace tab, main view and `runtime.jsonc` group, and none for one
//! that no longer exists, so a merged feature always comes with its live-check step.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A file's text with `\n` line ends, whatever the checkout used.
fn read_at(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .replace("\r\n", "\n")
}

fn read(path: &str) -> String {
    read_at(&root().join(path))
}

/// Every `"..."` literal in `text`, in order.
fn quoted(text: &str) -> Vec<String> {
    text.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// The text from the first `start` up to the next `end` after it.
fn between<'a>(text: &'a str, start: &str, end: &str) -> &'a str {
    let from = text
        .find(start)
        .unwrap_or_else(|| panic!("missing {start:?}"))
        + start.len();
    let to = text[from..]
        .find(end)
        .unwrap_or_else(|| panic!("missing {end:?}"));
    &text[from..from + to]
}

fn rust_sources(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// Every tool a run can offer, from where each kind of tool declares its name.
fn tool_names() -> BTreeSet<String> {
    // The toolkit's file tools and the per-run tools, as `skills.rs` lists them; `browser`
    // there stands for the `browser_*` tools, which are read from their own declarations.
    let mut names: BTreeSet<String> = quoted(between(
        &read("crates/core/src/skills.rs"),
        "const KNOWN_TOOL_NAMES",
        "];",
    ))
    .into_iter()
    .filter(|name| name != "browser")
    .collect();
    let mut sources = Vec::new();
    for directory in ["crates/core/src", "crates/desktop/src"] {
        rust_sources(&root().join(directory), &mut sources);
    }
    for source in sources {
        let text = read_at(&source);
        // Stub tools in test modules are not offered.
        let text = text.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
        // `#[tool(name = "...", ...)]`
        for declaration in text.split("#[tool(").skip(1) {
            let declaration = &declaration[..declaration.find(")]").unwrap_or(declaration.len())];
            if let Some(name) = declaration.split("name = \"").nth(1) {
                names.insert(name[..name.find('"').unwrap()].to_owned());
            }
        }
        // `impl ToolT for X { fn name(&self) -> &str { "..." } }`
        for implementation in text.split("impl ToolT for ").skip(1) {
            let body = between(implementation, "fn name(&self) -> &str {", "}");
            if let Some(name) = quoted(body).first() {
                names.insert(name.clone());
            }
        }
    }
    // The integration tools share one implementation, named from a list.
    let integrations = read("crates/desktop/src/state/integration_tools.rs");
    let list_end = integrations.find("Box::new(ManagementTool").unwrap();
    let list_start = integrations[..list_end].rfind('[').unwrap();
    names.extend(quoted(&integrations[list_start..list_end]));
    names
}

/// The ids of a TypeScript `[[id, label], ...]` list on one line.
fn pair_ids(line: &str) -> BTreeSet<String> {
    quoted(line).into_iter().step_by(2).collect()
}

fn line_with<'a>(text: &'a str, needle: &str) -> &'a str {
    text.lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("missing {needle:?}"))
}

fn expected() -> BTreeSet<String> {
    fn tag(prefix: &str, names: BTreeSet<String>) -> impl Iterator<Item = String> + '_ {
        names
            .into_iter()
            .map(move |name| format!("{prefix}:{name}"))
    }
    let mut ids = BTreeSet::new();
    ids.extend(tag("tool", tool_names()));
    let settings = read("web/src/screens/SettingsNav.tsx");
    ids.extend(tag(
        "settings",
        quoted(line_with(&settings, "export const SETTINGS_SECTIONS"))
            .into_iter()
            .collect(),
    ));
    let workspace = read("web/src/screens/WorkspacePanel.tsx");
    ids.extend(tag(
        "workspace",
        pair_ids(line_with(&workspace, "const TABS")),
    ));
    let sidebar = read("web/src/screens/Sidebar.tsx");
    ids.extend(tag("view", pair_ids(line_with(&sidebar, "const views"))));
    let reference = read("crates/core/src/configuration_reference.jsonc");
    let groups = reference
        .lines()
        .filter_map(|line| line.strip_prefix("  \""))
        .filter_map(|rest| rest.split_once("\":").map(|(key, _)| key.to_owned()))
        .filter(|key| key != "schema_version")
        .collect();
    ids.extend(tag("config", groups));
    ids
}

/// The table rows: (id, the code paths named in the Code column).
fn table() -> Vec<(String, Vec<String>)> {
    read("docs/live-verification.md")
        .lines()
        .filter(|line| line.starts_with("| `"))
        .map(|line| {
            let cells: Vec<&str> = line.split('|').collect();
            let id = between(cells[1], "`", "`").to_owned();
            let code = cells[3]
                .split('`')
                .skip(1)
                .step_by(2)
                .map(str::to_owned)
                .collect();
            (id, code)
        })
        .collect()
}

#[test]
fn every_feature_has_a_live_check_row_and_every_row_a_feature() {
    let rows = table();
    let listed: BTreeSet<String> = rows.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(listed.len(), rows.len(), "a coverage row is listed twice");
    let expected = expected();
    let missing: Vec<_> = expected.difference(&listed).collect();
    assert!(
        missing.is_empty(),
        "add a row to the coverage table in docs/live-verification.md for: {missing:?}"
    );
    let stale: Vec<_> = listed
        .iter()
        .filter(|id| !id.starts_with("feature:") && !expected.contains(*id))
        .collect();
    assert!(
        stale.is_empty(),
        "these coverage rows name nothing that exists any more: {stale:?}"
    );
    for (id, code) in &rows {
        assert!(!code.is_empty(), "{id} names no code");
        for path in code {
            assert!(root().join(path).exists(), "{id}: {path} does not exist");
        }
    }
}

#[test]
fn the_source_lists_are_read() {
    // Guards the readers above: a refactor that hides a list must not empty the check.
    let expected = expected();
    for (prefix, at_least) in [
        ("tool:", 30),
        ("settings:", 9),
        ("workspace:", 5),
        ("view:", 3),
        ("config:", 11),
    ] {
        let count = expected.iter().filter(|id| id.starts_with(prefix)).count();
        assert!(count >= at_least, "only {count} {prefix} ids were found");
    }
    for tool in [
        "read_file",
        "shell",
        "set_goal",
        "ask_user",
        "manage_automations",
        "browser_devtools",
    ] {
        assert!(
            expected.contains(&format!("tool:{tool}")),
            "{tool} was not found"
        );
    }
}
