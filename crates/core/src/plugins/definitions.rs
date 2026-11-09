//! The exact JSON shape of a skill, a hook and an MCP server definition: the reference the
//! `themis` CLI prints with `--help` and in its errors, and a check that reports every problem in
//! a definition at once (missing, mistyped and unknown fields, then the store's own validation).

use super::{Hook, McpServer, PluginSpec};
use crate::skills::Skill;
use serde_json::Value;

/// A JSON type a field takes.
#[derive(Clone, Copy)]
enum Kind {
    Text,
    Flag,
    Whole,
    Texts,
    TextMap,
    Object(&'static [Field]),
    Objects(&'static [Field]),
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Flag => "true/false",
            Self::Whole => "whole number",
            Self::Texts => "list of text",
            Self::TextMap => "object of text",
            Self::Object(_) => "object",
            Self::Objects(_) => "list of objects",
        }
    }

    fn matches(self, value: &Value) -> bool {
        match self {
            Self::Text => value.is_string(),
            Self::Flag => value.is_boolean(),
            Self::Whole => value
                .as_f64()
                .is_some_and(|number| number >= 0.0 && number.fract() == 0.0),
            Self::Texts => value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string)),
            Self::TextMap => value
                .as_object()
                .is_some_and(|items| items.values().all(Value::is_string)),
            Self::Object(_) => value.is_object(),
            Self::Objects(_) => value.is_array(),
        }
    }
}

/// One field of a definition.
#[derive(Clone, Copy)]
struct Field {
    name: &'static str,
    kind: Kind,
    required: bool,
    about: &'static str,
}

const fn field(name: &'static str, kind: Kind, about: &'static str) -> Field {
    Field {
        name,
        kind,
        required: false,
        about,
    }
}

const fn required(name: &'static str, kind: Kind, about: &'static str) -> Field {
    Field {
        name,
        kind,
        required: true,
        about,
    }
}

const SCRIPT: &[Field] = &[
    required(
        "name",
        Kind::Text,
        "File name only: letters, digits, '_', '.', '-', no leading dot.",
    ),
    required(
        "content",
        Kind::Text,
        "The script's text. All of a skill's scripts together: at most 64 KiB.",
    ),
];

const SKILL: &[Field] = &[
    required("id", Kind::Text, "A safe single name (letters, digits, '_', '.', '-', no leading dot, no '--', at most 100 characters), unique in its plugin."),
    required("name", Kind::Text, "Shown in the prompt and the composer's / menu."),
    field("description", Kind::Text, "One line for pickers and listings. Default: empty."),
    required("instructions", Kind::Text, "The skill's Markdown instructions, at most 256 KiB."),
    field("allowedTools", Kind::Texts, "Tool names the skill may use. Default: [] (restricts nothing and never grants a permission)."),
    field("scripts", Kind::Objects(SCRIPT), "Helper scripts {\"name\", \"content\"}, written to .themis/skills/<id>/. Default: []."),
];

const REWAKE: &[Field] = &[
    field(
        "message",
        Kind::Text,
        "The line above the report, for the model.",
    ),
    field(
        "summary",
        Kind::Text,
        "A short description of it, for the person.",
    ),
];

const HOOK: &[Field] = &[
    required("name", Kind::Text, "A safe single name, unique in its plugin (`hook add|update PLUGIN NAME FILE` sets it from NAME)."),
    required("event", Kind::Text, "When it runs; one of the events listed below."),
    field("command", Kind::Text, "The shell command to run; it gets the event as JSON on stdin. Required unless `prompt` is given."),
    field("prompt", Kind::Text, "Ask the chat's model this instead of running a command ($ARGUMENTS is the input). Leave `command` out then."),
    field("matcher", Kind::Text, "Tool events only: '|'-separated tool names, such as \"shell|apply_patch\". Default: every tool."),
    field("timeout_seconds", Kind::Whole, "1 to 600. Default: 10."),
    field("blocking", Kind::Flag, "A hook that fails also stops the action. Default: false."),
    field("failure_policy", Kind::Text, "\"ignore\" or \"abort\"."),
    field("background", Kind::Flag, "Runs without being waited for; it cannot block. Default: false."),
    field("command_prefix", Kind::Texts, "Only for shell calls whose command starts with these words."),
    field("command_glob", Kind::Text, "Only for shell calls whose command matches this pattern ('*' is any text)."),
    field("moment_matcher", Kind::Text, "Events not about a tool: what it must be about (\"resume\" on SessionStart, a skill's name on UserPromptExpansion)."),
    field("exit_two_blocks", Kind::Flag, "Exit code 2 asks to block, with the reason on stderr. Default: false."),
    field("rewake", Kind::Object(REWAKE), "Background hooks only: when it exits 2, what it wrote reaches the agent. {\"message\", \"summary\"}."),
    field("shell", Kind::Text, "Only \"bash\". Default: the platform's shell."),
    field("enabled", Kind::Flag, "A new or changed hook is saved switched off; the person switches it on."),
];

