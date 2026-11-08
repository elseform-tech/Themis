use super::*;

#[cfg(test)]
mod imported_hook_name_tests;

/// Whether a prompt hook's question says it must not stop anything, as paypal's does.
fn says_it_never_blocks(question: &str) -> bool {
    question.to_lowercase().contains("never block")
}

/// Themis tools for a Claude tool name in a hook matcher.
fn themis_tools(claude: &str) -> Option<&'static [&'static str]> {
    Some(match claude {
        // `Shell` is what other agents call it; plugins for several list both.
        "Bash" | "Shell" => &["shell", "background_task"],
        "Read" => &["read_file"],
        "Write" => &["write_file"],
        // Themis has no notebook tool: a notebook is edited like any other file.
        "Edit" | "MultiEdit" | "NotebookEdit" => &["apply_patch", "write_file"],
        // Elsewhere listing and searching go through the shell, whose hooks see a command, not
        // the pattern and path these hooks expect: they are skipped with a reason.
        "Grep" if cfg!(windows) => &["search_file"],
        "Glob" | "LS" if cfg!(windows) => &["list_dir", "search_file"],
        "WebFetch" => &["fetch_url"],
        "WebSearch" => &["web_search"],
        // Not a tool here: hooks fire for it when a skill is taken up (`hooks::skill_read_hooks`).
        "Skill" => &["Skill"],
        "Agent" | "Task" => &["spawn_subagents"],
        _ => return None,
    })
}

