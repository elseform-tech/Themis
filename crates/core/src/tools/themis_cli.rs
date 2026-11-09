//! The risk of a `themis` CLI call in an agent's shell (see [`crate::agent_cli`]).
//!
//! The CLI goes through the live app server, which refuses an agent's calls outside its allowed
//! list in every mode. What it allows still passes the shell's approval, at the level this module
//! gives it: reads (`plugin list`, `status`, `doctor`, `recall`, `memory search`) are reads, changes are writes or network,
//! tests that start code are executions, removals are destructive, and switching on a hook or an
//! MCP server is a [`ThemisCall::SwitchOn`], which is asked every time, never covered by a chat
//! grant or a broad allow rule, and in Auto mode is the person's to allow, never the reviewer's.
//!
//! Only a bare `themis` (found through the run's `PATH`, whose first folder holds the app's own
//! CLI) is ever given a lower risk than any other program. A `themis` reached through a path or a
//! wrapper (`sh -c`, `env`, `xargs`, a pipeline) keeps the ordinary command risk, and the
//! destructive and switch-on checks look inside wrappers and at every word naming `themis`, so a
//! wrapper never lowers what a call needs. Words this module does not know are read as the
//! strictest call they could be: unknown integration commands as switching something on.

use super::{commands_run_by, program_basename, RiskLevel};
use serde_json::Value;

/// What a `themis` CLI call does, for the shell's approval step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemisCall {
    /// Reads status, diagnostics, settings or integrations.
    Read,
    /// Saves, imports or switches off an integration through the store.
    Write,
    /// Fetches from the network: installs, updates, marketplace sources.
    Network,
    /// Starts code (`mcp test`, `hook test`, `skill verify`) or acts outside integrations.
    Execute,
    /// Removes or uninstalls something.
    Destructive,
    /// Switches on a hook, an MCP server or a whole plugin, or could (an unknown integration
    /// command): asked every time; in Auto mode only the person may allow it.
    SwitchOn,
}

impl ThemisCall {
    /// The risk class the approval step uses.
    #[must_use]
    pub fn risk(self) -> RiskLevel {
        match self {
            Self::Read => RiskLevel::Read,
            Self::Write => RiskLevel::Write,
            Self::Network => RiskLevel::Network,
            Self::Execute | Self::SwitchOn => RiskLevel::Execute,
            Self::Destructive => RiskLevel::Destructive,
        }
    }
}

/// App-server methods an agent's credential may call that only read (the server's allowed list
/// without `plugin_action`; a desktop test keeps the two in step).
pub const AGENT_READ_METHODS: &[&str] = &[
    "ping",
    "get_diagnostics",
    "query_diagnostic_logs",
    "get_effective_permissions",
    "get_usage_summary",
    "list_go_models",
    "list_model_providers",
    "get_secret_status",
    "mcp_config_status",
    "resource_usage",
    "get_runtime_configuration",
    "get_settings",
    "get_browser_settings",
    "get_web_settings",
    "get_subagent_settings",
    "get_memory_settings",
    "list_prompt_skills",
    "list_skills",
    "get_thread",
    "get_thread_history_page",
    "get_thread_tasks",
    "get_background_task_log",
    "get_context_usage",
    "get_chat_usage",
    // The run's own chat's saved originals and memory (see `agent_cli::CHAT_READ_TOOLS`).
    "recall",
    "search_memory",
];

/// Whether `program` names the `themis` CLI, wherever it lives.
fn names_themis(program: &str) -> bool {
    matches!(
        program_basename(program).as_str(),
        "themis" | "themis-desktop"
    )
}

/// What `command` does when it runs the `themis` CLI (`themis …`, or the desktop app started as
/// the CLI with `--themis-cli`); `None` for any other program.
#[must_use]
pub fn classify(command: &[String]) -> Option<ThemisCall> {
    let (program, mut words) = command.split_first()?;
    let name = program_basename(program);
    let cli_flag = words.first().is_some_and(|word| word == "--themis-cli");
    if !(name == "themis" || (name == "themis-desktop" && cli_flag)) {
        return None;
    }
    if cli_flag {
        words = &words[1..];
    }
    if words.first().is_some_and(|word| word == "--data-dir") {
        let Some(rest) = words.get(2..) else {
            return Some(ThemisCall::Execute);
        };
        words = rest;
    }
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    // The CLI prints its help and does nothing else when any word asks for it.
    if words.iter().any(|word| matches!(*word, "--help" | "-h")) {
        return Some(ThemisCall::Read);
    }
    Some(top_level(&words))
}

