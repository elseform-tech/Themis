use super::*;

/// Structured command for the approval dialog, from the raw model arguments
/// of a `shell` or `git` call; `None` for every other tool. `cwd` stays
/// `None` when the call uses the project root.
pub fn command_display_for_call(name: &str, args: &serde_json::Value) -> Option<CommandDisplay> {
    match name {
        "shell" => {
            let argv = shell_argv(args);
            if argv.is_empty() {
                return None;
            }
            Some(CommandDisplay {
                argv,
                cwd: args
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .filter(|cwd| !cwd.is_empty() && *cwd != ".")
                    .map(str::to_owned),
                background: args
                    .get("background")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                sandbox_blocked: None,
            })
        }
        "git" => {
            let rest = args.get("args").and_then(|v| v.as_array())?;
            Some(CommandDisplay {
                argv: std::iter::once("git".to_owned())
                    .chain(rest.iter().filter_map(|v| v.as_str()).map(str::to_owned))
                    .collect(),
                cwd: None,
                background: false,
                sandbox_blocked: None,
            })
        }
        _ => None,
    }
}

/// Repairs a `shell` or `git` call whose `args` list arrived as one string holding
/// that list as JSON (`"args": "[\"grep\", \"-L\"]"`), a shape some models send.
/// Such a call always fails as sent, so reading the list never changes a call that
/// would work; anything else, including a string that is not a list of strings,
/// is returned unchanged for the usual error.
pub fn repair_list_arguments(tool: &str, arguments: &str) -> String {
    if !matches!(tool, "shell" | "git") {
        return arguments.to_owned();
    }
    let Ok(Value::Object(mut fields)) = serde_json::from_str::<Value>(arguments) else {
        return arguments.to_owned();
    };
    let Some(Value::String(text)) = fields.get("args") else {
        return arguments.to_owned();
    };
    match argv_text(text) {
        Some(list) => {
            fields.insert("args".to_owned(), json!(list));
            Value::Object(fields).to_string()
        }
        None => arguments.to_owned(),
    }
}

/// Program and arguments of a `shell` tool call, as validated arguments or raw
/// model JSON. Empty when the call has no usable `command`.
pub fn shell_argv(args: &serde_json::Value) -> Vec<String> {
    let program = args
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let mut argv = vec![program.to_owned()];
    // The same reading as the tool itself: rules and prompts must see what would run.
    match args.get("args") {
        Some(Value::Array(rest)) => {
            argv.extend(rest.iter().filter_map(|v| v.as_str()).map(str::to_owned));
        }
        Some(Value::String(text)) => {
            argv.extend(argv_text(text).unwrap_or_default());
        }
        _ => {}
    }
    let argv = normalize_argv(argv);
    if argv.first().is_none_or(|program| program.trim().is_empty()) {
        return Vec::new();
    }
    argv
}

/// Whether [`shell_argv`] reads all of a call's arguments as the tool would: a program, and
/// `args` as a list of strings or the text of one. Anything else makes the tool fail, and a
/// guard shouldn't treat what it can't read as harmless.
pub fn shell_arguments_readable(args: &serde_json::Value) -> bool {
    args.get("command").is_some_and(Value::is_string) && args_list(args.get("args")).is_some()
}

/// `git` and its arguments, from a `git` tool call; `None` when its `args` can't be read.
pub fn git_argv(args: &serde_json::Value) -> Option<Vec<String>> {
    let rest = args.get("args").filter(|args| !args.is_null())?;
    let rest = args_list(Some(rest))?;
    Some(std::iter::once("git".to_owned()).chain(rest).collect())
}

/// Flags of `status`, `diff`, `log` and `show` that only change what is printed. Any other
/// flag needs approval: `--output` writes a file, `--ext-diff` and `--textconv` run programs,
/// and a flag not listed here may do either.
const GIT_READ_FLAGS: [&str; 24] = [
    "--short",
    "-s",
    "--porcelain",
    "--branch",
    "-b",
    "--stat",
    "--shortstat",
    "--numstat",
    "--name-only",
    "--name-status",
    "--cached",
    "--staged",
    "--oneline",
    "--graph",
    "--decorate",
    "--no-color",
    "-p",
    "--patch",
    "--no-patch",
    "--summary",
    "--all",
    "-n",
    "--reverse",
    "--",
];

/// Flags that take a value after `=`, where the value only shapes the output.
const GIT_READ_VALUE_FLAGS: [&str; 9] = [
    "--format=",
    "--pretty=",
    "--max-count=",
    "--since=",
    "--until=",
    "--author=",
    "--grep=",
    "--unified=",
    "--untracked-files=",
];