const OAUTH: &[Field] = &[
    field("client_id", Kind::Text, "A preregistered client."),
    field(
        "callback_port",
        Kind::Whole,
        "The local sign-in port, 1024 or above.",
    ),
    field(
        "auth_server_metadata_url",
        Kind::Text,
        "Where the authorization server's metadata is.",
    ),
    field("scopes", Kind::Text, "Space-separated scopes."),
    field(
        "client_secret_env",
        Kind::Text,
        "The NAME of the secret holding the client's secret.",
    ),
];

const MCP: &[Field] = &[
    field("command", Kind::Text, "A local server's program. Give `command` or `url`, not both."),
    field("args", Kind::Texts, "The program's arguments. Default: []."),
    field("cwd", Kind::Text, "Local servers only: a folder inside the project, or ${CLAUDE_PLUGIN_ROOT}/..."),
    field("env", Kind::TextMap, "Environment variable -> the NAME of the secret it reads (never a value): {\"API_TOKEN\": \"MY_TOKEN\"}."),
    field("env_values", Kind::TextMap, "Environment variable -> a plain, non-secret value."),
    field("url", Kind::Text, "A remote server's https:// address. Give `command` or `url`, not both."),
    field("sse", Kind::Flag, "Remote servers on the older SSE transport. Default: false."),
    field("bearer_env", Kind::Text, "Remote servers only: the NAME of the secret sent as a bearer token."),
    field("headers", Kind::TextMap, "Remote servers only: header -> \"prefix${NAME}\", NAME being a secret."),
    field("header_values", Kind::TextMap, "Remote servers only: header -> a plain, non-secret value."),
    field("defaults", Kind::TextMap, "NAME -> a plain value used when that key is not set."),
    field("oauth", Kind::Object(OAUTH), "Remote servers only: how it signs in."),
    field("tool_timeout_secs", Kind::Whole, "How long one tool call may run, 1 to 3600. Default: 300."),
    field("enabled", Kind::Flag, "A new or changed server that runs a command is saved switched off; the person switches it on."),
];

/// The top-level fields of a whole plugin (`themis plugin save FILE`).
const PLUGIN_FIELDS: &[&str] = &[
    "package_kind",
    "disabled_skills",
    "manual_skills",
    "origin",
    "skill_paths",
    "icon",
    "icon_asset",
    "brand",
    "name",
    "description",
    "version",
    "package_hash",
    "skills",
    "mcp",
    "hooks",
    "subagents",
    "starts_disabled",
    "files",
    "package",
    "executable_files",
    "unsupported",
    "notices",
    "import_issues",
];

/// A definition the CLI saves into a plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Definition {
    Skill,
    Hook,
    Mcp,
}

