use super::*;

/// What a matcher on an event that isn't about a tool is tested against: the field Claude's
/// documentation names for that event.
pub(super) fn moment_subject<'a>(event: &str, payload: &'a Value) -> Option<&'a str> {
    let field = match event {
        "SessionStart" | "ConfigChange" => "source",
        "SessionEnd" => "reason",
        "Notification" => "notification_type",
        "PostCompact" => "trigger",
        "UserPromptExpansion" => "command_name",
        "InstructionsLoaded" => "load_reason",
        _ => return None,
    };
    payload[field].as_str()
}

/// Whether a matcher is a list of exact names rather than a regular expression, by Claude's rule:
/// letters, digits, `_` and `|` only.
pub(super) fn plain_matcher(pattern: &str) -> bool {
    pattern
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '|')
}

/// Whether a matcher takes `subject`. Empty and `*` take everything; a pattern that doesn't compile
/// takes nothing.
#[must_use]
pub fn moment_matches(pattern: &str, subject: &str) -> bool {
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    if plain_matcher(pattern) {
        return pattern.split('|').any(|name| name == subject);
    }
    compile_matcher(pattern).is_ok_and(|regex| regex.is_match(subject))
}

/// What marks the regular expression at the end of a matcher: `shell|re:mcp__.*__query`. It
/// is matched against an MCP tool's Claude names, since those are what plugins write.
const MATCHER_REGEX: &str = "re:";
/// Largest compiled size of a matcher's regular expression.
const MATCHER_REGEX_BYTES: usize = 1 << 20;

pub(super) fn matcher_regex(matcher: &str) -> Option<&str> {
    if let Some(pattern) = matcher.strip_prefix(MATCHER_REGEX) {
        return Some(pattern);
    }
    matcher.split_once("|re:").map(|(_, pattern)| pattern)
}

/// A matcher's regular expression, compiled. Unanchored, like Claude's.
pub fn compile_matcher(pattern: &str) -> Result<regex::Regex, regex::Error> {
    regex::RegexBuilder::new(pattern)
        .size_limit(MATCHER_REGEX_BYTES)
        .build()
}

pub(super) fn matcher_covers(matcher: &str, tool: Option<&str>, mcp_names: &McpNames) -> bool {
    let names = match matcher_regex(matcher) {
        Some(pattern) => {
            let found = tool.and_then(|tool| mcp_names.get(tool));
            if found.is_some_and(|names| {
                compile_matcher(pattern)
                    .is_ok_and(|regex| names.iter().any(|name| regex.is_match(name)))
            }) {
                return true;
            }
            matcher
                .strip_suffix(pattern)
                .and_then(|rest| rest.strip_suffix(MATCHER_REGEX))
                .unwrap_or_default()
        }
        None => matcher,
    };
    names.split('|').any(|name| {
        name == "*"
            || tool == Some(name)
            // The `git` tool runs what `shell` runs: a guard on one holds on the other.
            || (name == "shell" && tool == Some("git"))
            // These write files as `write_file` and `apply_patch` do, so a guard on files covers
            // them. Reading the source of a copy isn't a write, but a move is.
            || (matches!(name, "write_file" | "apply_patch")
                && matches!(
                    tool,
                    Some("copy_file" | "move_file" | "delete_file" | "create_dir")
                ))
    })
}

/// Whether a tool call runs a command `wanted` accepts, or might. Arguments that can't be read
/// count as might, so a guard errs towards running. `background_task` only looks at or stops
/// tasks `shell` started, which met the guard then.
pub(super) fn runs_command(
    tool: &str,
    arguments: &Value,
    wanted: impl Fn(&[String]) -> bool,
) -> bool {
    let argv = match tool {
        "shell" if crate::tools::shell_arguments_readable(arguments) => {
            crate::tools::shell_argv(arguments)
        }
        "git" => crate::tools::git_argv(arguments).unwrap_or_default(),
        "shell" => return true,
        "background_task" => return !crate::tools::background::only_inspects(arguments),
        _ => return false,
    };
    if argv.is_empty() {
        return true;
    }
    crate::tools::commands_run_by(&argv)
        .is_none_or(|commands| commands.iter().any(|command| wanted(command)))
}

/// Whether a command, its words joined with spaces and its program by bare name or as written,
/// matches `pattern`.
pub(super) fn command_matches_glob(command: &[String], pattern: &str) -> bool {
    let Some((program, arguments)) = command.split_first() else {
        return false;
    };
    let line = |program: &str| {
        std::iter::once(program)
            .chain(arguments.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let bare = crate::tools::program_basename(program);
    glob_matches(pattern, &line(&bare)) || glob_matches(pattern, &line(program))
}

/// Whether all of `text` matches `pattern`, where `*` is any text and everything else itself.
#[must_use]
pub fn glob_matches(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let mut parts: Vec<&str> = parts.collect();
    let Some(last) = parts.pop() else {
        return rest.is_empty();
    };
    for part in parts {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// The same program, with the rule's other words among its arguments in order. Options may sit
/// between them (`git -C . push`), so this errs towards a guard running.
pub(super) fn starts_with(command: &[String], prefix: &[String]) -> bool {
    let (Some((program, arguments)), Some((wanted, words))) =
        (command.split_first(), prefix.split_first())
    else {
        return false;
    };
    if crate::tools::program_basename(program) != crate::tools::program_basename(wanted) {
        return false;
    }
    let mut arguments = arguments.iter();
    words
        .iter()
        .all(|word| arguments.any(|argument| argument == word))
}
