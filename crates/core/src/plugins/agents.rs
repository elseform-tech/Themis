//! Subagent profiles: the `agents/*.md` files plugins ship, kept in the plugin's registry entry
//! and named by `spawn_subagents`.
//!
//! These files come from third parties, so the reader is its own and stricter than the skill
//! one: a definition it can't read fails, and a tool list it can't read never turns into "all
//! tools". Real files do lean on loose YAML (descriptions running over unindented lines that
//! hold `Context: …`), so only known field names start a field.

use super::{safe_name, Plugin, PluginSpec};
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The tools a read-only subagent gets.
pub const READ_ONLY_TOOLS: [&str; 4] = ["read_file", "list_dir", "search_file", "shell"];
/// The tools a subagent with write paths also gets; the file guard holds them to those paths.
pub const WRITE_TOOLS: [&str; 3] = ["write_file", "apply_patch", "create_dir"];

const MAX_FRONTMATTER_BYTES: usize = 64 * 1024;
const MAX_DESCRIPTION_BYTES: usize = 1024;
const MAX_MODEL_BYTES: usize = 100;
const MAX_TOOL_NAMES: usize = 256;
const MAX_DROPPED_NAMES: usize = 32;
const MAX_DROPPED_NAME_CHARS: usize = 80;
/// What the spawn tool's description may spend on listing profiles.
const LISTING_BYTES: usize = 2048;
const LISTED_DESCRIPTION_CHARS: usize = 100;
/// What an error may spend on naming the profiles there are.
const ERROR_NAMES_BYTES: usize = 2048;