impl Definition {
    fn fields(self) -> &'static [Field] {
        match self {
            Self::Skill => SKILL,
            Self::Hook => HOOK,
            Self::Mcp => MCP,
        }
    }

    /// What it is called in messages.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Hook => "hook",
            Self::Mcp => "MCP server",
        }
    }

    fn command(self) -> &'static str {
        match self {
            Self::Skill => "themis skill save PLUGIN FILE",
            Self::Hook => "themis hook add|update PLUGIN NAME FILE",
            Self::Mcp => "themis mcp add|update PLUGIN NAME FILE",
        }
    }

    fn example(self) -> &'static str {
        match self {
            Self::Skill => {
                r#"{
  "id": "release-notes",
  "name": "Release notes",
  "description": "Drafts release notes from merged changes",
  "instructions": "Read the changes since the last tag and draft short release notes.",
  "allowedTools": ["read_file", "shell"],
  "scripts": []
}"#
            }
            Self::Hook => {
                r#"{
  "event": "BeforeTool",
  "matcher": "shell",
  "command": "python3 ~/hooks/check_command.py",
  "timeout_seconds": 10,
  "blocking": false
}"#
            }
            Self::Mcp => {
                r#"{"command": "node", "args": ["server.js"], "cwd": "tools/mcp"}
{"url": "https://example.com/mcp", "bearer_env": "EXAMPLE_TOKEN"}"#
            }
        }
    }

    /// The exact shape: every field with its type, whether it is required, and an example.
    #[must_use]
    pub fn reference(self) -> String {
        let article = if self == Self::Mcp { "An" } else { "A" };
        let mut text = format!(
            "{article} {} definition is one JSON object ({}). Fields:\n",
            self.label(),
            self.command()
        );
        render_fields(&mut text, self.fields(), "  ");
        match self {
            Self::Skill => text.push_str(&format!(
                "Tool names for allowedTools: {}.\n",
                crate::skills::known_tool_names().join(", ")
            )),
            Self::Hook => text.push_str(&format!(
                "Events: {}.\n",
                super::hooks::EVENTS.join(", ")
            )),
            Self::Mcp => text.push_str(
                "Exactly one of `command` (a local server) and `url` (a remote one) is required.\nA standard .mcp.json file ({\"mcpServers\": {...}}) is imported with `themis mcp import FILE PLUGIN` instead.\n",
            ),
        }
        text.push_str("Example:\n");
        text.push_str(self.example());
        text
    }

    /// Every problem with `value` as this definition: first its shape (missing, mistyped and
    /// unknown fields), then, when the shape is right, the store's own validation.
    #[must_use]
    pub fn problems(self, value: &Value) -> Vec<String> {
        let mut problems = Vec::new();
        shape_problems(self.fields(), value, "", &mut problems);
        if self == Self::Mcp && (value.get("mcpServers").is_some() || value.get("type").is_some()) {
            problems.push(
                "this looks like a standard .mcp.json entry; import it with `themis mcp import FILE PLUGIN`".into(),
            );
        }
        if problems.is_empty() {
            problems = self.store_problems(value);
        }
        problems
    }

    fn store_problems(self, value: &Value) -> Vec<String> {
        let parsed = match self {
            Self::Skill => serde_json::from_value::<Skill>(value.clone()).map(|skill| {
                let mut problems = Vec::new();
                if !super::safe_name(&skill.id) {
                    problems.push(
                        "id must be a safe single name: letters, digits, '_', '.', '-'".to_owned(),
                    );
                }
                problems.extend(crate::skills::skill_input_problems(
                    &skill.name,
                    &skill.instructions,
                    &skill.allowed_tools,
                    &skill.scripts,
                ));
                problems
            }),
            Self::Hook => serde_json::from_value::<Hook>(value.clone()).map(|hook| hook.problems()),
            Self::Mcp => {
                serde_json::from_value::<McpServer>(value.clone()).map(|server| server.problems())
            }
        };
        parsed.unwrap_or_else(|error| vec![error.to_string()])
    }
}

/// Every problem with a whole plugin definition (`themis plugin save FILE`): its components'
/// shapes, then the store's validation of the whole.
#[must_use]
pub fn plugin_problems(value: &Value) -> Vec<String> {
    let Some(object) = value.as_object() else {
        return vec!["the plugin definition must be a JSON object".into()];
    };
    let mut problems = Vec::new();
    if !object.get("name").is_some_and(Value::is_string) {
        problems.push("name: missing (required text)".into());
    }
    for key in object.keys() {
        if !PLUGIN_FIELDS.contains(&key.as_str()) {
            problems.push(unknown(key, PLUGIN_FIELDS.iter().copied(), ""));
        }
    }
    component_shapes(object, &mut problems);
    if problems.is_empty() {
        problems = match serde_json::from_value::<PluginSpec>(value.clone()) {
            Ok(spec) => super::validation::problems(&spec),
            Err(error) => vec![error.to_string()],
        };
    }
    problems
}