/// The risk of a `git` tool call for the approval step: `status`, `diff`, `log` and `show` with
/// only the flags above are reads; a destructive variant is destructive; anything else, and
/// arguments that can't be read, are writes.
pub fn git_call_risk(args: &serde_json::Value) -> RiskLevel {
    let Some(argv) = git_argv(args) else {
        return RiskLevel::Write;
    };
    if is_destructive_variant(&argv) {
        return RiskLevel::Destructive;
    }
    let read_only = matches!(
        argv.get(1).map(String::as_str),
        Some("status" | "diff" | "log" | "show")
    ) && argv[2..].iter().all(|argument| git_read_argument(argument));
    if read_only {
        RiskLevel::Read
    } else {
        RiskLevel::Write
    }
}

fn git_read_argument(argument: &str) -> bool {
    if !argument.starts_with('-') {
        // A path, a revision or a count, inside the project: `git diff` given paths outside
        // the repository compares those files, so an absolute path or a `..` step asks.
        let leaves = argument.split(['/', '\\']).any(|part| part == "..");
        return !leaves && !Path::new(argument).is_absolute() && !argument.starts_with('~');
    }
    let count = argument.strip_prefix('-').is_some_and(|digits| {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    });
    count
        || GIT_READ_FLAGS.contains(&argument)
        || GIT_READ_VALUE_FLAGS
            .iter()
            .any(|flag| argument.starts_with(flag))
}

/// The git subcommands the restricted git tool runs (YOLO runs any).
const GIT_ALLOWED: [&str; 8] = [
    "status", "diff", "log", "show", "add", "commit", "checkout", "branch",
];

pub(super) fn git_refusal(subcommand: &str) -> String {
    format!(
        "git: subcommand '{subcommand}' is denied by the allowlist (allowed: {}). {REFUSAL_NOTE}",
        GIT_ALLOWED.join(", ")
    )
}

/// Why the restricted git tool would refuse `argv` (`git <subcommand> …`), if it would: the
/// runtime refuses such a call before asking anyone, since approving it could not make it run.
pub fn git_allowlist_refusal(argv: &[String]) -> Option<String> {
    let subcommand = argv.get(1)?;
    (!GIT_ALLOWED.contains(&subcommand.as_str())).then(|| git_refusal(subcommand))
}

/// A list of strings, or the JSON text of one; nothing at all is an empty list.
fn args_list(args: Option<&Value>) -> Option<Vec<String>> {
    match args {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(Value::Array(rest)) => rest
            .iter()
            .map(|value| value.as_str().map(str::to_owned))
            .collect(),
        Some(Value::String(text)) => argv_text(text),
        Some(_) => None,
    }
}

/// A list of arguments from JSON text. Models often leave raw line breaks and tabs inside a
/// string, which is unambiguous, so those are read as what they plainly mean. Nothing else is
/// repaired, so a list with anything tacked on is refused rather than guessed at.
pub(super) fn argv_text(text: &str) -> Option<Vec<String>> {
    if let Ok(list) = serde_json::from_str(text) {
        return Some(list);
    }
    let mut escaped = String::with_capacity(text.len());
    let (mut in_string, mut after_backslash) = (false, false);
    for c in text.chars() {
        match c {
            '\n' if in_string => escaped.push_str("\\n"),
            '\r' if in_string => escaped.push_str("\\r"),
            '\t' if in_string => escaped.push_str("\\t"),
            _ => escaped.push(c),
        }
        if in_string && c == '"' && !after_backslash {
            in_string = false;
        } else if !in_string && c == '"' {
            in_string = true;
        }
        after_backslash = in_string && c == '\\' && !after_backslash;
    }
    serde_json::from_str(&escaped).ok()
}

/// `args` as a list, or that list sent as JSON text, which models often do
/// (`"args": "[\"-c\", \"print(1)\"]"`, 5 of 30 shell calls in a live goal). Other text is
/// refused rather than guessed at, so nothing is split differently from what was meant.
pub(super) fn argv_or_json_text<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Argv {
        List(Vec<String>),
        Text(String),
    }
    match Argv::deserialize(deserializer)? {
        Argv::List(list) => Ok(list),
        Argv::Text(text) => argv_text(&text)
            .ok_or_else(|| serde::de::Error::custom(crate::prompts::TOOL_SHELL_ARGS_INVALID)),
    }
}

/// Repairs the two argv shapes models commonly send, before approval, so the
/// person approves exactly what runs: an empty `command` with the program as
/// the first of `args`, and the program repeated as the first argument
/// (`command: "python3", args: ["python3", "x.py"]`, or its full path).
/// `echo`/`printf` keep a repeated word, since printing it is plausible.
pub fn normalize_argv(mut argv: Vec<String>) -> Vec<String> {
    if argv.len() > 1 && argv[0].trim().is_empty() {
        argv.remove(0);
    }
    let base = |word: &str| {
        Path::new(word)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    if argv.len() > 1 && !matches!(base(&argv[0]).as_str(), "echo" | "printf") {
        let (program, first) = (&argv[0], &argv[1]);
        let same = program == first
            || (Path::new(first).has_root() && base(first) == base(program))
            || (Path::new(program).has_root() && base(program) == *first);
        if same {
            argv.remove(1);
        }
    }
    argv
}