/// Frontmatter this reader knows. Anything else at the start of a line is either a description
/// running on, or a key listed as a notice.
const FIELDS: [&str; 18] = [
    "name",
    "description",
    "tools",
    "allowed-tools",
    "disallowedTools",
    "model",
    "color",
    "effort",
    "skills",
    "maxTurns",
    "memory",
    "background",
    "isolation",
    "permissionMode",
    "hooks",
    "mcpServers",
    "initialPrompt",
    "license",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentProfile {
    pub name: String,
    pub description: String,
    /// Placed before the brief.
    pub instructions: String,
    /// The Themis tools it is limited to; `None` keeps a subagent's usual ones. It only ever
    /// narrows: write tools still need write paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// The `model` frontmatter, matched against the allowed models when it starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Listed tool names with no Themis tool, left out: the profile is partial.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped_tools: Vec<String>,
}

impl SubagentProfile {
    pub fn allows(&self, tool: &str) -> bool {
        self.tools
            .as_ref()
            .is_none_or(|tools| tools.iter().any(|name| name == tool))
    }

    pub fn allows_writes(&self) -> bool {
        WRITE_TOOLS.iter().any(|tool| self.allows(tool))
    }

    pub(super) fn validate(&self) -> anyhow::Result<()> {
        if !safe_name(&self.name) {
            bail!("Subagent name must be a safe single path segment without '--'");
        }
        if self.description.trim().is_empty() || self.description.len() > MAX_DESCRIPTION_BYTES {
            bail!("Subagent description must be 1 to {MAX_DESCRIPTION_BYTES} bytes");
        }
        if self.instructions.trim().is_empty()
            || self.instructions.len() > crate::skills::MAX_INSTRUCTIONS_BYTES
        {
            bail!(
                "Subagent instructions must be 1 to {} bytes",
                crate::skills::MAX_INSTRUCTIONS_BYTES
            );
        }
        if self.tools.as_ref().is_some_and(|tools| {
            tools.iter().any(|tool| {
                !READ_ONLY_TOOLS.contains(&tool.as_str()) && !WRITE_TOOLS.contains(&tool.as_str())
            })
        }) {
            bail!("Subagent tools must be Themis file, search or shell tools");
        }
        if self.model.as_ref().is_some_and(|model| {
            model.len() > MAX_MODEL_BYTES || model.chars().any(char::is_control)
        }) {
            bail!("Invalid subagent model");
        }
        if self.dropped_tools.len() > MAX_DROPPED_NAMES {
            bail!("Too many dropped subagent tools");
        }
        Ok(())
    }
}

/// A profile read from a file, with the frontmatter it does not use.
#[derive(Debug)]
pub struct ParsedProfile {
    pub profile: SubagentProfile,
    pub unused: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum List {
    Tools,
    Allowed,
    Disallowed,
}

#[derive(Clone, Copy, PartialEq)]
enum Open {
    Nothing,
    Description,
    List(List),
}

#[derive(Default)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    model: Option<String>,
    tools: Option<Vec<String>>,
    allowed: Option<Vec<String>>,
    disallowed: Option<Vec<String>>,
    unused: Vec<String>,
}

impl Frontmatter {
    fn list(&mut self, list: List) -> &mut Option<Vec<String>> {
        match list {
            List::Tools => &mut self.tools,
            List::Allowed => &mut self.allowed,
            List::Disallowed => &mut self.disallowed,
        }
    }

    fn extend(&mut self, list: List, text: &str) -> anyhow::Result<()> {
        let names = self.list(list).get_or_insert_with(Vec::new);
        names.extend(split_names(text));
        if names.len() > MAX_TOOL_NAMES {
            bail!("lists more than {MAX_TOOL_NAMES} tools");
        }
        Ok(())
    }

    fn field(&mut self, key: &str, value: &str) -> anyhow::Result<Open> {
        let once = |taken: bool| {
            if taken {
                bail!("has more than one '{key}'");
            }
            Ok(())
        };
        match key {
            "name" => {
                once(self.name.is_some())?;
                self.name = Some(scalar(value).to_owned());
                Ok(Open::Nothing)
            }
            "description" => {
                once(self.description.is_some())?;
                self.description = Some(scalar(value).to_owned());
                Ok(Open::Description)
            }
            "model" => {
                once(self.model.is_some())?;
                self.model = Some(scalar(value).to_owned());
                Ok(Open::Nothing)
            }
            "tools" | "allowed-tools" | "disallowedTools" => {
                let list = match key {
                    "tools" => List::Tools,
                    "allowed-tools" => List::Allowed,
                    _ => List::Disallowed,
                };
                once(self.list(list).is_some())?;
                *self.list(list) = Some(Vec::new());
                self.extend(list, scalar(value))?;
                Ok(Open::List(list))
            }
            // Only how a card looks in Claude Code.
            "color" => Ok(Open::Nothing),
            _ => {
                self.unused.push(key.to_owned());
                Ok(Open::Nothing)
            }
        }
    }
}

/// A scalar without its quotes; the marker of a block scalar counts as nothing, its lines follow.
fn scalar(value: &str) -> &str {
    let value = value.trim();
    if matches!(value, "|" | ">" | "|-" | ">-" | "|+" | ">+") {
        return "";
    }
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// Tool names from `A, B(c, d), "E"` or `[A, B]`; a comma inside parentheses doesn't split.
fn split_names(text: &str) -> Vec<String> {
    let text = text.trim();
    let text = text
        .strip_prefix('[')
        .map_or(text, |rest| rest.strip_suffix(']').unwrap_or(rest));
    let mut names = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for character in text.chars() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                names.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(character);
    }
    names.push(current);
    names
        .iter()
        .map(|name| name.trim().trim_matches(['"', '\'']).trim())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A key at the very start of a line.
fn key_of(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once(':')?;
    (!key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)))
    .then_some((key, value))
}