fn component_shapes(object: &serde_json::Map<String, Value>, problems: &mut Vec<String>) {
    let lists = [("skills", Definition::Skill), ("hooks", Definition::Hook)];
    for (key, definition) in lists {
        match object.get(key) {
            None => {}
            Some(Value::Array(items)) => {
                for (index, item) in items.iter().enumerate() {
                    let at = format!("{key}[{index}].");
                    shape_problems(definition.fields(), item, &at, problems);
                }
            }
            Some(_) => problems.push(format!("{key}: must be a list of objects")),
        }
    }
    match object.get("mcp") {
        None => {}
        Some(Value::Object(servers)) => {
            for (name, server) in servers {
                shape_problems(MCP, server, &format!("mcp.{name}."), problems);
            }
        }
        Some(_) => problems.push("mcp: must be an object of name -> server".into()),
    }
}

/// `Ok` when there is no problem, else one error naming them all, separated by `; `.
pub fn none(problems: Vec<String>) -> anyhow::Result<()> {
    if problems.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("{}", problems.join("; ")))
    }
}

/// One problem list as one message: what it is about, then a line per problem.
#[must_use]
pub fn report(what: &str, problems: &[String]) -> String {
    match problems {
        [only] => format!("{what} has a problem: {only}"),
        _ => {
            let mut text = format!("{what} has {} problems:", problems.len());
            for problem in problems {
                text.push_str("\n  - ");
                text.push_str(problem);
            }
            text
        }
    }
}

fn shape_problems(fields: &[Field], value: &Value, at: &str, problems: &mut Vec<String>) {
    let Some(object) = value.as_object() else {
        problems.push(format!(
            "{}: must be a JSON object",
            at.trim_end_matches('.')
        ));
        return;
    };
    for field in fields {
        let name = format!("{at}{}", field.name);
        match object.get(field.name) {
            None | Some(Value::Null) if field.required => {
                problems.push(format!("{name}: missing (required {})", field.kind.label()));
            }
            None | Some(Value::Null) => {}
            Some(found) if !field.kind.matches(found) => problems.push(format!(
                "{name}: must be {}, not {}",
                field.kind.label(),
                json_type(found)
            )),
            Some(found) => nested_problems(field.kind, found, &name, problems),
        }
    }
    for key in object.keys() {
        let known = fields.iter().any(|field| field.name == key)
            || (key == "allowed_tools" && fields.iter().any(|f| f.name == "allowedTools"));
        if !known {
            problems.push(unknown(key, fields.iter().map(|field| field.name), at));
        }
    }
}

fn nested_problems(kind: Kind, value: &Value, name: &str, problems: &mut Vec<String>) {
    match (kind, value) {
        (Kind::Object(fields), _) => shape_problems(fields, value, &format!("{name}."), problems),
        (Kind::Objects(fields), Value::Array(items)) => {
            for (index, item) in items.iter().enumerate() {
                shape_problems(fields, item, &format!("{name}[{index}]."), problems);
            }
        }
        _ => {}
    }
}