fn top_level(words: &[&str]) -> ThemisCall {
    use ThemisCall::*;
    match words {
        [] | ["--version"] => Read,
        ["status" | "models"] | ["auth", "status"] | ["usage"] | ["usage", "--json"] => Read,
        ["doctor", flags @ ..] if flags.iter().all(|f| matches!(*f, "--json" | "--deep")) => Read,
        ["doctor", flags @ ..] => doctor_integrations(flags),
        ["permissions", ..] => Read,
        // Search this chat's saved originals, or what Themis remembers: reads only, whatever the
        // words (they are keywords or a name).
        ["recall", ..] | ["memory", "search" | "show", ..] => Read,
        ["plugin" | "skill" | "mcp" | "hook", ..] => integration(words),
        ["call", method] => call(method, None),
        ["call", method, args] => call(method, Some(args)),
        // Starting runs, servers, the window or the person's own setup: never a read.
        _ => Execute,
    }
}

/// `themis doctor --integrations [--json] [--no-start] [--project PATH]`: it starts each
/// switched-on MCP server to list its tools, so it is an execution; with `--no-start` it only
/// reads. Anything else after `doctor` keeps the ordinary risk.
fn doctor_integrations(flags: &[&str]) -> ThemisCall {
    let mut integrations = false;
    let mut starts = true;
    let mut index = 0;
    while let Some(flag) = flags.get(index) {
        match *flag {
            "--integrations" => integrations = true,
            "--json" => {}
            "--no-start" => starts = false,
            "--project" if flags.get(index + 1).is_some() => index += 1,
            _ => return ThemisCall::Execute,
        }
        index += 1;
    }
    if integrations && !starts {
        ThemisCall::Read
    } else {
        ThemisCall::Execute
    }
}

/// `themis call METHOD [JSON_ARGS|-]`.
fn call(method: &str, args: Option<&str>) -> ThemisCall {
    if method == "plugin_action" {
        let parsed = match args {
            None => Some(Value::Object(Default::default())),
            // `-` reads its arguments from stdin, which nobody sees here.
            Some(text) => serde_json::from_str::<Value>(text).ok(),
        };
        return match parsed {
            Some(args @ Value::Object(_)) => plugin_action(&args),
            _ => ThemisCall::SwitchOn,
        };
    }
    if AGENT_READ_METHODS.contains(&method) {
        ThemisCall::Read
    } else {
        ThemisCall::Execute
    }
}

/// An app-server `plugin_action` by its action (the server's default action is `list`).
#[must_use]
pub fn plugin_action(args: &Value) -> ThemisCall {
    use ThemisCall::*;
    match args["action"].as_str().unwrap_or("list") {
        "list"
        | "inventory"
        | "marketplaces"
        | "sources"
        | "updates"
        | "plugin_icon"
        | "mcp_tools"
        | "preview"
        | "preview_source"
        | "preview_repository"
        | "inspect_path"
        | "inspect_repository"
        | "inspect_marketplace"
        | "check_source"
        | "check_sources"
        | "catalog_check" => Read,
        "catalog" if args["refresh"] != true => Read,
        "catalog" | "import_repository" | "install" | "update" | "add_marketplace"
        | "scan_marketplace" => Network,
        "save"
        | "save_skill"
        | "save_hook"
        | "import_path"
        | "import_json"
        | "disable"
        | "set_marketplace_enabled" => Write,
        "set_component_enabled" if args["kind"] == "skill" || args["enabled"] == false => Write,
        "delete" | "remove_component" | "remove_marketplace" => Destructive,
        "test_mcp" | "test_hook" => Execute,
        // `enable`, switching on a hook or connection, and anything unknown.
        _ => SwitchOn,
    }
}

/// Options of `themis plugin|skill|mcp|hook …` that take a value.
const VALUE_OPTIONS: &[&str] = &[
    "--project",
    "--scope",
    "--expected",
    "--ref",
    "--subdirectory",
];

/// `themis plugin|skill|mcp|hook …`, read the way the CLI reads it: options anywhere, then the
/// positional words. An unknown option or shape is the strictest call it could be.
fn integration(words: &[&str]) -> ThemisCall {
    let mut positional = Vec::new();
    let mut from_repository = false;
    let mut index = 0;
    while let Some(word) = words.get(index) {
        if VALUE_OPTIONS.contains(word) {
            if words.get(index + 1).is_none() {
                return ThemisCall::SwitchOn;
            }
            from_repository |= matches!(*word, "--ref" | "--subdirectory");
            index += 2;
            continue;
        }
        // A lone `-` is a definition read from stdin, a positional word like a file path.
        if *word == "-" || !word.starts_with('-') {
            positional.push(*word);
        } else if *word != "--allow-partial" {
            return ThemisCall::SwitchOn;
        }
        index += 1;
    }
    integration_words(&positional, from_repository)
}