fn read_frontmatter(lines: &[&str]) -> anyhow::Result<Frontmatter> {
    let mut fields = Frontmatter::default();
    let mut open = Open::Nothing;
    // A flow list running over lines: which one, and its text so far.
    let mut flow: Option<(List, String)> = None;
    for line in lines {
        if let Some((list, mut text)) = flow.take() {
            text.push(' ');
            text.push_str(line.trim());
            if line.contains(']') {
                fields.extend(list, &text)?;
            } else {
                flow = Some((list, text));
            }
            continue;
        }
        let known = key_of(line).filter(|(key, _)| FIELDS.contains(key));
        if let Some((key, value)) = known {
            let value = value.trim();
            if let (true, Some(list)) = (
                value.starts_with('[') && !value.contains(']'),
                match key {
                    "tools" => Some(List::Tools),
                    "allowed-tools" => Some(List::Allowed),
                    "disallowedTools" => Some(List::Disallowed),
                    _ => None,
                },
            ) {
                open = fields.field(key, "")?;
                flow = Some((list, value.to_owned()));
            } else {
                open = fields.field(key, value)?;
            }
            continue;
        }
        let trimmed = line.trim();
        match open {
            Open::List(list) if !trimmed.is_empty() && line.starts_with([' ', '\t', '-']) => {
                fields.extend(list, trimmed.strip_prefix('-').unwrap_or(trimmed))?;
            }
            Open::List(_) if trimmed.is_empty() => {}
            Open::Description => {
                if let Some(description) = &mut fields.description {
                    description.push(' ');
                    description.push_str(trimmed);
                    // Only the opening is kept, so a long one doesn't need to grow further.
                    description.truncate(floor(description, 2 * MAX_DESCRIPTION_BYTES));
                }
            }
            _ => {
                open = Open::Nothing;
                if let Some((key, _)) = key_of(line) {
                    fields.unused.push(key.to_owned());
                }
            }
        }
    }
    if flow.is_some() {
        bail!("has a tool list that is never closed");
    }
    Ok(fields)
}

fn floor(text: &str, mut index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// One readable line: real and written-out line breaks become spaces, examples are left out.
fn clean_description(text: &str) -> String {
    let text = text.replace("\\n", " ");
    let opening = text.split("<example").next().unwrap_or_default();
    let line = opening
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let end = floor(&line, MAX_DESCRIPTION_BYTES);
    line[..end].to_owned()
}

/// The Themis tools a Claude tool name stands for.
fn mapped(name: &str) -> Option<&'static [&'static str]> {
    match name {
        // `LS` and `NotebookRead` are older names for what `Read` and `Glob` do now; `MultiEdit`
        // for what `Edit` does. A subagent has these tools without any list.
        "Read" | "Grep" | "Glob" | "LS" | "NotebookRead" => {
            Some(&["read_file", "list_dir", "search_file"])
        }
        "Bash" => Some(&["shell"]),
        "Edit" | "Write" | "MultiEdit" => Some(&WRITE_TOOLS),
        _ => None,
    }
}

/// What a tool list and a deny list leave, in a fixed order, and the listed names that have no
/// Themis tool. A scoped name such as `Bash(git diff:*)` can't be held to its scope, so it isn't
/// widened to the whole tool; denying one takes the whole tool away.
fn resolve_tools(
    listed: Option<&[String]>,
    denied: &[String],
) -> (Option<Vec<String>>, Vec<String>) {
    let all = READ_ONLY_TOOLS.iter().chain(WRITE_TOOLS.iter());
    let everything = listed.is_none_or(|names| names.iter().any(|name| name == "*"));
    let mut dropped = Vec::new();
    let mut kept = BTreeSet::new();
    if !everything {
        for name in listed.unwrap_or_default() {
            match mapped(name) {
                Some(tools) => kept.extend(tools.iter().copied()),
                None => dropped.push(name.chars().take(MAX_DROPPED_NAME_CHARS).collect()),
            }
        }
    } else {
        kept.extend(all.clone().copied());
    }
    let before = kept.len();
    for name in denied {
        let base = name.split('(').next().unwrap_or(name).trim();
        if base == "*" {
            kept.clear();
        } else if let Some(tools) = mapped(base) {
            for tool in tools {
                kept.remove(tool);
            }
        }
    }
    dropped.truncate(MAX_DROPPED_NAMES);
    if everything && kept.len() == before {
        return (None, dropped);
    }
    (
        Some(
            all.filter(|tool| kept.contains(**tool))
                .map(|tool| (*tool).to_owned())
                .collect(),
        ),
        dropped,
    )
}