/// An unknown field, with the known one it most likely means (`timeoutSeconds` for
/// `timeout_seconds`, `allowed_tools` for `allowedTools`).
fn unknown<'a>(key: &str, known: impl Iterator<Item = &'a str>, at: &str) -> String {
    let squash = |text: &str| text.replace(['_', '-'], "").to_ascii_lowercase();
    let wanted = squash(key);
    let names: Vec<&str> = known.collect();
    match names.iter().find(|name| squash(name) == wanted) {
        Some(meant) => format!("{at}{key}: unknown field; did you mean `{meant}`?"),
        None => format!(
            "{at}{key}: unknown field; known fields: {}",
            names.join(", ")
        ),
    }
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "true/false",
        Value::Number(_) => "a number",
        Value::String(_) => "text",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

fn render_fields(text: &mut String, fields: &[Field], indent: &str) {
    for field in fields {
        let need = if field.required {
            "required"
        } else {
            "optional"
        };
        text.push_str(&format!(
            "{indent}{:<17} {}, {need}. {}\n",
            field.name,
            field.kind.label(),
            field.about
        ));
        match field.kind {
            Kind::Object(inner) | Kind::Objects(inner) => {
                render_fields(text, inner, &format!("{indent}    "));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_problem_in_a_skill_is_reported_at_once() {
        let problems = Definition::Skill.problems(&json!({
            "id": "bad id", "instructions": 5, "allowed_tools": ["read_file"], "tools": []
        }));
        let text = problems.join("\n");
        assert!(text.contains("name: missing (required text)"), "{text}");
        assert!(
            text.contains("instructions: must be text, not a number"),
            "{text}"
        );
        assert!(text.contains("tools: unknown field"), "{text}");
        assert_eq!(problems.len(), 3, "{text}");
    }

    #[test]
    fn a_right_shape_goes_on_to_the_store_checks_and_lists_all_of_them() {
        let problems = Definition::Hook.problems(&json!({
            "name": "guard", "event": "Nope", "command": "", "timeout_seconds": 900
        }));
        let text = problems.join("\n");
        assert!(text.contains("event 'Nope' is not an event"), "{text}");
        assert!(text.contains("command is missing"), "{text}");
        assert!(
            text.contains("timeout_seconds 900 must be from 1 to 600"),
            "{text}"
        );
    }

    #[test]
    fn a_misspelled_field_names_the_one_it_means() {
        let problems = Definition::Hook.problems(&json!({
            "name": "guard", "event": "RunStart", "command": "true", "timeoutSeconds": 5
        }));
        assert_eq!(
            problems,
            ["timeoutSeconds: unknown field; did you mean `timeout_seconds`?"]
        );
    }

    #[test]
    fn a_standard_mcp_entry_points_to_import() {
        let problems = Definition::Mcp.problems(&json!({"type": "stdio", "command": "node"}));
        assert!(
            problems.iter().any(|p| p.contains("themis mcp import")),
            "{problems:?}"
        );
    }

    #[test]
    fn lists_are_optional_and_a_minimal_skill_has_no_problem() {
        let skill = json!({"id": "notes", "name": "Notes", "instructions": "Write notes."});
        assert!(Definition::Skill.problems(&skill).is_empty());
        let parsed: Skill = serde_json::from_value(skill).unwrap();
        assert!(parsed.allowed_tools.is_empty() && parsed.scripts.is_empty());
    }

    #[test]
    fn every_reference_names_every_field_and_parses_its_example() {
        for definition in [Definition::Skill, Definition::Hook, Definition::Mcp] {
            let reference = definition.reference();
            for field in definition.fields() {
                assert!(reference.contains(field.name), "{}", field.name);
            }
            assert!(reference.contains("required") && reference.contains("optional"));
            for example in definition
                .example()
                .lines()
                .filter(|l| l.starts_with('{') && l.ends_with('}'))
            {
                let value: Value = serde_json::from_str(example).unwrap();
                assert!(definition.problems(&value).is_empty(), "{example}");
            }
            if definition != Definition::Mcp {
                let value: Value = serde_json::from_str(definition.example()).unwrap();
                let mut value = value;
                if definition == Definition::Hook {
                    value["name"] = json!("check");
                }
                assert!(definition.problems(&value).is_empty(), "{value}");
            }
        }
    }

    #[test]
    fn a_whole_plugin_reports_problems_across_its_components() {
        let problems = plugin_problems(&json!({
            "name": "tools",
            "skills": [{"id": "a", "name": "A"}],
            "hooks": [{"name": "h", "command": "true"}],
            "mcp": {"s": {"command": "node", "args": "x"}},
            "colour": "blue"
        }));
        let text = problems.join("\n");
        for wanted in [
            "skills[0].instructions: missing",
            "hooks[0].event: missing",
            "mcp.s.args: must be list of text",
            "colour: unknown field",
        ] {
            assert!(text.contains(wanted), "{wanted} in {text}");
        }
    }

    #[test]
    fn report_lists_each_problem_on_its_own_line() {
        assert_eq!(
            report("The skill", &["a".into()]),
            "The skill has a problem: a"
        );
        assert_eq!(
            report("The skill", &["a".into(), "b".into()]),
            "The skill has 2 problems:\n  - a\n  - b"
        );
    }
}