/// `matcher` split at its top-level `|`: one inside a group or a class doesn't split.
fn matcher_alternatives(matcher: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut class, mut escaped, mut start) = (0usize, false, false, 0);
    for (index, character) in matcher.char_indices() {
        match character {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '[' => class = true,
            ']' => class = false,
            '(' if !class => depth += 1,
            ')' if !class => depth = depth.saturating_sub(1),
            '|' if !class && depth == 0 => {
                parts.push(&matcher[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&matcher[start..]);
    parts
}

/// A Claude tool matcher ("Edit|Write") as a Themis matcher, and the names with no Themis tool.
/// Plain names become Themis tool names (those already Themis names are kept). Alternatives
/// naming MCP tools (`mcp__.*__query`, `__exec$`) become a regular expression over Claude's
/// names for them. Any other pattern says nothing Themis can check, so the whole matcher is
/// unmapped: a guard must not silently match nothing, or everything.
fn map_tool_matcher(matcher: &str) -> (Option<String>, Vec<String>) {
    let mut mapped: Vec<String> = Vec::new();
    let mut unmapped = Vec::new();
    let mut patterns: Vec<&str> = Vec::new();
    let alternatives: Vec<&str> = matcher_alternatives(matcher)
        .into_iter()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    let plain = |name: &str| {
        name.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    };
    for name in alternatives {
        if name.contains("__") {
            patterns.push(name);
            continue;
        }
        if !plain(name) {
            return (None, vec![matcher.to_owned()]);
        }
        let tools: Vec<&str> = match themis_tools(name) {
            Some(tools) => tools.to_vec(),
            None if crate::skills::known_tool_names()
                .iter()
                .any(|known| known == name) =>
            {
                vec![name]
            }
            None => {
                unmapped.push(name.to_owned());
                continue;
            }
        };
        for tool in tools {
            if !mapped.iter().any(|known| known == tool) {
                mapped.push(tool.to_owned());
            }
        }
    }
    if !patterns.is_empty() {
        let pattern = patterns.join("|");
        if crate::plugins::hooks::compile_matcher(&pattern).is_err() {
            return (None, vec![matcher.to_owned()]);
        }
        mapped.push(format!("re:{pattern}"));
    }
    ((!mapped.is_empty()).then(|| mapped.join("|")), unmapped)
}

/// What a command hook's Claude options come to.
#[derive(Default)]
struct HookOptions {
    shell: Option<String>,
    background: bool,
    command_prefix: Option<Vec<String>>,
    command_glob: Option<String>,
    args: Option<Vec<String>>,
    rewake: Option<crate::plugins::hooks::HookRewake>,
}

/// A hook's options, or why the hook has to stay out because its meaning wouldn't hold.
/// `windows` because some only matter on the platform the check runs on.
fn read_hook_options(raw: &Value, windows: bool) -> Result<HookOptions, String> {
    let mut options = HookOptions::default();
    let fields = raw.as_object().into_iter().flatten();
    for (key, value) in fields {
        match key.as_str() {
            "type" | "command" | "timeout" => {}
            // A `prompt` hook's question. It runs on the chat's model, whichever it names, and a
            // no always reaches the agent, as `continueOnBlock` asks.
            "prompt" | "model" | "continueOnBlock" if raw["type"] == "prompt" => {}
            // Text for Claude's spinner.
            "statusMessage" => {}
            "async" => options.background |= value.as_bool() == Some(true),
            // Runs in the background, and what it reports reaches the agent.
            "asyncRewake" => {
                if value.as_bool() == Some(true) {
                    let label = |field: &str| {
                        raw[field]
                            .as_str()
                            .map(str::trim)
                            .filter(|text| !text.is_empty())
                            .map(|text| capped_label(text).to_owned())
                    };
                    options.background = true;
                    options.rewake = Some(crate::plugins::hooks::HookRewake {
                        message: label("rewakeMessage"),
                        summary: label("rewakeSummary"),
                    });
                }
            }
            "rewakeMessage" | "rewakeSummary" => {}
            "shell" => match value.as_str() {
                Some("bash") => options.shell = Some("bash".to_owned()),
                other => return Err(format!("shell {} is not available", other.unwrap_or("?"))),
            },
            // The hook's command for Windows: on any other platform it never applies. Written
            // for PowerShell, and Windows hooks here run in cmd.
            "commandWindows" if windows => {
                return Err("commandWindows needs PowerShell".to_owned());
            }
            "commandWindows" => {}
            "args" => {
                if windows {
                    return Err("args are quoted for a POSIX shell".to_owned());
                }
                let words = value.as_array().and_then(|words| {
                    words
                        .iter()
                        .map(|word| word.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                });
                options.args = Some(words.ok_or("args must be a list of words")?);
            }
            "if" => {
                let rule = value.as_str().unwrap_or_default();
                match condition_prefix(rule) {
                    Some(words) => options.command_prefix = Some(words),
                    None => {
                        options.command_glob = Some(
                            condition_glob(rule)
                                .ok_or_else(|| format!("condition {rule} can't be checked"))?,
                        );
                    }
                }
            }
            other => return Err(format!("option {other} has no Themis equivalent")),
        }
    }
    Ok(options)
}

/// The command words in an `if` rule: `Bash(git push *)`, `Bash(git push:*)` or
/// `Bash(git push)`. Patterns inside words can't be held to, so they aren't read.
fn condition_prefix(rule: &str) -> Option<Vec<String>> {
    let inner = rule.strip_prefix("Bash(")?.strip_suffix(')')?;
    let words = inner
        .strip_suffix(":*")
        .or_else(|| inner.strip_suffix(" *"))
        .unwrap_or(inner);
    let plain = |word: &str| !word.contains(['*', '?', '[', ']', '(', ')', '"', '\'', '\\', '$']);
    let words: Vec<String> = words.split_whitespace().map(str::to_owned).collect();
    (!words.is_empty() && words.iter().all(|word| plain(word))).then_some(words)
}

/// The pattern in an `if` rule that has `*` inside words: `Bash(git*commit*)`. Something other
/// than `*` must say what it is about, and `*` is the only pattern character checked.
fn condition_glob(rule: &str) -> Option<String> {
    let inner = rule.strip_prefix("Bash(")?.strip_suffix(')')?;
    let says_something = inner.bytes().any(|byte| byte.is_ascii_alphanumeric());
    let plain = !inner.contains(['?', '[', ']', '(', ')', '"', '\'', '\\', '$']);
    (says_something && plain && inner.contains('*')).then(|| inner.to_owned())
}

/// `text`, cut to what a hook's report label may hold.
fn capped_label(text: &str) -> &str {
    let mut end = text.len().min(crate::plugins::hooks::REWAKE_LABEL_LIMIT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A program and its arguments as one line for a POSIX shell, each word reaching the program as
/// it was written. Only `${CLAUDE_PLUGIN_ROOT}` is left for the shell to fill in.
fn quoted_command(program: &str, args: &[String]) -> String {
    const ROOT: &str = "${CLAUDE_PLUGIN_ROOT}";
    let quote = |word: &str| {
        if !word.is_empty()
            && word
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-./:=@%+,".contains(&byte))
        {
            return word.to_owned();
        }
        let escape = |part: &str| {
            part.chars().fold(String::new(), |mut text, character| {
                if matches!(character, '\\' | '"' | '`' | '$') {
                    text.push('\\');
                }
                text.push(character);
                text
            })
        };
        let parts: Vec<String> = word.split(ROOT).map(escape).collect();
        format!("\"{}\"", parts.join(ROOT))
    };
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .map(quote)
        .collect::<Vec<_>>()
        .join(" ")
}

/// What an imported hook gets when it asks for no time limit: Claude's scripts are written for
/// more than a native hook's ten seconds.
const IMPORTED_HOOK_TIMEOUT: u64 = 60;
/// How long a `prompt` hook's request has when it asks for no limit, as in Claude.
const PROMPT_HOOK_TIMEOUT: u64 = 30;

/// Whether a `PermissionRequest` hook's worth is answering yes, going by its command and the script
/// it runs. Hooks never grant permissions, so such a hook can't do what it is for, and the plugin
/// has to say so. Only text can be read: a compiled program that answers allow still has it
/// ignored when it runs, and logged.
fn answers_allow(command: &str, root: &Path) -> bool {
    static ALLOW: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"behavior[\\"']*\s*[:=]\s*[\\"']*allow"#).expect("valid pattern")
    });
    if ALLOW.is_match(command) {
        return true;
    }
    const ROOT: &str = "${CLAUDE_PLUGIN_ROOT}/";
    let Ok(folder) = root.canonicalize() else {
        return false;
    };
    command.match_indices(ROOT).any(|(at, _)| {
        let rest = &command[at + ROOT.len()..];
        let path = rest
            .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ';' | '|' | '&'))
            .next()
            .unwrap_or_default();
        let Ok(script) = root.join(path).canonicalize() else {
            return false;
        };
        script.starts_with(&folder)
            && fs::metadata(&script).is_ok_and(|meta| meta.len() <= 1024 * 1024)
            && fs::read(&script).is_ok_and(|bytes| ALLOW.is_match(&String::from_utf8_lossy(&bytes)))
    })
}

pub fn import_hooks(value: &Value, root: &Path, spec: &mut PluginSpec) -> anyhow::Result<()> {
    let events = value
        .get("hooks")
        .unwrap_or(value)
        .as_object()
        .context("Invalid hook configuration")?;
    for (event, groups) in events {
        let mapped = match event.as_str() {
            // Themis has these moments under the same names; see `hooks::EVENTS`.
            "SessionStart" => "SessionStart",
            "SessionEnd" => "SessionEnd",
            "UserPromptExpansion" => "UserPromptExpansion",
            "PermissionRequest" => "PermissionRequest",
            "PermissionDenied" => "PermissionDenied",
            "PostToolBatch" => "PostToolBatch",
            "PostCompact" => "PostCompact",
            "Notification" => "Notification",
            "ConfigChange" => "ConfigChange",
            "InstructionsLoaded" => "InstructionsLoaded",
            // Fires only when the run failed; `Stop` also runs then, as RunEnd always does.
            "StopFailure" => "RunFailed",
            "PreToolUse" => "BeforeTool",
            "PostToolUse" => "AfterTool",
            "Stop" => "Stop",
            "PreCompact" => "BeforeCompaction",
            "SubagentStart" => "SubagentStart",
            "SubagentStop" => "SubagentEnd",
            "UserPromptSubmit" => "PromptSubmit",
            // Fires only when the tool call failed.
            "PostToolUseFailure" => "ToolCallFailed",
            _ => {
                spec.unsupported.push(format!("Hook event {event}"));
                continue;
            }
        };
        let tool_event = matches!(
            event.as_str(),
            "PreToolUse"
                | "PostToolUse"
                | "PostToolUseFailure"
                | "PermissionRequest"
                | "PermissionDenied"
        );
        // What each event can be about here, and so what a matcher on it can ever match. An empty
        // list takes any name (a skill's). Claude takes no matcher on the last group.
        let moments: Option<&[&str]> = match event.as_str() {
            "SessionStart" => Some(&["startup", "resume"]),
            "SessionEnd" => Some(&["prompt_input_exit", "other"]),
            "PostCompact" => Some(&["auto"]),
            "Notification" => Some(&[
                "permission_prompt",
                "goal_reached",
                "goal_blocked",
                "goal_waiting",
                "goal_quota",
            ]),
            "ConfigChange" => Some(&["user_settings", "project_settings"]),
            "InstructionsLoaded" => Some(&["session_start"]),
            "UserPromptExpansion" => Some(&[]),
            _ => None,
        };
        let matcher_ignored = event == "PostToolBatch";
        for group in groups.as_array().context("Invalid hook groups")? {
            for raw in group["hooks"].as_array().context("Invalid hook list")? {
                // A `prompt` hook asks the chat's model; only where Claude has them and their answer
                // can mean something here. Nothing here asks a web address or starts an agent.
                let question = (raw["type"] == "prompt"
                    && matches!(
                        event.as_str(),
                        "PreToolUse"
                            | "PostToolUse"
                            | "PostToolUseFailure"
                            | "UserPromptSubmit"
                            | "UserPromptExpansion"
                            | "Stop"
                            | "PostToolBatch"
                    ))
                .then(|| raw["prompt"].as_str())
                .flatten()
                .map(str::trim)
                .filter(|text| !text.is_empty() && text.len() <= super::hooks::PROMPT_HOOK_BYTES);
                if raw["type"] != "command" && question.is_none() {
                    let kind = raw["type"].as_str().unwrap_or("other");
                    spec.unsupported
                        .push(format!("{event}: {kind} hook has no Themis equivalent"));
                    continue;
                }
                // Clearing, compacting and logging out have no equivalent: a matcher naming only
                // those would never run, which isn't the same as working.
                let mut moment_matcher = None;
                if let (Some(moments), Some(matcher)) = (
                    moments,
                    group["matcher"]
                        .as_str()
                        .filter(|matcher| !matcher.is_empty() && *matcher != "*"),
                ) {
                    let reachable = moments.is_empty()
                        || moments
                            .iter()
                            .any(|moment| super::hooks::moment_matches(matcher, moment));
                    let compiles = matcher
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '|')
                        || super::hooks::compile_matcher(matcher).is_ok();
                    if !reachable || !compiles {
                        spec.unsupported.push(format!(
                            "{event}: matcher {matcher} has no Themis equivalent"
                        ));
                        continue;
                    }
                    moment_matcher = Some(matcher.to_owned());
                }
                if !tool_event
                    && moments.is_none()
                    && !matcher_ignored
                    && group["matcher"]
                        .as_str()
                        .is_some_and(|matcher| !matcher.is_empty() && matcher != "*")
                {
                    spec.unsupported.push(format!(
                        "{event}: lifecycle matcher requires manual mapping"
                    ));
                    continue;
                }
                let options = match read_hook_options(raw, cfg!(windows)) {
                    Ok(options) => options,
                    Err(reason) => {
                        spec.unsupported.push(format!("{event}: {reason}"));
                        continue;
                    }
                };
                // Claude names its tools differently: a `Bash` guard must guard `shell`.
                let matcher = match group["matcher"]
                    .as_str()
                    .filter(|value| !value.is_empty() && !matches!(*value, "*" | ".*"))
                {
                    Some(matcher) if tool_event => {
                        let (mapped_tools, unmapped) = map_tool_matcher(matcher);
                        for name in &unmapped {
                            spec.unsupported
                                .push(format!("{event}: matcher {name} has no Themis tool"));
                        }
                        if mapped_tools.is_none() {
                            continue;
                        }
                        mapped_tools
                    }
                    _ if moments.is_some() || matcher_ignored => None,
                    other => other.map(str::to_owned),
                };
                let command = if question.is_some() {
                    ""
                } else {
                    raw["command"].as_str().context("Missing hook command")?
                };
                if event == "PermissionRequest" && answers_allow(command, root) {
                    spec.unsupported.push(format!(
                        "{event}: answers allow, and hooks never grant permissions"
                    ));
                    continue;
                }
                let command = match &options.args {
                    Some(args) => quoted_command(command, args),
                    None => command.to_owned(),
                };
                let (shell, background, command_prefix, command_glob, rewake) = (
                    options.shell,
                    options.background,
                    options.command_prefix,
                    options.command_glob,
                    options.rewake,
                );
                // Nobody waits for one that reports back, and a review takes its time.
                let default_timeout = if rewake.is_some() {
                    super::hooks::MAX_TIMEOUT_SECONDS
                } else if question.is_some() {
                    PROMPT_HOOK_TIMEOUT
                } else {
                    IMPORTED_HOOK_TIMEOUT
                };
                spec.hooks.push(Hook {
                    name: imported_hook_name(mapped, matcher.as_deref(), &command, &spec.hooks),
                    event: mapped.into(),
                    command,
                    enabled: false,
                    matcher,
                    // A prompt hook holds when it gets no answer, unless its plugin says it never
                    // blocks. That can only weaken its own check, and its plain no still refuses.
                    failure_policy: question
                        .filter(|question| says_it_never_blocks(question))
                        .map(|_| crate::plugins::hooks::HookFailurePolicy::Ignore),
                    shell,
                    background,
                    command_prefix,
                    command_glob,
                    prompt: question.map(str::to_owned),
                    // Claude's way to refuse; a SessionStart hook can't stop a turn there either.
                    // One that reports back says so the same way, whatever it runs at. A question
                    // is asked once a call, whatever it touches: each asking costs.
                    exit_two_blocks: question.is_none()
                        && (event != "SessionStart" || rewake.is_some()),
                    rewake,
                    timeout_seconds: raw["timeout"]
                        .as_u64()
                        .unwrap_or(default_timeout)
                        .clamp(1, super::hooks::MAX_TIMEOUT_SECONDS),
                    // A broken session hook must not stop every turn.
                    blocking: event == "PreToolUse" && !background,
                    plugin_root: None,
                    plugin_data: None,
                    env_file: None,
                    moment_matcher,
                    runtime_identity: None,
                });
            }
        }
    }
    Ok(())
}