/// Reads one `agents/*.md`. Fails on anything that isn't a whole, safe definition.
pub fn parse_profile(content: &str) -> anyhow::Result<ParsedProfile> {
    let normalized = content.replace("\r\n", "\n");
    let text = normalized.strip_prefix('\u{feff}').unwrap_or(&normalized);
    let tail = text
        .strip_prefix("---\n")
        .context("has no frontmatter (it must start with ---)")?;
    let mut lines = Vec::new();
    let mut offset = 0;
    let mut body = None;
    for line in tail.split_inclusive('\n') {
        offset += line.len();
        let plain = line.trim_end();
        if plain == "---" {
            body = Some(&tail[offset..]);
            break;
        }
        if offset > MAX_FRONTMATTER_BYTES {
            bail!("has frontmatter over {MAX_FRONTMATTER_BYTES} bytes");
        }
        lines.push(line.trim_end_matches('\n'));
    }
    let body = body.context("has frontmatter that is never closed")?;
    if lines.iter().any(|line| line.contains('\0')) {
        bail!("has a NUL byte in its frontmatter");
    }
    let fields = read_frontmatter(&lines)?;
    let name = fields
        .name
        .filter(|name| !name.is_empty())
        .context("has no name")?;
    // Some are written in words ("Company Researcher"); the one word is what gets asked for.
    let name = if name.contains(char::is_whitespace) {
        name.split_whitespace()
            .collect::<Vec<_>>()
            .join("-")
            .to_lowercase()
    } else {
        name
    };
    if !safe_name(&name) {
        bail!("has an unsafe name");
    }
    let description = clean_description(fields.description.as_deref().unwrap_or_default());
    if description.is_empty() {
        bail!("has no description");
    }
    let instructions = body.trim();
    if instructions.is_empty() {
        bail!("has no instructions");
    }
    let listed = fields.tools.as_deref().or(fields.allowed.as_deref());
    let (tools, dropped_tools) =
        resolve_tools(listed, fields.disallowed.as_deref().unwrap_or_default());
    let profile = SubagentProfile {
        name,
        description,
        instructions: instructions.to_owned(),
        tools,
        model: fields.model.filter(|model| {
            !model.is_empty()
                && model != "inherit"
                && model.len() <= MAX_MODEL_BYTES
                && !model.chars().any(char::is_control)
        }),
        dropped_tools,
    };
    profile.validate()?;
    Ok(ParsedProfile {
        profile,
        unused: fields.unused,
    })
}

/// Imports `agents/*.md` and the files or folders a plugin declares as `agents` (a folder's
/// direct `.md` children). A bad definition fails the import; files that left the plugin never
/// reach `spec.files`.
pub(super) fn import_profiles(spec: &mut PluginSpec, declared: &[String]) -> anyhow::Result<()> {
    let folders: Vec<&str> = std::iter::once("agents")
        .chain(declared.iter().map(String::as_str))
        .filter(|root| !root.is_empty())
        .collect();
    let mut paths = BTreeSet::new();
    let mut nested = Vec::new();
    for path in spec.files.keys().filter(|path| path.ends_with(".md")) {
        for folder in &folders {
            if path == folder {
                paths.insert(path.clone());
            } else if let Some(rest) = path.strip_prefix(&format!("{folder}/")) {
                if rest.contains('/') {
                    nested.push(path.clone());
                } else {
                    paths.insert(path.clone());
                }
            }
        }
    }
    nested.retain(|path| !paths.contains(path));
    for path in nested {
        spec.notices.push(format!(
            "{path}: not read as a subagent (too deep in its folder)"
        ));
    }
    let mut names = BTreeSet::new();
    for path in paths {
        let parsed = parse_profile(&spec.files[&path])
            .map_err(|error| anyhow::anyhow!("Subagent {path} {error:#}"))?;
        if !names.insert(parsed.profile.name.clone()) {
            bail!("Subagent {path} repeats the name '{}'", parsed.profile.name);
        }
        for key in parsed.unused {
            spec.notices
                .push(format!("{path}: subagent frontmatter {key}"));
        }
        if !parsed.profile.dropped_tools.is_empty() {
            spec.unsupported.push(format!(
                "Subagent tools left out: {path} ({})",
                parsed.profile.dropped_tools.join(", ")
            ));
        }
        spec.subagents.push(parsed.profile);
    }
    Ok(())
}

