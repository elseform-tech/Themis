use super::*;

/// A Themis tool's name in Claude's words, for scripts written for Claude.
pub(super) fn claude_tool_name(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "shell" | "background_task" | "git" => "Bash",
        "read_file" => "Read",
        "write_file" | "copy_file" | "move_file" | "delete_file" | "create_dir" => "Write",
        "apply_patch" => "Edit",
        "search_file" => "Grep",
        "list_dir" => "LS",
        "fetch_url" => "WebFetch",
        "web_search" => "WebSearch",
        "spawn_subagents" => "Agent",
        "Skill" => "Skill",
        _ => return None,
    })
}

/// The files a patch names: both sides of every file header, so a deleted file counts too. A line
/// in a hunk that reads like a header only adds a path to check, which errs towards checking.
pub(super) fn patch_paths(patch: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for line in patch.lines() {
        let Some(path) = line
            .strip_prefix("+++ ")
            .or_else(|| line.strip_prefix("--- "))
        else {
            continue;
        };
        let path = path.split('\t').next().unwrap_or_default().trim();
        let path = path
            .strip_prefix("b/")
            .or_else(|| path.strip_prefix("a/"))
            .unwrap_or(path);
        // Checked after the prefix goes, so a bare `a/` or `b/` names no file either.
        if path.is_empty() || path == "/dev/null" {
            continue;
        }
        let path = path.to_owned();
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// Every file a tool call writes, changes or removes; for a move, both ends.
pub(super) fn paths_touched(tool: &str, arguments: &Value) -> Vec<String> {
    let text = |key: &str| arguments[key].as_str().map(str::to_owned);
    match tool {
        "apply_patch" => arguments["patch"]
            .as_str()
            .map_or_else(Vec::new, patch_paths),
        "move_file" => [text("source_path"), text("destination_path")]
            .into_iter()
            .flatten()
            .collect(),
        "copy_file" => text("destination_path").into_iter().collect(),
        "delete_file" | "write_file" | "create_dir" => ["file_path", "path", "directory_path"]
            .into_iter()
            .find_map(text)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// A tool's arguments in the shape Claude's tools take theirs: the whole command as one string,
/// the file as `file_path` (the first, when a call touches several; see [`HookRuntime`]).
pub(super) fn claude_tool_input(tool: &str, arguments: &Value) -> Value {
    let mut input = arguments.clone();
    match tool {
        "shell" => input["command"] = crate::tools::shell_argv(arguments).join(" ").into(),
        "git" => {
            if let Some(argv) = crate::tools::git_argv(arguments) {
                input["command"] = argv.join(" ").into();
            }
        }
        _ => {
            let paths = paths_touched(tool, arguments);
            if let Some(first) = paths.first() {
                input["file_path"] = first.as_str().into();
            }
            if paths.len() > 1 {
                input["file_paths"] = json!(paths);
            }
        }
    }
    input
}

/// Adds the field names Claude's hooks read, beside Themis's own: a plugin's guard that reads
/// `tool_input.command` would otherwise find nothing, and let everything through.
pub(super) fn add_claude_fields(payload: &mut Value, runtime: &HookRuntime) {
    let event = match payload["event"].as_str().unwrap_or_default() {
        "PromptSubmit" => "UserPromptSubmit",
        "RunStart" => "SessionStart",
        "BeforeTool" => "PreToolUse",
        "AfterTool" if payload["ok"] == false => "PostToolUseFailure",
        "AfterTool" => "PostToolUse",
        "BeforeCompaction" => "PreCompact",
        "RunEnd" if payload["status"] == "failed" => "StopFailure",
        "RunEnd" => "Stop",
        "SubagentEnd" => "SubagentStop",
        other => other,
    }
    .to_owned();
    payload["hook_event_name"] = event.into();
    payload["session_id"] = runtime.thread_id.clone().into();
    payload["cwd"] = runtime.root.to_string_lossy().as_ref().into();
    payload["permission_mode"] = runtime.session.permission_mode.as_str().into();
    if let Some(path) = &runtime.session.transcript_path {
        payload["transcript_path"] = path.to_string_lossy().as_ref().into();
    }
    if payload.get("stop_hook_active").is_none()
        && matches!(payload["event"].as_str(), Some("Stop" | "SubagentEnd"))
    {
        payload["stop_hook_active"] = false.into();
    }
    let Some(tool) = payload["tool"].as_str().map(str::to_owned) else {
        return;
    };
    payload["tool_name"] = runtime.claude_name(&tool).into();
    payload["tool_input"] = claude_tool_input(&tool, &payload["arguments"]);
    if let Some(output) = payload["output"].as_str() {
        let response = serde_json::from_str(output).unwrap_or_else(|_| json!(output));
        payload["tool_response"] = response;
    }
}