/// A name for an imported hook, which has none of its own: the script it runs, or else when it
/// runs (`before-shell`), made unique within the plugin.
fn imported_hook_name(event: &str, matcher: Option<&str>, command: &str, taken: &[Hook]) -> String {
    const SCRIPTS: [&str; 9] = ["sh", "bash", "py", "js", "mjs", "cjs", "ts", "rb", "pl"];
    const GENERIC: [&str; 5] = ["index", "main", "run", "hook", "hooks"];
    let script = command
        .split_whitespace()
        .map(|word| word.trim_matches(|c| matches!(c, '"' | '\'' | ';')))
        .filter_map(|word| word.rsplit(['/', '\\']).next()?.rsplit_once('.'))
        .find(|(_, extension)| SCRIPTS.contains(extension))
        .map(|(stem, _)| {
            stem.to_ascii_lowercase()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("-")
        })
        .filter(|stem| !stem.is_empty() && !GENERIC.contains(&stem.as_str()));
    let tool = matcher
        .and_then(|matcher| matcher.split('|').next())
        .map(|tool| if tool.starts_with("re:") { "mcp" } else { tool });
    let base = script.unwrap_or_else(|| match (event, tool) {
        ("PromptSubmit", _) => "prompt-submit".into(),
        ("RunStart", _) => "run-start".into(),
        ("BeforeCompaction", _) => "before-compaction".into(),
        ("RunEnd", _) => "run-end".into(),
        ("Stop", _) => "stop".into(),
        ("RunFailed", _) => "run-failed".into(),
        ("BeforeTool", Some(tool)) => format!("before-{tool}").replace('_', "-"),
        ("AfterTool", Some(tool)) => format!("after-{tool}").replace('_', "-"),
        ("ToolCallFailed", Some(tool)) => format!("{tool}-failed").replace('_', "-"),
        ("BeforeTool", None) => "before-tool".into(),
        ("AfterTool", None) => "after-tool".into(),
        ("ToolCallFailed", None) => "tool-failed".into(),
        // `SessionEnd` is `session-end`.
        (other, _) => {
            let mut name = String::new();
            for letter in other.chars() {
                if letter.is_ascii_uppercase() && !name.is_empty() {
                    name.push('-');
                }
                name.push(letter.to_ascii_lowercase());
            }
            name
        }
    });
    let mut name = base.clone();
    let mut number = 1;
    while taken.iter().any(|hook| hook.name == name) {
        number += 1;
        name = format!("{base}-{number}");
    }
    name
}