/// A profile and the plugin it came from.
#[derive(Debug)]
pub struct NamedProfile {
    pub plugin: String,
    pub profile: SubagentProfile,
}

/// The profiles one run may use: those of its enabled plugins, captured when it starts.
#[derive(Debug, Default)]
pub struct ProfileCatalog {
    profiles: Vec<Arc<NamedProfile>>,
}

impl ProfileCatalog {
    /// A project plugin wins over a global one of the same name.
    pub fn from_plugins(plugins: &[Plugin]) -> Self {
        let mut seen = BTreeSet::new();
        let mut profiles = Vec::new();
        let local_first = plugins
            .iter()
            .filter(|plugin| plugin.enabled && plugin.scope == "local")
            .chain(
                plugins
                    .iter()
                    .filter(|plugin| plugin.enabled && plugin.scope != "local"),
            );
        for plugin in local_first {
            for profile in &plugin.spec.subagents {
                if seen.insert((plugin.spec.name.clone(), profile.name.clone())) {
                    profiles.push(Arc::new(NamedProfile {
                        plugin: plugin.spec.name.clone(),
                        profile: profile.clone(),
                    }));
                }
            }
        }
        profiles.sort_by(|a, b| (&a.profile.name, &a.plugin).cmp(&(&b.profile.name, &b.plugin)));
        Self { profiles }
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// The name to ask for it by: its own, or `plugin:name` when several plugins use that name.
    fn label(&self, named: &NamedProfile) -> String {
        let shared = self
            .profiles
            .iter()
            .filter(|other| other.profile.name == named.profile.name)
            .count()
            > 1;
        if shared {
            format!("{}:{}", named.plugin, named.profile.name)
        } else {
            named.profile.name.clone()
        }
    }

    /// One line per profile within the budget, and how many didn't fit.
    pub fn listing(&self) -> (String, usize) {
        let mut text = String::new();
        let mut more = 0;
        for named in &self.profiles {
            let description: String = named
                .profile
                .description
                .chars()
                .take(LISTED_DESCRIPTION_CHARS)
                .collect();
            let line = format!("- {}: {description}", self.label(named));
            if more > 0 || text.len() + line.len() + 1 > LISTING_BYTES {
                more += 1;
                continue;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&line);
        }
        (text, more)
    }

    fn names(&self) -> String {
        let mut names = String::new();
        for named in &self.profiles {
            let label = self.label(named);
            if names.len() + label.len() + 2 > ERROR_NAMES_BYTES {
                names.push_str(", …");
                break;
            }
            if !names.is_empty() {
                names.push_str(", ");
            }
            names.push_str(&label);
        }
        names
    }

    /// The profile `requested` names: `name`, or `plugin:name`.
    pub fn find(&self, requested: &str) -> Result<Arc<NamedProfile>, String> {
        let requested = requested.trim();
        let shown: String = requested.chars().take(100).collect();
        let (plugin, name) = match requested.split_once(':') {
            Some((plugin, name)) => (Some(plugin), name),
            None => (None, requested),
        };
        let found: Vec<&Arc<NamedProfile>> = self
            .profiles
            .iter()
            .filter(|named| {
                named.profile.name == name && plugin.is_none_or(|plugin| named.plugin == plugin)
            })
            .collect();
        match found.as_slice() {
            [one] => Ok(Arc::clone(one)),
            [] if self.is_empty() => Err(format!(
                "Unknown subagent profile '{shown}': no enabled plugin has any"
            )),
            [] => Err(format!(
                "Unknown subagent profile '{shown}'. Enabled profiles: {}",
                self.names()
            )),
            several => Err(format!(
                "Subagent profile '{shown}' is in several plugins: use one of {}",
                several
                    .iter()
                    .map(|named| self.label(named))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn parse(content: &str) -> SubagentProfile {
        parse_profile(content).unwrap().profile
    }

    fn with_tools(line: &str) -> SubagentProfile {
        parse(&format!("---\nname: a\ndescription: d\n{line}\n---\nBody"))
    }

    const READS: [&str; 3] = ["read_file", "list_dir", "search_file"];

    #[test]
    fn every_form_of_tool_list_is_read() {
        for form in [
            "tools: Read, Grep",
            "tools: [Read, Grep]",
            "tools: [\"Read\", 'Grep']",
            "tools: \"Read, Grep\"",
            "tools:\n  - Read\n  - Grep",
            "tools:\n- Read\n- Grep",
            "tools: [\n  Read,\n  Grep\n]",
            "tools: >-\n  Read, Grep",
            "allowed-tools: Read, Grep",
        ] {
            let profile = with_tools(form);
            assert_eq!(profile.tools.as_deref().unwrap(), READS, "{form}");
            assert!(profile.dropped_tools.is_empty(), "{form}");
        }
    }

    #[test]
    fn claude_names_map_to_themis_tools() {
        assert_eq!(with_tools("tools: Bash").tools.unwrap(), ["shell"]);
        assert_eq!(with_tools("tools: Edit").tools.unwrap(), WRITE_TOOLS);
        assert_eq!(with_tools("tools: Write").tools.unwrap(), WRITE_TOOLS);
        assert_eq!(with_tools("tools: *").tools, None);
        assert_eq!(parse("---\nname: a\ndescription: d\n---\nB").tools, None);
        // Older names for tools a subagent has anyway.
        assert_eq!(with_tools("tools: LS, NotebookRead").tools.unwrap(), READS);
        assert_eq!(with_tools("tools: MultiEdit").tools.unwrap(), WRITE_TOOLS);
    }

    #[test]
    fn names_a_subagent_has_no_tool_for_stay_out() {
        // A subagent has no web, plan or background tools, so naming them grants nothing.
        let names = "WebFetch, WebSearch, TodoWrite, KillShell, BashOutput, AskUserQuestion";
        let profile = with_tools(&format!("tools: Read, {names}"));
        assert_eq!(profile.tools.as_deref().unwrap(), READS);
        assert_eq!(profile.dropped_tools.join(", "), names);
    }

    #[test]
    fn a_list_it_cannot_read_never_becomes_all_tools() {
        // Empty lists, and lists of only unknown names, leave nothing.
        for form in ["tools:", "tools: []", "tools: WebFetch", "tools: read"] {
            assert_eq!(
                with_tools(form).tools.unwrap(),
                Vec::<String>::new(),
                "{form}"
            );
        }
        // A scoped tool isn't widened to the whole tool, and commas inside it don't split.
        let scoped = with_tools("tools: Read, Bash(git diff:*, git log:*), mcp__docs__*");
        assert_eq!(scoped.tools.as_deref().unwrap(), READS);
        assert_eq!(
            scoped.dropped_tools,
            ["Bash(git diff:*, git log:*)", "mcp__docs__*"]
        );
        assert!(!scoped.allows("shell") && !scoped.allows_writes());
    }

    #[test]
    fn a_deny_list_only_takes_away() {
        let denied = with_tools("disallowedTools: Bash(rm:*), Write");
        assert_eq!(denied.tools.as_deref().unwrap(), READS);
        let both = with_tools("tools: Read, Bash\ndisallowedTools: Bash");
        assert_eq!(both.tools.as_deref().unwrap(), READS);
        // An unknown name denies nothing it can name.
        assert_eq!(with_tools("disallowedTools: WebFetch").tools, None);
        assert_eq!(
            with_tools("disallowedTools: *").tools.unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn loose_descriptions_stay_in_the_description() {
        let profile = parse(
            "---\nname: a\ndescription: Use this agent when reviewing. Examples:\n\n<example>\nContext: a review\nuser: \"hi\"\n</example>\nmodel: sonnet\ntools: [\"Read\"]\n---\nBody",
        );
        assert_eq!(
            profile.description,
            "Use this agent when reviewing. Examples:"
        );
        assert_eq!(profile.model.as_deref(), Some("sonnet"));
        assert_eq!(profile.tools.as_deref().unwrap(), READS);
        let written_out = parse("---\nname: a\ndescription: \"First line\\n\\nSecond\"\n---\nBody");
        assert_eq!(written_out.description, "First line Second");
        let block = parse("---\nname: a\ndescription: |\n  Several\n  lines\n---\nBody");
        assert_eq!(block.description, "Several lines");
    }

    #[test]
    fn inherit_is_no_model_and_unused_keys_are_listed() {
        let parsed =
            parse_profile("---\nname: a\ndescription: d\nmodel: inherit\ncolor: red\neffort: high\nmaxTurns: 3\n---\nB").unwrap();
        assert_eq!(parsed.profile.model, None);
        assert_eq!(parsed.unused, ["effort", "maxTurns"]);
    }

    #[test]
    fn a_name_written_in_words_is_asked_for_in_one_word() {
        let parse_name = |name: &str| {
            parse_profile(&format!("---\nname: {name}\ndescription: d\n---\nB"))
                .map(|parsed| parsed.profile.name)
        };
        assert_eq!(
            parse_name("Company Researcher").unwrap(),
            "company-researcher"
        );
        assert_eq!(parse_name("\"quoted-name\"").unwrap(), "quoted-name");
        // Words don't excuse a path.
        for bad in ["../ evil", "a b/c", "a - b", ".. a"] {
            assert!(parse_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn bad_definitions_fail() {
        for bad in [
            "",
            "text",
            "---\n",
            "---\nname: a\ndescription: d\nBody",
            "---\ndescription: d\n---\nB",
            "---\nname: a\n---\nB",
            "---\nname: a\ndescription: <example>x</example>\n---\nB",
            "---\nname: ../a\ndescription: d\n---\nB",
            "---\nname: a/b\ndescription: d\n---\nB",
            "---\nname: a--b\ndescription: d\n---\nB",
            "---\nname: a\ndescription: d\n---\n   \n",
            "---\nname: a\ndescription: d\ntools: [Read\n---\nB",
            "---\nname: a\nname: b\ndescription: d\n---\nB",
            "---\nname: a\ndescription: d\ntools: Read\ntools: Bash\n---\nB",
            "---\nname: a\ndescription: d\nmodel: x\nmodel: y\n---\nB",
            "---\nname: a\0\ndescription: d\n---\nB",
        ] {
            assert!(parse_profile(bad).is_err(), "{bad:?}");
        }
        let over = format!("---\nname: a\ndescription: {}\n---\nB", "x".repeat(70_000));
        assert!(parse_profile(&over).is_err());
        let long = format!(
            "---\nname: a\ndescription: d\n---\n{}",
            "x".repeat(crate::skills::MAX_INSTRUCTIONS_BYTES + 1)
        );
        assert!(parse_profile(&long).is_err());
    }

    fn plugin(name: &str, scope: &str, enabled: bool, profiles: &[(&str, &str)]) -> Plugin {
        Plugin {
            scope: scope.into(),
            revision: "r".into(),
            enabled,
            source: None,
            spec: PluginSpec {
                name: name.into(),
                subagents: profiles
                    .iter()
                    .map(|(name, description)| SubagentProfile {
                        name: (*name).into(),
                        description: (*description).into(),
                        instructions: format!("{scope} {name}"),
                        tools: None,
                        model: None,
                        dropped_tools: vec![],
                    })
                    .collect(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn only_enabled_plugins_count_and_a_shared_name_needs_its_plugin() {
        let catalog = ProfileCatalog::from_plugins(&[
            plugin("one", "global", true, &[("review", "a"), ("docs", "b")]),
            plugin("two", "global", true, &[("review", "c")]),
            plugin("off", "global", false, &[("hidden", "d")]),
        ]);
        assert_eq!(catalog.find("docs").unwrap().plugin, "one");
        assert_eq!(catalog.find("two:review").unwrap().plugin, "two");
        let several = catalog.find("review").unwrap_err();
        assert!(
            several.contains("one:review") && several.contains("two:review"),
            "{several}"
        );
        let unknown = catalog.find("hidden").unwrap_err();
        assert!(
            unknown.contains("'hidden'") && unknown.contains("docs"),
            "{unknown}"
        );
        assert!(!catalog.listing().0.contains("hidden"));
        assert!(catalog.listing().0.contains("- two:review: c"));
    }

    #[test]
    fn a_project_plugin_wins_over_a_global_one_of_the_same_name() {
        let catalog = ProfileCatalog::from_plugins(&[
            plugin("same", "global", true, &[("review", "g")]),
            plugin("same", "local", true, &[("review", "l")]),
        ]);
        assert_eq!(catalog.find("review").unwrap().profile.description, "l");
    }

    #[test]
    fn the_listing_stays_within_its_budget_and_says_how_many_it_left_out() {
        let long = "d".repeat(900);
        let names: Vec<String> = (0..100)
            .map(|index| format!("profile-{index:03}"))
            .collect();
        let profiles: Vec<(&str, &str)> = names
            .iter()
            .map(|name| (name.as_str(), long.as_str()))
            .collect();
        let catalog = ProfileCatalog::from_plugins(&[plugin("big", "global", true, &profiles)]);
        let (text, more) = catalog.listing();
        assert!(text.len() <= LISTING_BYTES, "{}", text.len());
        assert_eq!(text.lines().count() + more, 100);
        assert!(more > 0);
        assert!(
            text.lines().all(|line| line.len() < 130),
            "descriptions are cut"
        );
        let error = catalog.find("nope").unwrap_err();
        assert!(error.len() < ERROR_NAMES_BYTES + 100, "{}", error.len());
    }

    proptest! {
        /// Any text: no panic, and whatever is accepted is valid, bounded and only ever names
        /// the tools a subagent may have.
        #[test]
        fn any_text_parses_or_fails_cleanly(content in "\\PC*") {
            if let Ok(parsed) = parse_profile(&content) {
                prop_assert!(parsed.profile.validate().is_ok());
            }
        }

        /// Text shaped like a definition reaches the deep paths.
        #[test]
        fn shaped_frontmatter_never_widens_a_list(
            name in "[a-z]{1,8}",
            names in proptest::collection::vec(
                prop_oneof![
                    Just("Read".to_owned()),
                    Just("Bash(git:*)".to_owned()),
                    Just("WebFetch".to_owned()),
                    Just("mcp__x__*".to_owned()),
                    "[ -~]{0,12}",
                ],
                0..8,
            ),
            noise in "[ -~\n]{0,60}",
        ) {
            let list = names.join(", ");
            let content = format!("---\nname: {name}\ndescription: d\n{noise}\ntools: {list}\n---\nBody");
            if let Ok(parsed) = parse_profile(&content) {
                let profile = parsed.profile;
                prop_assert!(profile.validate().is_ok());
                // With no plain Bash, Edit, Write or * named anywhere, nothing beyond reads.
                if !content.contains("Bash\n") && !content.contains("Bash,")
                    && !content.contains("Edit") && !content.contains("Write")
                    && !content.contains('*') && !content.contains("[")
                    && !content.contains("allowed-tools") && !content.contains("disallowedTools")
                {
                    prop_assert!(!profile.allows("shell"));
                    prop_assert!(!profile.allows_writes());
                }
            }
        }

        /// Any scoped or unknown name is dropped, never mapped.
        #[test]
        fn names_with_a_scope_are_never_mapped(scope in "[a-z :*]{0,10}") {
            for base in ["Read", "Bash", "Write"] {
                let name = format!("{base}({scope})");
                let (tools, dropped) = resolve_tools(Some(std::slice::from_ref(&name)), &[]);
                prop_assert_eq!(tools.unwrap(), Vec::<String>::new());
                prop_assert_eq!(dropped.len(), 1);
            }
        }
    }
}