fn integration_words(words: &[&str], from_repository: bool) -> ThemisCall {
    use ThemisCall::*;
    match words {
        ["plugin", "import", source] | ["plugin", "import", source, _] => {
            let remote =
                from_repository || source.starts_with("https://") || source.starts_with("http://");
            if remote {
                Network
            } else {
                Write
            }
        }
        ["plugin", "inspect", _] | ["plugin", "inspect", _, _] => Read,
        // A bare `themis hook` only prints its usage.
        [_] => Read,
        ["plugin", rest @ ..] => plugin_words(rest),
        ["skill" | "mcp" | "hook", "list"] => Read,
        ["skill", "verify", _] | ["skill", "create", _, _] => Execute,
        ["skill", "save", _, _] => Write,
        [_, "list" | "show", _] => Read,
        // A definition is a file, `-` or inline JSON: one word, whichever it is.
        ["mcp" | "hook", "add" | "update", _, _, _] | ["mcp", "add", _, _] => Write,
        [_, "delete" | "remove", _, _] => Destructive,
        ["skill", "enable", _, _] | [_, "disable", _, _] => Write,
        ["mcp" | "hook", "test", _, _] => Execute,
        // `mcp enable`, `hook enable`, and anything this does not know.
        _ => SwitchOn,
    }
}

/// `themis plugin …` after the word `plugin`.
fn plugin_words(words: &[&str]) -> ThemisCall {
    use ThemisCall::*;
    match words {
        ["list" | "updates"] | ["show" | "export" | "preview", _] => Read,
        ["create" | "save" | "edit", _] | ["disable", _] => Write,
        ["install" | "update", _] => Network,
        ["delete" | "uninstall", _] => Destructive,
        ["marketplace", rest @ ..] => marketplace_words(rest),
        // `plugin enable` switches on every hook the plugin has turned on.
        _ => SwitchOn,
    }
}

/// `themis plugin marketplace …`.
fn marketplace_words(words: &[&str]) -> ThemisCall {
    use ThemisCall::*;
    match words {
        ["list"] | ["changes" | "preview" | "browse" | "check", _] => Read,
        ["add", _] | ["add", _, _] | ["refresh", _] => Network,
        ["enable" | "disable", _] => Write,
        ["remove", _] => Destructive,
        _ => SwitchOn,
    }
}

/// Every `themis` call `command` makes: itself, those inside wrappers (`sh -c`, `env`, `xargs`,
/// pipelines) and, in each, a `themis` named as a later word (`find … -exec themis …`).
/// `None` when a wrapped script cannot be read.
fn themis_calls(command: &[String]) -> Option<Vec<ThemisCall>> {
    let mut calls = Vec::new();
    for run in commands_run_by(command)? {
        for start in 0..run.len() {
            if names_themis(&run[start]) {
                calls.extend(classify(&run[start..]));
            }
        }
    }
    Some(calls)
}

/// Whether `command` switches on a hook, an MCP server or a plugin through `themis`, or could:
/// a wrapped script that cannot be read and mentions `themis` counts.
#[must_use]
pub fn switches_on_code(command: &[String]) -> bool {
    match themis_calls(command) {
        Some(calls) => calls.contains(&ThemisCall::SwitchOn),
        None => command
            .iter()
            .any(|word| word.to_ascii_lowercase().contains("themis")),
    }
}

/// Whether `command` removes something through `themis` (directly or wrapped).
#[must_use]
pub(super) fn removes_through_themis(command: &[String]) -> bool {
    themis_calls(command).is_some_and(|calls| calls.contains(&ThemisCall::Destructive))
}

/// The risk of a bare `themis …` shell call (the app's own CLI, first on the run's `PATH`);
/// `None` for anything else, which keeps the ordinary command risk.
#[must_use]
pub fn direct_risk(command: &[String]) -> Option<RiskLevel> {
    if command.first().map(String::as_str) != Some("themis") {
        return None;
    }
    classify(command).map(ThemisCall::risk)
}

#[cfg(test)]
mod tests;
