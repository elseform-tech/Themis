"""Classifies each failed tool call by cause, from its tool, error text and the run's approval
mode only (never from a model's opinion):

- `ours`: a tool crashed or returned an internal error, a valid patch was refused, a sandbox or
  approval refusal of an action the run's mode allows, or an error missing the fact needed to
  recover (for example "file not found" without the path);
- `model`: invalid arguments, a path that does not exist, an ambiguous or mismatched patch, a
  command that failed on its own;
- `unclassified`: none of the patterns matched. Listed first in the report for a person to sort;
  add a pattern here so the list stays short.

The patterns start from the error texts in `crates/core/src/tools.rs`, `tools/patch.rs` and the
runtime's `execute_call`.
"""

import json
import re

# Custom-mode rules for the soak (decided 2026-10-25): every item must be read and written
# one at a time, so scripts, searches and other commands are denied. Only `python3 check.py`
# runs as a command. The default is deny, so nothing ever waits for an approval nobody answers.
ALLOWED_TOOLS = ("read_file", "list_dir", "write_file", "apply_patch", "recall", "set_goal", "git")
CHECK_COMMAND = ["python3", "check.py"]
APPROVAL = {
    "default": "deny",
    # Running outside the sandbox is refused outright, never left asking an absent person.
    "rules": [{"tool": "shell_unsandboxed", "action": "deny"},
              {"tool": "shell", "command": CHECK_COMMAND, "action": "allow"}]
             + [{"tool": tool, "action": "allow"} for tool in ALLOWED_TOOLS],
}

# Checked first: these are our faults whatever the tool.
OURS = (
    ("internal", r"execution task failed|worker failed|failed to wait for|panicked|internal error"
                 r"|event queue is full|not available in this context"),
    # YOLO allows every action inside the project: an approval refusal there is ours.
    ("approval_refused", r"\bdenied\b.*\bapprov|\bapproval\b.*\b(denied|refused|required)|not allowed by the user"),
    ("sandbox_refused_project", r"sandbox blocked this command: .*(?:\bout/|\bsrc/|check\.py|INDEX\.md)"),
    ("patch_refused_valid", r"apply_patch: .*(?:could not commit|failed to write|rename)"),
)
MODEL = (
    ("batch_limit", r"^Not run: this batch|Tool arguments exceed"),
    ("bad_arguments", r"argument '[^']+' is invalid|invalid arguments|invalid (?:type|value|JSON)"
                      r"|missing field|unknown field|must not be empty"
                      # a JSONL field asked of a file that has none; a cwd that is no directory
                      r"|Selected JSONL field is missing|working directory '[^']*' is not a directory"),
    ("unknown_tool", r"unknown tool '"),
    ("missing_path", r"(?:no such file|not found|does not exist|cannot find).{0,80}['`\"/]"
                     r"|['`\"][^'`\"]+['`\"].{0,40}(?:no such file|not found|does not exist)"),
    ("outside_project", r"escapes the project root|outside the project|Absolute paths are not allowed"),
    ("read_past_end", r"Read offset \d+ is past the end"),
    ("unsupported_url", r"only http and https URLs are supported"),
    ("patch_mismatch", r"patch does not apply|context mismatch|ambiguous|No files were changed"),
    ("command_failed", r"exit(?:ed)?(?: with)?(?: code| status)?\s*[:=]?\s*[1-9]|non-zero|command failed"
                       r"|failed to start '"),
    ("too_large", r"too large|exceeds? \d+ (?:bytes|characters)"),
)
# An error that names no path, file or quoted value cannot be acted on.
_MISSING_FACT = re.compile(r"^\W*(?:[a-z_]+: )?(?:file )?not found\W*$|^\W*(?:[a-z_]+: )?no such file or directory\W*$",
                           re.IGNORECASE)


def classify(tool, output, mode="yolo", summary=""):
    """(cause, code) for one failed call: cause in ours/model/unclassified, code a short name.
    `summary` is the call's own `tool_started` summary, from which a refused shell call's
    arguments are parsed."""
    text = (output or "").strip()
    if _MISSING_FACT.match(text):
        return "ours", "error_without_fact"
    if mode == "custom" and re.search(r"^\W*(?:tool '[^']+' failed: )?approval denied for '", text):
        # The soak's rules allow these; anything else denied is the model reaching past them.
        if tool == "shell":
            argv = _shell_argv(summary)
            if argv is None:
                if _differs_from_check(summary):
                    return "model", "denied_by_rule"
                return "unclassified", "unparsed_shell_call"
            allowed = argv == CHECK_COMMAND
        else:
            allowed = tool in ALLOWED_TOOLS
        return ("ours", "approval_refused") if allowed else ("model", "denied_by_rule")
    if re.search(r"Permission denied", text) and _writes_protected(summary):
        return "model", "protected_file"
    for code, pattern in OURS:
        if code == "approval_refused" and mode != "yolo":
            continue
        if re.search(pattern, text, re.IGNORECASE | re.DOTALL):
            return "ours", code
    for code, pattern in MODEL:
        if re.search(pattern, text, re.IGNORECASE | re.MULTILINE):
            return "model", code
    if tool == "shell" and re.search(r"Traceback|Error|error:", text):
        return "model", "command_failed"
    return "unclassified", "unknown"


def _shell_argv(summary):
    """A shell call's argv parsed from its `call `shell` with {...}` summary, or None."""
    _, _, raw = (summary or "").partition(" with ")
    try:
        arguments = json.loads(raw)
    except ValueError:
        return None
    if not isinstance(arguments, dict) or not isinstance(arguments.get("command"), str):
        return None
    rest = arguments.get("args") or []
    if not isinstance(rest, list) or not all(isinstance(item, str) for item in rest):
        return None
    return [arguments["command"], *rest]


# The complete leading strings of a summary cut short: `{"command":"…"` and `"args":["…"`.
_STRING = r'"((?:[^"\\]|\\.)*)"'
_PREFIX = re.compile(r'^\{"command":' + _STRING + r'(?:,"args":\[' + _STRING + r'[,\]])?')


def _differs_from_check(summary):
    """True when a cut-short shell summary already shows a command or first argument that is not
    the check command's. Only complete JSON strings count, so a cut never decides it."""
    _, _, raw = (summary or "").partition(" with ")
    match = _PREFIX.match(raw)
    if not match:
        return False
    command, first = (json.loads(f'"{part}"') if part is not None else None for part in match.groups())
    if command != CHECK_COMMAND[0]:
        return True
    return first is not None and first != CHECK_COMMAND[1]


# The files `run.protect` makes read-only.
PROTECTED = re.compile(r"^(?:\./)?(?:check\.py|FORMAT\.md|src/[^/]+)$")
_PATH = re.compile(r'^\{"file_path":' + _STRING)


def _writes_protected(summary):
    _, _, raw = (summary or "").partition(" with ")
    match = _PATH.match(raw)
    return bool(match) and bool(PROTECTED.match(json.loads(f'"{match.group(1)}"')))


def sample(output, limit=120):
    """A short, single-line excerpt of an unclassified error for the report."""
    return " ".join((output or "").split())[:limit]
